//! App state changes: start-up and restore, navigation, every menu/keyboard action, background work and updates.

use super::*;

impl App {
    /// `chan`: the channel main() made and gave to read_drives while the window was being made.
    pub(crate) fn new(ctx: &egui::Context, start: PathBuf, shot: Option<PathBuf>, chan: Option<(Sender<Msg>, Receiver<Msg>)>) -> Self {
        setup_style(ctx);
        // screenshot runs get no tray icon
        let tray = if shot.is_none() { make_tray(ctx) } else { (None, channel().1) };
        // the first start after an update says so from the tray, with what changed
        if let (Some(t), Some(notes)) = (&tray.0, update::just_updated()) {
            let text = if notes.is_empty() { t!("tray.updated_text").to_string() } else { notes };
            sys::tray_balloon(t.window_handle(), &t!("tray.updated", app = APP_NAME, v = update::version().0), &text);
        }
        // a second start of the window (the DirectX 12 fallback) makes its own
        let (tx, rx) = chan.unwrap_or_else(|| {
            let chan = channel();
            read_drives(chan.0.clone(), true);
            chan
        });
        let thumb_req = Arc::new(rx_core::Requests::<PathBuf>::new());
        {
            let (req, tx, ctx) = (thumb_req.clone(), tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                sys::init_com();
                loop {
                    let p = req.take();
                    let img = sys::shell_thumbnail(&p, 128, false).map(|(w, h, px)| ColorImage::from_rgba_premultiplied([w, h], &px));
                    if tx.send(Msg::Thumb(p, img)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }
        // one thread hands out the preview builds: a lone request at once, of a burst (a held arrow
        // key) only the newest once the selection rests for 120 ms. Each build has a thread of its
        // own and is waited for 2 s at most, so one stuck on a drive that does not answer does not
        // hold back the next file's; its result still shows if it comes while the file is selected
        let preview_req = Arc::new(rx_core::Latest::<(PathBuf, u32, bool)>::new());
        {
            let (req, tx, ctx) = (preview_req.clone(), tx.clone(), ctx.clone());
            std::thread::spawn(move || loop {
                let (p, px, read) = req.take(std::time::Duration::from_millis(120));
                let (tx, ctx, (done, built)) = (tx.clone(), ctx.clone(), channel());
                std::thread::spawn(move || {
                    sys::init_com();
                    // a panic in a decoder must not leave the preview "loading"
                    let raw = std::panic::catch_unwind(|| build_preview(&p, px, read)).unwrap_or_else(|_| Raw::Nothing(t!("actions.no_preview").into()));
                    let _ = done.send(tx.send(Msg::Preview(p, raw)).is_ok());
                    ctx.request_repaint();
                });
                // false: the app is gone
                if built.recv_timeout(std::time::Duration::from_secs(2)) == Ok(false) {
                    break;
                }
            });
        }
        // a drive that comes or goes (a pendrive) shows without F5: the letters in use are one cheap
        // call every 3 s; names are read only after a change and only of the drives that came, here
        // and not in the window's thread, as a network drive that does not answer takes seconds.
        // The first list is these letters under their own names, which no drive is asked for;
        // read_drives brings the names Explorer shows
        let mut mask = sys::drive_mask();
        let drives = sys::drive_letters(mask).into_iter().map(|(root, net)| (root.clone(), root.display().to_string(), net)).collect();
        let drives_unread = mask;
        {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                sys::init_com();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    let now = sys::drive_mask();
                    if now == mask {
                        continue;
                    }
                    let changed = now ^ mask;
                    mask = now;
                    if tx.send(Msg::Drives(changed, sys::drives_of(changed & now))).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }
        // changes other programs make to the folders on screen come as messages (App::watch_shown)
        let watcher = {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            sys::watch::Watcher::new(move |dir, gone| {
                let _ = tx.send(Msg::Folder(dir.to_path_buf(), gone));
                ctx.request_repaint();
            })
        };
        // a screenshot run shows no bookmarks of the machine's owner (README pictures show nothing of it)
        let bookmarks = bookmarks_file()
            .filter(|_| shot.is_none())
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from).collect())
            .unwrap_or_default();
        App {
            tabs: vec![Tab::new(start)],
            cur: 0,
            filter_open: false,
            index: None,
            index_busy: false,
            index_scanned: None,
            index_used: Instant::now(),
            index_gen: 0,
            space: false,
            space_data: None,
            space_ver: 0,
            tmap: None,
            tmap_menu: vec![],
            forgotten: vec![],
            space_stale: false,
            shot_biggest: false,
            kbd_focus: false,
            typed: String::new(),
            typed_at: 0.0,
            told_tray: false,
            inactive_pane: false,
            tree_at: PathBuf::new(),
            tree_full: false,
            tree_focus: None,
            tree_rows: vec![],
            tree_scroll: false,
            want_menu: None,
            menu_at: None,
            menus: [egui::Id::NULL; 4],
            about: false,
            ask_pw: None,
            vt: HashMap::new(),
            ask_vt_key: None,
            pw_queue: Default::default(),
            ask_pack: None,
            ask: None,
            arc_watch: vec![],
            pack_kind: "zip",
            pack_opts: rx_archive::PackOpts::default(),
            show_hidden: false,
            theme: egui::ThemePreference::System,
            lock_format: false,
            folder_sorts: HashMap::new(),
            view: ViewMode::Details,
            show_preview: true,
            preview_px: 1000,
            preview_req_px: 0,
            preview_details_h: 220.0,
            preview_zoom: (PathBuf::new(), 1.0),
            drop_on_row: false,
            drop_target: None,
            tab_target: None,
            split: false,
            other: 0,
            pc: None,
            pc_asked: false,
            // Atrybuty hidden, as in Explorer (a lone "a" on nearly every row); the header's
            // right-click menu shows it
            widths: [DEFAULT_W[0], DEFAULT_W[1], DEFAULT_W[2], 0.0],
            name_width: 0.0,
            names: HashMap::new(),
            tree_open: [PathBuf::from("::home"), PathBuf::from("::zakladki"), PathBuf::from("::pc")].into_iter().collect(),
            tree_kids: HashMap::new(),
            tree_asked: HashMap::new(),
            sizing: HashSet::new(),
            tree_dim: HashSet::new(),
            bookmarks,
            drives,
            drives_unread,
            // the places of Widok główny and the cloud folders come from read_drives
            known: vec![],
            places_read: false,
            clouds: vec![],
            clip: vec![],
            cut: false,
            clip_seq: sys::clip::seq(),
            clip_sys: false,
            clip_files: sys::clip::has_files(),
            pasting: None,
            ext: None,
            drag_out_done: false,
            drag_out_next: None,
            dragged_out: None,
            shell_menu_next: None,
            undo: Default::default(),
            rename: None,
            addr_edit: None,
            msg: None,
            msg_at: None,
            pending: vec![],
            tx,
            rx,
            thumb_req,
            thumbs: HashMap::new(),
            preview: None,
            preview_req,
            disk: None,
            disk_asked: None,
            title: String::new(),
            last_check: Instant::now(),
            watcher,
            watched: vec![],
            unsaved: None,
            shot,
            frames: 0,
            shot_first: None,
            shot_from: None,
            shot_done: Arc::new(AtomicBool::new(false)),
            shot_after: None,
            shot_idle: false,
            shot_scroll: None,
            shot_type: None,
            zoom: 1.0,
            logo: ctx.load_texture("ikona", ColorImage::from_rgba_unmultiplied([64, 64], &icon::rgba(64)), Default::default()),
            busy: Arc::new(AtomicUsize::new(0)),
            ops: Default::default(),
            update_rx: None,
            update_staged: None,
            last_input: 0.0,
            started: false,
            _tray: tray.0,
            tray_rx: tray.1,
            quitting: false,
            hidden: false,
            focus_pending: false,
            hwnd: 0,
            min_ppp: 0.0,
            last_window: None,
            minimized_since: None,
            trimmed: false,
            eco: false,
            restore_maximized: false,
        }
    }

    /// Restores what save() wrote: zoom, columns, view, panels, split and open tabs.
    /// Window position/size and panel widths come back through eframe itself.
    pub(crate) fn restore(&mut self, ctx: &egui::Context, text: &str, keep_start: bool) {
        let kv: HashMap<&str, &str> = text.lines().filter_map(|l| l.split_once('=')).collect();
        let num = |k: &str| kv.get(k).and_then(|v| v.parse::<f32>().ok());
        if let Some(z) = num("zoom").filter(|z| (0.5..=3.0).contains(z)) {
            ctx.set_zoom_factor(z);
            self.zoom = z;
        }
        if let Some(w) = kv.get("widths") {
            let v: Vec<f32> = w.split(',').filter_map(|x| x.parse().ok()).collect();
            if v.len() == 4 {
                // 0 is a column hidden from the header's menu
                self.widths = [v[0], v[1], v[2], v[3]].map(|x| if x <= 0.0 { 0.0 } else { x.clamp(50.0, 600.0) });
            }
        }
        if let Some(n) = num("namew") {
            self.name_width = if n > 0.0 { n.clamp(MIN_NAME_W, 2000.0) } else { 0.0 };
        }
        self.view = match num("view") {
            Some(1.0) => ViewMode::Thumbs,
            Some(2.0) => ViewMode::DetailsThumbs,
            _ => ViewMode::Details,
        };
        self.theme = match num("theme") {
            Some(1.0) => egui::ThemePreference::Light,
            Some(2.0) => egui::ThemePreference::Dark,
            _ => egui::ThemePreference::System,
        };
        ctx.set_theme(self.theme);
        self.show_preview = num("preview") != Some(0.0);
        self.show_hidden = num("hidden") == Some(1.0);
        self.tree_full = num("fulltree") == Some(1.0);
        self.told_tray = num("traynote") == Some(1.0);
        if !keep_start {
            let sides: Vec<u8> = kv.get("sides").map(|v| v.split(',').map(|x| (x == "1") as u8).collect()).unwrap_or_default();
            // not asked of the disk here (a share that does not answer would hold the window back):
            // a folder that is gone says so in its list
            let tabs: Vec<(PathBuf, u8)> = kv
                .get("tabs")
                .map(|t| t.split('|').map(PathBuf::from).zip(sides.iter().copied().chain(std::iter::repeat(0))).filter(|(p, _)| !p.as_os_str().is_empty()).collect())
                .unwrap_or_default();
            if !tabs.is_empty() {
                self.tabs = tabs
                    .into_iter()
                    .map(|(p, side)| {
                        let mut t = Tab::new(p);
                        t.side = side;
                        t
                    })
                    .collect();
                self.cur = (num("cur").unwrap_or(0.0) as usize).min(self.tabs.len() - 1);
                self.other = (num("other").unwrap_or(0.0) as usize).min(self.tabs.len() - 1);
            }
        }
        self.split = num("split") == Some(1.0);
        self.fix_panes();
    }

    /// The tab of `side` nearest to index `near` (the one before it, else after).
    pub(crate) fn last_on(&self, side: u8, near: usize) -> Option<usize> {
        let on: Vec<usize> = (0..self.tabs.len()).filter(|&k| self.tabs[k].side == side).collect();
        on.iter().rev().find(|&&k| k < near).or(on.first()).copied()
    }

    /// Keeps cur valid and, in split view, `other` on the opposite pane; ends the split when a pane is empty.
    pub(crate) fn fix_panes(&mut self) {
        self.cur = self.cur.min(self.tabs.len() - 1);
        if self.split {
            let side = 1 - self.tabs[self.cur].side;
            if self.other >= self.tabs.len() || self.tabs[self.other].side != side {
                match self.last_on(side, self.tabs.len()) {
                    Some(o) => self.other = o,
                    None => self.split = false,
                }
            }
        }
    }

    pub(crate) fn tab(&mut self) -> &mut Tab {
        &mut self.tabs[self.cur]
    }

    pub(crate) fn dname(&mut self, p: &Path) -> String {
        // first: its name follows the language, and "::pc" has a parent ("") like a folder
        if is_pc(p) {
            return t!("pc.this").into();
        }
        if let Some(n) = self.names.get(p) {
            return n.clone();
        }
        let own = || p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
        // a drive's name comes with the drive list, read off this thread
        if p.parent().is_none() {
            return self.drives.iter().find(|d| d.0 == p).map_or_else(own, |d| d.1.clone());
        }
        // the shell reads the folder for its name; place() keeps network paths off it
        let n = place(p);
        self.names.insert(p.to_path_buf(), n.clone());
        n
    }

    pub(crate) fn display_path(&mut self, p: &Path) -> String {
        if is_pc(p) {
            return t!("pc.this").into();
        }
        let mut parts: Vec<String> = vec![];
        for a in p.ancestors() {
            if a.parent().is_none() {
                parts.push(a.display().to_string().trim_end_matches('\\').to_string());
            } else {
                parts.push(self.dname(a));
            }
        }
        parts.reverse();
        parts.join("\\")
    }

    pub(crate) fn tab_label(&mut self, i: usize) -> String {
        let p = self.tabs[i].path.clone();
        if is_pc(&p) {
            return t!("pc.this").into();
        }
        // just the folder name, as in Explorer; the drive only tells apart two tabs of the same name
        let name = self.dname(&p);
        let others: Vec<PathBuf> = self.tabs.iter().map(|t| t.path.clone()).filter(|o| *o != p && !is_pc(o)).collect();
        if p.parent().is_some() && others.iter().any(|o| self.dname(o) == name) {
            let drive = p.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
            format!("{name} ({drive})")
        } else {
            name
        }
    }

    /// The folders in `p` as last read. They are read on a thread of its own when not asked
    /// about since the last change on disk (a network folder may take seconds), and come as
    /// Msg::Kids; until then the tree shows what it had.
    pub(crate) fn kids(&mut self, p: &Path, ctx: &egui::Context) -> Vec<PathBuf> {
        if !self.tree_asked.contains_key(p) {
            let number = self.tree_asked.entry(p.to_path_buf()).or_default().ask();
            // hidden and system folders only with Ctrl+H, as the list shows hidden files
            let (dir, show_hidden, tx, ctx) = (p.to_path_buf(), self.show_hidden, self.tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let mut dim = Vec::new();
                let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| {
                        let m = e.metadata().ok().filter(|m| m.is_dir())?;
                        let hidden = m.file_attributes() & 6 != 0;
                        if hidden && !show_hidden {
                            return None;
                        }
                        if hidden {
                            dim.push(e.path());
                        }
                        Some(e.path())
                    })
                    .collect();
                v.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
                let _ = tx.send(Msg::Kids(number, dir, v, dim));
                ctx.request_repaint();
            });
        }
        self.tree_kids.get(p).cloned().unwrap_or_default()
    }

    pub(crate) fn save_bookmarks(&self) {
        // nor does it write them: its empty list would replace the real one
        if self.shot.is_some() {
            return;
        }
        if let Some(f) = bookmarks_file() {
            let _ = std::fs::create_dir_all(f.parent().unwrap());
            let text: Vec<String> = self.bookmarks.iter().map(|p| p.display().to_string()).collect();
            if let Err(e) = std::fs::write(f, text.join("\n")) {
                eprintln!("zakładki: {e}");
            }
        }
    }

    pub(crate) fn navigate(&mut self, p: PathBuf) {
        let t = self.tab();
        if p == t.path {
            return;
        }
        let old = std::mem::replace(&mut t.path, p);
        t.back.push(old);
        t.fwd.clear();
        // opening a folder (also one found everywhere) goes back to searching where you are
        (t.search_all, t.search) = (false, String::new());
        t.arrive();
        self.apply_folder_sort(self.cur);
        self.pc_asked = false;
        self.addr_edit = None;
        self.rename = None;
        // thumbnails (64 KB each) are kept only for folders open in a tab; they used to pile up
        // for every folder ever shown
        let open: HashSet<&Path> = self.tabs.iter().map(|t| t.path.as_path()).collect();
        self.thumbs.retain(|p, _| p.ancestors().skip(1).any(|d| open.contains(d)));
    }

    pub(crate) fn after_fs_change(&mut self) {
        for t in &mut self.tabs {
            if t.found {
                // the index is older than this change: results that are gone (deleted, moved,
                // renamed) leave the list instead of coming back from it, at the next refresh_views
                // (callers run mid-frame, e.g. a rename committed while the rows are drawn)
                (t.prune, t.dirty) = (true, true);
                wake();
            } else {
                t.load();
            }
        }
        // the tree reads the folders it draws again, showing what it had until they answer; those
        // not drawn since the last change are forgotten
        if !self.tree_asked.is_empty() {
            self.tree_kids.retain(|p, _| self.tree_asked.contains_key(p));
            self.tree_asked.clear();
        }
        // folder sizes come from the index, which is older than this change now
        self.space_stale = self.space;
        // the status bar asks for the free space again
        self.disk_asked = None;
    }

    pub(crate) fn run(&mut self, a: Act, ctx: &egui::Context) {
        self.msg = None;
        if let Act::Drop(paths, into, tab, mv) = a {
            // the pane dropped on becomes the active one, as a click there would make it
            if self.split && tab == self.other {
                std::mem::swap(&mut self.cur, &mut self.other);
            }
            return self.transfer(paths, into, mv, ctx);
        }
        // a results list has no folder of its own; the one behind it is not on screen
        if self.tabs[self.cur].found && matches!(a, Act::Paste | Act::NewFolder | Act::Pack | Act::PackAs(..)) {
            self.msg = Some(t!("actions.results_no_folder").into());
            return;
        }
        // files copied or cut in another program (or before this one started) come from the
        // Windows clipboard; read only now, on Wklej
        if matches!(a, Act::Paste) && self.clip.is_empty() && self.clip_files {
            // Windows offers a cut until its move here ends: a second paste meanwhile would move the same files again
            if self.pasting.as_ref().is_some_and(|(s, job)| *s == sys::clip::seq() && Arc::strong_count(job) > 1) {
                self.msg = Some(t!("actions.cut_still_moving").into());
                return;
            }
            match sys::clip::files() {
                // reading another program's cut renders it, which may change the number paste_done compares
                Some((paths, cut)) => (self.clip, self.cut, self.clip_sys, self.clip_seq) = (paths, cut, true, sys::clip::seq()),
                None => {
                    self.msg = Some(t!("actions.clipboard_busy").into());
                    return;
                }
            }
        }
        // in a folder of an archive the commands that change it go through archive.rs (Rename
        // and Kopiuj/Wytnij work as anywhere; Rename ends in commit_rename)
        if self.tabs[self.cur].in_archive() {
            if let Some((arc, into)) = archive::split_archive(&self.tabs[self.cur].path) {
                match &a {
                    Act::Pack | Act::PackAs(..) | Act::MakeBak => {
                        self.msg = Some(t!("actions.not_in_archive").into());
                        return;
                    }
                    Act::NewFolder => return self.change_archive(arc, rx_archive::Change::NewFolder { into }, true, ctx),
                    Act::Paste => {
                        if !self.clip.is_empty() {
                            // the clip is emptied only once the paste really starts
                            self.paste_into_archive(self.clip.clone(), self.cut, ctx);
                        }
                        return;
                    }
                    Act::Delete => {
                        let names: Vec<String> = self.tab().selected().iter().filter_map(|p| archive::split_archive(p)).map(|x| x.1).filter(|n| !n.is_empty()).collect();
                        if names.is_empty() {
                            return;
                        }
                        let n = names.len();
                        let change = rx_archive::Change::Remove(names);
                        // RAR (and an archive in an archive) answer with their own question or note
                        if !rx_archive::can_update(&arc) || archive::is_temp(&arc) {
                            return self.change_archive(arc, change, false, ctx);
                        }
                        let name = arc.file_name().unwrap_or_default().to_string_lossy().into_owned();
                        self.ask = Some(archive::Ask {
                            title: t!("actions.arc_delete_title").into(),
                            text: if n == 1 { t!("actions.arc_delete_text_1", name = name) } else { t!("actions.arc_delete_text_n", n = n, name = name) },
                            yes: t!("actions.delete").into(),
                            act: Act::ArchiveChange(arc, change, false),
                        });
                        return;
                    }
                    _ => {}
                }
            }
        }
        let dir = self.tab().path.clone();
        let sel = self.tab().selected();
        match a {
            Act::NewTab(p) => {
                let side = self.tabs[self.cur].side;
                let mut t = Tab::new(p.unwrap_or(dir));
                t.side = side;
                self.tabs.push(t);
                self.cur = self.tabs.len() - 1;
                self.apply_folder_sort(self.cur);
            }
            Act::CloseTab(i) => {
                if self.tabs.len() > 1 {
                    let side = self.tabs[i].side;
                    let (was_cur, was_other) = (i == self.cur, i == self.other);
                    self.tabs.remove(i);
                    let fix = |k: usize| if k > i { k - 1 } else { k };
                    self.cur = fix(self.cur);
                    self.other = fix(self.other);
                    // the pane that lost its visible tab shows its neighbour on the same side
                    if was_cur {
                        self.cur = self.last_on(side, i).unwrap_or(0);
                    }
                    if was_other {
                        self.other = self.last_on(side, i).unwrap_or(0);
                    }
                    self.fix_panes();
                }
            }
            Act::ToggleSplit => {
                self.split = !self.split;
                if self.split {
                    let side = 1 - self.tabs[self.cur].side;
                    if self.last_on(side, 0).is_none() {
                        let mut t = Tab::new(dir.clone());
                        t.side = side;
                        self.tabs.push(t);
                        self.apply_folder_sort(self.tabs.len() - 1);
                    }
                    self.other = usize::MAX;
                    self.fix_panes();
                }
            }
            Act::SwitchPane => {
                // the same swap as a click in the other pane (see split_panes)
                if self.split {
                    std::mem::swap(&mut self.cur, &mut self.other);
                    self.tree_focus = None;
                    self.kbd_focus = true;
                }
            }
            Act::FilterNames => {
                self.filter_open = true;
                ctx.memory_mut(|m| m.request_focus(egui::Id::unique("filter")));
            }
            Act::ToggleSpace => self.set_space(!self.space, ctx),
            Act::Biggest => {
                if self.tab().biggest {
                    self.tab().load();
                } else {
                    self.show_biggest(ctx);
                }
            }
            Act::NewFolder => {
                let mut n = 1;
                let mut p = dir.join(t!("actions.new_folder_name"));
                while p.exists() {
                    n += 1;
                    p = dir.join(t!("actions.new_folder_name_n", n = n));
                }
                match std::fs::create_dir(&p) {
                    Ok(()) => {
                        self.undo.push(Undo::Created(vec![(p.clone(), rx_core::undo::stamp(&p))], "undo.what.new_folder"));
                        self.after_fs_change();
                        // selected, with its name being edited, once the list has it
                        self.then_select(rx_core::Select { paths: vec![p], rename: true, listed: false });
                    }
                    Err(e) => self.msg = Some(t!("actions.cant_create_folder", e = e)),
                }
            }
            Act::Rename => {
                if let Some(p) = sel.first() {
                    let text = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                    self.rename = Some(Rename { path: p.clone(), text, fresh: true });
                }
            }
            Act::Delete => {
                if !sel.is_empty() {
                    if self.space {
                        self.space_forget(&sel);
                    }
                    self.background(ctx, t!("actions.job.recycle").into(), names(&sel), move |_, _| recycled(sel));
                }
            }
            Act::Recycle(paths) => {
                self.space_forget(&paths);
                self.background(ctx, t!("actions.job.recycle").into(), names(&paths), move |_, _| recycled(paths));
            }
            Act::Reveal(p) => {
                if let Some(dir) = p.parent().map(Path::to_path_buf) {
                    // as navigate(): the folder shows, not results or a query left in the box
                    let t = self.tab();
                    (t.search_all, t.search) = (false, String::new());
                    if t.found {
                        t.load();
                    }
                    let (space, t) = (self.space, self.tab());
                    if space && dir.starts_with(&t.path) && dir != t.path {
                        // "Analiza miejsca": the tree opens down to it, the folder stays
                        let here = t.path.clone();
                        let mut at = Some(dir.as_path());
                        while let Some(a) = at.filter(|a| *a != here) {
                            t.expanded.insert(a.to_path_buf());
                            at = a.parent();
                        }
                        t.sized = None;
                    } else {
                        self.navigate(dir);
                    }
                    // once the folder's rows are in, if they are on their way
                    self.then_select(rx_core::Select { paths: vec![p], rename: false, listed: false });
                }
            }
            // taken at the top of run()
            Act::Drop(..) => {}
            Act::TabToSide(i, side) => {
                if i >= self.tabs.len() {
                    return;
                }
                let prev = self.cur;
                if !self.split {
                    // the dragged tab on the right, the others on the left
                    for t in &mut self.tabs {
                        t.side = 1 - side;
                    }
                }
                self.tabs[i].side = side;
                // the pane it leaves keeps a tab: the same folder when it was its only one
                let rest = 1 - side;
                if self.last_on(rest, 0).is_none() {
                    let mut t = Tab::new(self.tabs[i].path.clone());
                    t.side = rest;
                    self.tabs.push(t);
                    self.apply_folder_sort(self.tabs.len() - 1);
                }
                self.split = true;
                self.cur = i;
                // the other pane keeps showing what it showed, else the dragged tab's neighbour
                self.other = if prev != i && self.tabs[prev].side == rest { prev } else { self.last_on(rest, i).unwrap_or(usize::MAX) };
                self.fix_panes();
            }
            Act::VtCheck(p) => self.vt_check(p, ctx),
            Act::VtSend(p) => self.vt_send(p, ctx),
            Act::CloudLink(p) => {
                if self.cloud_link(&p) == Some("Google Drive") {
                    self.msg = Some(match google_link(&p) {
                        Ok(link) => {
                            ctx.copy_text(link);
                            t!("actions.google_link_copied").into()
                        }
                        Err(e) => e,
                    });
                } else {
                    let (owner, tx, ctx) = (self.hwnd, self.tx.clone(), ctx.clone());
                    std::thread::spawn(move || {
                        sys::init_com();
                        // OneDrive makes the link and puts it on the clipboard itself, a moment later
                        let note = sys::onedrive_copy_link(&p, owner).map_or_else(|e| e, |_| t!("actions.onedrive_copying_link").into());
                        let _ = tx.send(Msg::Note(note));
                        ctx.request_repaint();
                    });
                }
            }
            Act::CloudKeep(keep) => {
                let (tx, ctx) = (self.tx.clone(), ctx.clone());
                if keep {
                    self.msg = Some(t!("actions.keep_downloading").into());
                }
                std::thread::spawn(move || {
                    let note = match sel.iter().try_for_each(|p| sys::cloud_keep(p, keep)) {
                        Ok(()) if keep => t!("actions.kept").into(),
                        Ok(()) => t!("actions.freed").into(),
                        Err(e) => t!("actions.cloud_refused", e = e),
                    };
                    let _ = tx.send(Msg::Note(note));
                    ctx.request_repaint();
                });
            }
            Act::CloudFetch(p) => {
                let (tx, ctx) = (self.tx.clone(), ctx.clone());
                self.msg = Some(t!("actions.fetching").into());
                // the card waits with a spinner; Msg::Fetched has the preview built again
                if let Some(shown) = self.preview.as_mut().filter(|x| x.0 == p) {
                    shown.1 = Shown::Loading;
                }
                std::thread::spawn(move || {
                    let fetched = sys::cloud_fetch(&p);
                    let note = match &fetched {
                        Ok(()) => t!("actions.fetched").into(),
                        Err(e) => t!("actions.fetch_failed", e = e),
                    };
                    let _ = tx.send(Msg::Note(note));
                    let _ = tx.send(Msg::Fetched(p, fetched.is_ok()));
                    ctx.request_repaint();
                });
            }
            Act::OpenWith(p) => {
                let (owner, tx, ctx) = (self.hwnd, self.tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    sys::init_com();
                    if let Err(e) = sys::open_with(&p, owner) {
                        let _ = tx.send(Msg::Note(t!("actions.open_failed", e = e)));
                        ctx.request_repaint();
                    }
                });
            }
            Act::ShellMenu(p, at) => {
                // the shell makes one menu for items of one folder; a results list may mix folders
                let mut paths = if sel.contains(&p) { sel } else { vec![p.clone()] };
                if paths.iter().any(|q| q.parent() != p.parent()) {
                    paths = vec![p];
                    self.msg = Some(t!("actions.menu_mixed_folders").into());
                }
                if paths.iter().any(|q| archive::is_inside(q)) {
                    self.msg = Some(t!("actions.menu_in_archive").into());
                    return;
                }
                let ppp = ctx.pixels_per_point();
                let at = at.map(|a| ((a.x * ppp) as i32, (a.y * ppp) as i32));
                let (tx, ctx, asked) = (self.tx.clone(), ctx.clone(), Instant::now());
                // a share that stopped answering holds this thread, not the window; each item is
                // asked for here, the menu built on the window's thread from what came back
                std::thread::spawn(move || {
                    sys::init_com();
                    let _ = tx.send(Msg::ShellMenu(sys::menu::items(&paths), at, asked));
                    ctx.request_repaint();
                });
            }
            Act::VtKey => self.ask_vt_key = Some((String::new(), None, sys::secret(KEY_NAME).is_some())),
            Act::ToggleExpand(p) => {
                let t = self.tab();
                if t.expanded.contains(&p) {
                    // closed with everything opened inside it
                    t.expanded.retain(|q| !q.starts_with(&p));
                } else {
                    t.expanded.insert(p);
                }
                t.sized = None;
            }
            Act::Copy(cut) => {
                if !sel.is_empty() {
                    // items of an archive have no path other programs could use: only this program pastes them
                    self.clip_sys = !sel.iter().any(|p| archive::is_inside(p));
                    if !self.clip_sys {
                        // one clipboard content at a time, as any copy elsewhere replaces it
                        sys::clip::clear();
                    }
                    self.msg = Some(if self.clip_sys && !sys::clip::put(&sel, cut) {
                        self.clip_sys = false;
                        t!("actions.clipboard_busy_local").into()
                    } else if cut {
                        t!("actions.cut_to_clip", n = sel.len())
                    } else {
                        t!("actions.copied_to_clip", n = sel.len())
                    });
                    // a clip only this program can paste hides an older list on the Windows clipboard
                    (self.clip_seq, self.clip_files) = (sys::clip::seq(), self.clip_sys);
                    self.clip = sel;
                    self.cut = cut;
                }
            }
            Act::Paste => {
                if !self.clip.is_empty() {
                    let (clip, cut) = (self.clip.clone(), self.cut);
                    if cut {
                        self.clip.clear();
                    }
                    if cut && self.clip_sys {
                        // as after a paste in Explorer, the source hears of it and the clipboard stops offering what was moved
                        let seq = self.clip_seq;
                        let job = Arc::new(());
                        self.pasting = Some((seq, job.clone()));
                        self.background(ctx, t!("actions.job.move", place = place(&dir)), names(&clip), move |p, ctl| {
                            let _running = job;
                            let done = sys::transfer(&clip, &dir, true, ctl, p);
                            if done.0 {
                                sys::clip::paste_done(seq);
                            }
                            transferred(done)
                        });
                    } else {
                        self.transfer(clip, dir, cut, ctx);
                    }
                }
            }
            Act::SelectAll => {
                let t = self.tab();
                t.sel = t.view.iter().map(|&i| t.all[i].path.clone()).collect();
            }
            Act::Invert => {
                let t = self.tab();
                t.sel = t.view.iter().map(|&i| t.all[i].path.clone()).filter(|p| !t.sel.contains(p)).collect();
            }
            Act::Open => {
                // on Ten komputer the selection is the drive picked there (not in the file view)
                let pick = if is_pc(&dir) { self.tab().focus.clone() } else { sel.first().cloned() };
                if let Some(p) = pick {
                    self.run(Act::OpenPath(p), ctx);
                }
            }
            Act::OpenPath(p) => {
                let t = self.tab();
                let folder = match t.all.iter().find(|e| e.path == p) {
                    // a row knows what it is: no question to a drive that may have stopped answering
                    Some(e) => e.opens_as_folder() || (!t.in_archive() && rx_archive::can_extract(&p)),
                    // on Ten komputer a root is a drive (a file comes here too: from the map, out of an archive)
                    None => is_pc(&p) || (is_pc(&t.path) && p.parent().is_none()) || p.is_dir() || archive::is_folder(&p),
                };
                if folder {
                    // an archive (or a folder in one) opens as a folder, as in Explorer
                    self.navigate(p);
                } else if archive::is_inside(&p) {
                    self.open_from_archive(p, ctx);
                } else if let Err(e) = sys::open(&p) {
                    self.msg = Some(t!("actions.open_failed", e = e));
                }
            }
            Act::Properties => {
                if is_pc(&dir) {
                    if let Some(d) = self.tab().focus.clone() {
                        sys::properties(&d);
                    }
                } else if let Some((arc, _)) = archive::split_archive(sel.first().unwrap_or(&dir)) {
                    // an entry has no file of its own: the archive's properties
                    sys::properties(&arc);
                } else {
                    sys::properties(sel.first().unwrap_or(&dir));
                }
            }
            Act::MakeBak => {
                let mut made = vec![];
                for p in sel.iter().filter(|p| p.is_file()) {
                    let mut dst = PathBuf::from(format!("{}.bak", p.display()));
                    let mut n = 1;
                    while dst.exists() {
                        n += 1;
                        dst = PathBuf::from(format!("{}.bak{n}", p.display()));
                    }
                    match std::fs::copy(p, &dst) {
                        Ok(_) => made.push((dst.clone(), rx_core::undo::stamp(&dst))),
                        Err(e) => self.msg = Some(t!("actions.bak_failed", e = e)),
                    }
                }
                if !made.is_empty() {
                    self.undo.push(Undo::Created(made, "undo.what.bak"));
                    self.after_fs_change();
                }
            }
            a @ (Act::Extract | Act::ExtractHere) => {
                if self.tabs[self.cur].in_archive() {
                    // inside an archive: the selection (else this whole folder of it) goes next to the archive
                    return self.extract_here(sel, ctx);
                }
                let here = matches!(a, Act::ExtractHere);
                let arcs: Vec<PathBuf> = sel.into_iter().filter(|p| rx_archive::can_extract(p)).collect();
                if arcs.is_empty() {
                    self.msg = Some(t!("actions.select_archive").into());
                }
                // "a.zip" and "a.7z" in one batch unpack into "a" and "a (2)", not both into "a"
                let mut taken = Vec::new();
                for a in arcs {
                    taken.extend(self.start_extract(a, None, &taken, here, ctx));
                }
            }
            Act::Pack => {
                if sel.is_empty() {
                    self.msg = Some(t!("actions.select_to_pack").into());
                } else {
                    // one item gives its name to the archive ("raport.csv" -> raport.zip), several the folder's
                    let stem = match sel.as_slice() {
                        [one] if one.is_dir() => one.file_name(),
                        [one] => one.file_stem(),
                        _ => dir.file_name(),
                    };
                    let stem = stem.map_or(t!("actions.archive_stem").into(), |s| s.to_string_lossy().into_owned());
                    self.ask_pack = Some(archive::PackAsk::new(stem, names(&sel)));
                }
            }
            Act::ArchiveChange(arc, change, rename) => self.change_archive(arc, change, rename, ctx),
            Act::ConvertArchive(arc, inner) => self.convert_archive(arc, inner, ctx),
            Act::PackAs(stem, kind, opts) => {
                if sel.is_empty() {
                    // never silent: the dialog has closed, so say why nothing is packed
                    self.msg = Some(t!("actions.select_to_pack").into());
                } else {
                    let dest = free_name(&dir, &stem, kind, &[]);
                    let name = dest.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    let base = dir.clone();
                    self.archive_job(t!("actions.job.pack", name = name), names(&sel), ctx, move |p| {
                        let files = rx_archive::pack_with(&sel, &base, &dest, &opts, p)?;
                        Ok(t!("actions.packed", name = name, files = files))
                    });
                }
            }
            Act::ToggleHidden => {
                self.show_hidden = !self.show_hidden;
                // the tree reads its folders again, with or without the hidden ones
                self.tree_asked.clear();
                for t in &mut self.tabs {
                    t.dirty = true;
                }
            }
            Act::Refresh => {
                self.pc_asked = false;
                self.after_fs_change();
                read_drives(self.tx.clone(), false);
                self.preview = None;
            }
            // one at a time, after the operations it may depend on: a held Ctrl+Z took back dependent
            // records at once and out of order, and one during a paste took back the one before it
            Act::Undo if self.busy.load(AO::SeqCst) > 0 => self.msg = Some(t!("actions.undo.busy").into()),
            Act::Undo => match self.undo.pop() {
                Some(u @ Undo::Blocked(_)) => self.msg = Some(u.label()),
                Some(u) => {
                    self.msg = Some(format!("{}…", u.label()));
                    // the note says how it went; undo() says whether it was a full success
                    self.background(ctx, t!("actions.job.undo").into(), String::new(), move |_, _| {
                        let (ok, n) = undo(u);
                        (Ok(ok), vec![Msg::Note(n)])
                    });
                }
                None => self.msg = Some(t!("actions.undo.nothing").into()),
            },
            Act::Back => {
                let t = self.tab();
                // the biggest files of this folder: Back is the folder itself, not the one before
                if t.biggest {
                    t.load();
                } else if let Some(p) = t.back.pop() {
                    let old = std::mem::replace(&mut t.path, p);
                    t.fwd.push(old);
                    // as navigate(): the folder shows, not results left in the box
                    (t.search_all, t.search) = (false, String::new());
                    t.arrive();
                    self.apply_folder_sort(self.cur);
                }
            }
            Act::Forward => {
                let t = self.tab();
                if let Some(p) = t.fwd.pop() {
                    let old = std::mem::replace(&mut t.path, p);
                    t.back.push(old);
                    (t.search_all, t.search) = (false, String::new());
                    t.arrive();
                    self.apply_folder_sort(self.cur);
                }
            }
            Act::Up => {
                if let Some(p) = dir.parent().map(Path::to_path_buf).or_else(|| (!is_pc(&dir)).then(|| PathBuf::from(PC))) {
                    let from = dir.clone();
                    self.navigate(p);
                    self.then_select(rx_core::Select { paths: vec![from], rename: false, listed: false });
                }
            }
            Act::CalcSizes => {
                let dirs: Vec<PathBuf> = {
                    let t = self.tab();
                    t.view.iter().map(|&i| &t.all[i]).filter(|e| e.dir).map(|e| e.path.clone()).collect()
                };
                self.count_sizes(dirs, ctx);
            }
            Act::SizeSelected => {
                let dirs: Vec<PathBuf> = {
                    let t = self.tab();
                    t.all.iter().filter(|e| e.dir && t.sel.contains(&e.path)).map(|e| e.path.clone()).collect()
                };
                self.count_sizes(dirs, ctx);
            }
            Act::ToggleFlat => {
                let t = self.tab();
                t.flat = !t.flat;
                t.load();
            }
            Act::Goto(p) => {
                // "C:" typed in the address bar is the drive, as in Explorer
                let p = if p.as_os_str().len() == 2 && rx_core::drive_bit(&p.to_string_lossy()) != 0 { p.join("\\") } else { p };
                // the disk is not asked whether the folder is there (a share that does not answer
                // would hold the window for seconds): one that is not says so in its list
                if p.is_absolute() || is_pc(&p) {
                    self.navigate(p);
                } else {
                    self.msg = Some(t!("actions.folder_not_found", path = p.display()));
                }
            }
            Act::AddBookmark(p) => {
                if !self.bookmarks.contains(&p) {
                    self.bookmarks.push(p);
                    self.save_bookmarks();
                }
            }
            Act::RemoveBookmark(p) => {
                self.bookmarks.retain(|b| *b != p);
                self.save_bookmarks();
            }
            Act::Terminal => {
                use std::os::windows::process::CommandExt;
                let dir = archive::on_disk(&dir);
                if std::process::Command::new("wt").arg("-d").arg(&dir).spawn().is_err() {
                    let _ = std::process::Command::new("cmd").current_dir(&dir).creation_flags(0x10).spawn();
                }
            }
            Act::CopyPath => {
                // on a network drive the Linux path the server has (/home/ala/...), the owner's wish
                let shown = |p: &PathBuf| {
                    let s = p.display().to_string();
                    let unc = if s.starts_with(r"\\") { Some(s.clone()) } else { sys::drive_unc(p) };
                    unc.and_then(|u| rx_core::server_path(&u)).unwrap_or(s)
                };
                let text: Vec<String> = if sel.is_empty() { vec![shown(&dir)] } else { sel.iter().map(shown).collect() };
                ctx.copy_text(text.join("\r\n"));
                self.msg = Some(t!("actions.path_copied").into());
            }
            Act::Exit => self.quit(ctx),
        }
    }

    /// Counts the folders' sizes on a thread of its own (slow on a network drive); each lands in
    /// Wielkość as it comes, "…" until then. A folder already being counted is not counted twice.
    pub(crate) fn count_sizes(&mut self, dirs: Vec<PathBuf>, ctx: &egui::Context) {
        // an archive's folders come with their sizes (from its list); there is no folder to read
        let dirs: Vec<PathBuf> = dirs.into_iter().filter(|d| !archive::is_inside(d) && self.sizing.insert(d.clone())).collect();
        if dirs.is_empty() {
            return;
        }
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            for d in dirs {
                let s = dir_size(&d);
                let _ = tx.send(Msg::DirSize(d, s));
                ctx.request_repaint();
            }
        });
    }

    /// Unpacks `a` next to it, into a new folder of its name (never into one already there, nor
    /// into one in `taken`, picked by an earlier archive of the same batch), or with `here` straight
    /// into its folder (rx_archive::extract_here: a taken name gets a number, nothing is replaced).
    /// An encrypted archive comes back as a password question instead of an error. Returns the
    /// folder it picked.
    pub(crate) fn start_extract(&self, a: PathBuf, pw: Option<String>, taken: &[PathBuf], here: bool, ctx: &egui::Context) -> Option<PathBuf> {
        let (Some(at), Some(stem)) = (a.parent(), a.file_stem()) else { return None };
        let tx = self.tx.clone();
        if here {
            let at = at.to_path_buf();
            let title = t!("actions.job.extract", place = place(&at));
            self.archive_job(title, names(std::slice::from_ref(&a)), ctx, move |p| match rx_archive::extract_here(&a, &at, pw.as_deref(), p) {
                Ok((files, skipped, placed)) => {
                    let unsafe_note = if skipped > 0 { t!("actions.unsafe_skipped", n = skipped) } else { String::new() };
                    let _ = tx.send(Msg::Changed(placed, false));
                    Ok(t!("actions.extracted_here", files = files) + &unsafe_note)
                }
                Err(rx_archive::Fail::Password) => {
                    let _ = tx.send(Msg::AskPassword(a, pw.is_some(), true));
                    Ok(String::new())
                }
                Err(rx_archive::Fail::Other(e)) => Err(e),
            });
            return None;
        }
        let dest = free_name(at, &stem.to_string_lossy(), "", taken);
        let picked = dest.clone();
        let title = t!("actions.job.extract", place = place(&dest));
        self.archive_job(title, names(std::slice::from_ref(&a)), ctx, move |p| match rx_archive::extract(&a, &dest, pw.as_deref(), p) {
            Ok((files, skipped)) => {
                let to = dest.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let unsafe_note = if skipped > 0 { t!("actions.unsafe_skipped", n = skipped) } else { String::new() };
                Ok(t!("archive.extracted_to", dest = to, n = files) + &unsafe_note)
            }
            Err(rx_archive::Fail::Password) => {
                let _ = tx.send(Msg::AskPassword(a, pw.is_some(), false));
                Ok(String::new())
            }
            Err(rx_archive::Fail::Other(e)) => Err(e),
        });
        Some(picked)
    }

    /// A packing or unpacking job off the UI thread: its progress shows in the status bar and the
    /// Operacje panel, its outcome (or error) as a note, and the list reloads after. Counts as a
    /// file operation, so an update waits for it.
    pub(crate) fn archive_job(&self, title: String, source: String, ctx: &egui::Context, job: impl FnOnce(rx_archive::Progress) -> Result<String, String> + Send + 'static) {
        let (tx, busy, ops, ctx) = (self.tx.clone(), self.busy.clone(), self.ops.clone(), ctx.clone());
        busy.fetch_add(1, AO::SeqCst);
        let id = ops.start(title, source);
        std::thread::spawn(move || {
            let mut report = |done: u64, total: u64| {
                if ops.progress(id, done, total) {
                    ctx.request_repaint();
                }
            };
            // a panic in a decoder must still end the job, or "file operations running" stays on for good
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut report))).unwrap_or_else(|_| Err(t!("actions.archive_internal_error").into()));
            // an empty note: the job ended in a question (the password dialog) or only opened a
            // file, not an outcome; an error may still have changed an archive half way
            let done = !matches!(&r, Ok(n) if n.is_empty());
            let r = r.map_err(|e| t!("actions.archive_error", e = e));
            if done {
                ops.finish(id, r.clone());
            } else {
                ops.remove(id);
            }
            let note = r.unwrap_or_else(|e| e);
            if !note.is_empty() {
                let _ = tx.send(Msg::Note(note));
            }
            if done {
                let _ = tx.send(Msg::Undo(Undo::Blocked("undo.what.archive")));
            }
            // after the record: a Ctrl+Z waits for this job (Act::Undo), then finds it on top
            busy.fetch_sub(1, AO::SeqCst);
            let _ = tx.send(Msg::Reload);
            ctx.request_repaint();
        });
    }

    /// Copies (or with `cut` moves) `paths` into `dir`: Wklej and a drop. Out of an archive they are
    /// unpacked there (Explorer's copy from a ZIP; cut: taken out of the archive after).
    pub(crate) fn transfer(&mut self, paths: Vec<PathBuf>, dir: PathBuf, cut: bool, ctx: &egui::Context) {
        let (packed, plain): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|p| archive::is_inside(p));
        if !packed.is_empty() {
            self.unpack_into(&packed, &dir, cut, ctx);
        }
        if !plain.is_empty() {
            let title = if cut { t!("actions.job.move", place = place(&dir)) } else { t!("actions.job.copy", place = place(&dir)) };
            self.background(ctx, title, names(&plain), move |p, ctl| transferred(sys::transfer(&plain, &dir, cut, ctl, p)));
        }
    }

    /// Once a frame: the Windows clipboard's number (one cheap call) and files dragged in from
    /// another program, with where the pointer is, which winit does not say during such a drag.
    pub(crate) fn clipboard_and_drops(&mut self, ctx: &egui::Context) {
        let seq = sys::clip::seq();
        if seq != self.clip_seq {
            // copied in another program, or a cut from here pasted there: what was copied here is
            // not on offer any more, as in Explorer
            (self.clip_seq, self.clip_files, self.clip_sys) = (seq, sys::clip::has_files(), false);
            self.clip.clear();
        }
        // the frame after a system drag of ours: its drop, if it came back here, arrives now
        let own = self.dragged_out.take();
        let (dropped, hovered): (Vec<PathBuf>, Vec<PathBuf>) =
            ctx.input(|i| (i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect(), i.raw.hovered_files.iter().filter_map(|f| f.path.clone()).collect()));
        let at_drop = !dropped.is_empty();
        let paths = if at_drop { dropped } else { hovered };
        // the drop goes where the last frame's highlight showed, not where a slow frame finds the pointer
        let shown = self.ext.take().filter(|x| !x.2).map(|x| x.0);
        if paths.is_empty() || self.hwnd == 0 {
            return;
        }
        if let Some((x, y, _)) = sys::clip::pointer(self.hwnd) {
            let px = ctx.pixels_per_point();
            // a dialog or menu over the list: nothing under it takes the drop (no position matches)
            let blocked = self.about || self.ask.is_some() || self.ask_pw.is_some() || self.ask_pack.is_some() || self.ask_vt_key.is_some() || crate::ops::popup_blocks(ctx);
            let at = if blocked { pos2(f32::NAN, f32::NAN) } else { shown.filter(|_| at_drop).unwrap_or(pos2(x as f32 / px, y as f32 / px)) };
            // a drag of ours that left the window and came back keeps its pane (a copy, as the cursor said: drop_moves_now)
            let d = own.filter(|d| at_drop && d.0 == paths).unwrap_or_else(|| Arc::new(Dragged(paths, usize::MAX, Kind::Folder)));
            self.ext = Some((at, d, at_drop));
        }
        if !at_drop {
            // nothing else wakes the window while the pointer moves over it with the files
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// At the start of a frame, before the lists are drawn: Windows' menu and the system drag run
    /// loops of their own that block drawing until they end, so each waits for a frame drawn
    /// without the app's own menu, drag card or drop highlight.
    pub(crate) fn modal_loops(&mut self, ctx: &egui::Context) {
        if let Some((items, at)) = self.shell_menu_next.take() {
            match sys::menu::shell_menu(self.hwnd, at, &items) {
                // a shell command may delete, rename or create files; the watcher may be slower
                Ok(true) => self.after_fs_change(),
                Ok(false) => {}
                Err(e) => self.msg = Some(e),
            }
        }
        if let Some(d) = self.drag_out_next.take() {
            if sys::clip::pointer(self.hwnd).is_some_and(|p| p.2) {
                // back over the window within a frame: the drag in the window goes on, with its rules
                egui::DragAndDrop::set_payload(ctx, Dragged(d.0.clone(), d.1, d.2));
            } else {
                sys::clip::drag_out(self.hwnd, &d.0);
                self.dragged_out = Some(d);
                self.drag_out_done = true;
                // a move done by the place dropped on shows at once, not when the watcher tells
                let _ = self.tx.send(Msg::Reload);
                ctx.request_repaint();
            }
        }
        // dragged out of the window, or onto another program's window over it: a system drag takes
        // over next frame, so the files can go to Explorer, the desktop or a mail (items of an
        // archive have no path there; they stay in the window)
        let drag = egui::DragAndDrop::payload::<Dragged>(ctx);
        if let Some(d) = drag.filter(|d| self.hwnd != 0 && !d.0.iter().any(|p| archive::is_inside(p)) && ctx.input(|i| i.pointer.primary_down())) {
            if sys::clip::pointer(self.hwnd).is_some_and(|p| !p.2) {
                egui::DragAndDrop::clear_payload(ctx);
                self.drag_out_next = Some(d);
                ctx.request_repaint();
            }
        }
    }

    /// Shell operations show their own dialogs, so they run off the UI thread; what they send back
    /// (notes, undo records) is taken in, and the list reloads after. Each is a job of the
    /// Operacje panel; one that reports (done, total) through its argument gets a percentage,
    /// one that does not shows an indeterminate bar. `f` says whether it went through: a cancel
    /// leaves the panel without a confirmation (its note, if any, says what happened); an error
    /// stays in the history as a failed operation. A job that can be paused and cancelled from the
    /// panel arms the `Control` it gets (the app's own copy); the others ignore it.
    pub(crate) fn background(&self, ctx: &egui::Context, title: String, source: String, f: impl FnOnce(rx_archive::Progress, &rx_core::ops::Control) -> (Result<bool, String>, Vec<Msg>) + Send + 'static) {
        let (tx, busy, ops, ctx) = (self.tx.clone(), self.busy.clone(), self.ops.clone(), ctx.clone());
        busy.fetch_add(1, AO::SeqCst);
        let ctl = Arc::new(rx_core::ops::Control::default());
        let id = ops.start_with(title, source, Some(ctl.clone()));
        std::thread::spawn(move || {
            sys::init_com();
            let mut report = |done: u64, total: u64| {
                if ops.progress(id, done, total) {
                    ctx.request_repaint();
                }
            };
            // a panic must still end the job, or it runs in the panel and blocks updates for good
            let msgs = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut report, &ctl))) {
                Ok((Ok(true), msgs)) => {
                    ops.finish(id, Ok(String::new()));
                    msgs
                }
                Ok((Ok(false), msgs)) => {
                    ops.remove(id);
                    msgs
                }
                Ok((Err(e), msgs)) => {
                    ops.finish(id, Err(e));
                    msgs
                }
                Err(_) => {
                    ops.finish(id, Err(t!("actions.internal_error").into()));
                    vec![]
                }
            };
            for m in msgs {
                let _ = tx.send(m);
            }
            busy.fetch_sub(1, AO::SeqCst);
            let _ = tx.send(Msg::Reload);
            ctx.request_repaint();
        });
    }

    /// A newer build is swapped in only when nothing would be lost or disturbed: no file
    /// operation running, no rename or address being typed, and the window minimized, behind
    /// other windows for 2 minutes or untouched for 10 minutes. The app then
    /// closes as usual (saving its tabs) and the new build starts in its place.
    pub(crate) fn install_update(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if ctx.input(|i| !i.events.is_empty() || i.pointer.is_moving()) {
            self.last_input = now;
        }
        if let Some(p) = self.update_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.update_staged = Some(p);
        }
        let (minimized, focused) = ctx.input(|i| (i.viewport().minimized.unwrap_or(false), i.viewport().focused.unwrap_or(true)));
        let minimized = minimized || self.hidden;
        let Some(staged) = self.update_staged.clone() else {
            return;
        };
        let behind = !focused && now - self.last_input > 120.0;
        let quiet = minimized || behind || now - self.last_input > 600.0;
        let working = self.busy.load(AO::SeqCst) > 0
            // a copy picked in Windows' menu runs on the shell's own thread, here in the process
            || sys::shell_working()
            || self.rename.is_some()
            || self.addr_edit.is_some()
            || self.ask_pw.is_some()
            || self.ask.is_some()
            || !self.arc_watch.is_empty();
        if !quiet || working {
            ctx.request_repaint_after(std::time::Duration::from_secs(30));
            return;
        }
        self.update_staged = None;
        let start_arg = if self.hidden {
            Some(update::TRAY_ARG)
        } else {
            (minimized || behind).then_some(update::MINIMIZED_ARG)
        };
        match update::apply(&staged, start_arg) {
            Ok(()) => self.quit(ctx),
            // looked for again (the staged file is reused), e.g. after a passing file lock
            Err(_) => self.update_rx = update::start(wake),
        }
    }

    pub(crate) fn commit_rename(&mut self) {
        let Some(r) = self.rename.take() else { return };
        let new_name = r.text.trim();
        let old_name = r.path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        if new_name.is_empty() || new_name == old_name {
            return;
        }
        if new_name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
            self.msg = Some(t!("actions.bad_name_chars").into());
            return;
        }
        // an entry of an archive: renamed in the archive (a folder with what is inside it)
        if let Some((arc, from)) = archive::split_archive(&r.path).filter(|(_, i)| !i.is_empty()) {
            let to = match from.rsplit_once('/') {
                Some((up, _)) => format!("{up}/{new_name}"),
                None => new_name.to_string(),
            };
            self.pending.push(Act::ArchiveChange(arc, rx_archive::Change::Rename { from, to }, false));
            return;
        }
        let new = r.path.with_file_name(new_name);
        if new.exists() && !new_name.eq_ignore_ascii_case(&old_name) {
            self.msg = Some(t!("actions.name_exists", name = new_name));
            return;
        }
        match std::fs::rename(&r.path, &new) {
            Ok(()) => {
                self.undo.push(Undo::Rename(r.path, new.clone()));
                self.after_fs_change();
                self.then_select(rx_core::Select { paths: vec![new], rename: false, listed: false });
            }
            Err(e) => self.msg = Some(t!("actions.cant_rename", e = e)),
        }
    }

    /// Has the preview of `p`, if it is the file shown, built again from its content, also when
    /// its attributes say the content is in the cloud: it was asked for, or the list has it here.
    pub(crate) fn preview_read(&mut self, p: PathBuf) {
        if self.preview.as_ref().is_some_and(|x| x.0 == p) {
            self.preview = Some((p.clone(), Shown::Loading));
            self.preview_req.put((p, self.preview_req_px, true));
        }
    }

    /// Rows that changed get their view (indices into them) rebuilt before anything reads it.
    pub(crate) fn refresh_views(&mut self) {
        let (hidden, space) = (self.show_hidden, self.space);
        for t in self.tabs.iter_mut().filter(|t| t.dirty) {
            t.refresh_view(hidden, space);
        }
    }

    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Preview(p, raw) => {
                    if self.preview.as_ref().map(|x| &x.0) == Some(&p) {
                        let shown = match raw {
                            Raw::Image(img, info) => Shown::Image(ctx.load_texture("podglad", img, Default::default()), info),
                            Raw::Text(t) => Shown::Text(t),
                            Raw::Folder(d, f, b) => Shown::Folder(d, f, b),
                            Raw::Nothing(s) => Shown::Nothing(s),
                            Raw::Cloud(img, more) => Shown::Cloud(img.map(|i| ctx.load_texture("podglad", i, Default::default())), more),
                        };
                        self.preview = Some((p, shown));
                    }
                }
                Msg::Fetched(p, ok) => {
                    if ok {
                        // its content is asked for now, whatever its attributes still say (an
                        // offline file of a share's archive keeps them, and comes when read)
                        self.preview_read(p);
                    } else if self.preview.as_ref().is_some_and(|x| x.0 == p) {
                        // forgotten, so the request below asks for it anew: it shows as a cloud file again
                        self.preview = None;
                    }
                }
                Msg::Thumb(p, img) => {
                    // not for a folder left meanwhile
                    if let Some(t) = self.thumbs.get_mut(&p) {
                        t.tex = img.map(|i| ctx.load_texture(p.to_string_lossy(), i, Default::default()));
                    }
                }
                Msg::DirSize(p, s) => {
                    self.sizing.remove(&p);
                    for t in &mut self.tabs {
                        if let Some(e) = t.all.iter_mut().find(|e| e.path == p) {
                            e.size = Some(s);
                            t.dirty = true;
                        }
                    }
                }
                Msg::Index(ix) => {
                    self.index = Some(ix);
                    self.index_gen += 1;
                }
                Msg::Indexed(fresh) => {
                    // a fresh scan replaces what was read from disk; results re-run on the new one
                    self.index_busy = false;
                    if let Some(ix) = fresh {
                        self.index = Some(ix);
                        self.index_gen += 1;
                        self.index_scanned = Some(Instant::now());
                    }
                }
                Msg::Note(s) => self.msg = Some(s),
                Msg::Undo(u) => self.undo.push(u),
                Msg::AskPassword(p, wrong, here) if self.ask_pw.is_some() => self.pw_queue.push_back((p, wrong, here)),
                Msg::AskPassword(p, wrong, here) => self.ask_pw = Some((p, String::new(), wrong, here)),
                Msg::Reload => self.after_fs_change(),
                Msg::ShellMenu(items, at, asked) => match items {
                    Err(e) => self.msg = Some(t!("actions.menu_no_answer", e = e)),
                    // a menu popping up long after the click would land on whatever the user does now
                    Ok(_) if asked.elapsed() > std::time::Duration::from_secs(3) => self.msg = Some(t!("actions.menu_too_late").into()),
                    Ok(items) => {
                        self.shell_menu_next = Some((items, at));
                        ctx.request_repaint();
                    }
                },
                Msg::Listed(number, rows, in_arc) => self.listed(number, rows, in_arc),
                Msg::Kids(number, dir, kids, dim) => {
                    // an answer from before the last change on disk is not the one waited for
                    if self.tree_asked.get_mut(&dir).is_some_and(|a| a.take(number).is_some()) {
                        self.tree_dim.retain(|d| d.parent() != Some(dir.as_path()));
                        self.tree_dim.extend(dim);
                        self.tree_kids.insert(dir, kids);
                    }
                }
                Msg::Disk(p, free, total) => {
                    if self.disk_asked.as_ref() == Some(&p) {
                        self.disk = Some((p, free, total));
                    }
                }
                Msg::Pc(rows) => {
                    // a drive's rows replace its earlier ones; none for a drive gone meanwhile
                    let pc = self.pc.get_or_insert_with(Vec::new);
                    pc.retain(|r| !rows.iter().any(|n| n.0 == r.0));
                    pc.extend(rows.into_iter().filter(|r| self.drives.iter().any(|d| d.0 == r.0)));
                    pc.sort_by(|a, b| a.0.cmp(&b.0));
                }
                Msg::Places(known, clouds) => {
                    (self.known, self.clouds, self.places_read) = (known, clouds, true);
                    if self.bookmarks.is_empty() {
                        self.bookmarks = self.known.iter().take(2).map(|k| k.0.clone()).collect();
                    }
                }
                Msg::Folder(dir, gone) => {
                    // the tabs showing that folder, on screen or not; the others keep their lists
                    let shows = |t: &Tab| !t.flat && !t.found && self.watched.iter().any(|w| w.1 == dir && w.0 == t.path);
                    for i in (0..self.tabs.len()).filter(|&i| shows(&self.tabs[i])).collect::<Vec<_>>() {
                        let t = &mut self.tabs[i];
                        if gone {
                            // not read again: a drive that is gone has nothing to list
                            t.all.clear();
                            t.sel.clear();
                            (t.error, t.dirty) = (Some(t!("actions.folder_gone").into()), true);
                        } else {
                            // asked anew even when rows are on their way: those may have been
                            // read before this change
                            t.reload();
                            // the status bar asks for the free space again: only for the tab it
                            // shows, so another pane's busy folder does not have a drive that
                            // stopped answering asked over and over
                            if i == self.cur {
                                self.disk_asked = None;
                            }
                        }
                    }
                }
                Msg::Drives(changed, came) => {
                    let bit = |p: &Path| rx_core::drive_bit(&p.to_string_lossy());
                    let letters = |drives: &[(PathBuf, String, bool)]| drives.iter().fold(0, |m, d| m | bit(&d.0));
                    let before = letters(&self.drives);
                    self.drives.retain(|d| bit(&d.0) & changed == 0);
                    // none whose letter went while its name was being read (a share slow to
                    // answer): the 3 s look has told of that already and will not again
                    let there = sys::drive_mask();
                    self.drives.extend(came.into_iter().filter(|d| bit(&d.0) & there != 0));
                    self.drives.sort_by(|a, b| a.0.cmp(&b.0));
                    self.drives_unread &= !changed;
                    // a letter that came or went: folders and names cached for it are another
                    // volume's (drives only read again, at start and on F5, keep theirs)
                    let moved = before ^ letters(&self.drives);
                    let hit = |p: &Path| bit(p) & moved != 0;
                    self.tree_kids.retain(|p, _| !hit(p));
                    self.tree_asked.retain(|p, _| !hit(p));
                    self.names.retain(|p, _| !hit(p));
                    // "Ten komputer" loses the drives that went and asks for its rows again
                    if let Some(pc) = &mut self.pc {
                        pc.retain(|r| self.drives.iter().any(|d| d.0 == r.0));
                    }
                    self.pc_asked = false;
                    if hit(&self.tabs[self.cur].path) {
                        self.disk_asked = None;
                    }
                }
                Msg::Vt(p, v) => self.vt_result(p, v),
                Msg::Open(p) => self.pending.push(Act::OpenPath(p)),
                Msg::Watch(copy, arc, inner) => self.watch(copy, arc, inner),
                Msg::Changed(made, rename) => {
                    self.after_fs_change();
                    // what the change made, in the folder on screen, once its rows are in
                    self.then_select(rx_core::Select { paths: made, rename, listed: true });
                }
            }
        }
        // thumbnails nobody waits for: a request whose tile the last frame did not draw (scrolled
        // away, folder left) goes with its entry, so the tile asks again once it is back on screen
        let live = ctx.cumulative_pass_nr().saturating_sub(1);
        let thumbs = &mut self.thumbs;
        self.thumb_req.retain(|p| match thumbs.get(p) {
            Some(t) if t.seen >= live => true,
            _ => {
                thumbs.remove(p);
                false
            }
        });
        if thumbs.len() > THUMB_CAP {
            let from = rx_core::keep_from(thumbs.values().map(|t| t.seen).collect(), THUMB_CAP, live);
            thumbs.retain(|_, t| t.seen >= from);
        }
        // a file opened from an archive and saved since asks to go back into it: looked at every
        // 2 s, and only while there is one (an idle window does not wake up by itself; changes
        // other programs make to the folders on screen come as Msg::Folder)
        if !self.arc_watch.is_empty() {
            if self.last_check.elapsed().as_secs_f32() > 2.0 {
                self.last_check = Instant::now();
                self.check_watched();
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
        self.search_everywhere(ctx);
        self.space_sizes(ctx);
        self.refresh_views();
        // preview follows the single selected item
        let want = {
            let t = &self.tabs[self.cur];
            (t.sel.len() == 1).then(|| t.sel.iter().next().unwrap().clone())
        };
        let changed = want.as_ref() != self.preview.as_ref().map(|p| &p.0);
        // a PDF page renders again once the pane is much wider or narrower than it was rendered
        // for (not mid-drag), so its text stays sharp; a shell icon (empty info) has nothing to gain
        let px = self.preview_px.clamp(400, 2400);
        let rerender = !changed
            && matches!(&self.preview, Some((p, Shown::Image(_, info))) if !info.is_empty() && ext_of(&p.to_string_lossy()) == "pdf")
            && px.abs_diff(self.preview_req_px) * 5 > self.preview_req_px
            && !ctx.input(|i| i.pointer.any_down());
        if self.show_preview && (changed || rerender) {
            // a re-render keeps the old page on screen until the new one arrives
            if changed {
                self.preview = want.clone().map(|p| (p, Shown::Loading));
            }
            if let Some(p) = want {
                self.preview_req_px = px;
                self.preview_req.put((p, px, false));
            }
        }
        let path = self.tab().path.clone();
        // asked on a thread of its own (a drive that does not answer takes seconds); the bar keeps
        // the last answer until the new one comes, unless that was another drive's
        if !is_pc(&path) && self.disk_asked.as_ref() != Some(&path) {
            self.disk_asked = Some(path.clone());
            let root = |p: &Path| p.components().next().map(|c| c.as_os_str().to_owned());
            if self.disk.as_ref().is_some_and(|d| root(&d.0) != root(&path)) {
                self.disk = None;
            }
            let (p, tx, ctx) = (path.clone(), self.tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let (free, total) = sys::disk_space(&archive::on_disk(&p)).unwrap_or((0, 0));
                let _ = tx.send(Msg::Disk(p, free, total));
                ctx.request_repaint();
            });
        }
        let title = format!("{} — {APP_NAME}", self.display_path(&path));
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// The tree from the keyboard, as in Explorer: Up/Down/Home/End move, Right expands or steps
    /// into the first child, Left collapses or goes to the parent, Enter/Space opens the folder.
    fn tree_keys(&mut self, ctx: &egui::Context) {
        let k = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        let Some((at, id)) = self.tree_focus.clone() else { return };
        let rows = &self.tree_rows;
        // by index, since a bookmarked folder shows twice; by id once rows above it changed
        let Some(i) = rows.get(at).filter(|r| r.0 == id).map(|_| at).or_else(|| rows.iter().position(|r| r.0 == id)) else {
            self.tree_focus = None;
            return;
        };
        let (_, target, expandable, depth) = rows[i].clone();
        let last = rows.len() - 1;
        let open = self.tree_open.contains(&id);
        let to = if k(Key::ArrowDown) {
            Some((i + 1).min(last))
        } else if k(Key::ArrowUp) {
            Some(i.saturating_sub(1))
        } else if k(Key::Home) {
            Some(0)
        } else if k(Key::End) {
            Some(last)
        } else if k(Key::ArrowRight) {
            if expandable && !open {
                self.tree_open.insert(id.clone());
                None
            } else {
                open.then_some((i + 1).min(last))
            }
        } else if k(Key::ArrowLeft) {
            if expandable && open {
                self.tree_open.remove(&id);
                None
            } else {
                rows[..i].iter().rposition(|r| r.3 < depth)
            }
        } else {
            if k(Key::Enter) || k(Key::Space) {
                match target {
                    Some(t) => self.pending.push(Act::Goto(t)),
                    None if !self.tree_open.remove(&id) => {
                        self.tree_open.insert(id.clone());
                    }
                    None => {}
                }
            }
            None
        };
        if let Some(j) = to {
            self.tree_focus = Some((j, rows[j].0.clone()));
            self.tree_scroll = true;
        }
        // keeps the index right after the row moved (rows above it expanded)
        if to.is_none() && i != at {
            self.tree_focus = Some((i, id));
        }
    }

    pub(crate) fn keys(&mut self, ctx: &egui::Context) {
        // typing belongs to the text field; a button reached with Tab keeps only Enter and the
        // arrows, so one Tab press no longer switches every shortcut off
        // an open menu, the about box or the password dialog owns the keys (egui moves through a menu with the arrows)
        if ctx.text_edit_focused() || self.about || self.ask_pw.is_some() || self.ask_pack.is_some() || self.ask.is_some() || crate::ops::popup_blocks(ctx) {
            return;
        }
        let widget_focus = ctx.memory(|m| m.focused());
        let ctrl = Modifiers::CTRL;
        let k = |m: Modifiers, key: Key| ctx.input_mut(|i| i.consume_key(m, key));
        if let Some(id) = widget_focus {
            if k(Modifiers::NONE, Key::Escape) {
                ctx.memory_mut(|m| m.surrender_focus(id));
            }
        }
        // the context menu from the keyboard: the focused item's, else the list's own (before
        // F10, which consume_key would also match with Shift held)
        if k(Modifiers::SHIFT, Key::F10) || k(Modifiers::NONE, Key::Menu) {
            let t = &mut self.tabs[self.cur];
            let at = t.focus.as_ref().and_then(|f| t.view.iter().position(|&i| &t.all[i].path == f));
            self.want_menu = if is_pc(&t.path) {
                t.focus.clone()
            } else if let Some(i) = at {
                // scrolled into view, so its row is drawn and can carry the menu
                t.scroll_to = Some(i);
                t.focus.clone()
            } else {
                Some(t.path.clone())
            };
        }
        // the menu bar: F10 (or Alt+F) opens Plik, Alt+E/W/N the others; Alt+P stays the preview
        // pane, as in Explorer. Focus on the button lets the arrow keys enter the menu.
        let menu = if k(Modifiers::NONE, Key::F10) || k(Modifiers::ALT, Key::F) {
            Some(0)
        } else if k(Modifiers::ALT, Key::E) {
            Some(1)
        } else if k(Modifiers::ALT, Key::W) {
            Some(2)
        } else if k(Modifiers::ALT, Key::N) {
            Some(3)
        } else {
            None
        };
        if let Some(m) = menu {
            let id = self.menus[m];
            ctx.memory_mut(|mem| mem.request_focus(id));
            egui::Popup::open_id(ctx, id.with("popup"));
            return;
        }
        if k(Modifiers::ALT, Key::P) {
            self.show_preview = !self.show_preview;
        }
        // F6 cycles the regions, as in Explorer: tree, list (in split view: left, then right pane)
        if k(Modifiers::NONE, Key::F6) {
            let right = self.split && self.tabs[self.cur].side == 1;
            if self.tree_focus.is_some() {
                self.tree_focus = None;
                if right {
                    std::mem::swap(&mut self.cur, &mut self.other);
                }
                self.kbd_focus = true;
            } else if self.split && !right {
                std::mem::swap(&mut self.cur, &mut self.other);
                self.kbd_focus = true;
            } else {
                let here = self.tabs[self.cur].path.clone();
                let i = self.tree_rows.iter().position(|r| r.1.as_ref() == Some(&here)).unwrap_or(0);
                self.tree_focus = self.tree_rows.get(i).map(|r| (i, r.0.clone()));
                self.tree_scroll = true;
            }
        }
        let in_tree = self.tree_focus.is_some();
        // consume_key ignores extra Alt/Shift, so every chord comes before its plain key
        // (Alt+Enter before Enter, Ctrl+Shift+C before Ctrl+C)
        let table: [(Modifiers, Key, Act); 23] = [
            (ctrl | Modifiers::SHIFT, Key::F, Act::FilterNames),
            (ctrl, Key::H, Act::ToggleHidden),
            (Modifiers::ALT, Key::ArrowLeft, Act::Back),
            (Modifiers::ALT, Key::ArrowRight, Act::Forward),
            (Modifiers::ALT, Key::ArrowUp, Act::Up),
            (Modifiers::NONE, Key::Backspace, Act::Up),
            (Modifiers::NONE, Key::F5, Act::Refresh),
            (Modifiers::NONE, Key::F3, Act::ToggleSplit),
            (Modifiers::NONE, Key::F2, Act::Rename),
            (Modifiers::NONE, Key::Delete, Act::Delete),
            (Modifiers::ALT, Key::Enter, Act::Properties),
            (Modifiers::NONE, Key::Enter, Act::Open),
            (ctrl | Modifiers::SHIFT, Key::C, Act::CopyPath),
            (ctrl | Modifiers::SHIFT, Key::N, Act::NewFolder),
            (ctrl, Key::A, Act::SelectAll),
            (ctrl, Key::C, Act::Copy(false)),
            (ctrl, Key::X, Act::Copy(true)),
            (ctrl, Key::V, Act::Paste),
            (ctrl, Key::Z, Act::Undo),
            (ctrl, Key::T, Act::NewTab(None)),
            (ctrl, Key::W, Act::CloseTab(self.cur)),
            (ctrl, Key::B, Act::MakeBak),
            (Modifiers::NONE, Key::Space, Act::SizeSelected),
        ];
        for (m, key, a) in table {
            // a focused button takes Enter and Space itself
            if widget_focus.is_some() && m == Modifiers::NONE && matches!(key, Key::Enter | Key::Space) {
                continue;
            }
            // with the tree focused, keys meant for the list's selection do nothing (Enter
            // opens the tree's folder below)
            let on_sel = matches!(a, Act::Rename | Act::Delete | Act::Open | Act::Properties | Act::Copy(_) | Act::SelectAll | Act::MakeBak | Act::CopyPath | Act::SizeSelected);
            if in_tree && on_sel {
                continue;
            }
            if k(m, key) {
                self.pending.push(a);
            }
        }
        // the address bar and the search box, as in Explorer
        if k(ctrl, Key::L) || k(Modifiers::ALT, Key::D) || k(Modifiers::NONE, Key::F4) {
            let p = &self.tabs[self.cur].path;
            self.addr_edit = Some(if is_pc(p) { String::new() } else { p.display().to_string() });
        }
        if k(ctrl, Key::F) || k(ctrl, Key::E) {
            // the box may be showing the filter; Ctrl+F means search
            self.filter_open = false;
            ctx.memory_mut(|m| m.request_focus(egui::Id::unique("search")));
        }
        // Ctrl+Tab / Ctrl+Shift+Tab: the next / previous tab of the active pane
        let back = k(ctrl | Modifiers::SHIFT, Key::Tab);
        if back || k(ctrl, Key::Tab) {
            let side = self.tabs[self.cur].side;
            let on: Vec<usize> = (0..self.tabs.len()).filter(|&i| !self.split || self.tabs[i].side == side).collect();
            if let Some(at) = on.iter().position(|&i| i == self.cur) {
                let n = on.len();
                self.cur = on[if back { (at + n - 1) % n } else { (at + 1) % n }];
            }
        }
        if widget_focus.is_some() {
            return;
        }
        if in_tree {
            self.tree_keys(ctx);
            return;
        }
        // arrow keys move the selection; Shift extends it
        let shift = ctx.input(|i| i.modifiers.shift);
        let t = &mut self.tabs[self.cur];
        let n = t.view.len();
        if n == 0 {
            return;
        }
        let cur = t.focus.as_ref().and_then(|f| t.view.iter().position(|&i| &t.all[i].path == f));
        let step = |d: i64| cur.map_or(0, |c| (c as i64 + d).clamp(0, n as i64 - 1) as usize);
        // the Analiza miejsca tree, as in Explorer's folder pane: → opens a folder or steps into
        // it, ← closes it or goes up to the folder holding the row
        let mut into = None;
        if let (true, false, Some(c)) = (self.space, t.found, cur) {
            let e = &t.all[t.view[c]];
            let open = e.dir && t.expanded.contains(&e.path);
            if k(Modifiers::NONE, Key::ArrowRight) && e.dir {
                if !open {
                    self.pending.push(Act::ToggleExpand(e.path.clone()));
                } else if c + 1 < n && t.all[t.view[c + 1]].depth > e.depth {
                    into = Some(c + 1);
                }
            } else if k(Modifiers::NONE, Key::ArrowLeft) {
                if open {
                    self.pending.push(Act::ToggleExpand(e.path.clone()));
                } else if e.depth > 0 {
                    into = (0..c).rev().find(|&j| t.all[t.view[j]].depth < e.depth);
                }
            }
        }
        let target = if into.is_some() {
            into
        } else if k(Modifiers::NONE, Key::ArrowDown) || k(Modifiers::SHIFT, Key::ArrowDown) {
            Some(step(1))
        } else if k(Modifiers::NONE, Key::ArrowUp) || k(Modifiers::SHIFT, Key::ArrowUp) {
            Some(step(-1))
        } else if k(Modifiers::NONE, Key::Home) || k(Modifiers::SHIFT, Key::Home) {
            Some(0)
        } else if k(Modifiers::NONE, Key::End) || k(Modifiers::SHIFT, Key::End) {
            Some(n - 1)
        } else if k(Modifiers::NONE, Key::PageDown) {
            Some(step(20))
        } else if k(Modifiers::NONE, Key::PageUp) {
            Some(step(-20))
        } else {
            None
        };
        // type-ahead, as in Explorer: letters typed within 0.8 s jump to the next name starting with them
        let typed: String = ctx.input(|i| i.events.iter().filter_map(|e| if let egui::Event::Text(s) = e { Some(s.to_lowercase()) } else { None }).collect());
        let target = target.or_else(|| {
            if typed.trim().is_empty() {
                return None;
            }
            let now = ctx.input(|i| i.time);
            if now - self.typed_at > 0.8 {
                self.typed.clear();
            }
            self.typed_at = now;
            self.typed.push_str(&typed);
            // a fresh letter moves on from the current item; more letters refine it in place
            let start = cur.map_or(0, |c| if self.typed.chars().count() == 1 { c + 1 } else { c });
            (0..n).map(|k| (start + k) % n).find(|&k| t.all[t.view[k]].name.to_lowercase().starts_with(&self.typed))
        });
        if let Some(i) = target {
            // Shift with a typed letter only makes it a capital
            select_at(t, i, shift && typed.is_empty(), false);
            t.scroll_to = Some(i);
            self.kbd_focus = true;
        }
    }
}

/// Reads the drives with their names on a thread of its own (a network drive that does not
/// answer takes seconds) and sends them as Msg::Drives; `places`: on another one the places of
/// Widok główny and the cloud folders, read once at start. main() calls it before there is a
/// window, so the answers are there for the first frame.
pub(crate) fn read_drives(tx: Sender<Msg>, places: bool) {
    if places {
        let tx = tx.clone();
        std::thread::spawn(move || {
            sys::init_com();
            let mut known: Vec<(PathBuf, String, Kind)> = [
                (dirs::desktop_dir(), Kind::Desktop),
                (dirs::download_dir(), Kind::Downloads),
                (dirs::document_dir(), Kind::Doc),
                (dirs::picture_dir(), Kind::Image),
                (dirs::audio_dir(), Kind::Audio),
                (dirs::video_dir(), Kind::Video),
            ]
            .into_iter()
            .filter_map(|(p, k)| p.filter(|p| p.is_dir()).map(|p| (p.clone(), sys::display_name(&p).unwrap_or_default(), k)))
            .collect();
            // the cloud places follow in Widok główny: what Explorer pins (OneDrive…) and Google Drive's
            // mirrored folder; its streamed drive is added live by the tree, as it mounts after sign-in
            // shortcut: the mirrored folder is looked for at its default place only, home\My Drive
            let mut clouds = sys::cloud_folders();
            clouds.extend(dirs::home_dir().map(|h| h.join("My Drive")).filter(|p| p.is_dir()).map(|p| (p, "Google Drive".into())));
            for (p, name) in &clouds {
                if !known.iter().any(|k| k.0 == *p) {
                    known.push((p.clone(), name.clone(), Kind::Cloud));
                }
            }
            let _ = tx.send(Msg::Places(known, clouds.into_iter().map(|c| c.0).collect()));
            wake();
        });
    }
    std::thread::spawn(move || {
        sys::init_com();
        // this computer's drives first: a network drive that does not answer then holds up only
        // its own name
        let mask = sys::drive_mask();
        let net = sys::drive_letters(mask).iter().filter(|d| d.1).fold(0, |m, d| m | rx_core::drive_bit(&d.0.to_string_lossy()));
        for part in [mask & !net, net].into_iter().filter(|part| *part != 0) {
            let _ = tx.send(Msg::Drives(part, sys::drives_of(part)));
            wake();
        }
    });
}

/// `stem.ext` in `dir`, or `stem (2).ext`, `stem (3).ext`… when taken on disk or in `taken`; an
/// empty `ext` names a folder.
pub(crate) fn free_name(dir: &Path, stem: &str, ext: &str, taken: &[PathBuf]) -> PathBuf {
    let name = |n: usize| {
        let s = if n == 1 { stem.to_string() } else { format!("{stem} ({n})") };
        if ext.is_empty() { s } else { format!("{s}.{ext}") }
    };
    (1..).map(|n| dir.join(name(n))).find(|p| !p.exists() && !taken.contains(p)).unwrap()
}

/// Delete to the Recycle Bin (off the UI thread): what Ctrl+Z restores, and a note when part of it
/// was deleted for good.
fn recycled(paths: Vec<PathBuf>) -> (Result<bool, String>, Vec<Msg>) {
    let since = SystemTime::now();
    let ok = sys::recycle(&paths);
    let until = SystemTime::now();
    let (u, note) = rx_core::undo::deleted(&paths, |p| p.symlink_metadata().is_err(), sys::has_bin, since, until);
    (Ok(ok), std::iter::once(Msg::Undo(u)).chain(note.map(|n| Msg::Note(n.into()))).collect())
}

/// What an operation works on, for the Operacje panel: "a.txt", or "a.txt (+4)" for more.
pub(crate) fn names(paths: &[PathBuf]) -> String {
    let first = paths.first().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if paths.len() > 1 { format!("{first} (+{})", paths.len() - 1) } else { first }
}

/// Where an operation puts things, as Explorer names it ("Dokumenty", "Kopie (Z:)"). The shell
/// reads the folder for its name, and a share that does not answer would hold the window for
/// seconds: on a network path the folder's own name is shown.
pub(crate) fn place(p: &Path) -> String {
    let own = || p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
    let net = p.to_string_lossy().starts_with(r"\\") || p.ancestors().last().is_some_and(|root| sys::drive_type(root) == 4);
    if net { own() } else { sys::display_name(p).unwrap_or_else(own) }
}

/// A copy or move's undo record, and a note when some items cannot be taken back; with whether
/// nothing was cancelled, or what the app's own copy stopped on (shown as a note too): Err an
/// error (stays in the history), Ok a cancel from the panel (the job goes, the note says how far).
fn transferred((ok, u, left, stopped): (bool, Undo, usize, Option<Result<String, String>>)) -> (Result<bool, String>, Vec<Msg>) {
    let note = (left > 0).then(|| Msg::Note(t!("actions.taken_names_note", n = left)));
    let said = stopped.clone().map(|s| Msg::Note(s.unwrap_or_else(|e| e)));
    let msgs = std::iter::once(Msg::Undo(u)).chain(note).chain(said).collect();
    (match stopped {
        Some(Err(e)) => Err(e),
        _ => Ok(ok),
    }, msgs)
}

/// Takes back one operation (off the UI thread), never overwriting anything; the note says how it
/// went, with whether it was a full success.
fn undo(u: Undo) -> (bool, String) {
    let what = u.done();
    let failed = |f: Vec<(PathBuf, &str)>, n: usize| match f.first() {
        None => (true, what.clone()),
        Some((p, why)) => (false, t!("actions.undo.failed", k = f.len(), n = n, name = p.file_name().unwrap_or_default().to_string_lossy(), why = why)),
    };
    match u {
        Undo::Rename(old, new) => sys::rename_new(&new, &old).map_or_else(|e| (false, t!("actions.undo.rename_failed", e = e)), |_| (true, what)),
        Undo::Created(items, noun) => {
            // something else put at a recorded path since (a move with replacing, Explorer) is not
            // what this operation made, so it stays
            let (there, changed) = rx_core::undo::unchanged(items, rx_core::undo::stamp);
            let skipped = if changed > 0 { t!("actions.undo.skipped_changed", n = changed) } else { String::new() };
            if there.is_empty() {
                (false, if changed > 0 { t!("actions.undo.all_changed", n = changed) } else { t!("actions.undo.none_left").into() })
            } else if noun == "undo.what.copy" && !there.iter().all(|p| sys::has_bin(p)) {
                // the bin is the only safe way back for copies; a new folder or .bak gets the shell's
                // warning before a delete for good, as before Ctrl+Z had a stack
                (false, t!("actions.undo.no_bin").into())
            } else if sys::recycle(&there) {
                (true, t!("actions.undo.to_bin", what = what, n = there.len()) + &skipped)
            } else {
                (false, t!("actions.undo.stopped").into())
            }
        }
        Undo::Moved(items) => failed(sys::move_back(&items), items.len()),
        Undo::Recycled(paths, since, until) => match sys::bin::restore(&paths, since, until) {
            Ok(f) => failed(f, paths.len()),
            Err(e) => (false, t!("actions.undo.bin_no_answer", e = e)),
        },
        // taken on the UI thread
        Undo::Blocked(_) => (false, what),
    }
}