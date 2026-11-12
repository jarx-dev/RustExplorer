//! The app's own copy and move into a folder (Wklej, a drop): the whole tree planned first, then
//! several files at a time (one on a disk that seeks), progress in bytes. What it does not take
//! (`plan` says None) Windows' own copy still does: a name already taken in the target (its
//! conflict dialog), links, a source that cannot be read through, a folder into itself.
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::time::Duration;

use crate::undo::stamp;

/// What the platform does for a copy; Windows' in rx-sys, `std::fs` in the tests.
pub trait Fs: Sync {
    /// Copies one file to a new `to` (never over one), reporting the bytes copied so far.
    fn copy_file(&self, from: &Path, to: &Path, progress: &mut dyn FnMut(u64)) -> io::Result<()>;
    /// Renames on one volume, never over anything (std's rename on Windows replaces a file).
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Whether a rename can take `from` to `to` (one volume), rather than a copy and a delete.
    fn same_volume(&self, from: &Path, to: &Path) -> bool;
    /// After a folder was made at `to` for `from`: its attributes (hidden…), on Windows.
    fn dir_made(&self, _from: &Path, _to: &Path) {}
}

/// Work worked out before anything is touched.
#[derive(Debug, PartialEq)]
pub struct Plan {
    /// moved by a rename: (from, to)
    renames: Vec<(PathBuf, PathBuf)>,
    /// folders to make, parents first: (from, to)
    dirs: Vec<(PathBuf, PathBuf)>,
    /// (from, to, bytes); with `cut` each source goes once all of them were copied
    files: Vec<(PathBuf, PathBuf, u64)>,
    /// the items' places in the target, to tell which of them this operation made
    tops: HashSet<PathBuf>,
    total: u64,
    cut: bool,
}

/// The plan for `paths` into `dir`, or None for Windows' copy to do it (see the module).
pub fn plan(paths: &[PathBuf], dir: &Path, cut: bool, fs: &dyn Fs) -> Option<Plan> {
    let mut p = Plan { renames: vec![], dirs: vec![], files: vec![], tops: HashSet::new(), total: 0, cut };
    let mut names = HashSet::new();
    // Windows' names ignore case
    let low = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
    for src in paths {
        let name = src.file_name()?;
        let to = dir.join(name);
        // a name taken (or given twice), a folder into itself or below
        if to.symlink_metadata().is_ok() || !names.insert(name.to_string_lossy().to_lowercase()) || low(dir).starts_with(low(src)) {
            return None;
        }
        p.tops.insert(to.clone());
        if cut && fs.same_volume(src, dir) {
            src.symlink_metadata().ok()?;
            p.renames.push((src.clone(), to));
            continue;
        }
        walk(src, &to, &mut p)?;
    }
    Some(p)
}

fn walk(from: &Path, to: &Path, p: &mut Plan) -> Option<()> {
    let m = from.symlink_metadata().ok()?;
    if m.file_type().is_symlink() {
        return None;
    }
    if !m.is_dir() {
        p.total += m.len();
        p.files.push((from.to_path_buf(), to.to_path_buf(), m.len()));
        return Some(());
    }
    p.dirs.push((from.to_path_buf(), to.to_path_buf()));
    for e in std::fs::read_dir(from).ok()? {
        let e = e.ok()?;
        walk(&e.path(), &to.join(e.file_name()), p)?;
    }
    Some(())
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Done,
    /// refused before anything was made: Windows' copy may ask for the administrator's consent
    Denied,
    /// what went wrong, with the item's name; what was done before it stays
    Failed(String),
}

/// Carries out `plan`: renames, folders, then the files `workers` at a time; `progress` hears
/// (done, total) bytes on this thread about ten times a second. The first error stops the files
/// not started yet. Sources of a move go only after all of it was copied, and only those still
/// as they were when copied. With the outcome, the items' places in the target this run made
/// (what Ctrl+Z may take back: not an item someone else put there meanwhile).
pub fn run(plan: &Plan, workers: usize, fs: &dyn Fs, progress: &mut dyn FnMut(u64, u64)) -> (Outcome, Vec<PathBuf>) {
    let made = AtomicUsize::new(0);
    let tops = Mutex::new(vec![]);
    let made_it = |to: &Path| {
        made.fetch_add(1, Relaxed);
        if plan.tops.contains(to) {
            tops.lock().unwrap().push(to.to_path_buf());
        }
    };
    let fail = |p: &Path, e: io::Error| {
        if made.load(Relaxed) == 0 && e.kind() == io::ErrorKind::PermissionDenied {
            Outcome::Denied
        } else {
            Outcome::Failed(t!("copy.failed", name = name(p), e = e))
        }
    };
    let r = (|| {
        for (from, to) in &plan.renames {
            if let Err(e) = fs.rename(from, to) {
                return fail(from, e);
            }
            made_it(to);
        }
        for (from, to) in &plan.dirs {
            if let Err(e) = std::fs::create_dir(to) {
                return fail(from, e);
            }
            made_it(to);
        }
        let (next, done, stop) = (AtomicUsize::new(0), AtomicU64::new(0), AtomicBool::new(false));
        let error: Mutex<Option<(PathBuf, io::Error)>> = Mutex::new(None);
        // each source as it was copied: a move leaves one changed since (an editor's save)
        let was = Mutex::new(vec![None; plan.files.len()]);
        progress(0, plan.total);
        std::thread::scope(|s| {
            let all: Vec<_> = (0..workers.max(1))
                .map(|_| {
                    s.spawn(|| {
                        while !stop.load(Relaxed) {
                            let i = next.fetch_add(1, Relaxed);
                            let Some((from, to, len)) = plan.files.get(i) else { break };
                            was.lock().unwrap()[i] = stamp(from);
                            let mut seen = 0;
                            let r = fs.copy_file(from, to, &mut |n| {
                                done.fetch_add(n.saturating_sub(seen), Relaxed);
                                seen = seen.max(n);
                            });
                            match r {
                                Ok(()) => {
                                    // a file that grew or shrank since the plan still ends at its share
                                    done.fetch_add(len.saturating_sub(seen), Relaxed);
                                    made_it(to);
                                }
                                Err(e) => {
                                    stop.store(true, Relaxed);
                                    error.lock().unwrap().get_or_insert((from.clone(), e));
                                }
                            }
                        }
                    })
                })
                .collect();
            while !all.iter().all(|w| w.is_finished()) {
                progress(done.load(Relaxed).min(plan.total), plan.total);
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        if let Some((from, e)) = error.into_inner().unwrap() {
            return fail(&from, e);
        }
        // folders last: their attributes (read-only) could stop the files going in
        for (from, to) in &plan.dirs {
            fs.dir_made(from, to);
        }
        progress(plan.total, plan.total);
        if !plan.cut {
            return Outcome::Done;
        }
        // only what was copied, as it was: a file put into a source folder meanwhile stays, and its
        // folder too; one that cannot go (open elsewhere) does not stop the rest
        let (mut changed, mut stuck) = (0, None);
        for ((from, _, _), was) in plan.files.iter().zip(was.into_inner().unwrap()) {
            if stamp(from) != was {
                changed += 1;
            } else if let Err(e) = remove(from, false) {
                stuck.get_or_insert((from, e));
            }
        }
        for (from, _) in plan.dirs.iter().rev() {
            // not empty: what stayed above is in it
            match remove(from, true) {
                Err(e) if e.kind() != io::ErrorKind::DirectoryNotEmpty => _ = stuck.get_or_insert((from, e)),
                _ => {}
            }
        }
        match stuck {
            Some((from, e)) => Outcome::Failed(t!("copy.not_removed", name = name(from), e = e)),
            None if changed > 0 => Outcome::Failed(t!("copy.changed", n = changed)),
            None => Outcome::Done,
        }
    })();
    (r, tops.into_inner().unwrap())
}

/// A read-only file or folder goes too, as in Explorer's move.
fn remove(p: &Path, dir: bool) -> io::Result<()> {
    let rm = |p: &Path| if dir { std::fs::remove_dir(p) } else { std::fs::remove_file(p) };
    rm(p).or_else(|e| {
        let mut perm = p.symlink_metadata()?.permissions();
        if !perm.readonly() {
            return Err(e);
        }
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        std::fs::set_permissions(p, perm)?;
        rm(p)
    })
}

fn name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `std::fs` with a file that fails (by name) and one volume or two.
    struct T {
        same: bool,
        fail: Option<(&'static str, io::ErrorKind)>,
        /// a file saved anew (an editor) right after it was copied
        touch: Option<&'static str>,
    }

    const NONE: T = T { same: false, fail: None, touch: None };

    impl Fs for T {
        fn copy_file(&self, from: &Path, to: &Path, progress: &mut dyn FnMut(u64)) -> io::Result<()> {
            let named = |n: &str| from.file_name().is_some_and(|x| x == n);
            if let Some((n, kind)) = self.fail.filter(|f| named(f.0)) {
                return Err(io::Error::new(kind, n));
            }
            assert!(!to.exists(), "never over a file: {to:?}");
            let n = std::fs::copy(from, to)?;
            progress(n);
            if self.touch.is_some_and(named) {
                std::fs::write(from, "saved again, longer").unwrap();
            }
            Ok(())
        }
        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            assert!(!to.exists(), "never over anything: {to:?}");
            std::fs::rename(from, to)
        }
        fn same_volume(&self, _: &Path, _: &Path) -> bool {
            self.same
        }
    }

    fn base(name: &str) -> PathBuf {
        let b = std::env::temp_dir().join(format!("rx-copy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&b);
        for d in ["src/tree/sub", "dst"] {
            std::fs::create_dir_all(b.join(d)).unwrap();
        }
        for i in 0..20 {
            std::fs::write(b.join(format!("src/tree/sub/f{i}.txt")), vec![b'x'; 1000 + i]).unwrap();
        }
        std::fs::write(b.join("src/a.bin"), vec![7u8; 100_000]).unwrap();
        b
    }

    fn go(paths: &[PathBuf], dir: &Path, cut: bool, fs: &T) -> (Outcome, Vec<PathBuf>, Vec<(u64, u64)>) {
        let p = plan(paths, dir, cut, fs).expect("a plan");
        let mut seen = vec![];
        let (r, mut made) = run(&p, 4, fs, &mut |d, t| seen.push((d, t)));
        made.sort();
        (r, made, seen)
    }

    #[test]
    fn copies_a_tree_four_files_at_a_time_with_progress_in_bytes() {
        let b = base("tree");
        let (src, dst) = (b.join("src"), b.join("dst"));
        let (r, made, seen) = go(&[src.join("tree"), src.join("a.bin")], &dst, false, &NONE);
        let total = 100_000 + (0..20).map(|i| 1000 + i).sum::<u64>();
        assert_eq!(r, Outcome::Done);
        assert_eq!(made, [dst.join("a.bin"), dst.join("tree")]);
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0) && seen.iter().all(|s| s.1 == total) && seen.last() == Some(&(total, total)), "{seen:?}");
        assert_eq!(std::fs::read_dir(dst.join("tree/sub")).unwrap().count(), 20);
        assert_eq!(std::fs::read(dst.join("tree/sub/f7.txt")).unwrap(), vec![b'x'; 1007]);
        assert!(src.join("a.bin").exists());
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn leaves_to_windows_what_it_does_not_take() {
        let b = base("none");
        let (src, dst) = (b.join("src"), b.join("dst"));
        let fs = NONE;
        std::fs::write(dst.join("a.bin"), "there").unwrap();
        // a name taken in the target: Windows asks
        assert!(plan(&[src.join("a.bin")], &dst, false, &fs).is_none());
        // in its own folder (a copy named anew), into itself (in any letter case), the same name twice
        assert!(plan(&[src.join("tree")], &src, false, &fs).is_none());
        assert!(plan(&[src.join("tree")], &src.join("tree/sub"), false, &fs).is_none());
        assert!(plan(&[src.join("tree")], &src.join("TREE/sub"), false, &fs).is_none());
        assert!(plan(&[src.join("tree"), b.join("dst")], &src.join("tree/sub"), false, &fs).is_none());
        assert!(plan(&[src.join("gone")], &dst, false, &fs).is_none());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(src.join("a.bin"), src.join("tree/link")).unwrap();
            assert!(plan(&[src.join("tree")], &dst, false, &fs).is_none());
        }
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn a_move_renames_on_one_volume_and_copies_then_deletes_across() {
        let b = base("move");
        let (src, dst) = (b.join("src"), b.join("dst"));
        let same = T { same: true, ..NONE };
        let p = plan(&[src.join("tree")], &dst, true, &same).unwrap();
        assert!(p.files.is_empty() && p.renames.len() == 1);
        assert_eq!(run(&p, 4, &same, &mut |_, _| {}), (Outcome::Done, vec![dst.join("tree")]));
        assert!(!src.join("tree").exists() && dst.join("tree/sub/f3.txt").exists());

        let fs = NONE;
        let back = b.join("back");
        std::fs::create_dir(&back).unwrap();
        let p = plan(&[dst.join("tree"), src.join("a.bin")], &back, true, &fs).unwrap();
        // put in after the plan: not copied, so not deleted
        std::fs::write(dst.join("tree/sub/new.txt"), "new").unwrap();
        assert_eq!(run(&p, 4, &fs, &mut |_, _| {}).0, Outcome::Done);
        assert!(!src.join("a.bin").exists() && back.join("tree/sub/f3.txt").exists() && back.join("a.bin").exists());
        assert!(!dst.join("tree/sub/f3.txt").exists() && dst.join("tree/sub/new.txt").exists() && !back.join("tree/sub/new.txt").exists());

        // saved anew after it was copied: the new version stays where it was
        let fs = T { touch: Some("f4.txt"), ..NONE };
        let (r, _, _) = go(&[back.join("tree")], &src, true, &fs);
        assert!(matches!(&r, Outcome::Failed(m) if m.ends_with(": 1")), "{r:?}");
        assert_eq!(std::fs::read_to_string(back.join("tree/sub/f4.txt")).unwrap(), "saved again, longer");
        assert!(src.join("tree/sub/f4.txt").exists() && !back.join("tree/sub/f3.txt").exists());
        std::fs::remove_dir_all(&b).unwrap();
    }

    /// A source that cannot go (here its folder is read-only) does not keep the rest in place.
    #[cfg(unix)]
    #[test]
    fn a_source_that_cannot_go_leaves_only_itself() {
        use std::os::unix::fs::PermissionsExt;
        let b = base("stuck");
        let (src, dst) = (b.join("src"), b.join("dst"));
        std::fs::create_dir(src.join("tree/locked")).unwrap();
        std::fs::write(src.join("tree/locked/held.txt"), "x").unwrap();
        std::fs::set_permissions(src.join("tree/locked"), std::fs::Permissions::from_mode(0o555)).unwrap();
        let (r, _, _) = go(&[src.join("tree")], &dst, true, &NONE);
        std::fs::set_permissions(src.join("tree/locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(&r, Outcome::Failed(m) if m.contains("held.txt")), "{r:?}");
        assert!(src.join("tree/locked/held.txt").exists() && !src.join("tree/sub").exists() && dst.join("tree/sub/f19.txt").exists());
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn an_error_names_the_file_and_a_move_keeps_its_sources() {
        let b = base("fail");
        let (src, dst) = (b.join("src"), b.join("dst"));
        let fs = T { fail: Some(("f5.txt", io::ErrorKind::Other)), ..NONE };
        // a.bin first, on its own: the tree's file fails after it was made
        let (r, made, _) = go(&[src.join("a.bin"), src.join("tree")], &dst, true, &fs);
        assert!(matches!(&r, Outcome::Failed(m) if m.contains("f5.txt")), "{r:?}");
        assert!(src.join("tree/sub/f5.txt").exists() && src.join("a.bin").exists());
        // what it made, for Ctrl+Z: the folder was, the file maybe (four at a time)
        assert!(made.contains(&dst.join("tree")) && made.iter().all(|m| m.parent() == Some(dst.as_path())), "{made:?}");

        // refused before anything was made: Windows' copy gets to ask for consent
        let fs = T { fail: Some(("a.bin", io::ErrorKind::PermissionDenied)), ..NONE };
        let (d1, d2) = (b.join("d1"), b.join("d2"));
        std::fs::create_dir(&d1).unwrap();
        std::fs::create_dir(&d2).unwrap();
        assert_eq!(go(&[src.join("a.bin")], &d1, false, &fs).0, Outcome::Denied);
        // refused after a folder was made: an error like any other
        let fs = T { fail: Some(("f0.txt", io::ErrorKind::PermissionDenied)), ..NONE };
        assert!(matches!(go(&[src.join("tree")], &d2, false, &fs).0, Outcome::Failed(_)));
        std::fs::remove_dir_all(&b).unwrap();
    }
}
