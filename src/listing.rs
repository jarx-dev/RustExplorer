//! Reading folders into entries, folder sizes, and a tab's sorted, filtered view.

use super::*;

/// The shell's type name per extension; every entry shares the cached text instead of a copy.
pub(crate) fn type_of(name: &str, dir: bool) -> Arc<str> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<str>>>> = OnceLock::new();
    let key = if dir { "<dir>".to_string() } else { ext_of(name) };
    let mut c = CACHE.get_or_init(Default::default).lock().unwrap();
    c.entry(key.clone())
        .or_insert_with(|| {
            if let Some(own) = rx_core::known_type_name(&key) {
                return own.into();
            }
            let probe = if dir { name.to_string() } else { format!("x.{key}") };
            let t = sys::type_name(&probe, dir);
            let t = if !t.is_empty() {
                t
            } else if dir {
                "Folder plików".into()
            } else if key.is_empty() {
                "Plik".into()
            } else {
                format!("Plik {}", key.to_uppercase())
            };
            t.into()
        })
        .clone()
}

pub(crate) const FLAT_CAP: usize = 20_000;

impl Entry {
    /// A folder to go into: also a junction or a link to a folder, which `dir` leaves out (std
    /// calls a link neither a file nor a folder).
    pub(crate) fn opens_as_folder(&self) -> bool {
        self.dir || self.attrs & 0x10 != 0
    }
}

pub(crate) fn list_dir(root: &Path, flat: bool) -> Result<Vec<Entry>, String> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = match std::fs::read_dir(&d) {
            Ok(r) => r,
            Err(e) if d == root => return Err(e.to_string()),
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            let path = e.path();
            let dir = md.is_dir();
            if flat && dir {
                stack.push(path);
                continue;
            }
            let name = if flat {
                path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned()
            } else {
                e.file_name().to_string_lossy().into_owned()
            };
            let newest = md.modified().ok().max(md.created().ok());
            let recent = newest.and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|d| d.as_secs() < RECENT_SECS);
            out.push(Entry {
                recent,
                kind: kind_of(&name, dir),
                type_name: type_of(&name, dir),
                size: (!dir).then(|| md.len()),
                modified: md.modified().ok(),
                attrs: md.file_attributes(),
                name,
                path,
                dir,
                depth: 0,
            });
            if out.len() >= FLAT_CAP {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

/// What `t` shows of its folder, as the watcher compares it with the disk when the tab comes on
/// screen (sys::watch::print_of); 0, which no folder matches: the list is not from disk (an
/// archive's, or none came), so the folder is read again as soon as it can be.
fn shown_print(t: &Tab) -> u64 {
    if t.in_arc || t.error.is_some() {
        return 0;
    }
    sys::watch::print_of(t.all.iter().filter(|e| e.depth == 0).map(|e| (e.name.as_str(), if e.dir { 0 } else { e.size.unwrap_or(0) }, e.modified, e.attrs)))
}

/// Where a file of a cloud folder is, as Explorer's Stan column tells it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Cloud {
    /// only in the cloud; opening it downloads it
    Online,
    /// on this device
    Local,
    /// "Zawsze zachowuj na tym urządzeniu"
    Pinned,
}

pub(crate) fn cloud_of(attrs: u32) -> Cloud {
    if rx_core::only_in_cloud(attrs) {
        Cloud::Online
    } else if attrs & 0x80000 != 0 {
        Cloud::Pinned
    } else {
        Cloud::Local
    }
}

impl App {
    /// Gives the listings the tabs asked for (Tab::load) to threads, one each, so a share that
    /// does not answer holds up nothing else; the rows come back as Msg::Listed.
    pub(crate) fn send_listings(&mut self, ctx: &egui::Context) {
        for t in &mut self.tabs {
            if !std::mem::take(&mut t.unsent) || !t.asked.waiting() {
                continue;
            }
            let (path, flat, number, tx, ctx) = (t.path.clone(), t.flat, t.asked.number(), self.tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                sys::init_com();
                // a folder of an archive (or the archive itself) lists from the archive's list; no flat view there
                let arc = archive::split_archive(&path);
                let read = || match &arc {
                    Some((a, inner)) => archive::list(a, inner),
                    None => list_dir(&path, flat),
                };
                // a panic in an archive's decoder must not leave the list waiting for good
                let rows = std::panic::catch_unwind(read).unwrap_or_else(|_| Err("odczyt przerwany przez wewnętrzny błąd".into()));
                let _ = tx.send(Msg::Listed(number, rows, arc.is_some()));
                ctx.request_repaint();
            });
        }
    }

    /// The rows read for request `number` replace those of the tab still waiting for them (none
    /// is: asked again since, or the folder was left). The selection stays on what is still
    /// there, and what was to follow the rows (a selection, a rename) is done.
    pub(crate) fn listed(&mut self, number: u64, rows: Result<Vec<Entry>, String>, in_arc: bool) {
        let Some((i, then)) = self.tabs.iter_mut().enumerate().find_map(|(i, t)| Some((i, t.asked.take(number)?))) else { return };
        let t = &mut self.tabs[i];
        let (old, failed) = (std::mem::take(&mut t.all), t.error.is_some());
        (t.all, t.error) = match rows {
            Ok(v) => (v, None),
            Err(e) => (vec![], Some(e)),
        };
        t.in_arc = in_arc;
        if in_arc {
            t.flat = false;
        }
        let deep = if t.carry && t.error.is_none() { carry_over(&old, &t.sel, &mut t.all) } else { vec![] };
        let paths: HashSet<&PathBuf> = t.all.iter().map(|e| &e.path).collect();
        t.sel.retain(|p| paths.contains(p));
        t.sel.extend(deep);
        // rows nobody but the app asked for (a file operation, a new folder): the watcher gets the
        // new list (watch_shown) and compares it with the disk, so it does not report the change
        // back; rows read because it reported one are newer than what it holds. A folder that
        // could not be read is told again as well: only one that is there can get a watch
        if !t.carry || failed {
            self.watched.clear();
        }
        // a file its preview shows as only in the cloud that these rows have on this device
        // (opened, kept, fetched by another program): the preview is built from its content. Only
        // rows just read say so: older ones, or the index's, would have a freed file downloaded again
        if let Some((p, Shown::Cloud(..))) = &self.preview {
            if self.tabs[i].all.iter().any(|e| e.path == *p && !rx_core::only_in_cloud(e.attrs)) {
                self.preview_read(p.clone());
            }
        }
        let t = &mut self.tabs[i];
        // fresh entries have no folder sizes yet ("Analiza miejsca" fills them again)
        (t.sized, t.shown, t.dirty, t.carry) = (None, true, true, false);
        if let Some(s) = then {
            self.select(i, s);
        }
    }

    /// Selects in tab `i` what `s` names; the first gets the focus and is scrolled into view (the
    /// list finds its row by `focus` when it is drawn).
    pub(crate) fn select(&mut self, i: usize, s: rx_core::Select) {
        let t = &mut self.tabs[i];
        let rename = s.rename;
        let picks = s.pick(|p| t.all.iter().any(|e| e.path == p));
        let Some(first) = picks.first().cloned() else { return };
        t.sel = picks.into_iter().collect();
        (t.anchor, t.focus, t.scroll_to) = (None, Some(first.clone()), Some(0));
        if rename && i == self.cur {
            let text = first.file_name().unwrap_or_default().to_string_lossy().into_owned();
            self.rename = Some(Rename { path: first, text, fresh: true });
        }
    }

    /// The same in the current tab once the rows it waits for are in; at once when it waits for none.
    pub(crate) fn then_select(&mut self, s: rx_core::Select) {
        let t = &mut self.tabs[self.cur];
        if t.asked.waiting() {
            t.asked.then(s);
        } else {
            self.select(self.cur, s);
        }
    }

    /// Tells the watcher which folders are on screen: the current tab's and, in the split view,
    /// the other pane's; none while the window is minimized or in the tray. Every frame, at the
    /// cost of comparing a path or two. Each goes with what its list shows (`shown_print`), so a
    /// list that is not the folder's content any more is reported back and read again. A results
    /// list is no folder to watch, the flat view is refreshed by hand (F5) as before, and a
    /// folder whose rows have not come yet is watched once they have.
    // shortcut: the flat view would need its whole subtree watched; do that if it is used on
    // folders other programs write to
    pub(crate) fn watch_shown(&mut self, ctx: &egui::Context) {
        let away = self.hidden || ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        let on = [Some(self.cur), self.split.then_some(self.other)];
        let on = on.into_iter().flatten().filter(|_| !away).filter_map(|i| self.tabs.get(i)).filter(|t| !t.flat && !t.found && !is_pc(&t.path) && !t.loading());
        if on.clone().map(|t| &t.path).eq(self.watched.iter().map(|w| &w.0)) {
            return;
        }
        let mut dirs: Vec<(PathBuf, u64)> = vec![];
        self.watched.clear();
        for t in on {
            // a folder in an archive changes with the archive file, so its folder is watched
            let dir = if t.in_arc { archive::on_disk(&t.path) } else { t.path.clone() };
            let print = shown_print(t);
            match dirs.iter_mut().find(|d| d.0 == dir) {
                // one folder in both panes: read again unless both show the same list
                Some(d) if d.1 != print => d.1 = 0,
                Some(_) => {}
                None => dirs.push((dir.clone(), print)),
            }
            self.watched.push((t.path.clone(), dir));
        }
        self.watcher.show(dirs);
    }

    pub(crate) fn in_cloud(&self, p: &Path) -> bool {
        self.clouds.iter().any(|c| p.starts_with(c))
    }

    /// Whose sharing link "Kopiuj link" can ask for `p`: OneDrive's places (as Explorer names
    /// them) and Google Drive's streamed drive; the mirrored My Drive folder has no file ids.
    // shortcut: synced SharePoint libraries (OneDrive for Business, named after the company)
    // get none; telling them apart needs their NameSpace entry's handler
    pub(crate) fn cloud_link(&self, p: &Path) -> Option<&'static str> {
        let onedrive = self.known.iter().any(|k| k.2 == Kind::Cloud && k.1.starts_with("OneDrive") && p.starts_with(&k.0));
        let google = self.drives.iter().any(|d| d.1.starts_with("Google Drive") && p.starts_with(&d.0));
        if onedrive {
            Some("OneDrive")
        } else if google {
            Some("Google Drive")
        } else {
            None
        }
    }

    /// The cloud state of a file in a cloud folder; none for folders, elsewhere, in archives and
    /// for rows built from the index (search results, deeper rows of Analiza miejsca), whose
    /// attributes may be minutes old.
    pub(crate) fn cloud_mark(&self, e: &Entry) -> Option<Cloud> {
        // attributes 0: an archive's item (a real file has at least FILE_ATTRIBUTE_NORMAL)
        let live = e.depth == 0 && !self.tabs[self.cur].found;
        (live && !e.dir && e.attrs != 0 && self.in_cloud(&e.path)).then(|| cloud_of(e.attrs))
    }
}

/// The link Google Drive's own "Kopiuj link" gives: its file id, which Drive for desktop's
/// streamed drive serves as the stream `<path>:user.drive.id`.
// shortcut: the mirrored folder (home\My Drive) has no such stream, so it gets no "Kopiuj link";
// its ids are in DriveFS's mirror_sqlite.db, read them there if mirroring is used
pub(crate) fn google_link(p: &Path) -> Result<String, String> {
    let mut stream = p.as_os_str().to_owned();
    stream.push(":user.drive.id");
    let id = std::fs::read_to_string(stream).map_err(|_| "Google Drive nie podał adresu tego pliku".to_string())?;
    drive_link(id.trim())
}

fn drive_link(id: &str) -> Result<String, String> {
    // "local…": not uploaded yet
    if id.is_empty() || id.starts_with("local") {
        return Err("Plik nie jest jeszcze wysłany na Dysk Google".into());
    }
    Ok(format!("https://drive.google.com/open?id={id}&usp=drive_fs"))
}

pub(crate) fn dir_size(root: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if let Ok(md) = e.metadata() {
                if md.is_dir() {
                    stack.push(e.path());
                } else {
                    total += md.len();
                }
            }
        }
    }
    total
}

/// What the fresh listing `new` takes over from `old`, the rows it replaces after a change by
/// another program: the sizes counted for folders that are still there (Oblicz rozmiary, and
/// "Analiza miejsca" where the index does not reach). Returns the selected rows of expanded
/// folders that are still there, to stay selected: no listing has them, "Analiza miejsca" puts
/// them back from the index.
pub(crate) fn carry_over(old: &[Entry], sel: &HashSet<PathBuf>, new: &mut [Entry]) -> Vec<PathBuf> {
    let sizes: HashMap<&Path, u64> = old.iter().filter(|e| e.dir && e.depth == 0).filter_map(|e| Some((e.path.as_path(), e.size?))).collect();
    for e in new.iter_mut().filter(|e| e.dir && e.size.is_none()) {
        e.size = sizes.get(e.path.as_path()).copied();
    }
    let under = |p: &Path| new.iter().any(|n| n.dir && p.starts_with(&n.path));
    old.iter().filter(|e| e.depth > 0 && sel.contains(&e.path) && under(&e.path)).map(|e| e.path.clone()).collect()
}

impl Tab {
    pub(crate) fn new(path: PathBuf) -> Self {
        let mut t = Tab {
            path,
            back: vec![],
            fwd: vec![],
            all: vec![],
            view: vec![],
            hidden: 0,
            sel: HashSet::new(),
            filter: String::new(),
            search: String::new(),
            search_all: false,
            anchor: None,
            focus: None,
            sort: Col::Name,
            asc: true,
            flat: false,
            found: false,
            found_for: (String::new(), 0),
            found_total: 0,
            biggest: false,
            sized: None,
            sort_before_space: None,
            expanded: HashSet::new(),
            error: None,
            dirty: true,
            prune: false,
            scroll_to: None,
            scroll: (0.0, 0.0),
            side: 0,
            in_arc: false,
            asked: Default::default(),
            shown: false,
            unsent: false,
            asked_at: Instant::now(),
            carry: false,
        };
        t.arrive();
        t
    }

    /// Asks for the folder's rows. Nothing is read here: a worker thread reads them
    /// (App::send_listings) and they come back through App::listed, a share that does not answer
    /// holding up only itself. Until then the list keeps the rows it has.
    pub(crate) fn load(&mut self) {
        // the rows of a results list are not the folder's: none show until the folder's come
        if self.found {
            self.all.clear();
            (self.shown, self.dirty) = (false, true);
        }
        self.found = false;
        self.biggest = false;
        self.carry = false;
        if is_pc(&self.path) {
            self.all.clear();
            self.error = None;
            self.in_arc = false;
            self.dirty = true;
            self.asked.cancel();
            return;
        }
        self.asked.ask();
        (self.unsent, self.asked_at) = (true, Instant::now());
    }

    /// Reads the folder again for a change nobody here asked for (another program's), keeping
    /// what the list knew and a fresh one lacks (`carry_over`).
    pub(crate) fn reload(&mut self) {
        // rows already asked for after a file operation of the app's own start afresh, as asked
        let carry = self.carry || !self.asked.waiting();
        self.load();
        self.carry = carry;
    }

    /// The folder's first rows have not come yet, so the list has none to show.
    pub(crate) fn loading(&self) -> bool {
        self.asked.waiting() && !self.shown
    }

    /// The list shows a folder of an archive (not search results over it).
    pub(crate) fn in_archive(&self) -> bool {
        self.in_arc && !self.found
    }

    pub(crate) fn arrive(&mut self) {
        self.sel.clear();
        self.anchor = None;
        self.focus = None;
        self.flat = false;
        self.found = false;
        // a new folder starts with its tree closed
        self.expanded.clear();
        self.scroll_to = Some(0);
        // the rows were another folder's: they go, with what was to follow them once read again
        self.all.clear();
        (self.error, self.sized, self.shown, self.dirty) = (None, None, false, true);
        self.asked.cancel();
        // by its name until the rows come: a paste meanwhile still asks the disk where it goes (App::run)
        self.in_arc = self.path.ancestors().any(rx_archive::can_extract);
        self.load();
    }

    /// `space`: "Analiza miejsca", where sorting by size mixes folders and files (WizTree's way).
    /// Rows of an expanded folder (depth > 0) stay under it, in the order the tree gave them: the
    /// sorting and the search and name filters work on the folder's own items, each taking its rows along.
    pub(crate) fn refresh_view(&mut self, show_hidden: bool, space: bool) {
        // results of "szukaj wszędzie" already match the query (word by word, not as one string);
        // the biggest files are a list of this folder, searched like it
        let s = if self.found && !self.biggest { String::new() } else { self.search.to_lowercase() };
        if std::mem::take(&mut self.prune) && self.found {
            self.all.retain(|e| e.path.exists());
        }
        self.hidden = self.all.iter().filter(|e| e.depth == 0 && e.attrs & 2 != 0).count();
        let (all, filter) = (&self.all, self.filter.as_str());
        // each row's folder-level item: itself, or the last depth-0 row above it
        let mut head = Vec::with_capacity(all.len());
        for (i, e) in all.iter().enumerate() {
            head.push(if e.depth == 0 || i == 0 { i } else { head[i - 1] });
        }
        let shown = |e: &Entry| show_hidden || e.attrs & 2 == 0;
        let mut v: Vec<usize> = (0..all.len())
            .filter(|&i| {
                let h = &all[head[i]];
                shown(&all[i]) && shown(h) && (s.is_empty() || h.name.to_lowercase().contains(&s)) && glob_any(filter, &h.name)
            })
            .collect();
        let (col, asc) = (self.sort, self.asc);
        let mixed = space && col == Col::Size;
        // text keys once per entry, not twice per comparison (n·log n allocations before)
        let keys: Vec<String> = match col {
            Col::Type => all.iter().map(|e| e.type_name.to_lowercase()).collect(),
            Col::Attr => all.iter().map(|e| fmt_attrs(e.attrs)).collect(),
            _ => vec![],
        };
        v.sort_by(|&a, &b| {
            let (ha, hb) = (head[a], head[b]);
            let (x, y) = (&all[ha], &all[hb]);
            let grouped = if mixed { Ordering::Equal } else { y.dir.cmp(&x.dir) };
            grouped
                .then_with(|| {
                    let o = match col {
                        Col::Name => Ordering::Equal,
                        Col::Size => x.size.cmp(&y.size),
                        Col::Type | Col::Attr => keys[ha].cmp(&keys[hb]),
                        Col::Modified => x.modified.cmp(&y.modified),
                    }
                    .then_with(|| natural_cmp(&x.name, &y.name));
                    if asc { o } else { o.reverse() }
                })
                // the same item: its own row first, then its tree in the order it was built
                .then_with(|| a.cmp(&b))
        });
        self.view = v;
        self.dirty = false;
    }

    pub(crate) fn selected(&self) -> Vec<PathBuf> {
        self.view.iter().map(|&i| &self.all[i].path).filter(|p| self.sel.contains(*p)).cloned().collect()
    }
}

/// Where the whole-drive index waits between searches.
fn index_file() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join(APP_NAME).join("indeks.bin"))
}

/// The drives are being scanned right now. Not `App::index_busy`: that one is cleared in ui(),
/// which does not run for a hidden window.
pub(crate) static INDEX_SCANNING: AtomicBool = AtomicBool::new(false);

/// Most rows a search shows; the status bar tells how many names matched in all.
const FOUND_CAP: usize = 5000;

/// The sorting a folder gets when nothing is remembered for it.
const DEFAULT_SORT: (Col, bool) = (Col::Name, true);
const SORT_COLS: [(Col, &str); 5] = [(Col::Name, "nazwa"), (Col::Size, "rozmiar"), (Col::Type, "typ"), (Col::Modified, "data"), (Col::Attr, "atrybuty")];

/// A folder's key: Windows does not tell "Dane" from "dane".
fn sort_key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

/// Remembered sortings as stored: one folder a line, "column<TAB>asc|desc<TAB>path".
pub(crate) fn sorts_to_text(m: &HashMap<String, (Col, bool)>) -> String {
    let name = |c: Col| SORT_COLS.iter().find(|x| x.0 == c).map_or("nazwa", |x| x.1);
    let mut lines: Vec<String> = m.iter().map(|(p, &(c, asc))| format!("{}\t{}\t{p}", name(c), if asc { "asc" } else { "desc" })).collect();
    lines.sort();
    lines.join("\n")
}

pub(crate) fn sorts_from_text(text: &str) -> HashMap<String, (Col, bool)> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            let name = f.next()?;
            let col = SORT_COLS.iter().find(|x| x.1 == name)?.0;
            let asc = f.next()? == "asc";
            Some((f.next()?.to_string(), (col, asc)))
        })
        .collect()
}

impl App {
    /// Tab `i` sorted as its folder was last sorted (or by name), unless the sorting is held:
    /// "Blokada formatu" keeps it from folder to folder, "Analiza miejsca" sorts by size.
    pub(crate) fn apply_folder_sort(&mut self, i: usize) {
        if self.lock_format || self.space {
            return;
        }
        let t = &mut self.tabs[i];
        (t.sort, t.asc) = self.folder_sorts.get(&sort_key(&t.path)).copied().unwrap_or(DEFAULT_SORT);
        t.dirty = true;
    }

    /// After the sorting of the current folder changed: remembered for it (the default is forgotten).
    pub(crate) fn remember_sort(&mut self) {
        let t = &self.tabs[self.cur];
        if self.space || t.found || is_pc(&t.path) {
            return;
        }
        let (key, now) = (sort_key(&t.path), (t.sort, t.asc));
        if now == DEFAULT_SORT {
            self.folder_sorts.remove(&key);
        } else {
            self.folder_sorts.insert(key, now);
        }
    }

    /// "Szukaj wszędzie" needs the index: read from its file (results at once, from the last scan)
    /// and rescanned in the background when this session has not done so in 10 minutes (or there is
    /// no file to read). Every local drive; network drives are left out. Called when the query
    /// changes, not every frame, so a query left in the box does not keep rescanning.
    pub(crate) fn want_index(&mut self, ctx: &egui::Context) {
        let due = self.index_scanned.is_none_or(|t| t.elapsed().as_secs() > 600);
        if self.index_busy || (self.index.is_some() && !due) {
            return;
        }
        self.index_busy = true;
        // shortcut: a SUBST drive letter is scanned again under its host drive, which doubles its
        // files in results; skip such letters if the owner starts using them
        let roots: Vec<PathBuf> = self.drives.iter().filter(|d| !d.2).map(|d| d.0.clone()).collect();
        let (tx, ctx, have) = (self.tx.clone(), ctx.clone(), self.index.is_some());
        let first = self.index_scanned.is_none();
        std::thread::spawn(move || {
            let file = index_file();
            let loaded = (!have).then(|| file.as_deref().and_then(|f| rx_core::index::Index::load(f).ok())).flatten().map(Arc::new);
            if let Some(ix) = &loaded {
                let _ = tx.send(Msg::Index(ix.clone()));
            }
            ctx.request_repaint();
            // a file saved in the last 10 minutes (a session just closed, an update) stands for this
            // session's scan: no 5–25 s of disk work at every start
            let age = file.as_deref().and_then(|f| f.metadata().ok()?.modified().ok()?.elapsed().ok());
            let fresh = if first && loaded.is_some() && age.is_some_and(|a| a.as_secs() < 600) {
                loaded
            } else {
                // nothing to read (first use, a lost or broken file) also means a scan
                (due || !(have || loaded.is_some())).then(|| {
                    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
                    let t0 = Instant::now();
                    INDEX_SCANNING.store(true, AO::SeqCst);
                    // a rescan nobody waits for (the last index is on screen) yields to everything
                    // else, here and in every reader; a first scan with nothing to show does not
                    let quiet = have || loaded.is_some();
                    let lower = || {
                        if quiet {
                            sys::background_thread();
                        }
                    };
                    lower();
                    let ix = rx_core::index::Index::scan(&roots, threads, lower);
                    update::log(format!("indeks: {} pozycji w {:.1} s{}", ix.len(), t0.elapsed().as_secs_f64(), if quiet { " (w tle)" } else { "" }));
                    if let Some(f) = &file {
                        let _ = f.parent().map(std::fs::create_dir_all);
                        let _ = ix.save(f);
                    }
                    INDEX_SCANNING.store(false, AO::SeqCst);
                    Arc::new(ix)
                })
            };
            let _ = tx.send(Msg::Indexed(fresh));
            ctx.request_repaint();
        });
    }

    /// Puts the results for each tab's search box in that tab while its "szukaj wszędzie" is on
    /// (only when its query or the index changed) and brings the folder back when it is off. The
    /// index itself is dropped 5 minutes after the last query (App::logic), so nothing stays in RAM
    /// between searches; results already shown stay.
    pub(crate) fn search_everywhere(&mut self, ctx: &egui::Context) {
        for i in 0..self.tabs.len() {
            self.search_everywhere_in(i, ctx);
        }
    }

    fn search_everywhere_in(&mut self, i: usize, ctx: &egui::Context) {
        let t = &self.tabs[i];
        let q = t.search.trim();
        if !t.search_all || q.is_empty() {
            // load() turns the results back into the folder (the biggest files are not a search)
            if t.found && !t.biggest {
                self.tabs[i].load();
            }
            return;
        }
        let gen = self.index_gen;
        if t.found && !t.biggest && t.found_for.0 == q && t.found_for.1 == gen {
            return;
        }
        let new_query = !t.found || t.biggest || t.found_for.0 != q;
        let q = q.to_string();
        if new_query {
            self.index_used = Instant::now();
        }
        if new_query || self.index.is_none() {
            self.want_index(ctx);
        }
        let t = &mut self.tabs[i];
        (t.found, t.biggest, t.error, t.dirty) = (true, false, None, true);
        // the folder's rows, if on their way, are not waited for: the results take the list
        t.asked.cancel();
        // a new index under the same query (the rescan ending) keeps the selection and the place
        if new_query {
            t.sel.clear();
            (t.anchor, t.focus, t.scroll_to) = (None, None, Some(0));
        }
        t.found_for = (q, gen);
        // no index yet (the very first scan): an empty list that says so, filled when it comes
        let Some(ix) = self.index.clone() else {
            if new_query {
                t.all.clear();
                t.found_total = 0;
            }
            return;
        };
        let (hits, total) = ix.search(&t.found_for.0, FOUND_CAP);
        t.all = hits.iter().map(|&i| hit_entry(ix.hit(i as usize))).collect();
        t.found_total = total;
    }

    /// "Analiza miejsca" on or off. On: the index is read (rescanned when due) and every tab sorts
    /// biggest first; off: each tab gets its own sorting back and lists its folder again.
    pub(crate) fn set_space(&mut self, on: bool, ctx: &egui::Context) {
        self.space = on;
        if on {
            self.index_used = Instant::now();
            self.want_index(ctx);
            return;
        }
        for t in &mut self.tabs {
            if let Some((s, a)) = t.sort_before_space.take() {
                (t.sort, t.asc) = (s, a);
            }
            // the folder sizes and the biggest files were the mode's; search results stay
            if !t.found || t.biggest {
                t.load();
            }
        }
        // a tab may have moved to another folder while the mode was on: it gets that folder's sorting
        for i in 0..self.tabs.len() {
            if !self.tabs[i].found {
                self.apply_folder_sort(i);
            }
        }
        (self.space_data, self.tmap) = (None, None);
        self.forgotten.clear();
    }

    /// Sizes and folder contents worked out from the current index (once per index; ~50 ms and
    /// ~50 MB for 3 million entries), None while the index is not there.
    pub(crate) fn space_data(&mut self) -> Option<&SpaceData> {
        let ix = self.index.clone()?;
        if self.space_data.as_ref().is_none_or(|d| d.0 != self.index_gen) {
            let mut d = SpaceData { totals: ix.totals(), kids: ix.kids(), gone: HashSet::new() };
            // a deletion stays off a newer index too while the scan behind it still saw the thing
            // (it started before the deletion); once an index lacks it, it is let go
            self.forgotten.retain(|p| !p.exists() && ix.find_in(&d.kids, p).is_some());
            for p in &self.forgotten {
                forget(&ix, &mut d, p);
            }
            self.space_data = Some((self.index_gen, d));
        }
        self.space_data.as_ref().map(|d| &d.1)
    }

    /// Things just deleted leave the sizes at once: their bytes come off every folder above them,
    /// so the list and the treemap do not wait for the rescan the deletion starts.
    pub(crate) fn space_forget(&mut self, paths: &[PathBuf]) {
        self.forgotten.extend(paths.iter().cloned());
        let Some(ix) = self.index.clone() else { return };
        self.space_data();
        let Some((_, d)) = self.space_data.as_mut() else { return };
        for p in paths {
            forget(&ix, d, p);
        }
        self.space_ver += 1;
        for t in &mut self.tabs {
            t.sized = None;
        }
    }

    /// While "Analiza miejsca" is on: every folder in a list gets its whole size, from the index
    /// (one pass sums it for all folders) or, where the index does not reach (network drives,
    /// folders made since the scan), counted on the spot; lists sort biggest first, and an
    /// expanded folder brings its own items (from the index) under it, as WizTree's tree. Runs
    /// every frame and does work only for a list not built from the current index yet.
    pub(crate) fn space_sizes(&mut self, ctx: &egui::Context) {
        if !self.space {
            return;
        }
        // the index stays in memory while the mode is on (it is dropped 5 min after, App::logic)
        self.index_used = Instant::now();
        // after a file operation the drives are scanned again, but not more often than every
        // 2 minutes (deletions are taken off at once, space_forget)
        if self.space_stale && !self.index_busy && self.index_scanned.is_none_or(|t| t.elapsed().as_secs() > 120) {
            self.space_stale = false;
            self.index_scanned = None;
        }
        if self.index.is_none() || self.index_scanned.is_none() {
            self.want_index(ctx);
        }
        for t in &mut self.tabs {
            if t.sort_before_space.is_none() {
                t.sort_before_space = Some((t.sort, t.asc));
                (t.sort, t.asc, t.dirty) = (Col::Size, false, true);
            }
        }
        let gen = self.index_gen;
        self.space_data();
        let local: Vec<PathBuf> = self.drives.iter().filter(|d| !d.2).map(|d| d.0.clone()).collect();
        let mut live = vec![];
        for t in &mut self.tabs {
            // rows on their way: the sizes go on those, not on the ones they replace
            if t.found || t.flat || t.in_arc || is_pc(&t.path) || t.asked.waiting() || t.sized.is_some_and(|g| g == gen || g == u64::MAX) {
                continue;
            }
            let indexed = local.iter().any(|r| t.path.starts_with(r));
            // the tree's rows of the last build go; the folder's own items (listed live) stay
            t.all.retain(|e| e.depth == 0);
            t.dirty = true;
            let here = match (&self.index, &self.space_data) {
                (Some(ix), Some((_, d))) if indexed => {
                    t.sized = Some(gen);
                    ix.find_in(&d.kids, &t.path).map(|id| (ix, d, id))
                }
                // the index is on its way: its sizes come in a moment
                _ if indexed => continue,
                _ => {
                    t.sized = Some(u64::MAX);
                    None
                }
            };
            let by_name: HashMap<String, u32> = here
                .map(|(ix, d, id)| d.kids.of(id).iter().filter(|k| !d.gone.contains(k)).map(|&k| (ix.name(k as usize).to_lowercase(), k)).collect())
                .unwrap_or_default();
            for mut e in std::mem::take(&mut t.all) {
                let k = by_name.get(&e.name.to_lowercase()).copied();
                if e.dir {
                    match (k, here) {
                        (Some(k), Some((_, d, _))) => e.size = Some(d.totals[k as usize]),
                        // counted before and kept over a reload (carry_over): not counted again
                        _ if e.size.is_some() => {}
                        _ => live.push(e.path.clone()),
                    }
                }
                let open = e.dir && t.expanded.contains(&e.path);
                t.all.push(e);
                if let (true, Some(k), Some((ix, d, _))) = (open, k, here) {
                    tree_rows(ix, d, k, 1, &t.expanded, &mut t.all);
                }
            }
        }
        self.count_sizes(live, ctx);
        if self.shot_biggest && self.index.is_some() {
            self.shot_biggest = false;
            self.pending.push(Act::Biggest);
        }
    }

    /// The biggest files under the current folder at any depth, as a list in its place (Back or
    /// the button again bring the folder back).
    pub(crate) fn show_biggest(&mut self, ctx: &egui::Context) {
        let Some(ix) = self.index.clone() else {
            self.msg = Some("Wczytuję indeks dysków, spróbuj za chwilę".into());
            self.want_index(ctx);
            return;
        };
        let path = self.tabs[self.cur].path.clone();
        let id = match self.space_data() {
            Some(d) => ix.find_in(&d.kids, &path),
            None => ix.find(&path),
        };
        let Some(id) = id else {
            self.msg = Some("Tego folderu nie ma w indeksie dysków (np. dysk sieciowy albo folder nowszy niż indeks)".into());
            return;
        };
        let gone = self.space_data.as_ref().map(|d| &d.1.gone);
        let (ids, total) = ix.largest(Some(id), BIGGEST_CAP + gone.map_or(0, |g| g.len()));
        let t = &mut self.tabs[self.cur];
        t.all = ids.iter().filter(|i| gone.is_none_or(|g| !g.contains(i))).take(BIGGEST_CAP).map(|&i| hit_entry(ix.hit(i as usize))).collect();
        (t.found, t.biggest, t.found_total, t.error, t.dirty) = (true, true, total, None, true);
        t.asked.cancel();
        t.sel.clear();
        (t.anchor, t.focus, t.scroll_to) = (None, None, Some(0));
        (t.sort, t.asc) = (Col::Size, false);
    }
}

/// Takes `p` off the sizes: its bytes come off every folder above it, and it leaves the tree and
/// the treemap. Once per entry, however often it is asked.
fn forget(ix: &rx_core::index::Index, d: &mut SpaceData, p: &Path) {
    let Some(id) = ix.find_in(&d.kids, p) else { return };
    if !d.gone.insert(id) {
        return;
    }
    let gone = d.totals[id as usize];
    let mut at = ix.parent_of(id);
    while let Some(a) = at {
        d.totals[a as usize] = d.totals[a as usize].saturating_sub(gone);
        at = ix.parent_of(a);
    }
}

/// The rows under an expanded folder `dir`, biggest first, and under its expanded folders in turn.
fn tree_rows(ix: &rx_core::index::Index, d: &SpaceData, dir: u32, depth: u8, open: &HashSet<PathBuf>, out: &mut Vec<Entry>) {
    let mut kids: Vec<u32> = d.kids.of(dir).iter().copied().filter(|k| !d.gone.contains(k)).collect();
    kids.sort_unstable_by(|a, b| d.totals[*b as usize].cmp(&d.totals[*a as usize]));
    for k in kids {
        let mut e = hit_entry(ix.hit(k as usize));
        if e.dir {
            e.size = Some(d.totals[k as usize]);
        }
        e.depth = depth;
        let deeper = e.dir && open.contains(&e.path);
        out.push(e);
        if deeper {
            tree_rows(ix, d, k, depth.saturating_add(1), open, out);
        }
    }
}

/// How many of the biggest files "Analiza miejsca" lists.
const BIGGEST_CAP: usize = 1000;

/// What "Analiza miejsca" works out from one index: bytes under every entry, every folder's
/// entries, and the entries deleted since (out of the tree, the treemap and the biggest files).
pub(crate) struct SpaceData {
    pub(crate) totals: Vec<u64>,
    pub(crate) kids: rx_core::index::Kids,
    pub(crate) gone: HashSet<u32>,
}

/// A list row for an index entry: the name alone (sorting, the filter, type-ahead); a results
/// list adds the folder.
fn hit_entry(h: rx_core::index::Hit) -> Entry {
    let name = h.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Entry {
        recent: ago(h.modified).is_some(),
        kind: kind_of(&name, h.dir),
        type_name: type_of(&name, h.dir),
        size: h.size,
        modified: h.modified,
        attrs: h.attrs,
        name,
        path: h.path,
        dir: h.dir,
        depth: 0,
    }
}

pub(crate) fn select_at(t: &mut Tab, i: usize, shift: bool, ctrl: bool) {
    let p = t.all[t.view[i]].path.clone();
    if shift {
        let a = t.anchor.unwrap_or(i);
        let (lo, hi) = (a.min(i), a.max(i));
        t.sel = (lo..=hi).map(|k| t.all[t.view[k]].path.clone()).collect();
    } else if ctrl {
        if !t.sel.remove(&p) {
            t.sel.insert(p.clone());
        }
        t.anchor = Some(i);
    } else {
        t.sel = [p.clone()].into();
        t.anchor = Some(i);
    }
    t.focus = Some(p);
}

/// Drag and drop: the paths being dragged out of a list (the grabbed one first), the tab they
/// came from and the grabbed item's kind, for the card under the pointer.
pub(crate) struct Dragged(pub(crate) Vec<PathBuf>, pub(crate) usize, pub(crate) Kind);

/// Drag and drop: a tab being dragged by its header (dropped on the right, it splits the view).
pub(crate) struct DraggedTab(pub(crate) usize);

/// Drag and drop: a folder of the address bar; it only bookmarks (dropped on Zakładki), never copies.
pub(crate) struct DraggedPath(pub(crate) PathBuf);

/// As in Explorer: a drop moves on the same drive and copies to another (and out of an archive);
/// Ctrl copies, Shift moves.
pub(crate) fn drop_moves(paths: &[PathBuf], into: &Path, mods: egui::Modifiers) -> bool {
    let drive = |p: &Path| p.components().next().map(|c| c.as_os_str().to_string_lossy().to_lowercase());
    mods.shift || (!mods.ctrl && paths.first().is_some_and(|p| !archive::is_inside(p) && drive(p) == drive(into)))
}

/// Whether a drop moves. Through a system drag (`sys_drag`) Shift is asked of Windows, as another
/// window had the keyboard: a copy, as the cursor showed during it (winit's drop target always
/// offers a copy), also for a drag of ours that came back; Shift moves.
pub(crate) fn drop_moves_now(d: &Dragged, into: &Path, sys_drag: bool, mods: egui::Modifiers) -> bool {
    if !sys_drag {
        return drop_moves(&d.0, into, mods);
    }
    sys::clip::shift_ctrl().0
}

/// A clipboard key press as egui-winit reads it on Windows, given as its Ctrl+letter (C copies,
/// X cuts, V pastes): also Ctrl+Insert, Shift+Delete, Shift+Insert and the keyboard's own keys.
pub(crate) fn clip_key(m: egui::Modifiers, key: egui::Key) -> Option<egui::Key> {
    use egui::Key::*;
    match key {
        C | X | V if m.ctrl && !m.shift && !m.alt => Some(key),
        Copy => Some(C),
        Cut => Some(X),
        Paste => Some(V),
        Insert if m.ctrl => Some(C),
        Insert if m.shift => Some(V),
        Delete if m.shift => Some(X),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_sortings_survive_saving() {
        let m: HashMap<String, (Col, bool)> =
            [(r"c:\users\ala\pobrane".to_string(), (Col::Modified, false)), (r"d:\zdjęcia\2026".to_string(), (Col::Size, true))].into();
        let back = sorts_from_text(&sorts_to_text(&m));
        assert_eq!(back.len(), 2);
        assert!(back[r"c:\users\ala\pobrane"] == (Col::Modified, false));
        assert!(back[r"d:\zdjęcia\2026"] == (Col::Size, true));
        // a damaged or unknown line is skipped, not misread
        assert!(sorts_from_text("kolor\tasc\tc:\\x\nrozmiar").is_empty());
        assert_eq!(sort_key(Path::new(r"C:\Dane")), sort_key(Path::new(r"c:\dane")));
    }

    #[test]
    fn clipboard_keys_are_read_as_windows_has_them() {
        use egui::{Key, Modifiers as M};
        assert_eq!(clip_key(M::CTRL, Key::C), Some(Key::C));
        assert_eq!(clip_key(M::NONE, Key::Paste), Some(Key::V));
        assert_eq!(clip_key(M::SHIFT, Key::Delete), Some(Key::X));
        assert_eq!(clip_key(M::SHIFT, Key::Insert), Some(Key::V));
        assert_eq!(clip_key(M::CTRL, Key::Insert), Some(Key::C));
        // Ctrl+Shift+C stays a key (Kopiuj ścieżkę), plain Delete stays the delete
        assert_eq!(clip_key(M::CTRL | M::SHIFT, Key::C), None);
        assert_eq!(clip_key(M::NONE, Key::Delete), None);
    }

    #[test]
    fn a_reload_keeps_counted_sizes_and_the_selection_inside_expanded_folders() {
        let row = |path: &str, dir: bool, size: Option<u64>, depth: u8| Entry {
            name: path.into(),
            path: path.into(),
            dir,
            size,
            modified: None,
            attrs: 0,
            kind: Kind::File,
            type_name: "".into(),
            recent: false,
            depth,
        };
        let old = [
            row(r"C:\a", true, Some(700), 0),
            row(r"C:\a\w.txt", false, Some(5), 1),
            row(r"C:\b", true, Some(9), 0),
            row(r"C:\b\x.txt", false, Some(5), 1),
            row(r"C:\c", true, None, 0),
        ];
        let sel: HashSet<PathBuf> = [r"C:\a\w.txt".into(), r"C:\b\x.txt".into(), r"C:\b".into()].into();
        // b is gone (with the selected file in it), d is new, and a file has taken c's place
        let mut new = [row(r"C:\a", true, None, 0), row(r"C:\c", false, Some(1), 0), row(r"C:\d", true, None, 0)];
        let deep = carry_over(&old, &sel, &mut new);
        assert_eq!(new.iter().map(|e| e.size).collect::<Vec<_>>(), [Some(700), Some(1), None]);
        assert_eq!(deep, [PathBuf::from(r"C:\a\w.txt")]);
    }

    #[test]
    fn a_junction_opens_as_a_folder() {
        let dir = std::env::temp_dir().join(format!("rx-lacze-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("cel")).unwrap();
        std::fs::write(dir.join("plik.txt"), "x").unwrap();
        let made = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(dir.join("lacze")).arg(dir.join("cel")).output().unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        let rows = list_dir(&dir, false).unwrap();
        let opens = |name: &str| rows.iter().find(|e| e.name == name).unwrap().opens_as_folder();
        assert!(opens("lacze") && opens("cel") && !opens("plik.txt"));
        // the junction first, so that nothing is removed through it
        std::fs::remove_dir(dir.join("lacze")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cloud_file_tells_where_it_is() {
        // as OneDrive's files read on the test laptop: hydrated, and a placeholder
        assert_eq!(cloud_of(0x420), Cloud::Local);
        assert_eq!(cloud_of(0x401620), Cloud::Online);
        assert_eq!(cloud_of(0x80420), Cloud::Pinned);
        // pinned but not downloaded yet is still in the cloud
        assert_eq!(cloud_of(0x481620), Cloud::Online);
        // as this program reads the same files: Windows hides the reparse and offline flags from it
        assert_eq!(cloud_of(0x20), Cloud::Local);
        assert_eq!(cloud_of(0x500020), Cloud::Online);
        // Drive's own link form; a file still waiting to upload has a local id
        assert_eq!(drive_link("1AbC_x").as_deref(), Ok("https://drive.google.com/open?id=1AbC_x&usp=drive_fs"));
        assert!(drive_link("local-123").is_err());
        assert!(drive_link("").is_err());
    }

    #[test]
    fn a_drop_moves_on_the_same_drive_and_copies_to_another() {
        let (none, ctrl, shift) = (egui::Modifiers::NONE, egui::Modifiers::CTRL, egui::Modifiers::SHIFT);
        let from = [PathBuf::from(r"C:\Dane\a.txt")];
        assert!(drop_moves(&from, Path::new(r"c:\inne"), none));
        assert!(!drop_moves(&from, Path::new(r"D:\inne"), none));
        assert!(!drop_moves(&from, Path::new(r"C:\inne"), ctrl));
        assert!(drop_moves(&from, Path::new(r"D:\inne"), shift));
    }
}
