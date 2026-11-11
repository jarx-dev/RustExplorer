//! Ctrl+Z: what each file operation leaves to take back, and what it refuses. The window does the
//! taking back (rx-sys); these are the rules, tested without Windows.
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Records kept; older ones go first.
pub const CAP: usize = 30;

#[derive(Clone, Debug, PartialEq)]
pub enum Undo {
    /// renamed from the first to the second
    Rename(PathBuf, PathBuf),
    /// made by the operation (named by the noun, "kopiowanie"), each as it was then: taken back to
    /// the Recycle Bin only while still the same item
    Created(Vec<(PathBuf, Stamp)>, &'static str),
    /// moved: (where it is now, where it was)
    Moved(Vec<(PathBuf, PathBuf)>),
    /// sent to the Recycle Bin between these two times: restored from there
    Recycled(Vec<PathBuf>, SystemTime, SystemTime),
    /// an operation that cannot be taken back, named in the genitive ("usunięcia na stałe")
    Blocked(&'static str),
}

impl Undo {
    /// The menu item for it.
    pub fn label(&self) -> String {
        match self {
            Undo::Rename(..) => "Cofnij zmianę nazwy".into(),
            Undo::Created(_, what) => format!("Cofnij {what}"),
            Undo::Moved(_) => "Cofnij przeniesienie".into(),
            Undo::Recycled(..) => "Cofnij usunięcie".into(),
            Undo::Blocked(what) => format!("Nie da się cofnąć {what}"),
        }
    }
}

/// What tells an item from another put at its path later: size, last write and creation time
/// (None: nothing there).
pub type Stamp = Option<(u64, Option<SystemTime>, Option<SystemTime>)>;

pub fn stamp(p: &Path) -> Stamp {
    p.symlink_metadata().ok().map(|m| (m.len(), m.modified().ok(), m.created().ok()))
}

/// Of the items a record made, those still as they were made (`stamp` asks now), and how many
/// were replaced or changed since; those gone are neither.
pub fn unchanged(items: Vec<(PathBuf, Stamp)>, stamp: impl Fn(&Path) -> Stamp) -> (Vec<PathBuf>, usize) {
    let mut changed = 0;
    let same = items
        .into_iter()
        .filter_map(|(p, was)| match stamp(&p) {
            None => None,
            now if now == was => Some(p),
            _ => {
                changed += 1;
                None
            }
        })
        .collect();
    (same, changed)
}

/// The undo list, newest last, at most `CAP` long.
#[derive(Default)]
pub struct Stack(Vec<Undo>);

impl Stack {
    /// A record with nothing in it (every item was left out) is not kept.
    pub fn push(&mut self, u: Undo) {
        let empty = match &u {
            Undo::Created(v, _) => v.is_empty(),
            Undo::Recycled(v, ..) => v.is_empty(),
            Undo::Moved(v) => v.is_empty(),
            _ => false,
        };
        if empty {
            return;
        }
        if self.0.len() >= CAP {
            self.0.remove(0);
        }
        self.0.push(u);
    }
    pub fn pop(&mut self) -> Option<Undo> {
        self.0.pop()
    }
    pub fn top(&self) -> Option<&Undo> {
        self.0.last()
    }
}

/// What a copy (with `cut`: a move) of `srcs` into `dir` can be taken back by, and how many items
/// were left out. `before`: whether each source's name in `dir` was taken before the operation;
/// `renamed`: the (old, new) paths the shell gave items it named anew on a collision; `exists` is
/// asked after it. An item that landed on a name taken before (it replaced or was merged into what
/// was there, or was skipped) is left out: taking it back could lose what was there. With every
/// item left out the record says the operation cannot be taken back, so Ctrl+Z does not fall
/// through to an older one.
pub fn transferred(srcs: &[PathBuf], dir: &Path, cut: bool, before: &[bool], renamed: &[(PathBuf, PathBuf)], stamp: impl Fn(&Path) -> Stamp) -> (Undo, usize) {
    let exists = |p: &Path| stamp(p).is_some();
    let (mut made, mut moved, mut left) = (vec![], vec![], 0);
    for (src, &taken) in srcs.iter().zip(before) {
        let Some(name) = src.file_name() else { continue };
        let target = dir.join(name);
        let to = match renamed.iter().find(|(old, _)| old == src || *old == target) {
            Some((_, new)) => Some(new.clone()),
            None if !taken => Some(target),
            // onto itself (a move into its own folder): nothing happened to it
            None if *src == target => None,
            None => {
                left += 1;
                None
            }
        };
        let Some(to) = to.filter(|t| t != src && exists(t)) else { continue };
        if !cut {
            made.push((to.clone(), stamp(&to)));
        } else if !exists(src) {
            moved.push((to, src.clone()));
        }
    }
    let u = match cut {
        _ if left > 0 && made.is_empty() && moved.is_empty() => Undo::Blocked(if cut { "przeniesienia na zajęte nazwy" } else { "kopiowania na zajęte nazwy" }),
        true => Undo::Moved(moved),
        false => Undo::Created(made, "kopiowanie"),
    };
    (u, left)
}

/// What a delete to the Recycle Bin leaves: the items gone from a drive with a bin (`has_bin`) went
/// there and can come back; those gone from one without were deleted for good, which only a
/// `Blocked` record and the returned note say.
pub fn deleted(paths: &[PathBuf], gone: impl Fn(&Path) -> bool, has_bin: impl Fn(&Path) -> bool, since: SystemTime, until: SystemTime) -> (Undo, Option<&'static str>) {
    let (binned, lost): (Vec<PathBuf>, Vec<PathBuf>) = paths.iter().filter(|p| gone(p)).cloned().partition(|p| has_bin(p));
    match (binned.is_empty(), lost.is_empty()) {
        (_, true) => (Undo::Recycled(binned, since, until), None),
        (true, false) => (Undo::Blocked("usunięcia z dysku bez Kosza"), Some("Usunięto na stałe (ten dysk nie ma Kosza): tego nie da się cofnąć")),
        (false, false) => (Undo::Recycled(binned, since, until), Some("Część usunięto na stałe (dysk bez Kosza): Ctrl+Z przywróci tylko to, co jest w Koszu")),
    }
}

/// Whether a Recycle Bin item, deleted from folder `from` under the name `name` at `at`, is `path`
/// sent there between `since` and `until`. The bin may show the name without its extension, as
/// Explorer does when extensions are hidden.
pub fn bin_match(path: &Path, from: &str, name: &str, at: SystemTime, since: SystemTime, until: SystemTime) -> bool {
    // the bin's clock is the moment it wrote its record, a little after the shell started
    let slack = Duration::from_secs(2);
    if at + slack < since || at > until + slack {
        return false;
    }
    let s = path.to_string_lossy();
    let Some((dir, file)) = s.rsplit_once(['\\', '/']) else { return false };
    let stem = file.rsplit_once('.').map_or(file, |x| x.0);
    let same = |a: &str, b: &str| a.trim_end_matches(['\\', '/']).eq_ignore_ascii_case(b.trim_end_matches(['\\', '/']));
    // "C:\" is the parent of a file in a drive's root
    let dir = if dir.ends_with(':') { format!("{dir}\\") } else { dir.to_string() };
    same(&dir, from) && (name.eq_ignore_ascii_case(file) || (!stem.is_empty() && name.eq_ignore_ascii_case(stem)))
}

/// Whether the bin's own file of an item (`$R…` plus the original's extension) has the extension of
/// `path`: with extensions hidden the bin shows "a.txt" and "a.log" both as "a", and this tells them apart.
pub fn bin_same_ext(path: &Path, bin_file: &str) -> bool {
    let ext = |f: &str| f.rsplit(['\\', '/']).next().unwrap_or_default().rsplit_once('.').map_or(String::new(), |x| x.1.to_lowercase());
    !bin_file.is_empty() && ext(&path.to_string_lossy()) == ext(bin_file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    const ONE: Stamp = Some((1, None, None));

    #[test]
    fn a_copy_takes_back_only_what_it_made() {
        let srcs = [p("/a/new.txt"), p("/a/old.txt"), p("/a/same.txt"), p("/a/skipped.txt")];
        let renamed = [(p("/a/same.txt"), p("/b/same (2).txt"))];
        // old.txt was in /b before: replaced or skipped, never sent to the bin
        let before = [false, true, true, false];
        let stamp = |q: &Path| ONE.filter(|_| q != Path::new("/b/skipped.txt"));
        let (u, left) = transferred(&srcs, Path::new("/b"), false, &before, &renamed, stamp);
        assert_eq!(u, Undo::Created(vec![(p("/b/new.txt"), ONE), (p("/b/same (2).txt"), ONE)], "kopiowanie"));
        assert_eq!(left, 1);
        // all of it onto taken names: nothing to take back, said so instead of an older record's undo
        let (u, left) = transferred(&srcs[1..2], Path::new("/b"), false, &[true], &[], stamp);
        assert_eq!((u, left), (Undo::Blocked("kopiowania na zajęte nazwy"), 1));
    }

    #[test]
    fn an_item_replaced_since_is_not_taken_back() {
        let made = vec![(p("/b/a.txt"), ONE), (p("/b/moved-in.txt"), ONE), (p("/b/gone.txt"), ONE)];
        let now = |q: &Path| match q.to_str() {
            Some("/b/a.txt") => ONE,
            Some("/b/moved-in.txt") => Some((7, None, None)),
            _ => None,
        };
        assert_eq!(unchanged(made, now), (vec![p("/b/a.txt")], 1));
    }

    #[test]
    fn a_move_goes_back_only_where_nothing_was_merged() {
        let srcs = [p("/a/x"), p("/a/merged"), p("/a/stayed"), p("/b/here")];
        let gone = [p("/a/x"), p("/a/merged")];
        let stamp = |q: &Path| ONE.filter(|_| !gone.iter().any(|g| g == q));
        let (u, left) = transferred(&srcs, Path::new("/b"), true, &[false, true, false, true], &[], stamp);
        assert_eq!(u, Undo::Moved(vec![(p("/b/x"), p("/a/x"))]));
        assert_eq!(left, 1);
    }

    #[test]
    fn a_delete_without_a_bin_is_blocked_and_said() {
        let t = SystemTime::UNIX_EPOCH;
        let paths = [p("C:/a"), p("N:/b"), p("C:/kept")];
        let gone = |q: &Path| q != Path::new("C:/kept");
        let bin = |q: &Path| q.starts_with("C:");
        let (u, note) = deleted(&paths, gone, bin, t, t);
        assert_eq!(u, Undo::Recycled(vec![p("C:/a")], t, t));
        assert!(note.is_some());
        let (u, note) = deleted(&paths[1..2], gone, bin, t, t);
        assert_eq!(u, Undo::Blocked("usunięcia z dysku bez Kosza"));
        assert!(note.is_some());
    }

    #[test]
    fn the_stack_is_bounded_and_skips_empty_records() {
        let mut s = Stack::default();
        s.push(Undo::Moved(vec![]));
        assert!(s.top().is_none());
        for i in 0..CAP + 5 {
            s.push(Undo::Rename(p(&i.to_string()), p("x")));
        }
        assert_eq!(s.0.len(), CAP);
        assert_eq!(s.0[0], Undo::Rename(p("5"), p("x")));
        assert_eq!(s.top().unwrap().label(), "Cofnij zmianę nazwy");
    }

    #[test]
    fn a_bin_item_matches_by_folder_name_and_time() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let f = Path::new(r"C:\Users\x\rx-test.txt");
        assert!(bin_match(f, r"C:\Users\X", "rx-test.txt", t, t, t));
        assert!(bin_match(f, r"C:\Users\x\", "rx-test", t + Duration::from_secs(1), t, t));
        assert!(!bin_match(f, r"C:\Users", "rx-test.txt", t, t, t));
        assert!(!bin_match(f, r"C:\Users\x", "rx-test.log", t, t, t));
        assert!(!bin_match(f, r"C:\Users\x", "rx-test.txt", t - Duration::from_secs(60), t, t));
        assert!(bin_match(Path::new(r"D:\a.txt"), r"D:\", "a.txt", t, t, t));
        assert!(bin_same_ext(f, r"C:\$Recycle.Bin\S-1\$RAB12CD.TXT"));
        assert!(!bin_same_ext(f, r"C:\$Recycle.Bin\S-1\$RAB12CD.log"));
        assert!(bin_same_ext(Path::new(r"C:\x\Makefile"), r"C:\$Recycle.Bin\S-1\$RAB12CD"));
        assert!(!bin_same_ext(f, ""));
    }
}
