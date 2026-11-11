//! Archives opened as folders, as in Explorer: "C:\Pobrane\paczka.zip\Dokumenty" lists that folder
//! of the archive (read from its list, nothing unpacked). A file opened or previewed from it is
//! unpacked to a temporary folder first, and a copy opened for editing is watched: once saved, the
//! app asks to write it back. ZIP and 7z take every change (paste in, new folder, rename, delete)
//! through rx_archive::update, which never leaves a half-written archive; RAR is read-only and
//! offers a ZIP copy instead. Copying out of an archive unpacks into the folder pasted to.

use super::*;
use rx_archive::Change;

/// The archive file a path goes into and the place inside it, '/'-separated ("" for its top):
/// "C:\a\b.zip\x\y" → ("C:\a\b.zip", "x/y"). None for a path on disk. The outermost archive wins
/// ("a.zip\b.zip" is an entry of a.zip); a folder named like an archive is a folder.
pub(crate) fn split_archive(p: &Path) -> Option<(PathBuf, String)> {
    let mut up: Vec<&Path> = p.ancestors().collect();
    up.reverse();
    let arc = up.into_iter().find(|a| rx_archive::can_extract(a) && a.is_file())?;
    let inner = p.strip_prefix(arc).ok()?.iter().map(|c| c.to_string_lossy()).collect::<Vec<_>>().join("/");
    Some((arc.to_path_buf(), inner))
}

/// A path inside an archive (not the archive file itself).
pub(crate) fn is_inside(p: &Path) -> bool {
    split_archive(p).is_some_and(|(_, inner)| !inner.is_empty())
}

/// The folder on disk for a path that may be inside an archive: the one holding the archive.
pub(crate) fn on_disk(p: &Path) -> PathBuf {
    match split_archive(p) {
        Some((arc, _)) => arc.parent().map_or(arc.clone(), Path::to_path_buf),
        None => p.to_path_buf(),
    }
}

/// An entry's path: the archive's path with the entry's name parts after it.
fn path_of(arc: &Path, name: &str) -> PathBuf {
    name.split('/').filter(|s| !s.is_empty()).fold(arc.to_path_buf(), |p, part| p.join(part))
}

/// The archive's list, read once per version of the file (its size and time).
fn entries(arc: &Path) -> Result<Arc<Vec<rx_archive::Entry>>, String> {
    type Cached = (PathBuf, (u64, Option<SystemTime>), Arc<Vec<rx_archive::Entry>>);
    static CACHE: OnceLock<Mutex<Vec<Cached>>> = OnceLock::new();
    let md = std::fs::metadata(arc).map_err(|e| e.to_string())?;
    let key = (md.len(), md.modified().ok());
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap().iter().find(|c| c.0 == arc && c.1 == key) {
        return Ok(hit.2.clone());
    }
    let list = Arc::new(rx_archive::list(arc).map_err(|e| match e {
        rx_archive::Fail::Password => t!("archive.list_encrypted").to_string(),
        rx_archive::Fail::Other(e) => t!("archive.cant_open", e = e),
    })?);
    let mut c = cache.lock().unwrap();
    c.retain(|x| x.0 != arc);
    c.insert(0, (arc.to_path_buf(), key, list.clone()));
    // a few archives at hand (tabs, Back); a big list is tens of MB
    c.truncate(4);
    Ok(list)
}

/// Folder `inner` is in the list: as an entry of its own, or in its files' names (ZIP).
fn has_folder(all: &[rx_archive::Entry], inner: &str) -> bool {
    all.iter().any(|e| e.name == inner && e.dir || e.name.strip_prefix(inner).is_some_and(|r| r.starts_with('/')))
}

/// A path that opens as a folder: an archive file, or a folder inside one.
pub(crate) fn is_folder(p: &Path) -> bool {
    match split_archive(p) {
        Some((_, inner)) if inner.is_empty() => true,
        Some((arc, inner)) => entries(&arc).is_ok_and(|all| has_folder(&all, &inner)),
        None => false,
    }
}

/// The rows of folder `inner` of `arc`, as list_dir gives a folder's (folders with their sizes).
pub(crate) fn list(arc: &Path, inner: &str) -> Result<Vec<Entry>, String> {
    let all = entries(arc)?;
    let kids = rx_archive::folder(&all, inner);
    if kids.is_empty() && !inner.is_empty() && !has_folder(&all, inner) {
        return Err(t!("archive.no_folder", inner = inner));
    }
    Ok(kids
        .into_iter()
        .map(|e| {
            let name = e.name.rsplit('/').next().unwrap_or_default().to_string();
            let recent = e.modified.and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|d| d.as_secs() < RECENT_SECS);
            Entry {
                recent,
                kind: kind_of(&name, e.dir),
                type_name: type_of(&name, e.dir),
                size: Some(e.size),
                modified: e.modified,
                attrs: 0,
                path: path_of(arc, &e.name),
                name,
                dir: e.dir,
                depth: 0,
            }
        })
        .collect())
}

/// Entries of archives grouped per archive, each with the free name it gets in `dir` ("x (2)" when
/// "x" is there or taken by an earlier pick).
fn picks(paths: &[PathBuf], dir: &Path) -> Vec<(PathBuf, Vec<(String, PathBuf)>)> {
    let mut out: Vec<(PathBuf, Vec<(String, PathBuf)>)> = vec![];
    let mut taken = vec![];
    for p in paths {
        let Some((arc, inner)) = split_archive(p).filter(|(_, i)| !i.is_empty()) else { continue };
        let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let to = if entries(&arc).is_ok_and(|all| has_folder(&all, &inner)) {
            crate::actions::free_name(dir, &name, "", &taken)
        } else {
            let (stem, ext) = name.rsplit_once('.').filter(|(s, _)| !s.is_empty()).unwrap_or((&name, ""));
            crate::actions::free_name(dir, stem, ext, &taken)
        };
        taken.push(to.clone());
        match out.iter_mut().find(|g| g.0 == arc) {
            Some(g) => g.1.push((inner, to)),
            None => out.push((arc, vec![(inner, to)])),
        }
    }
    out
}

fn fail_text(e: rx_archive::Fail) -> String {
    match e {
        rx_archive::Fail::Password => t!("archive.files_encrypted").into(),
        rx_archive::Fail::Other(e) => e,
    }
}

fn temp_root() -> PathBuf {
    std::env::temp_dir().join(APP_NAME).join("archiwa")
}

/// An archive that is itself a temporary copy (an archive opened from an archive): changes to it
/// would go to the copy, not to the archive it came from.
pub(crate) fn is_temp(arc: &Path) -> bool {
    arc.starts_with(temp_root())
}

fn hash_of(x: impl std::hash::Hash) -> String {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    x.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// A temporary copy of an archive's file, for opening and the preview: unpacked once per version
/// of the archive, then reused while it still is the entry. A copy edited since is never deleted
/// (its changes may not be in the archive yet): a fresh one goes into the next free folder.
pub(crate) fn temp_copy(p: &Path, progress: rx_archive::Progress) -> Result<PathBuf, String> {
    let (arc, inner) = split_archive(p).filter(|(_, i)| !i.is_empty()).ok_or(t!("archive.not_inside"))?;
    let md = std::fs::metadata(&arc).map_err(|e| e.to_string())?;
    let version = temp_root().join(hash_of((&arc, md.len(), md.modified().ok())));
    std::fs::create_dir_all(&version).map_err(|e| e.to_string())?;
    // used now: clean_temp keeps it for a day from here
    let _ = std::fs::write(version.join(".uzyte"), b"");
    let entry = entries(&arc)?.iter().find(|e| e.name == inner).cloned().ok_or(t!("archive.no_file"))?;
    let name = p.file_name().ok_or(t!("archive.no_name"))?;
    // a folder per entry: "A.txt" and "a.txt" of one archive are two files, not one on Windows
    let key = hash_of(&inner);
    for k in 0..100 {
        let dir = version.join(if k == 0 { key.clone() } else { format!("{key}-{k}") });
        let to = dir.join(name);
        if let Ok(have) = std::fs::metadata(&to) {
            let same_time = match (have.modified().ok(), entry.modified) {
                (Some(a), Some(b)) => a.duration_since(b).or_else(|_| b.duration_since(a)).is_ok_and(|d| d.as_secs() < 3),
                _ => true,
            };
            if have.len() == entry.size && same_time {
                return Ok(to);
            }
            continue;
        }
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        return match rx_archive::extract_entries(&arc, &[(inner, to.clone())], None, progress) {
            // another job (the preview, Open) may have unpacked it at the same moment
            _ if to.exists() => Ok(to),
            Ok(_) => Err(t!("archive.no_file").into()),
            Err(e) => Err(fail_text(e)),
        };
    }
    Err(t!("archive.too_many_copies").into())
}

/// The preview of a path inside an archive: a folder's counts from the list; a picture, PDF or
/// text from its temporary copy. Other files, files over 64 MB and anything from a 7z or RAR over
/// 64 MB (decoded from its start, every time the selection moves) wait for Open.
pub(crate) fn preview(p: &Path, px: u32) -> Raw {
    let Some((arc, inner)) = split_archive(p) else { return Raw::Nothing(String::new()) };
    if is_folder(p) {
        return match list(&arc, &inner) {
            Ok(v) => {
                let d = v.iter().filter(|e| e.dir).count();
                let b = v.iter().filter(|e| !e.dir).filter_map(|e| e.size).sum();
                Raw::Folder(d, v.len() - d, b)
            }
            Err(e) => Raw::Nothing(e),
        };
    }
    let ext = ext_of(&inner);
    if !(IMAGE_EXT.contains(&ext.as_str()) || TEXT_EXT.contains(&ext.as_str()) || ext == "pdf") {
        return Raw::Nothing(t!("archive.open_to_see").into());
    }
    let size = entries(&arc).ok().and_then(|all| all.iter().find(|e| e.name == inner).map(|e| e.size));
    let streamed = !ext_of(&arc.to_string_lossy()).eq("zip") && std::fs::metadata(&arc).is_ok_and(|m| m.len() > 64 << 20);
    if size.is_some_and(|s| s > 64 << 20) || streamed {
        return Raw::Nothing(t!("archive.too_big").into());
    }
    match temp_copy(p, &mut |_, _| {}) {
        Ok(copy) => build_preview(&copy, px, false),
        Err(e) => Raw::Nothing(t!("archive.no_preview", e = e)),
    }
}

/// Copies unpacked for opening go a day after their last use or edit (an editor may still hold
/// a newer one).
pub(crate) fn clean_temp() {
    let day = std::time::Duration::from_secs(24 * 3600);
    for e in std::fs::read_dir(temp_root()).into_iter().flatten().flatten() {
        // the newest time anywhere inside: the use mark (temp_copy) or a file saved by an editor
        let mut newest = None;
        let mut stack = vec![e.path()];
        while let Some(d) = stack.pop() {
            for f in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let Ok(m) = f.metadata() else { continue };
                if m.is_dir() {
                    stack.push(f.path());
                }
                newest = newest.max(m.modified().ok());
            }
        }
        if newest.and_then(|t| t.elapsed().ok()).is_none_or(|a| a > day) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// One change of an archive at a time: a second one would start from the archive before the first.
static WRITING: Mutex<()> = Mutex::new(());

fn work_dir() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    temp_root().join(format!("wklej-{}-{}", std::process::id(), N.fetch_add(1, AO::SeqCst)))
}

/// A file opened from an archive for editing: its copy, the archive and the entry, and the copy's
/// time when last seen (a new time means it was saved).
pub(crate) struct Watched {
    copy: PathBuf,
    arc: PathBuf,
    inner: String,
    seen: Option<SystemTime>,
    opened: std::time::Instant,
}

/// How long a copy opened from an archive is watched for saves (and holds off the update restart).
const WATCH_FOR: std::time::Duration = std::time::Duration::from_secs(4 * 3600);

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl App {
    /// Unpacks entries of archives into folder `dir` on disk, each under its own name or
    /// "name (2)": the Paste of a copy out of an archive. A cut takes them out of the archive after.
    pub(crate) fn unpack_into(&self, paths: &[PathBuf], dir: &Path, cut: bool, ctx: &egui::Context) {
        let groups = picks(paths, dir);
        if groups.is_empty() {
            return;
        }
        let title = t!("archive.extracting_to", place = crate::actions::place(dir));
        self.archive_job(title, crate::actions::names(paths), ctx, move |p| {
            // a cut holds the lock from unpacking to removing, so no save lands in between and is lost
            let _one = cut.then(|| WRITING.lock().unwrap_or_else(|e| e.into_inner()));
            let (mut files, mut kept) = (0, vec![]);
            for (arc, picks) in &groups {
                let (n, left_out) = rx_archive::extract_entries(arc, picks, None, p).map_err(fail_text)?;
                files += n;
                kept.push(left_out);
            }
            let mut note = t!("archive.extracted", n = files);
            if kept.iter().any(|&n| n > 0) {
                note += &t!("archive.skipped_more", n = kept.iter().sum::<usize>());
            }
            if cut {
                for ((arc, picks), left_out) in groups.iter().zip(kept) {
                    // what could not be written stays in the archive, and so does its folder
                    if left_out > 0 || !rx_archive::can_update(arc) || is_temp(arc) {
                        note += t!("archive.originals_kept");
                        continue;
                    }
                    rx_archive::update(arc, &Change::Remove(picks.iter().map(|x| x.0.clone()).collect()), p)?;
                }
            }
            Ok(note)
        });
    }

    /// Opens a file of an archive: unpacked to a temporary folder, then opened as any file there
    /// (an archive in an archive opens as a folder). A file of a ZIP or 7z is watched, so saving
    /// it asks to write it back.
    pub(crate) fn open_from_archive(&self, p: PathBuf, ctx: &egui::Context) {
        let tx = self.tx.clone();
        let source = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        self.archive_job(t!("archive.opening").into(), source, ctx, move |prog| {
            let copy = temp_copy(&p, prog)?;
            if let Some((arc, inner)) = split_archive(&p) {
                if rx_archive::can_update(&arc) && !is_temp(&arc) && !rx_archive::can_extract(&copy) {
                    let _ = tx.send(Msg::Watch(copy.clone(), arc, inner));
                }
            }
            let _ = tx.send(Msg::Open(copy));
            Ok(String::new())
        });
    }

    pub(crate) fn watch(&mut self, copy: PathBuf, arc: PathBuf, inner: String) {
        self.arc_watch.retain(|w| w.copy != copy);
        let seen = modified(&copy);
        self.arc_watch.push(Watched { copy, arc, inner, seen, opened: std::time::Instant::now() });
    }

    /// While any is watched (App::poll looks every 2 s): a watched copy saved since asks once
    /// whether to write it back into its archive.
    pub(crate) fn check_watched(&mut self) {
        self.arc_watch.retain(|w| w.copy.exists() && w.opened.elapsed() < WATCH_FOR);
        if self.ask.is_some() {
            return;
        }
        for w in &mut self.arc_watch {
            let now = modified(&w.copy);
            if now != w.seen {
                w.seen = now;
                let file = w.copy.file_name().unwrap_or_default().to_string_lossy();
                let arc = w.arc.file_name().unwrap_or_default().to_string_lossy();
                self.ask = Some(Ask {
                    title: t!("archive.save.title").into(),
                    text: t!("archive.save.text", file = file, arc = arc),
                    yes: t!("archive.save.yes").into(),
                    act: Act::ArchiveChange(w.arc.clone(), Change::Replace { name: w.inner.clone(), from: w.copy.clone() }, false),
                });
                return;
            }
        }
    }

    /// Makes `change` to archive `arc` in the background: the list then shows the archive's new
    /// state with what was made selected (`rename`: the first one opens for renaming, a new folder).
    /// A RAR asks to make a ZIP copy instead; an archive opened from an archive cannot be changed.
    pub(crate) fn change_archive(&mut self, arc: PathBuf, change: Change, rename: bool, ctx: &egui::Context) {
        if is_temp(&arc) {
            self.msg = Some(t!("archive.nested").into());
            return;
        }
        if !rx_archive::can_update(&arc) {
            let inner = split_archive(&self.tabs[self.cur].path).map(|x| x.1).unwrap_or_default();
            self.ask = Some(Ask {
                title: t!("archive.rar.title").into(),
                text: t!("archive.rar.text").into(),
                yes: t!("archive.rar.yes").into(),
                act: Act::ConvertArchive(arc, inner),
            });
            return;
        }
        let tx = self.tx.clone();
        let name = arc.file_name().unwrap_or_default().to_string_lossy().into_owned();
        self.archive_job(t!("archive.saving").into(), name.clone(), ctx, move |p| {
            let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
            let made = rx_archive::update(&arc, &change, p)?;
            let _ = tx.send(Msg::Changed(made.iter().map(|n| path_of(&arc, n)).collect(), rename));
            // a note is an outcome: archive_job then records that Ctrl+Z cannot take it back
            Ok(t!("archive.saved", name = name))
        });
    }

    /// Paste into a folder of an archive: files from disk go in as they are; entries of an archive
    /// (this one or another) are unpacked to a work folder first. A cut removes the originals after.
    pub(crate) fn paste_into_archive(&mut self, clip: Vec<PathBuf>, cut: bool, ctx: &egui::Context) {
        let Some((arc, into)) = split_archive(&self.tabs[self.cur].path) else { return };
        // RAR and archives opened from archives cannot be written: change_archive says why
        if is_temp(&arc) || !rx_archive::can_update(&arc) {
            return self.change_archive(arc, Change::NewFolder { into }, false, ctx);
        }
        let (mut packed, plain): (Vec<PathBuf>, Vec<PathBuf>) = clip.into_iter().partition(|p| is_inside(p));
        // one file may be reached by paths spelled differently (case, subst): compare real paths
        let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let arc_real = real(&arc);
        let low = |s: &str| s.to_lowercase();
        let own = |p: &PathBuf| split_archive(p).filter(|(a, _)| real(a) == arc_real).map(|(_, i)| i);
        let into_low = low(&into);
        if plain.iter().any(|p| arc_real.starts_with(real(p)))
            || packed.iter().filter_map(own).any(|i| into_low == low(&i) || into_low.starts_with(&(low(&i) + "/")))
        {
            self.msg = Some(t!("archive.into_itself").into());
            return;
        }
        if cut {
            // cut and pasted into the folder it already is in: nothing to do
            packed.retain(|p| own(p).is_none_or(|i| low(i.rsplit_once('/').map_or("", |x| x.0)) != into_low));
            if packed.is_empty() && plain.is_empty() {
                return;
            }
            self.clip.clear();
        }
        // a cut on the Windows clipboard: the source is told once the originals are gone
        let from_windows = self.clip_sys.then_some(self.clip_seq);
        let job = Arc::new(());
        if let Some(seq) = from_windows.filter(|_| cut) {
            self.pasting = Some((seq, job.clone()));
        }
        let work = work_dir();
        let groups = picks(&packed, &work);
        let tx = self.tx.clone();
        let name = arc.file_name().unwrap_or_default().to_string_lossy().into_owned();
        self.archive_job(t!("archive.pasting").into(), name.clone(), ctx, move |p| {
            // held from unpacking to removing the cut originals, so no save in between is lost
            let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
            let _running = &job;
            let mut from = plain.clone();
            let (mut kept, mut note) = (vec![], String::new());
            let r = (|| {
                std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
                for (src, picks) in &groups {
                    kept.push(rx_archive::extract_entries(src, picks, None, p).map_err(fail_text)?.1);
                    from.extend(picks.iter().map(|x| x.1.clone()).filter(|t| t.exists()));
                }
                let made = rx_archive::update(&arc, &Change::Add { from: from.clone(), into: into.clone() }, p)?;
                if kept.iter().any(|&n| n > 0) {
                    note = t!("archive.skipped", n = kept.iter().sum::<usize>());
                }
                if cut {
                    let mut stayed = false;
                    for ((src, picks), left_out) in groups.iter().zip(&kept) {
                        // an entry Windows cannot name was not copied, so its original stays
                        if *left_out == 0 && rx_archive::can_update(src) && !is_temp(src) {
                            rx_archive::update(src, &Change::Remove(picks.iter().map(|x| x.0.clone()).collect()), p)?;
                        } else {
                            stayed = true;
                        }
                    }
                    if stayed {
                        note += if note.is_empty() { t!("archive.originals_kept_source") } else { t!("archive.originals_kept_source_more") };
                    }
                }
                Ok::<_, String>(made)
            })();
            let _ = std::fs::remove_dir_all(&work);
            let made = r?;
            if cut && !plain.is_empty() {
                sys::init_com();
                if sys::recycle(&plain) {
                    if let Some(seq) = from_windows {
                        sys::clip::paste_done(seq);
                    }
                }
            }
            let _ = tx.send(Msg::Changed(made.iter().map(|n| path_of(&arc, n)).collect(), false));
            // a note is an outcome: archive_job then records that Ctrl+Z cannot take it back
            Ok(if note.is_empty() { t!("archive.pasted", name = name) } else { note })
        });
    }

    /// Wypakuj inside an archive: an archive in it is unpacked into a folder of its name next to
    /// the outer archive; anything else is copied out there as it is.
    pub(crate) fn extract_here(&mut self, sel: Vec<PathBuf>, ctx: &egui::Context) {
        let dir = self.tabs[self.cur].path.clone();
        let out = on_disk(&dir);
        if split_archive(&dir).is_some_and(|(arc, _)| is_temp(&arc)) {
            self.msg = Some(t!("archive.nested_extract").into());
            return;
        }
        if let [one] = sel.as_slice() {
            if rx_archive::can_extract(one) {
                let (p, tx) = (one.clone(), self.tx.clone());
                let title = t!("archive.extracting_to", place = crate::actions::place(&out));
                self.archive_job(title, crate::actions::names(std::slice::from_ref(&p)), ctx, move |prog| {
                    let copy = temp_copy(&p, prog)?;
                    let stem = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    let dest = crate::actions::free_name(&out, &stem, "", &[]);
                    let (files, _) = rx_archive::extract(&copy, &dest, None, prog).map_err(fail_text)?;
                    let _ = tx.send(Msg::Reload);
                    Ok(t!("archive.extracted_to", dest = dest.display(), n = files))
                });
                return;
            }
        }
        let picked = if sel.is_empty() { self.tab().all.iter().map(|e| e.path.clone()).collect() } else { sel };
        self.unpack_into(&picked, &out, false, ctx);
        self.msg = Some(t!("archive.extracting_msg", dir = out.display()));
    }

    /// A RAR made into a ZIP next to it, and the list moves to the same folder in the copy.
    pub(crate) fn convert_archive(&mut self, arc: PathBuf, inner: String, ctx: &egui::Context) {
        let tx = self.tx.clone();
        let source = arc.file_name().unwrap_or_default().to_string_lossy().into_owned();
        self.archive_job(t!("archive.converting").into(), source, ctx, move |p| {
            let (dir, stem) = (arc.parent().ok_or(t!("archive.no_parent"))?, arc.file_stem().unwrap_or_default().to_string_lossy().into_owned());
            let dest = crate::actions::free_name(dir, &stem, "zip", &[]);
            let files = rx_archive::convert(&arc, &dest, None, p).map_err(fail_text)?;
            let _ = tx.send(Msg::Open(path_of(&dest, &inner)));
            Ok(t!("archive.converted", name = dest.file_name().unwrap_or_default().to_string_lossy(), n = files))
        });
    }
}

/// A question with one action on yes (the Tak/Anuluj dialog).
pub(crate) struct Ask {
    pub(crate) title: String,
    pub(crate) text: String,
    pub(crate) yes: String,
    pub(crate) act: Act,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_into_archives_split_at_the_archive_file() {
        let d = std::env::temp_dir().join(format!("rx-app-arc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("folder.zip")).unwrap();
        std::fs::write(d.join("a.zip"), b"").unwrap();
        assert_eq!(split_archive(&d.join("a.zip")), Some((d.join("a.zip"), String::new())));
        assert_eq!(split_archive(&d.join("a.zip").join("x").join("b.zip")), Some((d.join("a.zip"), "x/b.zip".into())));
        // a folder that only looks like an archive is a folder
        assert_eq!(split_archive(&d.join("folder.zip").join("x")), None);
        assert!(is_inside(&d.join("a.zip").join("x")) && !is_inside(&d.join("a.zip")));
        assert_eq!(on_disk(&d.join("a.zip").join("x")), d);
        assert_eq!(path_of(&d.join("a.zip"), "x/y.txt"), d.join("a.zip").join("x").join("y.txt"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_copy_is_unpacked_once_and_again_when_edited() {
        let d = std::env::temp_dir().join(format!("rx-app-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src").join("A.txt"), "duże").unwrap();
        std::fs::write(d.join("src").join("b.txt"), "b").unwrap();
        let arc = d.join("p.zip");
        rx_archive::pack(&[d.join("src")], &d, &arc, &mut |_, _| {}).unwrap();
        let entry = arc.join("src").join("A.txt");
        let copy = temp_copy(&entry, &mut |_, _| {}).unwrap();
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "duże");
        assert_eq!(temp_copy(&entry, &mut |_, _| {}).unwrap(), copy, "reused");
        // an edited copy is not the entry any more: opened again, the archive's content comes back
        std::fs::write(&copy, "zmienione w edytorze").unwrap();
        assert_eq!(std::fs::read_to_string(temp_copy(&entry, &mut |_, _| {}).unwrap()).unwrap(), "duże");
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "zmienione w edytorze", "the edited copy is kept");
        assert_ne!(temp_copy(&arc.join("src").join("b.txt"), &mut |_, _| {}).unwrap().parent(), copy.parent(), "a folder per entry");
        assert!(temp_copy(&arc.join("src").join("nie ma.txt"), &mut |_, _| {}).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
