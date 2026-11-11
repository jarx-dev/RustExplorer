//! Whole-drive file name index for "szukaj wszędzie", in the spirit of voidtools Everything:
//! a parallel scan of the drives, flat columns in memory, a linear scan per query and a file on
//! disk between searches (nothing stays in RAM while search is not in use).
//!
//! shortcut: a full rescan per use instead of the NTFS USN journal; the laptop's journal (32 MB)
//! holds only ~4 h of changes, so it cannot catch up after weeks. Upgrade to the journal if search
//! is used many times a day.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Condvar, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const NONE: u32 = u32::MAX;
const MAGIC: &[u8; 8] = b"RXIDX01\n";

/// One folded name per entry plus what a result row shows; entry i's parent is a folder entry or
/// NONE for a drive root (whose name is the root path itself, "C:\").
#[derive(Default)]
pub struct Index {
    names: String,
    name_end: Vec<u32>,
    folded: String,
    fold_end: Vec<u32>,
    parent: Vec<u32>,
    /// u64::MAX for folders
    size: Vec<u64>,
    /// seconds since 1970, 0 when unknown
    mtime: Vec<u64>,
    attrs: Vec<u32>,
}

/// What a result row needs.
pub struct Hit {
    pub path: PathBuf,
    pub dir: bool,
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    pub attrs: u32,
}

/// Every folder's entries (see Index::kids): folder i's are `list[start[i]..start[i + 1]]`.
pub struct Kids {
    start: Vec<u32>,
    list: Vec<u32>,
}

impl Kids {
    pub fn of(&self, i: u32) -> &[u32] {
        let i = i as usize;
        match (self.start.get(i), self.start.get(i + 1)) {
            (Some(&a), Some(&b)) => &self.list[a as usize..b as usize],
            _ => &[],
        }
    }
}

/// Lower case without Polish diacritics, so "zolw" finds "Żółw.txt".
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'ą' => 'a',
            'ć' => 'c',
            'ę' => 'e',
            'ł' => 'l',
            'ń' => 'n',
            'ó' => 'o',
            'ś' => 's',
            'ź' | 'ż' => 'z',
            c => c,
        })
        .collect()
}

fn wild(p: &[u8], n: &[u8]) -> bool {
    // iterative * and ? over bytes; '?' may match half of a multi-byte letter, harmless for names
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            pi += 1;
            mark = ni;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
}

impl Index {
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    fn push(&mut self, name: &str, parent: u32, size: u64, mtime: u64, attrs: u32) -> u32 {
        self.names.push_str(name);
        self.name_end.push(self.names.len() as u32);
        self.folded.push_str(&fold(name));
        self.fold_end.push(self.folded.len() as u32);
        self.parent.push(parent);
        self.size.push(size);
        self.mtime.push(mtime);
        self.attrs.push(attrs);
        (self.parent.len() - 1) as u32
    }

    /// Entry i's own name (a drive root's is its path, "C:\").
    pub fn name(&self, i: usize) -> &str {
        let a = if i == 0 { 0 } else { self.name_end[i - 1] as usize };
        &self.names[a..self.name_end[i] as usize]
    }

    fn folded(&self, i: usize) -> &str {
        let a = if i == 0 { 0 } else { self.fold_end[i - 1] as usize };
        &self.folded[a..self.fold_end[i] as usize]
    }

    /// The entries directly in folder `dir`. A child always comes after its folder (scan and load
    /// keep that order), so the search starts there.
    pub fn children(&self, dir: u32) -> impl Iterator<Item = u32> + '_ {
        (dir as usize + 1..self.len()).filter(move |&k| self.parent[k] == dir).map(|k| k as u32)
    }

    /// The drive roots ("C:\"); they come first, pushed before the scan starts.
    pub fn roots(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.len()).take_while(|&i| self.parent[i] == NONE).map(|i| i as u32)
    }

    pub fn parent_of(&self, i: u32) -> Option<u32> {
        Some(self.parent[i as usize]).filter(|&p| p != NONE)
    }

    pub fn is_dir(&self, i: u32) -> bool {
        self.size[i as usize] == u64::MAX
    }

    /// Every folder's entries at once, for walking many folders (a treemap) without a pass over
    /// the whole index per folder.
    pub fn kids(&self) -> Kids {
        let n = self.len();
        let mut start = vec![0u32; n + 1];
        for &p in &self.parent {
            if p != NONE {
                start[p as usize + 1] += 1;
            }
        }
        for i in 0..n {
            start[i + 1] += start[i];
        }
        let mut next = start.clone();
        let mut list = vec![0u32; start[n] as usize];
        for (i, &p) in self.parent.iter().enumerate() {
            if p != NONE {
                list[next[p as usize] as usize] = i as u32;
                next[p as usize] += 1;
            }
        }
        Kids { start, list }
    }

    /// The entry of `path`, when the index holds it; names compare without case, as on Windows.
    pub fn find(&self, path: &Path) -> Option<u32> {
        self.find_by(path, |at| self.children(at).collect())
    }

    /// As `find`, through `kids` (a lookup per folder instead of a pass over the whole index).
    pub fn find_in(&self, kids: &Kids, path: &Path) -> Option<u32> {
        self.find_by(path, |at| kids.of(at).to_vec())
    }

    fn find_by(&self, path: &Path, kids_of: impl Fn(u32) -> Vec<u32>) -> Option<u32> {
        self.roots().find_map(|r| {
            let rest = path.strip_prefix(self.name(r as usize)).ok()?;
            rest.components().try_fold(r, |at, c| {
                let want = c.as_os_str().to_string_lossy().to_lowercase();
                kids_of(at).into_iter().find(|&k| self.name(k as usize).to_lowercase() == want)
            })
        })
    }

    /// Bytes under every entry: a file's own size, a folder's files at any depth. One pass from
    /// the end adds each entry into its folder, which comes earlier, before that folder is added on.
    pub fn totals(&self) -> Vec<u64> {
        let mut t: Vec<u64> = self.size.iter().map(|&s| if s == u64::MAX { 0 } else { s }).collect();
        for i in (0..self.len()).rev() {
            let p = self.parent[i];
            if p != NONE {
                t[p as usize] = t[p as usize].saturating_add(t[i]);
            }
        }
        t
    }

    /// The `cap` biggest files in folder `under` at any depth (on every drive when None), biggest
    /// first, and how many files it holds in all.
    pub fn largest(&self, under: Option<u32>, cap: usize) -> (Vec<u32>, usize) {
        let n = self.len();
        let inside = |from: usize| {
            // marked in id order: a folder is decided before its children
            let mut v = vec![false; n];
            v[from] = true;
            for i in from + 1..n {
                let p = self.parent[i];
                v[i] = p != NONE && v[p as usize];
            }
            v
        };
        let within = under.filter(|&u| (u as usize) < n).map(|u| inside(u as usize));
        let mut files: Vec<u32> = (0..n).filter(|&i| self.size[i] != u64::MAX && within.as_ref().is_none_or(|v| v[i])).map(|i| i as u32).collect();
        let total = files.len();
        let bigger = |a: &u32, b: &u32| self.size[*b as usize].cmp(&self.size[*a as usize]);
        if files.len() > cap {
            files.select_nth_unstable_by(cap, bigger);
            files.truncate(cap);
        }
        files.sort_by(bigger);
        (files, total)
    }

    pub fn path(&self, i: usize) -> PathBuf {
        let mut chain = vec![self.name(i)];
        let mut p = self.parent[i];
        while p != NONE {
            chain.push(self.name(p as usize));
            p = self.parent[p as usize];
        }
        chain.iter().rev().collect()
    }

    pub fn hit(&self, i: usize) -> Hit {
        let dir = self.size[i] == u64::MAX;
        let m = self.mtime[i];
        Hit {
            path: self.path(i),
            dir,
            size: (!dir).then_some(self.size[i]),
            // checked: a damaged file must not turn a date into a panic
            modified: (m > 0).then(|| UNIX_EPOCH.checked_add(std::time::Duration::from_secs(m))).flatten(),
            attrs: self.attrs[i],
        }
    }

    /// Entries whose name holds every word of `query` (Everything's rule: words are ANDed, a
    /// word with * or ? matches the whole name), at most `cap` of them, and how many matched.
    pub fn search(&self, query: &str, cap: usize) -> (Vec<u32>, usize) {
        let words: Vec<String> = query.split_whitespace().map(fold).collect();
        if words.is_empty() {
            return (vec![], 0);
        }
        let (mut out, mut total) = (vec![], 0);
        // drive roots ("C:\") are not results
        let matches = |i: usize| {
            let n = self.folded(i);
            self.parent[i] != NONE && words.iter().all(|w| if w.contains(['*', '?']) { wild(w.as_bytes(), n.as_bytes()) } else { n.contains(w.as_str()) })
        };
        let mut take = |i: usize| {
            total += 1;
            if out.len() < cap {
                out.push(i as u32);
            }
        };
        // the longest plain run of any word ("*.pdf" -> ".pdf") is looked up with SIMD across all
        // names at once; only the names it lands in are checked word by word
        let key = words.iter().flat_map(|w| w.split(['*', '?'])).max_by_key(|s| s.len()).unwrap_or("");
        if key.is_empty() {
            (0..self.len()).filter(|&i| matches(i)).for_each(&mut take);
            return (out, total);
        }
        let plain = words.len() == 1 && key.len() == words[0].len();
        let hay = self.folded.as_bytes();
        let finder = memchr::memmem::Finder::new(key);
        let (mut at, mut from) = (0usize, 0usize);
        while let Some(p) = finder.find(&hay[at..]) {
            let pos = at + p;
            // the entry holding pos: the first whose end lies past it (a linear walk over the ends
            // costs a few ms for the whole index, less than a binary search per dense hit)
            let mut i = from;
            while self.fold_end[i] as usize <= pos {
                i += 1;
            }
            // names are not separated, so a match may straddle two. One plain word: the match is
            // the answer when it ends inside the name (a later one in the same name would straddle
            // too); otherwise every word is checked.
            let ok = if plain { pos + key.len() <= self.fold_end[i] as usize && self.parent[i] != NONE } else { matches(i) };
            if ok {
                take(i);
            }
            at = self.fold_end[i] as usize;
            from = i + 1;
        }
        (out, total)
    }

    /// Scans every root with `threads` readers; folders it cannot read are skipped, links and
    /// junctions are listed but not followed. `in_reader` runs once at the start of every reader
    /// thread (the app lowers its priority there: a thread's priority is its own).
    pub fn scan(roots: &[PathBuf], threads: usize, in_reader: impl Fn() + Sync) -> Index {
        let mut ix = Index::default();
        let work = Mutex::new((Vec::<(u32, PathBuf)>::new(), 0usize));
        let cv = Condvar::new();
        for r in roots {
            let id = ix.push(&r.to_string_lossy(), NONE, u64::MAX, 0, 0);
            work.lock().unwrap().0.push((id, r.clone()));
        }
        type Batch = (u32, Vec<(String, bool, Option<PathBuf>, u64, u64, u32)>);
        let (tx, rx) = mpsc::channel::<Batch>();
        std::thread::scope(|s| {
            for _ in 0..threads.max(1) {
                let (tx, work, cv, in_reader) = (tx.clone(), &work, &cv, &in_reader);
                s.spawn(move || {
                    in_reader();
                    loop {
                        let (id, dir) = {
                            let mut w = work.lock().unwrap();
                            loop {
                                if let Some(job) = w.0.pop() {
                                    w.1 += 1;
                                    break job;
                                }
                                // nothing queued and nothing being read: every folder is done
                                if w.1 == 0 {
                                    cv.notify_all();
                                    return;
                                }
                                w = cv.wait(w).unwrap();
                            }
                        };
                        let mut kids = vec![];
                        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                            let (Ok(ft), Ok(md)) = (e.file_type(), e.metadata()) else { continue };
                            #[cfg(windows)]
                            let attrs = std::os::windows::fs::MetadataExt::file_attributes(&md);
                            #[cfg(not(windows))]
                            let attrs = 0;
                            // a link to a folder shows as a folder but is not entered (loops, doubles)
                            let is_dir = ft.is_dir() || (ft.is_symlink() && attrs & 0x10 != 0);
                            let mtime = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                            // the folder to enter keeps its real (OS) path; the stored name may be lossy
                            kids.push((e.file_name().to_string_lossy().into_owned(), is_dir, ft.is_dir().then(|| e.path()), md.len(), mtime, attrs));
                        }
                        if tx.send((id, kids)).is_err() {
                            return;
                        }
                    }
                });
            }
            drop(tx);
            // the collector numbers the entries and queues subfolders, then marks the folder done,
            // so a worker never sees "nothing queued, nothing in flight" before its children exist
            while let Ok((id, kids)) = rx.recv() {
                let mut sub = vec![];
                for (name, is_dir, enter, size, mtime, attrs) in kids {
                    let k = ix.push(&name, id, if is_dir { u64::MAX } else { size }, mtime, attrs);
                    if let Some(p) = enter {
                        sub.push((k, p));
                    }
                }
                let mut w = work.lock().unwrap();
                w.0.extend(sub);
                w.1 -= 1;
                cv.notify_all();
            }
        });
        // the columns grew by doubling: up to half of each is spare (tens of MB for a drive)
        ix.names.shrink_to_fit();
        ix.folded.shrink_to_fit();
        for v in [&mut ix.name_end, &mut ix.fold_end, &mut ix.parent, &mut ix.attrs] {
            v.shrink_to_fit();
        }
        ix.size.shrink_to_fit();
        ix.mtime.shrink_to_fit();
        ix
    }

    /// Writes the index to `path` atomically (a .tmp file renamed over it).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let tmp = path.with_extension("tmp");
        {
            let mut f = io::BufWriter::new(std::fs::File::create(&tmp)?);
            f.write_all(MAGIC)?;
            for n in [self.len(), self.names.len(), self.folded.len()] {
                f.write_all(&(n as u64).to_le_bytes())?;
            }
            f.write_all(self.names.as_bytes())?;
            f.write_all(self.folded.as_bytes())?;
            for v in [&self.name_end, &self.fold_end, &self.parent, &self.attrs] {
                v.iter().try_for_each(|x| f.write_all(&x.to_le_bytes()))?;
            }
            for v in [&self.size, &self.mtime] {
                v.iter().try_for_each(|x| f.write_all(&x.to_le_bytes()))?;
            }
            f.flush()?;
        }
        std::fs::rename(tmp, path)
    }

    pub fn load(path: &Path) -> io::Result<Index> {
        // read column by column, so loading never holds the file and the index at once
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        let mut r = io::BufReader::with_capacity(1 << 16, file);
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "uszkodzony indeks");
        let mut magic = [0u8; 8];
        r.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(bad());
        }
        let mut num = || -> io::Result<u64> {
            let mut b = [0u8; 8];
            r.read_exact(&mut b)?;
            Ok(u64::from_le_bytes(b))
        };
        let (n, nl, fl) = (num()?, num()?, num()?);
        // the counts must add up to the file's length before anything is allocated from them
        let body = n.checked_mul(4 * 4 + 8 * 2).and_then(|c| c.checked_add(nl)?.checked_add(fl)?.checked_add(8 * 4));
        if body != Some(len) || n > u32::MAX as u64 {
            return Err(bad());
        }
        let (n, nl, fl) = (n as usize, nl as usize, fl as usize);
        let mut text = |k: usize| -> io::Result<String> {
            let mut v = vec![0u8; k];
            r.read_exact(&mut v)?;
            String::from_utf8(v).map_err(|_| bad())
        };
        let (names, folded) = (text(nl)?, text(fl)?);
        let mut u32s = || -> io::Result<Vec<u32>> {
            let mut v = Vec::with_capacity(n);
            let mut b = [0u8; 4];
            for _ in 0..n {
                r.read_exact(&mut b)?;
                v.push(u32::from_le_bytes(b));
            }
            Ok(v)
        };
        let (name_end, fold_end, parent, attrs) = (u32s()?, u32s()?, u32s()?, u32s()?);
        let mut u64s = || -> io::Result<Vec<u64>> {
            let mut v = Vec::with_capacity(n);
            let mut b = [0u8; 8];
            for _ in 0..n {
                r.read_exact(&mut b)?;
                v.push(u64::from_le_bytes(b));
            }
            Ok(v)
        };
        let (size, mtime) = (u64s()?, u64s()?);
        let ix = Index { names, name_end, folded, fold_end, parent, size, mtime, attrs };
        // offsets must stay inside the text and on letter boundaries, or slicing would panic
        let ok = |text: &str, ends: &[u32]| ends.windows(2).all(|w| w[0] <= w[1]) && ends.last().is_none_or(|&e| e as usize <= text.len()) && ends.iter().all(|&e| text.is_char_boundary(e as usize));
        let parents_ok = ix.parent.iter().enumerate().all(|(i, &p)| p == NONE || (p as usize) < i);
        if !ok(&ix.names, &ix.name_end) || !ok(&ix.folded, &ix.fold_end) || !parents_ok {
            return Err(bad());
        }
        Ok(ix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> (PathBuf, Index) {
        let root = std::env::temp_dir().join(format!("rx-index-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["Dokumenty/Faktury 2025", "Zdjęcia", "src/deep/er"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in ["Dokumenty/Faktury 2025/FV-0412.pdf", "Dokumenty/Żółw.TXT", "Zdjęcia/wakacje.jpg", "src/main.rs", "src/deep/er/lib.rs"] {
            std::fs::write(root.join(f), b"x").unwrap();
        }
        let ix = Index::scan(std::slice::from_ref(&root), 4, || ());
        (root, ix)
    }

    #[test]
    fn the_hook_runs_once_in_every_reader_thread() {
        let seen = Mutex::new(std::collections::HashSet::new());
        Index::scan(&[], 3, || {
            seen.lock().unwrap().insert(std::thread::current().id());
        });
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(!seen.contains(&std::thread::current().id()));
    }

    fn names(ix: &Index, q: &str) -> Vec<String> {
        let mut v: Vec<String> = ix.search(q, 100).0.iter().map(|&i| ix.name(i as usize).to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn scans_searches_and_round_trips() {
        let (root, ix) = tree();
        // root + 6 folders + 5 files
        assert_eq!(ix.len(), 12);
        assert_eq!(names(&ix, "zolw"), ["Żółw.TXT"]);
        assert_eq!(names(&ix, "ŻÓŁW"), ["Żółw.TXT"]);
        assert_eq!(names(&ix, "*.rs"), ["lib.rs", "main.rs"]);
        assert_eq!(names(&ix, "faktury 2025"), ["Faktury 2025"]);
        assert_eq!(names(&ix, "fv ?412"), Vec::<String>::new());
        assert_eq!(names(&ix, "fv-0412"), ["FV-0412.pdf"]);
        assert!(ix.search("", 10).0.is_empty());
        let (hits, total) = ix.search("s", 2);
        assert_eq!((hits.len(), total > 2), (2, true));
        let i = ix.search("lib.rs", 10).0[0] as usize;
        assert_eq!(ix.path(i), root.join("src/deep/er/lib.rs"));
        let h = ix.hit(i);
        assert_eq!((h.dir, h.size), (false, Some(1)));
        assert!(h.modified.is_some());
        assert!(ix.hit(ix.search("deep", 10).0[0] as usize).dir);

        let file = root.join("ix.bin");
        ix.save(&file).unwrap();
        let back = Index::load(&file).unwrap();
        assert_eq!(back.len(), ix.len());
        assert_eq!(back.path(i), ix.path(i));
        assert_eq!(names(&back, "zolw"), ["Żółw.TXT"]);
        std::fs::write(&file, b"RXIDX01\nbroken").unwrap();
        assert!(Index::load(&file).is_err());
        // a count that does not fit the file is refused before anything is allocated from it
        let mut huge = MAGIC.to_vec();
        for n in [u64::MAX / 2, 1, 1] {
            huge.extend(n.to_le_bytes());
        }
        std::fs::write(&file, &huge).unwrap();
        assert!(Index::load(&file).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_match_across_two_names_is_not_a_hit() {
        let mut ix = Index::default();
        let r = ix.push("C:\\", NONE, u64::MAX, 0, 0);
        ix.push("ab", r, 1, 0, 0);
        ix.push("cd", r, 1, 0, 0);
        ix.push("xbcx", r, 1, 0, 0);
        assert_eq!(names(&ix, "bc"), ["xbcx"]);
        assert_eq!(names(&ix, "bc x"), ["xbcx"]);
        assert_eq!(names(&ix, "*bc*"), ["xbcx"]);
        assert_eq!(ix.search("b", 10).1, 2);
    }

    #[test]
    fn folder_sizes_and_the_biggest_files() {
        let mut ix = Index::default();
        let root = ix.push("/data", NONE, u64::MAX, 0, 0);
        let users = ix.push("Users", root, u64::MAX, 0, 0);
        ix.push("a.bin", users, 100, 0, 0);
        let docs = ix.push("Dokumenty", users, u64::MAX, 0, 0);
        let b = ix.push("b.bin", docs, 300, 0, 0);
        ix.push("pusty", docs, u64::MAX, 0, 0);
        let z = ix.push("z.txt", root, 5, 0, 0);
        let t = ix.totals();
        assert_eq!((t[root as usize], t[users as usize], t[docs as usize], t[b as usize]), (405, 400, 300, 300));

        assert_eq!(ix.find(Path::new("/data/users/DOKUMENTY")), Some(docs));
        assert_eq!(ix.find(Path::new("/data")), Some(root));
        assert_eq!(ix.find(Path::new("/data/Users/brak")), None);
        assert_eq!(ix.find(Path::new("/inne/Users")), None);
        let mut kids: Vec<&str> = ix.children(users).map(|k| ix.name(k as usize)).collect();
        kids.sort();
        assert_eq!(kids, ["Dokumenty", "a.bin"]);
        let all = ix.kids();
        for d in [root, users, docs, b] {
            assert_eq!(all.of(d), ix.children(d).collect::<Vec<_>>().as_slice());
        }
        assert!(all.of(999).is_empty());
        for p in ["/data/users/DOKUMENTY", "/data", "/data/Users/brak", "/inne/Users"] {
            assert_eq!(ix.find_in(&all, Path::new(p)), ix.find(Path::new(p)), "{p}");
        }
        assert_eq!(ix.roots().collect::<Vec<_>>(), [root]);
        assert_eq!((ix.parent_of(docs), ix.parent_of(root)), (Some(users), None));
        assert!(ix.is_dir(docs) && !ix.is_dir(b));

        let names = |v: Vec<u32>| v.iter().map(|&i| ix.name(i as usize).to_string()).collect::<Vec<_>>();
        let (top, total) = ix.largest(Some(users), 1);
        assert_eq!((names(top), total), (vec!["b.bin".to_string()], 2));
        let (all, total) = ix.largest(None, 10);
        assert_eq!((names(all), total), (vec!["b.bin".to_string(), "a.bin".into(), "z.txt".into()], 3));
        assert_eq!(ix.largest(Some(z), 10), (vec![z], 1));
        assert_eq!(ix.largest(Some(users), 0), (vec![], 2));
    }

    #[test]
    fn wildcards_match_whole_names() {
        assert!(wild(b"*.pdf", b"fv.pdf"));
        assert!(!wild(b"*.pdf", b"fv.pdf.bak"));
        assert!(wild(b"fv-0?12*", b"fv-0412.pdf"));
        assert!(wild(b"*", b""));
        assert!(!wild(b"a*b", b"ac"));
    }
}
