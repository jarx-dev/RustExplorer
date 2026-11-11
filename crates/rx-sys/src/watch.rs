//! Tells the app that a folder on screen changed, so the window need not wake up to ask. No handle
//! is kept on a shown folder: Windows would then refuse other programs a rename or move of the
//! folders above it. One thread waits on a single watch of the whole tree at the root of each
//! volume in use and picks out the changes that concern the folders shown. That is done for a
//! fixed disk that cannot be unplugged only, as a handle on a root holds up "Safely remove";
//! folders anywhere else (network, removable, external and virtual drives) are compared with their
//! lists every 10 s on a second thread, as a drive that does not answer takes seconds.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_NOTIFY_ENUM_DIR, HANDLE, WAIT_FAILED, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Ioctl::{IOCTL_STORAGE_GET_HOTPLUG_INFO, STORAGE_HOTPLUG_INFO};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects, INFINITE};
use windows::Win32::System::IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED};

/// A folder written to all the time (a download) is compared, and so reported, at most this often.
const THROTTLE: Duration = Duration::from_secs(1);
/// How often a folder is compared where no watch is relied on.
const SLOW: Duration = Duration::from_secs(10);
/// A volume's changes are left to gather this long before the next are read: a busy disk wakes
/// the thread twice a second, not once per change (the first change of a quiet one is read at once).
const GATHER: Duration = Duration::from_millis(500);
/// Most items compared, as the app's own list is capped.
const CAP: usize = 20_000;
/// The buffer for a volume's changes, in 4-byte units (their records start on such a boundary).
const BUF: usize = 16 * 1024;

type Told = Arc<dyn Fn(&Path, bool) + Send + Sync>;

enum Order {
    Show(Vec<(PathBuf, u64)>),
    Quit,
}

pub struct Watcher {
    tx: Sender<Order>,
    wake: usize,
}

impl Watcher {
    /// `changed` runs on a watcher thread with a shown folder whose content changed; true: the
    /// folder cannot be read (deleted, its drive gone), so there is nothing to list.
    pub fn new(changed: impl Fn(&Path, bool) + Send + Sync + 'static) -> Self {
        let (tx, rx) = channel();
        // no event (should not happen): no thread, and folders are refreshed by hand
        let Ok(wake) = (unsafe { CreateEventW(None, false, false, PCWSTR::null()) }) else { return Watcher { tx, wake: 0 } };
        let wake = wake.0 as usize;
        let told: Told = Arc::new(changed);
        // the folders that are only compared have a thread of their own: a share that does not
        // answer must not hold up the changes of a local disk
        let (slow_tx, slow_rx) = channel();
        let slow_told = told.clone();
        std::thread::spawn(move || compare(slow_rx, slow_told));
        std::thread::spawn(move || run(rx, HANDLE(wake as *mut _), slow_tx, told));
        Watcher { tx, wake }
    }

    /// The folders on screen from now on, each with the `print_of` the list shown for it; the
    /// others are let go. A folder that differs from its list is reported.
    pub fn show(&self, dirs: Vec<(PathBuf, u64)>) {
        self.order(Order::Show(dirs));
    }

    fn order(&self, o: Order) {
        let _ = self.tx.send(o);
        unsafe {
            let _ = SetEvent(HANDLE(self.wake as *mut _));
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.order(Order::Quit);
    }
}

/// A folder's list as one number, from each item's name, size (0 for a folder), write time and
/// attributes. Never 0, and the same in any order (a share may list in another one).
pub fn print_of<'a>(items: impl Iterator<Item = (&'a str, u64, Option<SystemTime>, u32)>) -> u64 {
    let sum = items.fold(1u64, |sum, item| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        item.hash(&mut h);
        sum.wrapping_add(h.finish())
    });
    sum.max(1)
}

/// `print_of` the folder as it is on disk now; 0 when it cannot be read.
fn print(dir: &Path) -> u64 {
    use std::os::windows::fs::MetadataExt;
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let items: Vec<_> = rd.flatten().filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), e.metadata().ok()?))).take(CAP).collect();
    print_of(items.iter().map(|(name, m)| (name.as_str(), if m.is_dir() { 0 } else { m.len() }, m.modified().ok(), m.file_attributes())))
}

/// A folder on screen.
struct Dir {
    path: PathBuf,
    /// its list as the app holds it (`print_of`); 0: none, the folder could not be read
    print: u64,
    /// something may have changed it: compared with the list at the next turn
    check: bool,
    /// when it is compared unasked; never where its volume's watch is relied on
    due: Option<Instant>,
    looked: Option<Instant>,
    /// for a watched folder: its volume's root (`Vol::root`) and its real path under it, lower-case
    /// with no backslash at either end, as the volume's changes are matched (`rx_core::concerns`)
    root: String,
    under: String,
}

impl Dir {
    /// A folder no watch covers: compared now and every SLOW.
    fn compared(path: PathBuf, shown: u64) -> Dir {
        Dir { path, print: shown, check: false, due: Some(Instant::now()), looked: None, root: String::new(), under: String::new() }
    }

    /// A folder followed through the watch of its volume, which is started if it is the first one
    /// there; None where there can be no watch, and the folder is to be compared.
    fn watched(path: &Path, shown: u64, vols: &mut Vec<Vol>) -> Option<Dir> {
        // by the drive letter alone, before anything is read: a network path must not hold up this thread
        if path.to_string_lossy().starts_with(r"\\") || crate::drive_type(path.ancestors().last()?) != 3 {
            return None;
        }
        // junctions and substituted letters are followed: changes are told under the real path
        let real = std::fs::canonicalize(path).ok()?;
        let real = real.to_str()?.strip_prefix(r"\\?\").filter(|p| p.as_bytes().get(1) == Some(&b':'))?.to_string();
        let mut root = [0u16; 262];
        unsafe { GetVolumePathNameW(&HSTRING::from(real.as_str()), &mut root) }.ok()?;
        let root = crate::from_wide(&root);
        let low = root.to_lowercase();
        let under = real.to_lowercase().strip_prefix(&low)?.trim_end_matches('\\').to_string();
        if !vols.iter().any(|v| v.root == low) {
            let at = Path::new(&root);
            if crate::drive_type(at) != 3 || unpluggable(&root) {
                return None;
            }
            // other file systems (a cloud drive is FAT32 by name) may not tell all: compared as well
            vols.push(Vol::open(&root, !matches!(crate::fs_name(at).as_str(), "NTFS" | "ReFS"))?);
        }
        let timed = vols.iter().find(|v| v.root == low)?.timed;
        // watched before the first look below: what changes meanwhile is told
        Some(Dir { path: path.to_path_buf(), print: shown, check: true, due: timed.then(Instant::now), looked: None, root: low, under })
    }

    /// Compares the folder with the list the app holds, which is taken to be the new one from
    /// then on; Some when they differ, true: the folder cannot be read.
    fn look(&mut self) -> Option<bool> {
        let t0 = Instant::now();
        let now = print(&self.path);
        let differs = now != self.print;
        (self.print, self.check, self.looked) = (now, false, Some(Instant::now()));
        if self.due.is_some() {
            // a drive that takes seconds to answer is asked less often
            self.due = Some(Instant::now() + SLOW.max(t0.elapsed() * 5));
        }
        differs.then_some(now == 0)
    }

    /// Looks at the folder if it is time: its own, or after a change, at most once in THROTTLE.
    /// Returns when it is to be tended next.
    fn tend(&mut self, told: &Told) -> Option<Instant> {
        let now = Instant::now();
        let held = self.looked.map(|t| t + THROTTLE).filter(|t| *t > now);
        if self.due.is_some_and(|t| t <= now) || (self.check && held.is_none()) {
            if let Some(gone) = self.look() {
                told(&self.path, gone);
            }
        }
        [self.due, held.filter(|_| self.check)].into_iter().flatten().min()
    }
}

/// Keeps of `dirs` those still in `want` (one whose list the app holds differently now is compared
/// again) and returns the wanted folders that are not among them.
fn keep(dirs: &mut Vec<Dir>, want: Vec<(PathBuf, u64)>) -> Vec<(PathBuf, u64)> {
    dirs.retain(|d| want.iter().any(|w| w.0 == d.path));
    let new = |(p, shown): &(PathBuf, u64)| match dirs.iter_mut().find(|d| d.path == *p) {
        Some(d) => {
            if d.print != *shown {
                (d.print, d.check) = (*shown, true);
            }
            false
        }
        None => true,
    };
    want.into_iter().filter(new).collect()
}

/// The thread of the folders that are only compared: `rx` brings them, all of them each time.
fn compare(rx: Receiver<Vec<(PathBuf, u64)>>, told: Told) {
    let mut dirs: Vec<Dir> = vec![];
    let mut next: Option<Instant> = None;
    loop {
        let got = match next {
            Some(t) => rx.recv_timeout(t.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match got {
            Ok(want) => {
                let new = keep(&mut dirs, want);
                dirs.extend(new.into_iter().map(|(p, shown)| Dir::compared(p, shown)));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        next = dirs.iter_mut().filter_map(|d| d.tend(&told)).min();
    }
}

/// Whether the disk under the volume at `root` can be unplugged (USB, a hot-swap bay), or that
/// cannot be told: a handle kept on such a volume would hold up "Safely remove".
fn unpluggable(root: &str) -> bool {
    unsafe {
        let mut name = [0u16; 64];
        if GetVolumeNameForVolumeMountPointW(&HSTRING::from(root), &mut name).is_err() {
            return true;
        }
        // the volume itself ("\\?\Volume{…}" with no backslash at the end) answers for its disk; no rights are asked
        let name = crate::from_wide(&name);
        let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
        let Ok(vol) = CreateFileW(&HSTRING::from(name.trim_end_matches('\\')), 0, share, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) else { return true };
        let (mut info, mut n) = (STORAGE_HOTPLUG_INFO::default(), 0u32);
        let asked = DeviceIoControl(vol, IOCTL_STORAGE_GET_HOTPLUG_INFO, None, 0, Some(&mut info as *mut _ as *mut _), size_of_val(&info) as u32, Some(&mut n), None);
        let _ = CloseHandle(vol);
        asked.is_err() || info.MediaRemovable || info.MediaHotplug || info.DeviceHotplug
    }
}

/// The watch of a volume's whole tree, held on its root folder, where it is in no rename's way.
struct Vol {
    /// lower-case, ending in the backslash
    root: String,
    /// its folders are compared every SLOW as well
    timed: bool,
    dir: HANDLE,
    /// boxed: Windows writes to both while a read is under way, wherever the Vol itself moves
    io: Box<OVERLAPPED>,
    buf: Box<[u32]>,
    reading: bool,
    /// when the next read is asked for (GATHER)
    again: Instant,
    /// the watch ended: its folders go over to being compared
    dead: bool,
}

impl Vol {
    fn open(root: &str, timed: bool) -> Option<Vol> {
        unsafe {
            let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
            let dir = CreateFileW(&HSTRING::from(root), FILE_LIST_DIRECTORY.0, share, None, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED, None).ok()?;
            let Ok(event) = CreateEventW(None, true, false, PCWSTR::null()) else {
                let _ = CloseHandle(dir);
                return None;
            };
            let io = Box::new(OVERLAPPED { hEvent: event, ..Default::default() });
            let mut v = Vol { root: root.to_lowercase(), timed, dir, io, buf: vec![0u32; BUF].into_boxed_slice(), reading: false, again: Instant::now(), dead: false };
            v.read();
            (!v.dead).then_some(v)
        }
    }

    /// Asks for the changes from now on; they are waited for on `io.hEvent`.
    fn read(&mut self) {
        // attributes too: a cloud file downloaded or freed up changes nothing else
        let what = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_ATTRIBUTES;
        self.reading = unsafe { ReadDirectoryChangesW(self.dir, self.buf.as_mut_ptr() as *mut _, (BUF * 4) as u32, true, what, None, Some(&mut *self.io), None) }.is_ok();
        self.dead = !self.reading;
    }

    /// The changes a finished read brought, as `records`; None: more than the buffer holds (or the
    /// watch ended), so any folder of the volume may have changed.
    fn changes(&mut self) -> Option<Vec<(String, bool)>> {
        (self.reading, self.again) = (false, Instant::now() + GATHER);
        let mut n = 0u32;
        match unsafe { GetOverlappedResult(self.dir, &*self.io, &mut n, false) } {
            // rounded up: the last name may end inside a 4-byte unit
            Ok(()) if n > 0 => Some(records(&self.buf[..(n as usize).div_ceil(4).min(BUF)])),
            Ok(()) => None,
            Err(e) => {
                self.dead = e.code() != ERROR_NOTIFY_ENUM_DIR.to_hresult();
                None
            }
        }
    }
}

impl Drop for Vol {
    fn drop(&mut self) {
        unsafe {
            if self.reading {
                // called off and waited for: until the read has ended Windows writes to `io` and `buf`
                let _ = CancelIoEx(self.dir, Some(&*self.io));
                let _ = GetOverlappedResult(self.dir, &*self.io, &mut 0, true);
            }
            let _ = CloseHandle(self.dir);
            let _ = CloseHandle(self.io.hEvent);
        }
    }
}

/// The changes in a buffer filled by ReadDirectoryChangesW: each one's path under the watched
/// root, lower-case, and whether the thing was only modified (not added, removed or renamed).
fn records(buf: &[u32]) -> Vec<(String, bool)> {
    let mut out = vec![];
    let mut at = 0;
    // a record: the bytes to the next one (0 in the last), what happened, the name's bytes, the name in UTF-16
    while let Some(&[next, action, len]) = buf.get(at..at + 3) {
        let units = len as usize / 2;
        let Some(name) = buf.get(at + 3..at + 3 + units.div_ceil(2)) else { break };
        // the 4-byte units read as twice as many 2-byte ones
        let name = unsafe { std::slice::from_raw_parts(name.as_ptr() as *const u16, units) };
        out.push((String::from_utf16_lossy(name).to_lowercase(), action == FILE_ACTION_MODIFIED.0));
        if next == 0 {
            break;
        }
        at += next as usize / 4;
    }
    out
}

fn run(rx: Receiver<Order>, wake: HANDLE, slow: Sender<Vec<(PathBuf, u64)>>, told: Told) {
    let mut dirs: Vec<Dir> = vec![];
    let mut vols: Vec<Vol> = vec![];
    'run: loop {
        loop {
            match rx.try_recv() {
                Ok(Order::Show(want)) => {
                    let mut rest = vec![];
                    for (p, shown) in keep(&mut dirs, want) {
                        match Dir::watched(&p, shown, &mut vols) {
                            Some(d) => dirs.push(d),
                            None => rest.push((p, shown)),
                        }
                    }
                    vols.retain(|v| dirs.iter().any(|d| d.root == v.root));
                    let _ = slow.send(rest);
                }
                Ok(Order::Quit) | Err(TryRecvError::Disconnected) => break 'run,
                Err(TryRecvError::Empty) => break,
            }
        }
        let mut next: Option<Instant> = None;
        for v in vols.iter_mut().filter(|v| !v.reading) {
            if v.again <= Instant::now() {
                v.read();
            } else {
                next = Some(next.map_or(v.again, |n| n.min(v.again)));
            }
        }
        // a watch that ended (should not happen on a fixed disk): its folders are compared from now on
        vols.retain(|v| {
            for d in dirs.iter_mut().filter(|d| v.dead && d.root == v.root) {
                d.due = Some(Instant::now());
            }
            !v.dead
        });
        for d in &mut dirs {
            next = [next, d.tend(&told)].into_iter().flatten().min();
        }
        let reading: Vec<usize> = (0..vols.len()).filter(|&i| vols[i].reading).collect();
        let handles: Vec<HANDLE> = std::iter::once(wake).chain(reading.iter().map(|&i| vols[i].io.hEvent)).collect();
        // rounded up: a wait that ends a moment early would come round again for nothing
        let ms = next.map_or(INFINITE, |n| n.saturating_duration_since(Instant::now()).as_millis() as u32 + 1);
        let r = unsafe { WaitForMultipleObjects(&handles, false, ms) };
        if r == WAIT_FAILED {
            // should not happen: without its event the thread cannot be woken and ends; a volume's
            // event gone bad, the folders are compared
            if reading.is_empty() {
                break;
            }
            vols.iter_mut().for_each(|v| v.dead = true);
        } else if let Some(&i) = reading.get((r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize).wrapping_sub(1)) {
            let changes = vols[i].changes();
            for d in dirs.iter_mut().filter(|d| d.root == vols[i].root) {
                d.check |= changes.as_ref().is_none_or(|c| c.iter().any(|(path, modified)| rx_core::concerns(&d.under, path, *modified)));
            }
        }
    }
    drop(vols);
    unsafe {
        let _ = CloseHandle(wake);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn changes_are_read_from_the_buffer_as_lower_case_paths() {
        let record = |next: u32, action: u32, name: &str| {
            let name: Vec<u16> = name.encode_utf16().collect();
            let words = name.chunks(2).map(|c| c[0] as u32 | (*c.get(1).unwrap_or(&0) as u32) << 16);
            [next, action, name.len() as u32 * 2].into_iter().chain(words).collect::<Vec<u32>>()
        };
        // "A\Ż.TXT" modified (7 units: 12 + 14 bytes, so the next record starts 28 on), then "c" added
        let buf = [record(28, 3, "A\\Ż.TXT"), record(0, 1, "c")].concat();
        assert_eq!(records(&buf), [("a\\ż.txt".to_string(), true), ("c".to_string(), false)]);
        // a record cut short is left out, not read past the end
        assert!(records(&buf[..5]).is_empty());
    }

    #[test]
    fn a_change_is_told_once_a_second_at_most_and_no_handle_holds_the_folders_above() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("rx-watch-{}", std::process::id()));
        let inner = dir.join("a").join("b");
        std::fs::create_dir_all(&inner).unwrap();
        let file = inner.join("plik.txt");
        std::fs::write(&file, "x").unwrap();
        let told = Arc::new(Mutex::new(vec![]));
        let t2 = told.clone();
        let w = Watcher::new(move |_, gone| t2.lock().unwrap().push(gone));
        let count = || told.lock().unwrap().len();
        let last = || told.lock().unwrap().last().copied();
        let wait = |ms| std::thread::sleep(Duration::from_millis(ms));
        // shown with the list as it is: nothing to tell
        w.show(vec![(inner.clone(), print(&inner))]);
        wait(1200);
        assert_eq!(count(), 0);
        // a file written in place (no name changes): told at once, then once a second
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(2500) {
            std::fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(b"dalej").unwrap();
            wait(50);
        }
        let n = count();
        assert!((2..=4).contains(&n), "told {n} times in 2.5 s");
        wait(1500);
        // off screen and back with the list it had: told only when the folder changed meanwhile
        let (old, n) = (print(&inner), count());
        w.show(vec![]);
        w.show(vec![(inner.clone(), old)]);
        wait(300);
        assert_eq!(count(), n);
        w.show(vec![]);
        std::fs::write(inner.join("nowy.txt"), "x").unwrap();
        w.show(vec![(inner.clone(), old)]);
        wait(300);
        assert_eq!(count(), n + 1);
        // still shown, but the app holds another list than the one last compared (an archive's): told
        w.show(vec![(inner.clone(), 0)]);
        wait(1500);
        assert_eq!((count(), last()), (n + 2, Some(false)));
        // a change deeper down or beside the folder is not its own
        let deep = inner.join("c").join("gleboko.txt");
        std::fs::create_dir(inner.join("c")).unwrap();
        std::fs::write(&deep, "x").unwrap();
        wait(2500);
        let n = count();
        std::fs::OpenOptions::new().append(true).open(&deep).unwrap().write_all(b"dalej").unwrap();
        std::fs::write(dir.join("a").join("obok.txt"), "x").unwrap();
        wait(1500);
        assert_eq!(count(), n);
        // no handle is held on the folder, so one above it can be renamed while it is shown: told
        // as gone, and as back once the name returns
        std::fs::rename(dir.join("a"), dir.join("a2")).unwrap();
        wait(1500);
        assert_eq!(last(), Some(true));
        std::fs::rename(dir.join("a2"), dir.join("a")).unwrap();
        wait(1500);
        assert_eq!(last(), Some(false));
        // deleted and made again at once: the new, empty folder is told, not an error that stays
        std::fs::remove_dir_all(&inner).unwrap();
        std::fs::create_dir(&inner).unwrap();
        wait(2500);
        assert_eq!(last(), Some(false));
        std::fs::remove_dir_all(&inner).unwrap();
        wait(1500);
        assert_eq!(last(), Some(true));
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
