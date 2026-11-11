//! Archives handled inside the app, so nothing waits on an outside program (Windows' tar.exe once
//! hung for over an hour and held the "file operations running" state): ZIP and 7z are packed and
//! unpacked, RAR only unpacked (only WinRAR may create RAR archives).
//!
//! Unpacking writes only below the target folder: entry names that are absolute or climb out with
//! ".." are skipped ("zip slip").

use std::fs::File;
use std::io::{self, BufReader, BufWriter};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

/// Bytes done and in all, called as work goes on.
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64);

/// The archive kinds this crate can open, by extension.
pub fn can_extract(path: &Path) -> bool {
    matches!(ext(path).as_str(), "zip" | "7z" | "rar")
}

fn ext(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// A name part a file may have: on Windows "C:x" as a part would replace the whole path
/// (PathBuf::push of a drive-relative path), "x:y" names an alternate data stream, "CON",
/// "NUL.txt", "COM1"… open devices instead of files, and a trailing dot or space is dropped by
/// Windows ("a." would land on "a").
fn plain(p: &std::ffi::OsStr) -> bool {
    let s = p.to_string_lossy();
    let stem = s.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
    let b = stem.as_bytes();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || (b.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && (b'1'..=b'9').contains(&b[3]));
    !s.contains(':') && !device && !s.ends_with(['.', ' '])
}

/// A ZIP entry's name as slash() takes it. zip's own enclosed_name() is not used: on Windows
/// (zip 9.1.2) it panics on a name like "Żółw.txt", cutting it at byte 3 inside a letter.
fn zip_name<R: io::Read>(f: &zip::read::ZipFile<'_, R>) -> Option<String> {
    slash(Path::new(&*f.name().ok()?))
}

/// `name` inside `dest` when it stays there: relative, without "..", and no part with a drive or
/// a stream (":"), else None.
fn inside(dest: &Path, name: &Path) -> Option<PathBuf> {
    let mut out = dest.to_path_buf();
    for c in name.components() {
        match c {
            Component::Normal(p) if plain(p) => out.push(p),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (out != dest).then_some(out)
}

/// Why unpacking stopped: the archive wants a password (none given, or a wrong one), or anything else.
#[derive(Debug, PartialEq)]
pub enum Fail {
    Password,
    Other(String),
}

impl From<String> for Fail {
    fn from(s: String) -> Self {
        Fail::Other(s)
    }
}

/// Unpacks `archive` into `dest`, a folder this call creates (it must not exist yet, so two jobs
/// never share one), with `password` for an encrypted one. Returns how many files were written
/// and how many entries were skipped as unsafe. A failure, or a panic in a decoder, removes the
/// folder again.
pub fn extract(archive: &Path, dest: &Path, password: Option<&str>, progress: Progress) -> Result<(usize, usize), Fail> {
    std::fs::create_dir(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let r = run(archive, &|n| inside(dest, n), password, progress);
    // the folder is this call's own (create_dir above), so a failure takes it away whole
    if r.is_err() {
        let _ = std::fs::remove_dir_all(dest);
    }
    r
}

/// Unpacks `archive` straight into `dir` (7-Zip's "extract here"): into a work folder next to the
/// archive (same drive, so moving out is a rename), then each top item moved up into `dir`. A name
/// already taken there gets "name (2)", "name (3)"…, so nothing in `dir` is replaced. Returns the
/// files written, the entries skipped as unsafe, and the items placed in `dir`.
pub fn extract_here(archive: &Path, dir: &Path, pw: Option<&str>, progress: Progress) -> Result<(usize, usize, Vec<PathBuf>), Fail> {
    let stem = archive.file_stem().unwrap_or_default().to_string_lossy();
    let work = free(dir, &format!(".{stem} (wypakowywanie)"), true);
    let (files, skipped) = extract(archive, &work, pw, progress)?;
    let mut placed = vec![];
    for item in items_of(&work) {
        let name = item.file_name().unwrap_or_default().to_string_lossy().into_owned();
        // shortcut: the free name is checked, then renamed onto; std's rename replaces a file made
        // there in between (MOVEFILE_REPLACE_EXISTING), a race with another program only
        let to = free(dir, &name, item.is_dir());
        if let Err(e) = std::fs::rename(&item, &to) {
            return Err(Fail::Other(format!("{}: {e}; reszta została w „{}”", to.display(), work.display())));
        }
        placed.push(to);
    }
    let _ = std::fs::remove_dir(&work);
    Ok((files, skipped, placed))
}

/// `name` in `dir`, or "stem (2).ext", "stem (3).ext"… (a folder: "name (2)") when taken.
fn free(dir: &Path, name: &str, folder: bool) -> PathBuf {
    let p = Path::new(name);
    let (stem, ext) = match (folder, p.file_stem(), p.extension()) {
        (false, Some(s), Some(e)) => (s.to_string_lossy().into_owned(), format!(".{}", e.to_string_lossy())),
        _ => (name.to_string(), String::new()),
    };
    let at = |n: usize| dir.join(if n == 1 { name.to_string() } else { format!("{stem} ({n}){ext}") });
    (1..).map(at).find(|p| p.symlink_metadata().is_err()).unwrap()
}

/// Where an entry goes: its file or folder to write, or None to pass it by.
type Map<'a> = &'a dyn Fn(&Path) -> Option<PathBuf>;

/// Unpacks the entries `map` places; returns the files written and the entries passed by. A panic
/// in a decoder is an error, not a crash.
fn run(archive: &Path, map: Map, password: Option<&str>, progress: Progress) -> Result<(usize, usize), Fail> {
    let go = std::panic::AssertUnwindSafe(|| match ext(archive).as_str() {
        "zip" => extract_zip(archive, map, password, progress),
        "7z" => extract_7z(archive, map, password, progress),
        "rar" => extract_rar(archive, map, password, progress),
        e => Err(Fail::Other(format!("Nieobsługiwany format: .{e}"))),
    });
    std::panic::catch_unwind(go).unwrap_or_else(|_| Err(Fail::Other("uszkodzone archiwum (przerwano)".into())))
}

/// An entry's name as '/'-separated parts, or None when unpacking would refuse it (inside()).
fn slash(name: &Path) -> Option<String> {
    let mut parts = vec![];
    for c in name.components() {
        match c {
            Component::Normal(p) if plain(p) => parts.push(p.to_string_lossy()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// One entry of an archive's list: `name` '/'-separated from the archive's top.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// Every entry of `archive` without unpacking anything (ZIP's central directory, the 7z header,
/// RAR's headers). Unsafe names are left out, as unpacking skips them; an archive whose list is
/// encrypted asks for its password (Fail::Password).
pub fn list(archive: &Path) -> Result<Vec<Entry>, Fail> {
    let go = std::panic::AssertUnwindSafe(|| match ext(archive).as_str() {
        "zip" => list_zip(archive),
        "7z" => list_7z(archive),
        "rar" => list_rar(archive),
        e => Err(Fail::Other(format!("Nieobsługiwany format: .{e}"))),
    });
    std::panic::catch_unwind(go).unwrap_or_else(|_| Err(Fail::Other("uszkodzone archiwum".into())))
}

fn list_zip(archive: &Path) -> Result<Vec<Entry>, Fail> {
    let mut z = zip::ZipArchive::new(BufReader::new(File::open(archive).map_err(|e| e.to_string())?)).map_err(|e| e.to_string())?;
    let mut out = vec![];
    for i in 0..z.len() {
        let f = z.by_index_raw(i).map_err(|e| e.to_string())?;
        if let Some(name) = zip_name(&f) {
            out.push(Entry { name, dir: f.is_dir(), size: f.size(), modified: f.last_modified().and_then(zip_time) });
        }
    }
    Ok(out)
}

fn list_7z(archive: &Path) -> Result<Vec<Entry>, Fail> {
    use sevenz_rust2::Error;
    let file = File::open(archive).map_err(|e| e.to_string())?;
    let r = sevenz_rust2::ArchiveReader::new(BufReader::new(file), sevenz_rust2::Password::empty()).map_err(|e| match e {
        Error::PasswordRequired | Error::MaybeBadPassword(_) => Fail::Password,
        e => Fail::Other(e.to_string()),
    })?;
    Ok(r.archive()
        .files
        .iter()
        .filter_map(|e| {
            let name = slash(Path::new(&e.name))?;
            Some(Entry { name, dir: e.is_directory, size: e.size, modified: e.has_last_modified_date.then(|| SystemTime::from(e.last_modified_date)) })
        })
        .collect())
}

fn list_rar(archive: &Path) -> Result<Vec<Entry>, Fail> {
    let fail = |e: rars::Error| match e.kind() {
        rars::ErrorKind::PasswordRequired | rars::ErrorKind::BadPassword => Fail::Password,
        _ => Fail::Other(e.to_string()),
    };
    let open = |p: &PathBuf| rars::ArchiveReader::read_path_with_options(p, rars::ArchiveReadOptions::with_optional_password(None));
    let vols: Vec<rars::Archive> = rar_volumes(archive).iter().map(open).collect::<rars::Result<_>>().map_err(fail)?;
    let members = rars::volume_members(&vols).map_err(fail)?;
    let rar5 = members.first().is_some_and(|m| m.meta.family == rars::ArchiveFamily::Rar50Plus);
    Ok(members
        .iter()
        .filter_map(|m| {
            let name = rars::entry_relative_path(&m.meta.name).ok().and_then(|n| slash(&n))?;
            Some(Entry { name, dir: m.meta.is_directory, size: m.meta.unpacked_size, modified: m.meta.file_time.and_then(|t| rar_time(t, rar5)) })
        })
        .collect())
}

/// What folder `base` ("" for the top, else '/'-separated) of a list holds, as a folder listing
/// shows it: each child once, named in full; folders that only appear in their files' names
/// (common in ZIP) are there too, with the size of everything inside them.
pub fn folder(entries: &[Entry], base: &str) -> Vec<Entry> {
    let mut kids: std::collections::BTreeMap<String, Entry> = Default::default();
    for e in entries {
        let rest = if base.is_empty() { Some(e.name.as_str()) } else { e.name.strip_prefix(base).and_then(|r| r.strip_prefix('/')) };
        let Some(rest) = rest.filter(|r| !r.is_empty()) else { continue };
        let (first, deeper) = match rest.split_once('/') {
            Some((a, b)) => (a, !b.is_empty()),
            None => (rest, false),
        };
        let name = if base.is_empty() { first.to_string() } else { format!("{base}/{first}") };
        if deeper || e.dir {
            let k = kids.entry(name.clone()).or_insert(Entry { name, dir: true, size: 0, modified: None });
            k.dir = true;
            if !e.dir {
                k.size += e.size;
            }
            // the newest time inside: a folder only seen in its files' names has none of its own
            k.modified = k.modified.max(e.modified);
        } else {
            kids.insert(name.clone(), Entry { name, ..e.clone() });
        }
    }
    kids.into_values().collect()
}

/// Unpacks some entries of `archive`: each pick is an entry's name (a file, or a folder with
/// everything in it) and where it goes, a path that must not exist. A failure leaves no part of
/// its own work behind. Returns how many files were written and how many inside the picks were
/// left out as names Windows cannot hold; a pick naming no entry makes nothing.
pub fn extract_entries(archive: &Path, picks: &[(String, PathBuf)], password: Option<&str>, progress: Progress) -> Result<(usize, usize), Fail> {
    if let Some((_, to)) = picks.iter().find(|(_, to)| to.exists()) {
        return Err(Fail::Other(format!("{} już istnieje", to.display())));
    }
    // each pick unpacks into a folder of its own claimed next to its target (create_dir), then
    // moves into place: two jobs never write into one another's output, and a failure removes
    // only its own work
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let mut stages = vec![];
    for (_, to) in picks {
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let stage = to.with_file_name(format!("~rx-{}-{n}.part", std::process::id()));
        if let Err(e) = std::fs::create_dir(&stage) {
            for s in &stages {
                let _ = std::fs::remove_dir_all(s);
            }
            return Err(Fail::Other(format!("{}: {e}", stage.display())));
        }
        stages.push(stage);
    }
    let item = |k: usize| stages[k].join("x");
    // entries inside a pick whose name cannot be written (a device name, "a:b"…): counted, so a
    // cut does not take them out of the archive unseen
    let left_out = std::cell::Cell::new(0);
    let under_pick = |s: &str| picks.iter().any(|(pick, _)| s == pick || s.strip_prefix(pick.as_str()).is_some_and(|r| r.starts_with('/')));
    let map = |n: &Path| {
        let Some(s) = slash(n) else {
            if under_pick(n.to_string_lossy().replace('\\', "/").trim_start_matches('/')) {
                left_out.set(left_out.get() + 1);
            }
            return None;
        };
        picks.iter().enumerate().find_map(|(k, (pick, _))| {
            if s == *pick {
                Some(item(k))
            } else {
                s.strip_prefix(pick.as_str()).and_then(|r| r.strip_prefix('/')).and_then(|r| inside(&item(k), Path::new(r)))
            }
        })
    };
    let r = run(archive, &map, password, progress).and_then(|(files, _)| {
        for (k, (_, to)) in picks.iter().enumerate() {
            let made = item(k);
            if !made.exists() {
                continue;
            }
            // rename would replace a file: the target is checked once more right before
            if to.exists() {
                return Err(Fail::Other(format!("{} już istnieje", to.display())));
            }
            std::fs::rename(&made, to).map_err(|e| format!("{}: {e}", to.display()))?;
        }
        Ok((files, left_out.get()))
    });
    for s in &stages {
        let _ = std::fs::remove_dir_all(s);
    }
    r
}

/// With a password given, data that does not decode or fails its checksum means the password is
/// wrong (a stored 7z entry only shows it in the CRC); a full disk or a denied write stays an error.
fn bad_password(password: Option<&str>, e: &io::Error) -> bool {
    password.is_some() && matches!(e.kind(), io::ErrorKind::InvalidData | io::ErrorKind::Other)
}

/// A new file, never one already there: two entries that Windows sees as one name ("Readme.txt"
/// and "README.txt") stop the job instead of the second silently replacing the first (a cut or a
/// 7z change would then drop the lost one from the archive).
fn create_new(path: &Path) -> io::Result<File> {
    File::create_new(path).map_err(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => io::Error::new(e.kind(), "w archiwum są dwa pliki o tej samej nazwie (Windows nie rozróżnia wielkości liter)"),
        _ => e,
    })
}

/// An entry's data passed through with the bytes reported as they go (every MiB), so one big file
/// moves the bar along instead of only at its end. `at` is the count before this entry.
struct Counting<'a> {
    r: &'a mut dyn io::Read,
    at: u64,
    next: u64,
    total: u64,
    progress: &'a mut dyn FnMut(u64, u64),
}

impl<'a> Counting<'a> {
    fn new(r: &'a mut dyn io::Read, at: u64, total: u64, progress: &'a mut dyn FnMut(u64, u64)) -> Self {
        Counting { r, at, next: at + (1 << 20), total, progress }
    }
}

impl io::Read for Counting<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.r.read(buf)?;
        self.at += n as u64;
        if self.at >= self.next && self.at < self.total {
            (self.progress)(self.at, self.total);
            self.next = self.at + (1 << 20);
        }
        Ok(n)
    }
}

fn write_file(path: &Path, from: &mut dyn io::Read, modified: Option<SystemTime>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = create_new(path)?;
    let mut w = BufWriter::new(&f);
    io::copy(from, &mut w)?;
    // flushed here: dropping a BufWriter would swallow a full disk
    io::Write::flush(&mut w)?;
    drop(w);
    if let Some(t) = modified {
        let _ = f.set_modified(t);
    }
    Ok(())
}

fn zip_time(d: zip::DateTime) -> Option<SystemTime> {
    use chrono::{Local, NaiveDate, TimeZone};
    let n = NaiveDate::from_ymd_opt(d.year() as i32, d.month() as u32, d.day() as u32)?.and_hms_opt(d.hour() as u32, d.minute() as u32, d.second() as u32)?;
    Local.from_local_datetime(&n).earliest().map(SystemTime::from)
}

fn extract_zip(archive: &Path, map: Map,password: Option<&str>, progress: Progress) -> Result<(usize, usize), Fail> {
    use zip::result::ZipError;
    let fail = |e: ZipError| match e {
        ZipError::InvalidPassword => Fail::Password,
        ZipError::UnsupportedArchive(m) if m == ZipError::PASSWORD_REQUIRED => Fail::Password,
        e => Fail::Other(e.to_string()),
    };
    let mut z = zip::ZipArchive::new(BufReader::new(File::open(archive).map_err(|e| e.to_string())?)).map_err(fail)?;
    // raw: names and sizes are readable without the password; only placed entries are decoded
    let mut place = vec![];
    let mut total = 0;
    for i in 0..z.len() {
        let f = z.by_index_raw(i).map_err(fail)?;
        // an unsafe name still goes to map (which refuses it), so extract_entries can count it
        let to = match zip_name(&f) {
            Some(n) => map(Path::new(&n)),
            None => map(Path::new(&*f.name().unwrap_or_default())),
        };
        total += to.as_ref().map_or(0, |_| f.size());
        place.push(to);
    }
    let (mut done, mut files, mut skipped) = (0, 0, 0);
    for (i, to) in place.into_iter().enumerate() {
        let Some(path) = to else {
            skipped += 1;
            continue;
        };
        let opts = zip::read::ZipReadOptions::new().password(password.map(str::as_bytes));
        let mut f = z.by_index_with_options(i, opts).map_err(fail)?;
        if f.is_dir() {
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            continue;
        }
        let when = f.last_modified().and_then(zip_time);
        // a wrong ZipCrypto password can pass the header check and fail only on the data
        write_file(&path, &mut Counting::new(&mut f, done, total, progress), when).map_err(|e| if bad_password(password, &e) { Fail::Password } else { Fail::Other(format!("{}: {e}", path.display())) })?;
        files += 1;
        done += f.size();
        progress(done, total);
    }
    Ok((files, skipped))
}

fn extract_7z(archive: &Path, map: Map,password: Option<&str>, progress: Progress) -> Result<(usize, usize), Fail> {
    use sevenz_rust2::Error;
    let fail = |e: Error| match e {
        Error::PasswordRequired | Error::MaybeBadPassword(_) => Fail::Password,
        e => Fail::Other(e.to_string()),
    };
    let file = File::open(archive).map_err(|e| e.to_string())?;
    let pw = password.map_or(sevenz_rust2::Password::empty(), sevenz_rust2::Password::from);
    let mut r = sevenz_rust2::ArchiveReader::new(BufReader::new(file), pw).map_err(fail)?;
    let total: u64 = r.archive().files.iter().filter(|e| map(Path::new(&e.name)).is_some()).map(|e| e.size).sum();
    let placed = r.archive().files.iter().filter(|e| map(Path::new(&e.name)).is_some()).count();
    let skipped = r.archive().files.len() - placed;
    let (mut done, mut files, mut left) = (0, 0, placed);
    let mut failed = None;
    r.for_each_entries(|entry, data| {
        // once every placed entry is out, the rest is not decoded (a file picked from a big solid
        // archive); false ends only the current block, so each later call ends at once too
        if left == 0 || failed.is_some() {
            return Ok(false);
        }
        // a solid archive is decoded in order: entries not placed are read through, not skipped
        let Some(path) = map(Path::new(&entry.name)) else {
            io::copy(data, &mut io::sink())?;
            return Ok(true);
        };
        left -= 1;
        if entry.is_directory {
            std::fs::create_dir_all(&path)?;
            return Ok(left > 0);
        }
        let when = entry.has_last_modified_date.then(|| SystemTime::from(entry.last_modified_date));
        if let Err(e) = write_file(&path, &mut Counting::new(data, done, total, &mut *progress), when) {
            // with a password, data that does not decode means the password is wrong
            failed = Some(if bad_password(password, &e) { Fail::Password } else { Fail::Other(format!("{}: {e}", path.display())) });
            return Ok(false);
        }
        files += 1;
        done += entry.size;
        progress(done, total);
        Ok(left > 0)
    })
    .map_err(fail)?;
    failed.map_or(Ok((files, skipped)), Err)
}

/// The volumes of a RAR set starting at `first`: "x.part1.rar" brings x.part2.rar…, "x.rar"
/// brings x.r00, x.r01… when they are there; any other archive is a set of one.
fn rar_volumes(first: &Path) -> Vec<PathBuf> {
    let mut out = vec![first.to_path_buf()];
    // ASCII-only lower case keeps byte positions, so the index below also fits the real name
    let name = first.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let dir = first.parent().unwrap_or(Path::new("."));
    let stem = &name[..name.len().saturating_sub(4)];
    if let Some(i) = stem.rfind(".part").filter(|&i| stem[i + 5..].chars().all(|c| c.is_ascii_digit()) && stem.len() > i + 5) {
        let (base, width) = (&first.file_name().unwrap().to_string_lossy()[..i], stem.len() - i - 5);
        for n in 2.. {
            let p = dir.join(format!("{base}.part{n:0width$}.rar"));
            if !p.exists() {
                break;
            }
            out.push(p);
        }
    } else {
        for n in 0..1000 {
            let p = first.with_extension(format!("r{n:02}"));
            if !p.exists() {
                break;
            }
            out.push(p);
        }
    }
    out
}

/// A RAR entry's stored time: Unix seconds in RAR5, a DOS date and time (local) in older ones.
fn rar_time(t: u32, rar5: bool) -> Option<SystemTime> {
    if rar5 {
        return Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(t.into()));
    }
    use chrono::{Local, NaiveDate, TimeZone};
    let (d, tm) = (t >> 16, t & 0xFFFF);
    let n = NaiveDate::from_ymd_opt(1980 + (d >> 9) as i32, (d >> 5) & 15, d & 31)?.and_hms_opt(tm >> 11, (tm >> 5) & 63, (tm & 31) * 2)?;
    Local.from_local_datetime(&n).earliest().map(SystemTime::from)
}

fn extract_rar(archive: &Path, map: Map,password: Option<&str>, progress: Progress) -> Result<(usize, usize), Fail> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    let fail = |e: rars::Error| match e.kind() {
        rars::ErrorKind::PasswordRequired | rars::ErrorKind::BadPassword => Fail::Password,
        _ => Fail::Other(e.to_string()),
    };
    let pw = password.map(str::as_bytes);
    // the password is needed already to read archives whose headers are encrypted
    let open = |p: &PathBuf| rars::ArchiveReader::read_path_with_options(p, rars::ArchiveReadOptions::with_optional_password(pw));
    let vols: Vec<rars::Archive> = rar_volumes(archive).iter().map(open).collect::<rars::Result<_>>().map_err(fail)?;
    let members = rars::volume_members(&vols).map_err(fail)?;
    let placed = |name: &[u8]| rars::entry_relative_path(name).ok().and_then(|n| map(&n));
    let total: u64 = members.iter().filter(|m| placed(&m.meta.name).is_some()).map(|m| m.meta.unpacked_size).sum();
    let rar5 = members.first().is_some_and(|m| m.meta.family == rars::ArchiveFamily::Rar50Plus);
    // bytes written so far, counted by the writers (they must be 'static); read when the next entry opens
    let done = Arc::new(AtomicU64::new(0));
    // straight to the file, no buffer: rars drops each writer without a flush, which would hide a
    // full disk; it hands over data in large blocks anyway
    struct Counted(File, Arc<AtomicU64>);
    impl io::Write for Counted {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            let n = self.0.write(b)?;
            self.1.fetch_add(n as u64, Ordering::Relaxed);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
    let (mut files, mut skipped) = (0, 0);
    // dates are set once everything is written (a file's writer lives until the next entry opens)
    let mut dates = vec![];
    rars::extract_volumes_to(&vols, pw, |meta| {
        progress(done.load(Ordering::Relaxed), total);
        let path = placed(&meta.name);
        let Some(path) = path else {
            skipped += 1;
            return Ok(Box::new(io::sink()));
        };
        if meta.is_directory {
            std::fs::create_dir_all(&path)?;
            return Ok(Box::new(io::sink()));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        files += 1;
        if let Some(t) = meta.file_time.and_then(|t| rar_time(t, rar5)) {
            dates.push((path.clone(), t));
        }
        Ok(Box::new(Counted(create_new(&path)?, done.clone())))
    })
    .map_err(fail)?;
    progress(total, total);
    for (p, t) in dates {
        if let Ok(f) = File::options().write(true).open(&p) {
            let _ = f.set_modified(t);
        }
    }
    Ok((files, skipped))
}

/// Every file under `paths` with its name inside the archive: relative to `base` (the folder the
/// selection is seen from, so the flat view keeps subfolders apart) or the bare name, '/'
/// separated. Folders are listed too, so empty ones survive; `skip` (the archive being written) is left out.
fn gather(paths: &[PathBuf], base: &Path, skip: &Path) -> Result<Vec<(PathBuf, String, bool)>, String> {
    let top = |p: &PathBuf| {
        let rel = p.strip_prefix(base).ok().filter(|r| !r.as_os_str().is_empty()).map(|r| r.iter().map(|c| c.to_string_lossy()).collect::<Vec<_>>().join("/"));
        Some((p.clone(), rel.or_else(|| Some(p.file_name()?.to_string_lossy().into_owned()))?))
    };
    // what cannot be read stops the job: a cut removes its sources after, so nothing may be left out
    let fail = |p: &Path, e: &dyn std::fmt::Display| format!("{}: {e}", p.display());
    let mut out = vec![];
    let mut stack: Vec<(PathBuf, String)> = paths.iter().filter_map(top).collect();
    while let Some((p, name)) = stack.pop() {
        if p == skip {
            continue;
        }
        let md = std::fs::symlink_metadata(&p).map_err(|e| fail(&p, &e))?;
        if md.is_dir() {
            out.push((p.clone(), name.clone(), true));
            for e in std::fs::read_dir(&p).map_err(|e| fail(&p, &e))? {
                let e = e.map_err(|e| fail(&p, &e))?;
                stack.push((e.path(), format!("{name}/{}", e.file_name().to_string_lossy())));
            }
        } else if md.is_file() {
            out.push((p, name, false));
        } else {
            return Err(fail(&p, &"to skrót systemowy (dowiązanie), którego archiwum nie obejmie"));
        }
    }
    Ok(out)
}

/// Packs `paths` (files and folders, named relative to `base`) into `dest`, a new .zip or .7z by
/// its extension. The archive file is claimed first (it must not exist), so two jobs never write
/// one; a failed pack removes only what it made.
pub fn pack(paths: &[PathBuf], base: &Path, dest: &Path, progress: Progress) -> Result<usize, String> {
    let kind = ext(dest);
    if kind != "zip" && kind != "7z" {
        return Err(format!("Nie umiem tworzyć archiwów .{kind}"));
    }
    let file = File::create_new(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let run = std::panic::AssertUnwindSafe(|| {
        let items = gather(paths, base, dest)?;
        let total: u64 = items.iter().filter(|i| !i.2).filter_map(|i| std::fs::metadata(&i.0).ok()).map(|m| m.len()).sum();
        if kind == "zip" { pack_zip(&items, file, total, progress) } else { pack_7z(&items, file, total, progress) }
    });
    let r = std::panic::catch_unwind(run).unwrap_or_else(|_| Err("przerwano przez wewnętrzny błąd".into()));
    if r.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    r
}

fn pack_zip(items: &[(PathBuf, String, bool)], file: File, total: u64, progress: Progress) -> Result<usize, String> {
    let mut w = zip::ZipWriter::new(BufWriter::new(file));
    let (mut done, mut files) = (0, 0);
    for (path, name, dir) in items {
        done += zip_add(&mut w, Some(path), name, *dir)?;
        if !*dir {
            files += 1;
            progress(done, total);
        }
    }
    zip_finish(w)?;
    Ok(files)
}

type ZipOut = zip::ZipWriter<BufWriter<File>>;

/// Writes entry `name` (a folder when `dir`) from file `path` (none: an empty folder), deflated,
/// with the file's local date as Explorer's "Wyślij do > Folder skompresowany". Returns its bytes.
fn zip_add(w: &mut ZipOut, path: Option<&Path>, name: &str, dir: bool) -> Result<u64, String> {
    use zip::write::SimpleFileOptions;
    let err = |e: &dyn std::fmt::Display| e.to_string();
    let md = path.and_then(|p| std::fs::metadata(p).ok());
    let mut opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    if let Some(t) = md.as_ref().and_then(|m| m.modified().ok()) {
        use chrono::{Datelike, Timelike};
        let d = chrono::DateTime::<chrono::Local>::from(t);
        if let Ok(zt) = zip::DateTime::from_date_and_time(d.year() as u16, d.month() as u8, d.day() as u8, d.hour() as u8, d.minute() as u8, d.second() as u8) {
            opts = opts.last_modified_time(zt);
        }
    }
    if dir {
        w.add_directory(name, opts).map_err(|e| err(&e))?;
        return Ok(0);
    }
    let path = path.ok_or("brak pliku do dodania")?;
    let len = md.map_or(0, |m| m.len());
    w.start_file(name, opts.large_file(len >= 0xFFFF_FFFF)).map_err(|e| err(&e))?;
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    io::copy(&mut f, w).map_err(|e| err(&e))?;
    Ok(len)
}

/// The buffer is flushed by hand: a dropped BufWriter would hide a full disk.
fn zip_finish(w: ZipOut) -> Result<(), String> {
    let mut out = w.finish().map_err(|e| e.to_string())?;
    io::Write::flush(&mut out).map_err(|e| e.to_string())
}

fn pack_7z(items: &[(PathBuf, String, bool)], file: File, total: u64, progress: Progress) -> Result<usize, String> {
    let err = |e: &dyn std::fmt::Display| e.to_string();
    let mut w = sevenz_rust2::ArchiveWriter::new(file).map_err(|e| err(&e))?;
    let (mut done, mut files) = (0, 0);
    for (path, name, dir) in items {
        let entry = sevenz_rust2::ArchiveEntry::from_path(path, name.clone());
        if *dir {
            w.push_archive_entry::<File>(entry, None).map_err(|e| err(&e))?;
            continue;
        }
        let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let len = f.metadata().map_or(0, |m| m.len());
        w.push_archive_entry(entry, Some(BufReader::new(f))).map_err(|e| err(&e))?;
        files += 1;
        done += len;
        progress(done, total);
    }
    w.finish().map_err(|e| err(&e))?;
    Ok(files)
}

/// One change to an archive's contents; names are '/'-separated from the archive's top.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// files and folders from disk into folder `into` ("" for the top), each under a name free
    /// there ("raport (2).csv" when "raport.csv" is in it)
    Add { from: Vec<PathBuf>, into: String },
    /// entries, a folder with everything in it
    Remove(Vec<String>),
    /// an entry (a folder with everything in it) to a new full name
    Rename { from: String, to: String },
    /// an empty folder in `into`: "Nowy folder", or "Nowy folder (2)"… when taken
    NewFolder { into: String },
    /// entry `name` gets the contents of file `from` (a copy edited outside)
    Replace { name: String, from: PathBuf },
}

/// ZIP and 7z can be changed; RAR only by WinRAR (convert() makes a ZIP of it).
pub fn can_update(path: &Path) -> bool {
    matches!(ext(path).as_str(), "zip" | "7z")
}

/// What a change does to the entries: names dropped (each with what is inside it), one rename,
/// what is written anew (from a file on disk, or an empty folder), and the names it made.
struct Plan {
    drop: Vec<String>,
    rename: Option<(String, String)>,
    add: Vec<(Option<PathBuf>, String, bool)>,
    made: Vec<String>,
}

/// `name` is `top` or inside it ("ab/x" is not inside "a").
fn under(name: &str, top: &str) -> bool {
    name == top || name.strip_prefix(top).is_some_and(|r| r.starts_with('/'))
}

impl Plan {
    fn drops(&self, name: &str) -> bool {
        self.drop.iter().any(|d| under(name, d))
    }

    fn renamed(&self, name: &str) -> Option<String> {
        let (from, to) = self.rename.as_ref()?;
        if name == from {
            return Some(to.clone());
        }
        name.strip_prefix(from.as_str())?.strip_prefix('/').map(|r| format!("{to}/{r}"))
    }
}

/// Every name of the list and every folder above one, lower case: what a new name must not hit
/// (Windows sees "A.txt" and "a.txt" as one file).
fn taken(all: &[Entry]) -> std::collections::HashSet<String> {
    let mut t = std::collections::HashSet::new();
    for e in all {
        let n = e.name.to_lowercase();
        for (i, _) in n.match_indices('/') {
            t.insert(n[..i].to_string());
        }
        t.insert(n);
    }
    t
}

/// `name` in folder `into`, else "name (2).ext", "name (3).ext"…; the name picked is taken then.
fn free_in(taken: &mut std::collections::HashSet<String>, into: &str, name: &str, dir: bool) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !dir && !s.is_empty() => (s, Some(e)),
        _ => (name, None),
    };
    let mut n = 1;
    loop {
        let leaf = match (n, ext) {
            (1, _) => name.to_string(),
            (_, Some(e)) => format!("{stem} ({n}).{e}"),
            _ => format!("{stem} ({n})"),
        };
        let full = if into.is_empty() { leaf } else { format!("{into}/{leaf}") };
        if taken.insert(full.to_lowercase()) {
            return full;
        }
        n += 1;
    }
}

fn plan(all: &[Entry], change: &Change, archive: &Path) -> Result<Plan, String> {
    let mut taken = taken(all);
    let mut p = Plan { drop: vec![], rename: None, add: vec![], made: vec![] };
    let leaf = |n: &str| n.rsplit('/').next().unwrap_or(n).to_string();
    match change {
        Change::Add { from, into } => {
            for src in from {
                let Some(name) = src.file_name() else { continue };
                let top = free_in(&mut taken, into, &name.to_string_lossy(), src.is_dir());
                // gather names it by its own name; here it may have become "x (2)"
                for (path, name, dir) in gather(std::slice::from_ref(src), src.parent().unwrap_or(Path::new("")), archive)? {
                    let full = match name.split_once('/') {
                        Some((_, rest)) => format!("{top}/{rest}"),
                        None => top.clone(),
                    };
                    p.add.push((Some(path), full, dir));
                }
                p.made.push(top);
            }
        }
        Change::Remove(names) => p.drop = names.clone(),
        Change::Rename { from, to } => {
            if !taken.contains(&from.to_lowercase()) {
                return Err(format!("W archiwum nie ma „{}”", leaf(from)));
            }
            // a name the list would not show (a device, a trailing dot…) would hide the entry
            if slash(Path::new(to)).as_deref() != Some(to.as_str()) {
                return Err(format!("Nazwy „{}” nie da się zapisać w Windows", leaf(to)));
            }
            // a change of letter case only is a rename too
            if to.to_lowercase() != from.to_lowercase() && taken.contains(&to.to_lowercase()) {
                return Err(format!("„{}” już jest w tym folderze archiwum", leaf(to)));
            }
            if under(to, from) {
                return Err("Folderu nie można przenieść do niego samego".into());
            }
            p.rename = Some((from.clone(), to.clone()));
            p.made.push(to.clone());
        }
        Change::NewFolder { into } => {
            let name = free_in(&mut taken, into, "Nowy folder", true);
            p.add.push((None, name.clone(), true));
            p.made.push(name);
        }
        Change::Replace { name, from } => {
            // renamed or deleted since it was opened: writing it back would bring it back
            if !all.iter().any(|e| e.name == *name && !e.dir) {
                return Err(format!("Pliku „{}” nie ma już w archiwum (zmieniono jego nazwę lub go usunięto)", leaf(name)));
            }
            p.drop.push(name.clone());
            p.add.push((Some(from.clone()), name.clone(), false));
            p.made.push(name.clone());
        }
    }
    // a folder whose last item goes stays as an empty folder, as on disk (in a ZIP a folder may
    // exist only in its files' names, and would vanish with them)
    let gone = p.drop.iter().chain(p.rename.iter().map(|r| &r.0));
    let ups: Vec<String> = gone.filter_map(|n| n.rsplit_once('/').map(|(up, _)| up.to_string())).collect();
    for up in ups {
        let stays = |n: &str| !p.drops(n) && p.renamed(n).is_none();
        // the folder itself going (removed with it, renamed), or kept by its own entry
        if !stays(&up) || all.iter().any(|e| e.dir && e.name == up) {
            continue;
        }
        let kept = all.iter().any(|e| under(&e.name, &up) && stays(&e.name));
        let added = p.add.iter().any(|a| under(&a.1, &up));
        if !kept && !added {
            p.add.push((None, up, true));
        }
    }
    Ok(p)
}

/// Applies `change` to `archive` (ZIP or 7z). A new archive is written next to it and takes its
/// place only when complete, so a failure (or a crash) leaves the archive as it was. ZIP entries
/// are copied as stored (fast, no re-compression; encrypted ones keep their password); a 7z is
/// unpacked and packed again. Returns the names made (Add, NewFolder, Rename, Replace), for the
/// list to select.
pub fn update(archive: &Path, change: &Change, progress: Progress) -> Result<Vec<String>, String> {
    let kind = ext(archive);
    if !can_update(archive) {
        return Err(format!("Archiwów .{kind} nie da się zmieniać: tworzy je tylko WinRAR"));
    }
    let all = list(archive).map_err(|e| match e {
        Fail::Password => "lista plików archiwum jest zaszyfrowana".to_string(),
        Fail::Other(e) => e,
    })?;
    let plan = plan(&all, change, archive)?;
    // the new archive ends in the archive's own extension (pack() goes by it)
    let name = archive.file_name().unwrap_or_default().to_string_lossy();
    let tmp = archive.with_file_name(format!("~{name}.{}.{kind}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let go = std::panic::AssertUnwindSafe(|| if kind == "zip" { update_zip(archive, &tmp, &plan, progress) } else { update_7z(archive, &tmp, &plan, progress) });
    // on the disk before it takes the archive's place: a power cut after the rename must not find
    // a renamed file whose data never got written
    let synced = |()| File::options().write(true).open(&tmp).and_then(|f| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()));
    let r = std::panic::catch_unwind(go)
        .unwrap_or_else(|_| Err("przerwano przez wewnętrzny błąd".into()))
        .and_then(synced)
        .and_then(|()| std::fs::rename(&tmp, archive).map_err(|e| format!("{}: {e}", archive.display())));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r.map(|()| plan.made)
}

fn update_zip(archive: &Path, tmp: &Path, plan: &Plan, progress: Progress) -> Result<(), String> {
    use zip::HasZipMetadata;
    let err = |e: zip::result::ZipError| e.to_string();
    let mut z = zip::ZipArchive::new(BufReader::new(File::open(archive).map_err(|e| e.to_string())?)).map_err(err)?;
    // what a rewrite could not keep is refused before anything is written
    if z.has_duplicate_names() {
        return Err("w archiwum są dwa wpisy o tej samej nazwie; zmiana zostawiłaby jeden z nich".into());
    }
    if z.offset() > 0 {
        return Err("archiwum ma dane przed pierwszym plikiem (samorozpakowujące się); zmiana by je usunęła".into());
    }
    let out = File::create_new(tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let mut w = zip::ZipWriter::new(BufWriter::new(out));
    w.set_raw_comment(z.comment().into()).map_err(err)?;
    let total = (z.len() + plan.add.len()) as u64;
    for i in 0..z.len() {
        let f = z.by_index_raw(i).map_err(err)?;
        // a name zip writes back as UTF-8: an old code-page name (CP852 from old Windows tools)
        // would come back mangled once renamed or replaced
        let legacy = !f.name_raw().is_ascii() && std::str::from_utf8(f.name_raw()).is_err();
        let legacy_note = || "nazwy plików w tym archiwum są w starym kodowaniu (sprzed UTF-8); zmiana by je zniekształciła".to_string();
        let kept = match zip_name(&f) {
            Some(n) if plan.drops(&n) => {
                if legacy && plan.add.iter().any(|a| a.1 == n) {
                    return Err(legacy_note());
                }
                None
            }
            Some(n) => match plan.renamed(&n) {
                Some(_) if legacy => return Err(legacy_note()),
                Some(to) => Some(Some(if f.is_dir() { format!("{to}/") } else { to })),
                None => Some(None),
            },
            // a name the list leaves out (a device name, "a:b"…): gone with its folder, else kept
            // as it was; a rename of its folder would split the folder, so it is refused
            None => {
                let raw = f.name().map(|n| n.replace('\\', "/")).unwrap_or_default();
                let raw = raw.trim_start_matches('/');
                if plan.drops(raw) {
                    None
                } else if plan.renamed(raw).is_some() {
                    return Err(format!("folder zawiera plik o nazwie niedozwolonej w Windows („{raw}”); zmiana nazwy rozdzieliłaby go"));
                } else {
                    Some(None)
                }
            }
        };
        if let Some(rename) = kept {
            // ZipCrypto (the old ZIP password) loses its flags in a raw copy and the entry would no
            // longer open; AES entries keep theirs
            if f.encrypted() && f.get_metadata().aes_settings().is_none() {
                return Err("pliki są zaszyfrowane starą metodą ZIP (ZipCrypto): zmiana by je uszkodziła; wypakuj archiwum, aby zmieniać".into());
            }
            match rename {
                Some(to) => w.raw_copy_file_rename(f, to).map_err(err)?,
                None => w.raw_copy_file(f).map_err(err)?,
            }
        }
        progress(i as u64 + 1, total);
    }
    for (k, (path, name, dir)) in plan.add.iter().enumerate() {
        zip_add(&mut w, path.as_deref(), name, *dir)?;
        progress((z.len() + k + 1) as u64, total);
    }
    zip_finish(w)
}

/// A folder of its own in the temp folder, for unpacking an archive whole.
fn work_dir(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    std::env::temp_dir().join(format!("rx-{tag}-{}-{n}", std::process::id()))
}

/// The top-level items of folder `dir`, for pack().
fn items_of(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect()
}

fn update_7z(archive: &Path, tmp: &Path, plan: &Plan, progress: Progress) -> Result<(), String> {
    let dir = work_dir("7z");
    let _ = std::fs::remove_dir_all(&dir);
    let mut work = || -> Result<(), String> {
        let (_, skipped) = extract(archive, &dir, None, &mut |d, t| progress(d, t * 2)).map_err(|e| match e {
            Fail::Password => "zaszyfrowanego archiwum 7z nie da się zmieniać".to_string(),
            Fail::Other(e) => e,
        })?;
        // a 7z is packed again from what was unpacked: an entry Windows cannot hold would be lost
        if skipped > 0 {
            return Err(format!("archiwum zawiera nazwy, których Windows nie zapisze (plików: {skipped}); zmiana by je usunęła"));
        }
        let at = |n: &str| inside(&dir, Path::new(n)).ok_or_else(|| format!("zła nazwa: {n}"));
        let io = |e: io::Error| e.to_string();
        for d in &plan.drop {
            let p = at(d)?;
            if p.is_dir() {
                std::fs::remove_dir_all(&p).map_err(io)?;
            } else if p.exists() {
                std::fs::remove_file(&p).map_err(io)?;
            }
        }
        if let Some((from, to)) = &plan.rename {
            let to = at(to)?;
            if let Some(up) = to.parent() {
                std::fs::create_dir_all(up).map_err(io)?;
            }
            std::fs::rename(at(from)?, &to).map_err(io)?;
        }
        for (src, name, is_dir) in &plan.add {
            let p = at(name)?;
            if *is_dir {
                std::fs::create_dir_all(&p).map_err(io)?;
                continue;
            }
            if let Some(up) = p.parent() {
                std::fs::create_dir_all(up).map_err(io)?;
            }
            let src = src.as_deref().ok_or("brak pliku do dodania")?;
            std::fs::copy(src, &p).map_err(|e| format!("{}: {e}", src.display()))?;
        }
        pack(&items_of(&dir), &dir, tmp, &mut |d, t| progress(t + d, t * 2)).map(|_| ())
    };
    let r = work();
    let _ = std::fs::remove_dir_all(&dir);
    r
}

/// `archive` (a RAR, which cannot be changed) packed again as `dest`, a new .zip or .7z with the
/// same files; refused when a name could not be carried over. Returns how many files it holds.
pub fn convert(archive: &Path, dest: &Path, password: Option<&str>, progress: Progress) -> Result<usize, Fail> {
    let dir = work_dir("conv");
    let _ = std::fs::remove_dir_all(&dir);
    let r = extract(archive, &dir, password, &mut |d, t| progress(d, t * 2)).and_then(|(_, skipped)| {
        if skipped > 0 {
            return Err(Fail::Other(format!("archiwum zawiera nazwy, których Windows nie zapisze (plików: {skipped}); kopia byłaby niepełna")));
        }
        pack(&items_of(&dir), &dir, dest, &mut |d, t| progress(t + d, t * 2)).map_err(Fail::Other)
    });
    let _ = std::fs::remove_dir_all(&dir);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rx-archive-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample(root: &Path) -> Vec<PathBuf> {
        std::fs::create_dir_all(root.join("Dokumenty/pusty")).unwrap();
        std::fs::write(root.join("Dokumenty/Żółw.txt"), "zażółć gęślą jaźń".repeat(100)).unwrap();
        std::fs::write(root.join("raport.csv"), b"a;b\n1;2\n").unwrap();
        vec![root.join("Dokumenty"), root.join("raport.csv")]
    }

    fn round_trip(ext: &str) {
        let d = tmp(ext);
        let src = d.join("src");
        let paths = sample(&src);
        let arc = d.join(format!("paczka.{ext}"));
        let mut calls = 0;
        assert_eq!(pack(&paths, &src, &arc, &mut |_, _| calls += 1).unwrap(), 2);
        assert!(calls >= 2);
        // never over an existing archive
        assert!(pack(&paths, &src, &arc, &mut |_, _| {}).is_err());
        assert!(arc.exists(), "a refused pack must not delete the archive that was there");
        let out = d.join("out");
        let (files, skipped) = extract(&arc, &out, None, &mut |_, _| {}).unwrap();
        assert_eq!((files, skipped), (2, 0));
        assert_eq!(std::fs::read_to_string(out.join("Dokumenty/Żółw.txt")).unwrap(), "zażółć gęślą jaźń".repeat(100));
        assert_eq!(std::fs::read(out.join("raport.csv")).unwrap(), b"a;b\n1;2\n");
        assert!(out.join("Dokumenty/pusty").is_dir());
        // browsing it as a folder: the top, a subfolder, then some entries unpacked on their own
        let all = list(&arc).unwrap();
        let top = folder(&all, "");
        let names: Vec<_> = top.iter().map(|e| (e.name.as_str(), e.dir)).collect();
        assert_eq!(names, [("Dokumenty", true), ("raport.csv", false)]);
        let text = "zażółć gęślą jaźń".repeat(100).len() as u64;
        assert_eq!(top[0].size, text, "a folder is as big as its files");
        let sub: Vec<_> = folder(&all, "Dokumenty").into_iter().map(|e| e.name).collect();
        assert_eq!(sub, ["Dokumenty/pusty", "Dokumenty/Żółw.txt"]);
        let picked = d.join("picked");
        std::fs::create_dir(&picked).unwrap();
        let picks = [("Dokumenty".to_string(), picked.join("Dok")), ("raport.csv".to_string(), picked.join("r.csv"))];
        assert_eq!(extract_entries(&arc, &picks, None, &mut |_, _| {}).unwrap(), (2, 0));
        assert_eq!(std::fs::read(picked.join("r.csv")).unwrap(), b"a;b\n1;2\n");
        assert!(picked.join("Dok/Żółw.txt").is_file() && picked.join("Dok/pusty").is_dir());
        // never over what is there; nothing of the refused job is left
        assert!(extract_entries(&arc, &picks, None, &mut |_, _| {}).is_err());
        assert!(picked.join("r.csv").exists(), "an existing file is not removed by a refused job");
        // a name that is not in the archive makes nothing, and its work folder goes
        assert_eq!(extract_entries(&arc, &[("nie ma".into(), picked.join("nic"))], None, &mut |_, _| {}).unwrap(), (0, 0));
        let mut left: Vec<_> = std::fs::read_dir(&picked).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["Dok", "r.csv"]);
        // one file of a (solid) 7z or a ZIP on its own
        assert_eq!(extract_entries(&arc, &[("Dokumenty/Żółw.txt".into(), picked.join("ż.txt"))], None, &mut |_, _| {}).unwrap(), (1, 0));
        assert_eq!(std::fs::read_to_string(picked.join("ż.txt")).unwrap(), "zażółć gęślą jaźń".repeat(100));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn one_big_file_reports_progress_inside_it() {
        for ext in ["zip", "7z"] {
            let d = tmp(&format!("big-{ext}"));
            let src = d.join("src");
            std::fs::create_dir(&src).unwrap();
            std::fs::write(src.join("duzy.bin"), vec![7u8; 4 << 20]).unwrap();
            let arc = d.join(format!("a.{ext}"));
            pack(&[src.join("duzy.bin")], &src, &arc, &mut |_, _| {}).unwrap();
            let mut seen = vec![];
            extract(&arc, &d.join("out"), None, &mut |a, b| seen.push((a, b))).unwrap();
            assert!(seen.len() >= 3, "{ext}: {seen:?}");
            assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0) && seen.last() == Some(&(4 << 20, 4 << 20)), "{ext}: {seen:?}");
            std::fs::remove_dir_all(&d).unwrap();
        }
    }

    #[test]
    fn extract_here_never_replaces_what_is_there() {
        let d = tmp("here");
        let src = d.join("src");
        let arc = d.join("paczka.zip");
        pack(&sample(&src), &src, &arc, &mut |_, _| {}).unwrap();
        let out = d.join("out");
        std::fs::create_dir(&out).unwrap();
        std::fs::write(out.join("raport.csv"), b"moje").unwrap();
        let (files, skipped, mut placed) = extract_here(&arc, &out, None, &mut |_, _| {}).unwrap();
        placed.sort();
        assert_eq!((files, skipped), (2, 0));
        assert_eq!(placed, [out.join("Dokumenty"), out.join("raport (2).csv")]);
        assert_eq!(std::fs::read(out.join("raport.csv")).unwrap(), b"moje");
        assert_eq!(std::fs::read(out.join("raport (2).csv")).unwrap(), b"a;b\n1;2\n");
        assert!(out.join("Dokumenty/Żółw.txt").is_file());
        // a second time: the folder gets a number too, and no work folder is left behind
        let (_, _, again) = extract_here(&arc, &out, None, &mut |_, _| {}).unwrap();
        assert!(again.contains(&out.join("Dokumenty (2)")) && again.contains(&out.join("raport (3).csv")));
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 5);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_folder_seen_only_in_file_names_is_listed() {
        let e = |name: &str, dir, size| Entry { name: name.into(), dir, size, modified: None };
        let all = [e("a/b/c.txt", false, 3), e("a/d.txt", false, 4), e("x.txt", false, 1)];
        assert_eq!(folder(&all, ""), [e("a", true, 7), e("x.txt", false, 1)]);
        assert_eq!(folder(&all, "a"), [e("a/b", true, 3), e("a/d.txt", false, 4)]);
        assert_eq!(folder(&all, "a/b"), [e("a/b/c.txt", false, 3)]);
        // "ab" is not inside "a"
        assert!(folder(&[e("ab/z", false, 1)], "a").is_empty());
    }

    #[test]
    fn picked_entries_cannot_climb_out() {
        let d = tmp("pick-slip");
        let arc = d.join("zly.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&arc).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("a/../../poza.txt", o).unwrap();
            io::Write::write_all(&mut w, b"x").unwrap();
            w.start_file("a/ok.txt", o).unwrap();
            io::Write::write_all(&mut w, b"y").unwrap();
            w.finish().unwrap();
        }
        assert_eq!(list(&arc).unwrap().len(), 1, "an unsafe name is not even listed");
        assert_eq!(extract_entries(&arc, &[("a".into(), d.join("out"))], None, &mut |_, _| {}).unwrap(), (1, 1), "one written, one left out");
        assert!(!d.join("poza.txt").exists() && d.join("out/ok.txt").is_file());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn zip_round_trip() {
        round_trip("zip");
    }

    fn names(arc: &Path) -> Vec<String> {
        let mut v: Vec<String> = list(arc).unwrap().into_iter().map(|e| e.name).collect();
        v.sort();
        v
    }

    /// Every change, as the list's commands make it: paste in (with a name already there), a new
    /// folder twice, a folder renamed with what is inside, refusals that leave the archive alone,
    /// removal, and an edited copy written back.
    fn updates(ext: &str) {
        let d = tmp(&format!("upd-{ext}"));
        let src = d.join("src");
        let arc = d.join(format!("paczka.{ext}"));
        pack(&sample(&src), &src, &arc, &mut |_, _| {}).unwrap();
        let extra = d.join("extra");
        std::fs::create_dir_all(extra.join("Folder/w środku")).unwrap();
        std::fs::write(extra.join("Folder/w środku/plik.txt"), "głęboko").unwrap();
        std::fs::write(extra.join("RAPORT.CSV"), "nowy").unwrap();
        let none = &mut |_: u64, _: u64| {};
        let add = |from: Vec<PathBuf>, into: &str| Change::Add { from, into: into.into() };
        // "RAPORT.CSV" is "raport.csv" on Windows: it gets a number, not the old one's place
        assert_eq!(update(&arc, &add(vec![extra.join("RAPORT.CSV"), extra.join("Folder")], ""), none).unwrap(), ["RAPORT (2).CSV", "Folder"]);
        assert_eq!(update(&arc, &add(vec![extra.join("RAPORT.CSV")], "Dokumenty"), none).unwrap(), ["Dokumenty/RAPORT.CSV"]);
        let folder = Change::NewFolder { into: "Dokumenty".into() };
        assert_eq!(update(&arc, &folder, none).unwrap(), ["Dokumenty/Nowy folder"]);
        assert_eq!(update(&arc, &folder, none).unwrap(), ["Dokumenty/Nowy folder (2)"]);
        update(&arc, &Change::Rename { from: "Dokumenty".into(), to: "Papiery".into() }, none).unwrap();
        let before = std::fs::read(&arc).unwrap();
        for bad in [
            Change::Rename { from: "raport.csv".into(), to: "Papiery".into() },
            Change::Rename { from: "nie ma".into(), to: "x".into() },
            Change::Rename { from: "Papiery".into(), to: "Papiery/w sobie".into() },
        ] {
            assert!(update(&arc, &bad, none).is_err(), "{bad:?}");
        }
        assert_eq!(std::fs::read(&arc).unwrap(), before, "a refused change leaves the archive byte for byte");
        update(&arc, &Change::Remove(vec!["raport.csv".into(), "Folder".into()]), none).unwrap();
        let edited = d.join("edited.txt");
        std::fs::write(&edited, "poprawione").unwrap();
        update(&arc, &Change::Replace { name: "Papiery/Żółw.txt".into(), from: edited }, none).unwrap();
        assert_eq!(
            names(&arc),
            ["Papiery", "Papiery/Nowy folder", "Papiery/Nowy folder (2)", "Papiery/RAPORT.CSV", "Papiery/pusty", "Papiery/Żółw.txt", "RAPORT (2).CSV"]
        );
        let out = d.join("out");
        extract(&arc, &out, None, none).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("Papiery/Żółw.txt")).unwrap(), "poprawione");
        assert_eq!(std::fs::read_to_string(out.join("RAPORT (2).CSV")).unwrap(), "nowy");
        assert!(out.join("Papiery/Nowy folder (2)").is_dir());
        // nothing of the work is left next to the archive
        let left: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with('~')).collect();
        assert!(left.is_empty(), "{left:?}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn zip_changes() {
        updates("zip");
    }

    #[test]
    fn sevenz_changes() {
        updates("7z");
    }

    /// A rewrite refuses what it could not carry over, and leaves the archive byte for byte.
    #[test]
    fn changes_that_would_lose_something_are_refused() {
        let d = tmp("refuse");
        let none = &mut |_: u64, _: u64| {};
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let new_folder = Change::NewFolder { into: String::new() };
        let unchanged = |arc: &Path, change: &Change, why: &str| {
            let before = std::fs::read(arc).unwrap();
            let e = update(arc, change, &mut |_, _| {}).expect_err(why);
            assert_eq!(std::fs::read(arc).unwrap(), before, "{why}: {e}");
        };
        // the old ZIP password (ZipCrypto) loses its flags in a raw copy
        let zc = d.join("zipcrypto.zip");
        std::fs::copy(fixtures.join("zipcrypto.zip"), &zc).unwrap();
        unchanged(&zc, &new_folder, "ZipCrypto");
        // an old code-page name (not UTF-8) would come back mangled when renamed; other changes keep it
        let legacy = d.join("legacy.zip");
        std::fs::copy(fixtures.join("legacy.zip"), &legacy).unwrap();
        unchanged(&legacy, &Change::Rename { from: "Dok".into(), to: "Papiery".into() }, "legacy name");
        update(&legacy, &Change::Remove(vec!["inny.txt".into()]), none).unwrap();
        assert_eq!(names(&legacy).len(), 1, "the legacy entry stays, raw");
        // names a list would hide, an entry deleted meanwhile
        let arc = d.join("p.zip");
        pack(&sample(&d.join("src")), &d.join("src"), &arc, none).unwrap();
        for (bad, why) in [("Dokumenty/..", ".."), ("aux.txt", "device"), ("kropka.", "trailing dot")] {
            unchanged(&arc, &Change::Rename { from: "raport.csv".into(), to: bad.into() }, why);
        }
        unchanged(&arc, &Change::Replace { name: "nie ma.txt".into(), from: d.join("src/raport.csv") }, "gone meanwhile");
        // a 7z holding a name Windows cannot keep would lose it in the repack
        let sz = d.join("linux.7z");
        {
            let mut w = sevenz_rust2::ArchiveWriter::new(File::create(&sz).unwrap()).unwrap();
            for name in ["src/aux.c", "src/main.c"] {
                w.push_archive_entry(sevenz_rust2::ArchiveEntry::new_file(name), Some(&b"int x;"[..])).unwrap();
            }
            w.finish().unwrap();
        }
        unchanged(&sz, &new_folder, "7z with a device name");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_zip_keeps_its_comment_and_its_emptied_folders() {
        let d = tmp("keep");
        let arc = d.join("k.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&arc).unwrap());
            w.set_comment("Wyciągi 2026").unwrap();
            // a folder seen only in its file's name, as many tools write ZIPs
            w.start_file("Dok/x.txt", zip::write::SimpleFileOptions::default()).unwrap();
            io::Write::write_all(&mut w, b"x").unwrap();
            w.finish().unwrap();
        }
        update(&arc, &Change::Remove(vec!["Dok/x.txt".into()]), &mut |_, _| {}).unwrap();
        assert_eq!(names(&arc), ["Dok"], "the folder stays, empty");
        let z = zip::ZipArchive::new(File::open(&arc).unwrap()).unwrap();
        assert_eq!(z.comment(), "Wyciągi 2026".as_bytes());
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// "A.txt" and "a.txt" are one file on Windows: unpacking both stops instead of the second
    /// replacing the first (on Linux they are two files).
    #[test]
    fn names_windows_sees_as_one_do_not_overwrite_each_other() {
        let d = tmp("case");
        let arc = d.join("case.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&arc).unwrap());
            for name in ["Dok/Readme.txt", "Dok/README.txt"] {
                w.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
                io::Write::write_all(&mut w, name.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        let r = extract_entries(&arc, &[("Dok".into(), d.join("out"))], None, &mut |_, _| {});
        if cfg!(windows) {
            assert!(r.is_err(), "{r:?}");
            assert!(!d.join("out").exists(), "nothing half-made is left");
        } else {
            assert_eq!(r.unwrap(), (2, 0));
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn encrypted_zip_entries_keep_their_password_through_changes() {
        let d = tmp("upd-aes");
        let arc = d.join("tajne.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&arc).unwrap());
            let o = zip::write::SimpleFileOptions::default().with_aes_encryption(zip::AesMode::Aes256, "sekret");
            w.start_file("wyciąg.pdf", o).unwrap();
            io::Write::write_all(&mut w, b"%PDF tajne").unwrap();
            w.finish().unwrap();
        }
        let note = d.join("notatka.txt");
        std::fs::write(&note, "jawna").unwrap();
        let none = &mut |_: u64, _: u64| {};
        // copied as stored: no password needed to change the archive around it
        update(&arc, &Change::Add { from: vec![note], into: String::new() }, none).unwrap();
        update(&arc, &Change::Rename { from: "wyciąg.pdf".into(), to: "stary wyciąg.pdf".into() }, none).unwrap();
        assert_eq!(names(&arc), ["notatka.txt", "stary wyciąg.pdf"]);
        let pick = |name: &str, to: &str| vec![(name.to_string(), d.join(to))];
        assert_eq!(extract_entries(&arc, &pick("stary wyciąg.pdf", "a.pdf"), None, none), Err(Fail::Password));
        assert_eq!(extract_entries(&arc, &pick("stary wyciąg.pdf", "a.pdf"), Some("sekret"), none).unwrap().0, 1);
        assert_eq!(std::fs::read(d.join("a.pdf")).unwrap(), b"%PDF tajne");
        assert_eq!(extract_entries(&arc, &pick("notatka.txt", "n.txt"), None, none).unwrap().0, 1);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn rar_is_read_only_and_converts_to_zip() {
        let rar = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/solid_rar300.rar");
        assert!(!can_update(&rar) && can_update(Path::new("a.ZIP")) && can_update(Path::new("a.7z")));
        assert!(update(&rar, &Change::NewFolder { into: String::new() }, &mut |_, _| {}).is_err());
        let d = tmp("conv");
        let zip = d.join("solid.zip");
        assert_eq!(convert(&rar, &zip, None, &mut |_, _| {}).unwrap(), 3);
        let files = |a: &Path| {
            let mut v: Vec<String> = list(a).unwrap().into_iter().filter(|e| !e.dir).map(|e| e.name).collect();
            v.sort();
            v
        };
        assert_eq!(files(&zip), files(&rar));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn flat_view_names_keep_their_folders_and_skip_the_archive() {
        let d = tmp("gather");
        let src = d.join("src");
        sample(&src);
        // a flat view selects a file deep down; its name stays relative to the folder shown
        let arc = src.join("paczka.zip");
        std::fs::write(&arc, b"").unwrap();
        let got = gather(&[src.join("Dokumenty/Żółw.txt"), src.join("raport.csv"), arc.clone()], &src, &arc).unwrap();
        let mut names: Vec<_> = got.into_iter().map(|(_, n, _)| n).collect();
        names.sort();
        assert_eq!(names, ["Dokumenty/Żółw.txt", "raport.csv"]);
        // a source that cannot be read stops the job instead of being left out
        assert!(gather(&[src.join("nie ma")], &src, &arc).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn sevenz_round_trip() {
        round_trip("7z");
    }

    #[test]
    fn rar_extracts() {
        // samples from the unrar and rars crates' tests: RAR5 solid, RAR 1.5-4 single, RAR 3.0 solid,
        // and a RAR5 set of three volumes opened from its first part
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        for (name, want) in [("solid.rar", 1), ("version.rar", 1), ("solid_rar300.rar", 3), ("multivol.part1.rar", 1)] {
            let out = tmp(&format!("rar-{name}"));
            let mut last = (0, 0);
            let (files, skipped) = extract(&dir.join(name), &out.join("x"), None, &mut |d, t| last = (d, t)).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!((files, skipped), (want, 0), "{name}");
            assert_eq!(last.0, last.1, "{name}: progress ends at 100 %");
            let files = list(&dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e:?}")).into_iter().filter(|e| !e.dir).count();
            assert_eq!(files, want, "{name}: listed as unpacked");
            std::fs::remove_dir_all(&out).unwrap();
        }
        assert_eq!(rar_volumes(&dir.join("multivol.part1.rar")).len(), 3);
        // a lone middle volume is an error, and the folder made for it is removed
        let out = tmp("rar-mid");
        assert!(extract(&dir.join("multivol.part2.rar"), &out.join("x"), None, &mut |_, _| {}).is_err());
        assert!(!out.join("x").exists());
        std::fs::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn names_that_climb_out_are_refused() {
        let dest = Path::new("/x/out");
        assert_eq!(inside(dest, Path::new("a/b.txt")), Some(PathBuf::from("/x/out/a/b.txt")));
        assert_eq!(inside(dest, Path::new("../evil.txt")), None);
        assert_eq!(inside(dest, Path::new("a/../../evil.txt")), None);
        assert_eq!(inside(dest, Path::new("/etc/passwd")), None);
        assert_eq!(inside(dest, Path::new(".")), None);
        // on Windows "C:evil.txt" pushed onto a path replaces it; "a:b" is a hidden data stream
        assert_eq!(inside(dest, Path::new("x/C:evil.txt")), None);
        assert_eq!(inside(dest, Path::new("plik.txt:ukryty")), None);
        assert_eq!(slash(Path::new("x/C:evil.txt")), None, "listed only what unpacking accepts");
        // devices, not files, on Windows; a name merely starting like one is a file
        for device in ["CON", "nul.txt", "Com1.log", "LPT9", "aux .x"] {
            assert_eq!(inside(dest, Path::new(device)), None, "{device}");
        }
        for file in ["CONSOLE.txt", "COM0", "COM10", "Żółw.txt", "ŻóCOM1"] {
            assert!(inside(dest, Path::new(file)).is_some(), "{file}");
        }
    }

    #[test]
    fn zip_slip_entries_are_skipped() {
        let d = tmp("slip");
        let arc = d.join("zly.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&arc).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("../poza.txt", o).unwrap();
            io::Write::write_all(&mut w, b"x").unwrap();
            w.start_file("dobry.txt", o).unwrap();
            io::Write::write_all(&mut w, b"y").unwrap();
            w.finish().unwrap();
        }
        let out = d.join("out");
        assert_eq!(extract(&arc, &out, None, &mut |_, _| {}).unwrap(), (1, 1));
        assert!(!d.join("poza.txt").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn passwords_are_asked_for_and_checked() {
        let d = tmp("pw");
        // WinZip AES, as bank statements come
        let arc = d.join("tajne.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&arc).unwrap());
            let o = zip::write::SimpleFileOptions::default().with_aes_encryption(zip::AesMode::Aes256, "sekret");
            w.start_file("wyciąg.pdf", o).unwrap();
            io::Write::write_all(&mut w, b"%PDF tajne").unwrap();
            w.finish().unwrap();
        }
        let rar = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/crypted.rar");
        for (a, good) in [(&arc, "sekret"), (&rar, "unrar")] {
            let out = d.join("out");
            assert_eq!(extract(a, &out, None, &mut |_, _| {}), Err(Fail::Password), "{a:?} without a password");
            assert!(!out.exists(), "nothing left behind");
            assert_eq!(extract(a, &out, Some("zle"), &mut |_, _| {}), Err(Fail::Password), "{a:?} with a wrong one");
            assert_eq!(extract(a, &out, Some(good), &mut |_, _| {}).unwrap().0, 1, "{a:?}");
            std::fs::remove_dir_all(&out).unwrap();
        }
        std::fs::remove_dir_all(&d).unwrap();
    }
}
