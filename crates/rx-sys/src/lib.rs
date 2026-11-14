//! Small wrappers over the Windows shell: type names, display names, drives,
//! free space, Recycle Bin / copy operations, thumbnails and PDF pages.
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

pub mod bin;
pub mod caption;
pub mod clip;
pub mod menu;
pub mod note;
pub mod watch;

use windows::core::{HSTRING, PCWSTR, w};
use windows::Win32::Foundation::{HWND, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOW, SW_SHOWNORMAL};

fn from_wide(s: &[u16]) -> String {
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end])
}

/// Background threads that use the shell or WinRT call this once.
pub fn init_com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// Windows' own type name for a file name ("Dokument tekstowy", "Folder plików"...).
pub fn type_name(name: &str, dir: bool) -> String {
    let mut info = SHFILEINFOW::default();
    let attr = if dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
    unsafe {
        SHGetFileInfoW(
            &HSTRING::from(name),
            attr,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES,
        );
    }
    from_wide(&info.szTypeName)
}

/// The localised name Explorer shows ("Dokumenty" for Documents, "Dysk lokalny (C:)").
pub fn display_name(path: &Path) -> Option<String> {
    let mut info = SHFILEINFOW::default();
    let ok = unsafe {
        SHGetFileInfoW(
            &HSTRING::from(path),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_DISPLAYNAME,
        )
    };
    let s = from_wide(&info.szDisplayName);
    (ok != 0 && !s.is_empty()).then_some(s)
}

/// Set by screenshot runs with `--no-network`: network drives are left out (pictures for the README
/// must not show the machine's own shares).
pub static HIDE_NETWORK_DRIVES: AtomicBool = AtomicBool::new(false);

/// One bit per drive letter in use: a single call without disk access, to see a drive come or go.
pub fn drive_mask() -> u32 {
    let mask = unsafe { GetLogicalDrives() };
    if !HIDE_NETWORK_DRIVES.load(Relaxed) {
        return mask;
    }
    drive_letters(mask).iter().filter(|d| d.1).fold(mask, |m, d| m & !rx_core::drive_bit(&d.0.to_string_lossy()))
}

/// Drive roots of the letters in `mask` (bit 0: A, as `drive_mask` gives them) and whether each
/// is a network drive; no drive is asked anything.
pub fn drive_letters(mask: u32) -> Vec<(PathBuf, bool)> {
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| {
            let root = format!("{}:\\", (b'A' + i) as char);
            let remote = unsafe { GetDriveTypeW(&HSTRING::from(root.as_str())) } == 4;
            (PathBuf::from(root), remote)
        })
        .collect()
}

/// The same with their shell names, which a network drive that does not answer may hold up for seconds.
pub fn drives_of(mask: u32) -> Vec<(PathBuf, String, bool)> {
    drive_letters(mask)
        .into_iter()
        .map(|(root, remote)| {
            let name = display_name(&root).unwrap_or_else(|| root.display().to_string());
            (root, name, remote)
        })
        .collect()
}

/// Windows drive type: 2 removable, 3 fixed, 4 network, 5 optical.
pub fn drive_type(root: &Path) -> u32 {
    unsafe { GetDriveTypeW(&HSTRING::from(root)) }
}

/// File system name of a volume ("NTFS", "FAT32"...), empty when unknown.
pub fn fs_name(root: &Path) -> String {
    let mut buf = [0u16; 64];
    let ok = unsafe { GetVolumeInformationW(&HSTRING::from(root), None, None, None, None, Some(&mut buf)) };
    if ok.is_ok() { from_wide(&buf) } else { String::new() }
}

/// (free, total) bytes of the volume holding `path`.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(&HSTRING::from(path), Some(&mut free), Some(&mut total), None).ok()? };
    Some((free, total))
}

fn multi_sz(paths: &[PathBuf]) -> Vec<u16> {
    let mut v: Vec<u16> = paths.iter().flat_map(|p| p.as_os_str().encode_wide().chain([0])).collect();
    v.push(0);
    v
}

/// Copy / move / delete through the Windows shell, so the user gets the usual
/// progress, conflict and Recycle Bin behaviour. `to`: one folder, or with FOF_MULTIDESTFILES a
/// path for each item. Returns true when nothing was aborted, and the (old, new) paths of items
/// the shell named anew on a collision.
fn shell_op(func: u32, from: &[PathBuf], to: &[PathBuf], flags: u32) -> (bool, Vec<(PathBuf, PathBuf)>) {
    let src = multi_sz(from);
    let dst = (!to.is_empty()).then(|| multi_sz(to));
    let mut op = SHFILEOPSTRUCTW {
        wFunc: func,
        pFrom: PCWSTR(src.as_ptr()),
        pTo: dst.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
        fFlags: (FOF_ALLOWUNDO.0 | FOF_WANTMAPPINGHANDLE.0 | flags) as u16,
        ..Default::default()
    };
    let ok = unsafe { SHFileOperationW(&mut op) == 0 && !op.fAnyOperationsAborted.as_bool() };
    let mut renamed = vec![];
    if !op.hNameMappings.is_null() {
        // the handle is the shell's HANDLETOMAPPINGS: a count and an array of SHNAMEMAPPINGW
        #[repr(C)]
        struct Mappings {
            n: u32,
            list: *const SHNAMEMAPPINGW,
        }
        unsafe {
            let m = &*(op.hNameMappings as *const Mappings);
            let path = |s: windows::core::PWSTR, n: i32| PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(s.0, n.max(0) as usize)));
            for i in 0..m.n as usize {
                let x = &*m.list.add(i);
                renamed.push((path(x.pszOldPath, x.cchOldPath), path(x.pszNewPath, x.cchNewPath)));
            }
            SHFreeNameMappings(Some(windows::Win32::Foundation::HANDLE(op.hNameMappings)));
        }
    }
    (ok, renamed)
}

pub fn recycle(paths: &[PathBuf]) -> bool {
    // to the Recycle Bin without a question, as in Windows 11; but a file that cannot go there
    // (network drive, too big) is deleted for good only after the shell's warning
    shell_op(FO_DELETE, paths, &[], FOF_NOCONFIRMATION.0 | FOF_WANTNUKEWARNING.0).0
}

/// Whether deleting on the drive of `path` goes to the Recycle Bin: only a fixed disk has one
/// (a removable or network drive deletes for good).
pub fn has_bin(path: &Path) -> bool {
    path.ancestors().last().is_some_and(|root| drive_type(root) == 3)
}

/// Copies (with `cut` moves) `paths` into `dir` through the shell; true when nothing was aborted,
/// with what Ctrl+Z may take back and how many items it must leave out (rx_core::undo::transferred).
pub fn copy_into(paths: &[PathBuf], dir: &Path, cut: bool) -> (bool, rx_core::undo::Undo, usize) {
    copy_into_with(paths, dir, cut, &mut |_, _| {})
}

/// `copy_into` that also reports (done, total) bytes to `progress` as it goes, on the calling
/// thread, from the shell's IFileOperationProgressSink::UpdateProgress (as often as the shell
/// reports: the caller thins them out). (0, 0) while the sources' size is still being counted;
/// `done == total` at the end of a finished operation. No report when the shell's old
/// SHFileOperationW had to do it.
pub fn copy_into_with(paths: &[PathBuf], dir: &Path, cut: bool, progress: &mut dyn FnMut(u64, u64)) -> (bool, rx_core::undo::Undo, usize) {
    let same = paths.iter().any(|p| p.parent() == Some(dir));
    let flags = if same && !cut { FOF_RENAMEONCOLLISION.0 } else { 0 };
    let before: Vec<bool> = paths.iter().map(|p| p.file_name().is_some_and(|n| dir.join(n).symlink_metadata().is_ok())).collect();
    let (ok, renamed) = file_op(paths, dir, cut, flags, progress)
        .unwrap_or_else(|| shell_op(if cut { FO_MOVE } else { FO_COPY }, paths, &[dir.to_path_buf()], flags));
    let (undo, left) = rx_core::undo::transferred(paths, dir, cut, &before, &renamed, rx_core::undo::stamp);
    // a move cancelled in Windows' window half way through a folder, with nothing whole to take
    // back: the folder is in both places, and with no record Ctrl+Z would undo the operation before
    let partial = cut && !ok && matches!(&undo, rx_core::undo::Undo::Moved(m) if m.is_empty())
        && paths.iter().zip(&before).any(|(p, &was)| !was && p.exists() && p.file_name().is_some_and(|n| dir.join(n).exists()));
    let undo = if partial { rx_core::undo::Undo::Blocked("undo.what.move_partial") } else { undo };
    (ok, undo, left)
}

/// Wklej and a drop: the app's own copy (rx_core::copy) wherever it can, else Windows' own
/// (`copy_into_with`: a name already taken, links, a folder needing the administrator's consent).
/// Like `copy_into_with`, with the note saying where the app's own copy stopped: Err for an error,
/// Ok for a cancel from the panel (`ctl`; Windows' copy never arms it, so the panel shows no
/// buttons: Windows' own window has them).
pub fn transfer(paths: &[PathBuf], dir: &Path, cut: bool, ctl: &rx_core::ops::Control, progress: &mut dyn FnMut(u64, u64)) -> (bool, rx_core::undo::Undo, usize, Option<Result<String, String>>) {
    use rx_core::copy::{self, Outcome};
    if let Some(plan) = copy::plan(paths, dir, cut, &WinFs) {
        // a disk with a head to move reads and writes one file at a time; SSDs and shares take four
        let workers = if seeks(dir) || paths.first().is_some_and(|p| seeks(p)) { 1 } else { 4 };
        let (r, made) = copy::run(&plan, workers, &WinFs, ctl, progress);
        if r != Outcome::Denied {
            // only what this run made: the plan took no name already there, and an item put in the
            // target by someone else meanwhile is not this operation's to take back
            let made: std::collections::HashSet<PathBuf> = made.into_iter().collect();
            let done: Vec<PathBuf> = paths.iter().filter(|p| p.file_name().is_some_and(|n| made.contains(&dir.join(n)))).cloned().collect();
            let (undo, left) = rx_core::undo::transferred(&done, dir, cut, &vec![false; done.len()], &[], rx_core::undo::stamp);
            // a folder moved in part (cancelled, or a source that could not go) is in both places:
            // no record could take it back, and without one Ctrl+Z would undo the operation before
            let partial = cut && r != Outcome::Done && done.iter().any(|p| p.exists());
            let undo = if partial { rx_core::undo::Undo::Blocked("undo.what.move_partial") } else { undo };
            let note = match r {
                Outcome::Failed(e) => Some(Err(e)),
                Outcome::Cancelled(n) => Some(Ok(n)),
                _ => None,
            };
            return (note.is_none(), undo, left, note);
        }
    }
    let (ok, undo, left) = copy_into_with(paths, dir, cut, progress);
    (ok, undo, left, None)
}

/// rx_core::copy on Windows. CopyFileExW keeps a file's attributes, times and alternate streams.
/// Through Windows' file cache: past it (COPY_FILE_NO_BUFFERING, robocopy /J) measured slower on
/// the laptop's SSD for 500 MB and for 3 GB alike.
struct WinFs;

impl rx_core::copy::Fs for WinFs {
    fn copy_file(&self, from: &Path, to: &Path, progress: &mut dyn FnMut(u64) -> bool) -> std::io::Result<()> {
        unsafe extern "system" fn told(_: i64, done: i64, _: i64, _: i64, _: u32, _: LPPROGRESS_ROUTINE_CALLBACK_REASON, _: windows::Win32::Foundation::HANDLE, _: windows::Win32::Foundation::HANDLE, data: *const std::ffi::c_void) -> COPYPROGRESSROUTINE_PROGRESS {
            let f = unsafe { &mut *(data as *mut &mut dyn FnMut(u64) -> bool) };
            // PROGRESS_CANCEL: Windows stops and deletes the part of `to` already written
            if f(done as u64) { PROGRESS_CONTINUE } else { PROGRESS_CANCEL }
        }
        let mut f: &mut dyn FnMut(u64) -> bool = progress;
        unsafe { CopyFileExW(&long(from), &long(to), Some(told), Some(&mut f as *mut _ as *const _), None, COPY_FILE_FAIL_IF_EXISTS) }.map_err(io_error)
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        // no MOVEFILE_REPLACE_EXISTING: a file put there meanwhile is an error, not replaced
        unsafe { MoveFileExW(&long(from), &long(to), MOVE_FILE_FLAGS(0)) }.map_err(io_error)
    }

    fn same_volume(&self, from: &Path, to: &Path) -> bool {
        volume(from).zip(volume(to)).is_some_and(|(a, b)| a.eq_ignore_ascii_case(&b))
    }

    fn dir_made(&self, from: &Path, to: &Path) {
        // what Explorer carries over to a copied folder: hidden (.git), system, read-only, archive
        let keep = FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0 | FILE_ATTRIBUTE_READONLY.0 | FILE_ATTRIBUTE_ARCHIVE.0 | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED.0;
        unsafe {
            let a = GetFileAttributesW(&long(from));
            if a != INVALID_FILE_ATTRIBUTES && a & keep != 0 {
                let _ = SetFileAttributesW(&long(to), FILE_FLAGS_AND_ATTRIBUTES(a & keep));
            }
        }
    }
}

/// `p` for a Win32 call: past 259 characters with the \\?\ prefix, which works without
/// LongPathsEnabled (std does the same for its own calls). The app's paths are absolute.
fn long(p: &Path) -> HSTRING {
    let w: Vec<u16> = p.as_os_str().encode_wide().collect();
    let u = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let w = if w.len() < 260 || w.starts_with(&u(r"\\?\")) {
        w
    } else if w.starts_with(&u(r"\\")) {
        [u(r"\\?\UNC\"), w[2..].to_vec()].concat()
    } else {
        [u(r"\\?\"), w].concat()
    };
    HSTRING::from_wide(&w)
}

/// A Win32 failure as std reads it (its kind, Windows' own words): `windows` hands std the
/// HRESULT, which std does not know as "access denied" and the like.
fn io_error(e: windows::core::Error) -> std::io::Error {
    let c = e.code().0 as u32;
    std::io::Error::from_raw_os_error(if c >> 16 == 0x8007 { (c & 0xFFFF) as i32 } else { c as i32 })
}

/// The root of the volume `p` is on ("C:\", "\\serwer\udział\", a folder a disk is mounted on).
fn volume(p: &Path) -> Option<String> {
    let mut buf = [0u16; 1024];
    unsafe { GetVolumePathNameW(&HSTRING::from(p), &mut buf) }.ok()?;
    Some(from_wide(&buf))
}

/// Whether `p` is on a disk with a head to move (Windows says "seek penalty"); a share or a disk
/// that does not answer counts as not.
fn seeks(p: &Path) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::*;
    let Some(root) = volume(p) else { return false };
    unsafe {
        let mut name = [0u16; 64];
        if GetVolumeNameForVolumeMountPointW(&HSTRING::from(root), &mut name).is_err() {
            return false;
        }
        // the volume itself ("\\?\Volume{…}" with no backslash at the end) answers for its disk; no rights are asked
        let name = from_wide(&name);
        let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
        let Ok(vol) = CreateFileW(&HSTRING::from(name.trim_end_matches('\\')), 0, share, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) else { return false };
        let q = STORAGE_PROPERTY_QUERY { PropertyId: StorageDeviceSeekPenaltyProperty, QueryType: PropertyStandardQuery, ..Default::default() };
        let (mut d, mut n) = (DEVICE_SEEK_PENALTY_DESCRIPTOR::default(), 0u32);
        let asked = DeviceIoControl(vol, IOCTL_STORAGE_QUERY_PROPERTY, Some(&q as *const _ as *const _), size_of_val(&q) as u32, Some(&mut d as *mut _ as *mut _), size_of_val(&d) as u32, Some(&mut n), None);
        let _ = CloseHandle(vol);
        asked.is_ok() && d.IncursSeekPenalty
    }
}

/// Hears an IFileOperation: its progress, and the top-level items that got a new name.
#[windows::core::implement(IFileOperationProgressSink)]
struct Sink {
    // the caller's items by their folded path: one lookup per item the shell reports, not a scan
    srcs: std::collections::HashMap<String, PathBuf>,
    renamed: std::cell::RefCell<Vec<(PathBuf, PathBuf)>>,
    // (done, total) to the calling thread; taken when the operation ends, so a reference the shell
    // keeps to the sink cannot hold the caller up
    progress: std::cell::RefCell<Option<std::sync::mpsc::Sender<(u64, u64)>>>,
}

fn item_path(item: windows::core::Ref<IShellItem>) -> Option<PathBuf> {
    let p = unsafe { item.as_ref()?.GetDisplayName(SIGDN_FILESYSPATH).ok()? };
    let s = unsafe { p.to_string() };
    unsafe { CoTaskMemFree(Some(p.0 as _)) };
    s.ok().map(PathBuf::from)
}

impl Sink {
    fn post(&self, item: windows::core::Ref<IShellItem>, hr: windows::core::HRESULT, new: windows::core::Ref<IShellItem>) {
        if hr.is_err() {
            return;
        }
        // items inside a copied folder come here too; only the ones the caller named count
        let Some(old) = item_path(item) else { return };
        let Some(src) = self.srcs.get(&fold(old.as_os_str())) else { return };
        let Some(new) = item_path(new) else { return };
        if !new.file_name().zip(src.file_name()).is_some_and(|(a, b)| same(a, b)) {
            self.renamed.borrow_mut().push((src.clone(), new));
        }
    }
}

// shortcut: Unicode lower case, not NTFS's own case table; differs only for rare letters
fn fold(a: &std::ffi::OsStr) -> String {
    a.to_string_lossy().to_lowercase()
}

fn same(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    a.eq_ignore_ascii_case(b) || fold(a) == fold(b)
}

#[allow(non_snake_case)]
impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _: windows::core::HRESULT) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: windows::core::Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(&self, _: u32, _: windows::core::Ref<IShellItem>, _: &PCWSTR, _: windows::core::HRESULT, _: windows::core::Ref<IShellItem>) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreMoveItem(&self, _: u32, _: windows::core::Ref<IShellItem>, _: windows::core::Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(&self, _: u32, item: windows::core::Ref<IShellItem>, _: windows::core::Ref<IShellItem>, _: &PCWSTR, hr: windows::core::HRESULT, new: windows::core::Ref<IShellItem>) -> windows::core::Result<()> {
        self.post(item, hr, new);
        Ok(())
    }
    fn PreCopyItem(&self, _: u32, _: windows::core::Ref<IShellItem>, _: windows::core::Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(&self, _: u32, item: windows::core::Ref<IShellItem>, _: windows::core::Ref<IShellItem>, _: &PCWSTR, hr: windows::core::HRESULT, new: windows::core::Ref<IShellItem>) -> windows::core::Result<()> {
        self.post(item, hr, new);
        Ok(())
    }
    fn PreDeleteItem(&self, _: u32, _: windows::core::Ref<IShellItem>) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostDeleteItem(&self, _: u32, _: windows::core::Ref<IShellItem>, _: windows::core::HRESULT, _: windows::core::Ref<IShellItem>) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: windows::core::Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(&self, _: u32, _: windows::core::Ref<IShellItem>, _: &PCWSTR, _: &PCWSTR, _: u32, _: windows::core::HRESULT, _: windows::core::Ref<IShellItem>) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, total: u32, done: u32) -> windows::core::Result<()> {
        if let Some(tx) = &*self.progress.borrow() {
            let _ = tx.send((done as u64, total as u64));
        }
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Copy / move through IFileOperation, with the same window, conflicts and undo as `shell_op`, and
/// progress to `progress`. Runs on a thread of its own: IFileOperation needs a single-threaded COM
/// apartment and the app's background threads are multithreaded. None when it could not be set up
/// (nothing done yet), for `shell_op` to do it.
fn file_op(paths: &[PathBuf], dir: &Path, cut: bool, flags: u32, progress: &mut dyn FnMut(u64, u64)) -> Option<(bool, Vec<(PathBuf, PathBuf)>)> {
    let (tx, rx) = std::sync::mpsc::channel();
    // the shell counts in its own units (about 32 KiB each, plus some per item), turned into bytes by
    // the sources' size: counted beside the operation, given up when it ends, unknown until then
    let (size, stop) = (AtomicU64::new(UNKNOWN), AtomicBool::new(false));
    std::thread::scope(|s| {
        let job = s.spawn(move || unsafe {
            // first, so that any way out drops the sender and ends the caller's loop below
            let sink = windows::core::ComObject::new(Sink { srcs: paths.iter().map(|p| (fold(p.as_os_str()), p.clone())).collect(), renamed: Default::default(), progress: Some(tx).into() });
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).ok().ok()?;
            let r = (|| {
                let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL).ok()?;
                // Windows' own progress window (no FOF_SILENT): this copy runs where the app's own
                // could not (a name taken, …), and the window holds its conflict questions and the
                // pause and cancel the Operacje panel cannot give it. Silent in tests: a window
                // there would fail the test that this process shows none
                let silent = if cfg!(test) { FOF_SILENT.0 } else { 0 };
                op.SetOperationFlags(FILEOPERATION_FLAGS(FOF_ALLOWUNDO.0 | silent | flags)).ok()?;
                let to: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(dir), None).ok()?;
                for p in paths {
                    let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(p.as_path()), None).ok()?;
                    if cut { op.MoveItem(&item, &to, PCWSTR::null(), None) } else { op.CopyItem(&item, &to, PCWSTR::null(), None) }.ok()?;
                }
                let cookie = op.Advise(&sink.to_interface::<IFileOperationProgressSink>()).ok();
                // cancelled or failed: an error here; skipped items: aborted
                let ok = op.PerformOperations().is_ok() && !op.GetAnyOperationsAborted().is_ok_and(|a| a.as_bool());
                if let Some(c) = cookie {
                    let _ = op.Unadvise(c);
                }
                Some((ok, sink.renamed.take()))
            })();
            sink.progress.take();
            drop(sink);
            CoUninitialize();
            r
        });
        s.spawn(|| {
            #[cfg(test)]
            tests::hold_count(&stop);
            let n = paths.iter().map(|p| p.symlink_metadata().map_or(0, |m| bytes(p, &m, &stop))).sum();
            if !stop.load(Relaxed) {
                size.store(n, Relaxed);
            }
        });
        // (0, 0) while the size is unknown; a move may outrun the count, so done never passes it
        let in_bytes = |done: u64, total: u64| match size.load(Relaxed) {
            UNKNOWN => (0, 0),
            n if total == 0 => (0, n),
            n => ((done as u128 * n as u128 / total as u128).min(n as u128) as u64, n),
        };
        let mut last = (0, 0);
        for (done, total) in rx {
            last = in_bytes(done, total);
            progress(last.0, last.1);
        }
        stop.store(true, Relaxed);
        let r = job.join().ok().flatten();
        // the shell's last report may stop short of the end, or come before the size was known
        let n = size.load(Relaxed);
        if r.as_ref().is_some_and(|r| r.0) && n != UNKNOWN && last != (n, n) {
            progress(n, n);
        }
        r
    })
}

const UNKNOWN: u64 = u64::MAX;

/// Bytes in a file, or in a folder and all below it (links not followed); stops early on `stop`.
fn bytes(p: &Path, m: &std::fs::Metadata, stop: &AtomicBool) -> u64 {
    if !m.is_dir() {
        return m.len();
    }
    let Ok(d) = std::fs::read_dir(p) else { return 0 };
    // DirEntry::metadata comes with the listing on Windows: no call per file
    d.flatten().take_while(|_| !stop.load(Relaxed)).map(|e| e.metadata().map_or(0, |m| bytes(&e.path(), &m, stop))).sum()
}

/// Moves each (now, was) item back to `was`, never onto anything: an item whose place is taken
/// or that is no longer where it was moved is left, with the reason.
pub fn move_back(items: &[(PathBuf, PathBuf)]) -> Vec<(PathBuf, &'static str)> {
    let mut failed = vec![];
    let (mut from, mut to) = (vec![], vec![]);
    for (now, was) in items {
        if was.symlink_metadata().is_ok() {
            failed.push((was.clone(), rx_core::t!("rx_sys.place_taken")));
        } else if now.symlink_metadata().is_err() {
            failed.push((was.clone(), rx_core::t!("rx_sys.moved_away")));
        } else {
            from.push(now.clone());
            to.push(was.clone());
        }
    }
    if !from.is_empty() {
        // no FOF_NOCONFIRMATION: should a place be taken meanwhile, the shell asks rather than replaces
        shell_op(FO_MOVE, &from, &to, FOF_MULTIDESTFILES.0);
        failed.extend(to.into_iter().filter(|w| w.symlink_metadata().is_err()).map(|w| (w, rx_core::t!("rx_sys.move_failed"))));
    }
    failed
}

/// Renames `from` to `to` only when nothing is there (a change of letter case of the same item works).
pub fn rename_new(from: &Path, to: &Path) -> Result<(), String> {
    unsafe { MoveFileExW(&HSTRING::from(from), &HSTRING::from(to), MOVE_FILE_FLAGS(0)) }.map_err(|e| e.message())
}

/// Opens with the file's program. Err is Windows' reason, for the caller to show (an online-only
/// OneDrive file while OneDrive is not running used to fail without a word).
pub fn open(path: &Path) -> Result<(), String> {
    let file = HSTRING::from(path);
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("open"),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut sei) }.map_err(|e| e.message())
}

/// Windows' "Otwórz za pomocą" window for a file: pick a program, and if wanted keep it for the
/// file's type. It stays until closed, so it is called off the UI thread. Err as for `open`; a
/// window closed without a choice is no error.
pub fn open_with(path: &Path, owner: isize) -> Result<(), String> {
    let file = HSTRING::from(path);
    let info = OPENASINFO { pcszFile: PCWSTR(file.as_ptr()), pcszClass: PCWSTR::null(), oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC };
    match unsafe { SHOpenWithDialog(Some(HWND(owner as _)), &info) } {
        Err(e) if e.code() != windows::core::HRESULT::from_win32(1223) => Err(e.message()), // 1223: ERROR_CANCELLED
        _ => Ok(()),
    }
}

/// Explorer's "Zawsze zachowuj na tym urządzeniu" (`keep`) or "Zwolnij miejsce" for a file, or a
/// folder with all in it, of a Cloud Files provider (OneDrive, Dropbox, iCloud): the pin
/// attributes of `attrib +P -U` and `-P +U`, which the provider follows. A kept file is also
/// fetched here, so it does not wait for the provider. Called off the UI thread: a download takes its time.
// shortcut: freeing the space is left to the provider; CfDehydratePlaceholder would do it at once,
// but it never returned with OneDrive 26.204 on the test laptop (2026-11-09), from any session
pub fn cloud_keep(path: &Path, keep: bool) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    let (on, off) = if keep { (0x80000, 0x100000) } else { (0x100000, 0x80000) };
    unsafe { SetFileAttributesW(&HSTRING::from(path), FILE_FLAGS_AND_ATTRIBUTES(meta.file_attributes() & !off | on))? };
    if meta.is_dir() {
        for e in std::fs::read_dir(path)? {
            cloud_keep(&e?.path(), keep)?;
        }
    } else if keep {
        cloud_fetch(path)?;
    }
    Ok(())
}

/// Downloads the content of a Cloud Files provider's file to this device, as opening it does: it
/// is not pinned, so the provider may free the space again. A file that is no such placeholder
/// (offline on a share's archive, or another kind of cloud drive) has nothing to fetch here: it
/// comes when it is read. Called off the UI thread.
pub fn cloud_fetch(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Storage::CloudFilters::{CfHydratePlaceholder, CF_HYDRATE_FLAG_NONE};
    // recall on open, recall on data access: what a placeholder not on this device has
    if std::fs::symlink_metadata(path)?.file_attributes() & (0x40000 | 0x400000) == 0 {
        return Ok(());
    }
    let f = std::fs::File::open(path)?;
    unsafe { CfHydratePlaceholder(windows::Win32::Foundation::HANDLE(f.as_raw_handle()), 0, -1, CF_HYDRATE_FLAG_NONE, None)? };
    Ok(())
}

/// Has OneDrive put a sharing link to `path` on the clipboard, as "Kopiuj link" in Explorer's
/// menu does; the link comes a moment after this returns. Called off the UI thread.
// undocumented: OneDrive.exe's own COM server (FileSyncClient), its IFileSyncClient9 and the
// method in vtable slot 20, InvokeMultipleItemCommandWithHandle(paths, command, selection,
// isEditLink, owner), command 12 = ItemCommandCopyLink: all from the type library inside
// OneDrive.exe 26.204, tried on the test laptop 2026-11-09. Without an owner window it does nothing.
pub fn onedrive_copy_link(path: &Path, owner: isize) -> Result<(), String> {
    use windows::core::{IUnknown, Interface, BSTR, GUID, HRESULT};
    use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement};
    use windows::Win32::System::Variant::VT_BSTR;
    const FILE_SYNC_CLIENT: GUID = GUID::from_u128(0x7b37e4e2_c62f_4914_9620_8fb5062718cc);
    const IFILE_SYNC_CLIENT9: GUID = GUID::from_u128(0xee15bbbb_9e60_4c52_abcb_7540ff3df6b3);
    type Invoke = unsafe extern "system" fn(*mut std::ffi::c_void, *const windows::Win32::System::Com::SAFEARRAY, i32, i32, i32, i64) -> HRESULT;
    let fail = |e: windows::core::Error| rx_core::t!("rx_sys.onedrive_link", e = e);
    unsafe {
        let client: IUnknown = CoCreateInstance(&FILE_SYNC_CLIENT, None, CLSCTX_LOCAL_SERVER).map_err(fail)?;
        let mut raw = std::ptr::null_mut();
        client.query(&IFILE_SYNC_CLIENT9, &mut raw).ok().map_err(fail)?;
        let nine = IUnknown::from_raw(raw);
        let paths = SafeArrayCreateVector(VT_BSTR, 0, 1);
        if paths.is_null() {
            return Err(rx_core::t!("rx_sys.no_memory").into());
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        let name = BSTR::from_wide(&wide);
        // the array keeps its own copy of the string
        let r = SafeArrayPutElement(paths, &0, name.as_ptr() as *const _).map_err(fail);
        let r = r.and_then(|_| {
            let invoke: Invoke = std::mem::transmute(*(*(nine.as_raw() as *const *const usize)).add(20));
            // selection: 1 a file, 2 a folder
            invoke(nine.as_raw(), paths, 12, if path.is_dir() { 2 } else { 1 }, 0, owner as i64).ok().map_err(fail)
        });
        let _ = SafeArrayDestroy(paths);
        r
    }
}

/// The Windows "Properties" dialog for a file or folder.
pub fn properties(path: &Path) {
    let file = HSTRING::from(path);
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST,
        lpVerb: w!("properties"),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOW.0,
        ..Default::default()
    };
    unsafe {
        let _ = ShellExecuteExW(&mut sei);
    }
}

/// Thumbnail (or icon, when there is none) from the shell, as RGBA pixels. `thumb_only`: no icon,
/// for a file that is only in the cloud: the shell's thumbnails do not read such a file (they come
/// from its provider or the cache), while a program's or a shortcut's icon is in its content.
pub fn shell_thumbnail(path: &Path, size: i32, thumb_only: bool) -> Option<(usize, usize, Vec<u8>)> {
    unsafe {
        let f: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
        let flags = if thumb_only { SIIGBF_THUMBNAILONLY } else { SIIGBF_RESIZETOFIT };
        let hbmp = f.GetImage(SIZE { cx: size, cy: size }, flags).ok()?;
        let obj = HGDIOBJ(hbmp.0);
        let mut bm = BITMAP::default();
        GetObjectW(obj, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _));
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        if w <= 0 || h <= 0 {
            let _ = DeleteObject(obj);
            return None;
        }
        let mut bi = BITMAPINFO::default();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let n = GetDIBits(dc, hbmp, 0, h as u32, Some(buf.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        let _ = DeleteObject(obj);
        if n == 0 {
            return None;
        }
        let no_alpha = buf.chunks(4).all(|p| p[3] == 0);
        for p in buf.chunks_mut(4) {
            p.swap(0, 2);
            if no_alpha {
                p[3] = 255;
            }
        }
        Some((w as usize, h as usize, buf))
    }
}

/// First page of a PDF rendered by Windows (PNG bytes) and the page count.
pub fn pdf_first_page(path: &Path, width: u32) -> windows::core::Result<(Vec<u8>, u32)> {
    use windows::core::Interface;
    use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
    use windows_future::IAsyncOperation;
    use windows::Storage::StorageFile;
    use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};

    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path))?.join()?;
    let doc = PdfDocument::LoadFromFileAsync(&file)?.join()?;
    let pages = doc.PageCount()?;
    let page = doc.GetPage(0)?;
    // a long page (a receipt, a web capture) stays within the GPU's texture limit (8192 on DX12)
    let s = page.Size()?;
    let width = width.min((8192.0 * s.Width / s.Height.max(1.0)) as u32).max(1);
    let stream = InMemoryRandomAccessStream::new()?;
    let opts = PdfPageRenderOptions::new()?;
    opts.SetDestinationWidth(width)?;
    page.RenderWithOptionsToStreamAsync(&stream, &opts)?.join()?;
    let n = stream.Size()? as u32;
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
    let load: IAsyncOperation<u32> = reader.LoadAsync(n)?.cast()?;
    load.join()?;
    let mut buf = vec![0u8; n as usize];
    reader.ReadBytes(&mut buf)?;
    Ok((buf, pages))
}

/// Ends the process at once, without the DLL teardown of a normal exit (no atexit work runs, so
/// everything worth keeping must already be written).
pub fn end_process(code: u32) -> ! {
    use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    unsafe {
        let _ = TerminateProcess(GetCurrentProcess(), code);
    }
    std::process::exit(code as i32)
}

/// Gives the process's idle pages back to Windows (they come back from the standby list on use).
pub fn trim_memory() {
    use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

/// Windows 11's efficiency mode (EcoQoS) for the whole process: lower clock and frugal cores, for
/// a window nobody sees. Off hands the choice back to Windows; older systems refuse, which is fine.
pub fn eco_mode(on: bool) {
    use windows::Win32::System::Threading::{
        GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
    };
    let mask = if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 };
    let state = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: mask, StateMask: mask };
    unsafe {
        let _ = SetProcessInformation(GetCurrentProcess(), ProcessPowerThrottling, &state as *const _ as *const _, size_of_val(&state) as u32);
    }
}

/// Whether the process has a window on screen: called with the main one hidden or minimized, it
/// tells of another, as the shell's Properties sheet, which lives in this process and which
/// efficiency mode must not slow down.
pub fn shows_a_window() -> bool {
    use windows::core::BOOL;
    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowLongW, GetWindowThreadProcessId, GWL_EXSTYLE, GWL_STYLE, WS_EX_TOOLWINDOW, WS_MINIMIZE, WS_VISIBLE};
    unsafe extern "system" fn each(h: HWND, found: LPARAM) -> BOOL {
        let found = &mut *(found.0 as *mut bool);
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        // shown and not minimized, by its own styles: IsWindowVisible is false for every window
        // of a session with no screen, where the tests run
        let shown = GetWindowLongW(h, GWL_STYLE) as u32 & (WS_VISIBLE | WS_MINIMIZE).0 == WS_VISIBLE.0;
        // no tool window: winit keeps an empty one "visible" for its events as long as the app runs
        let tool = GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0;
        *found = pid == GetCurrentProcessId() && shown && !tool;
        // false ends the enumeration
        BOOL::from(!*found)
    }
    let mut found = false;
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut found as *mut bool as isize));
    }
    found
}

// the count behind the process reference (SHCreateThreadRef keeps it), and its value at rest
static SHELL_REFS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static SHELL_REFS_IDLE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(i32::MAX);

/// Gives the shell this process's reference (SHSetInstanceExplorer): threads the shell starts here
/// for work picked in Windows' menu ("Wyślij do" to a drive, "Folder skompresowany", a Properties
/// sheet) hold it while they run, which `shell_working` then tells. Called once at start.
pub fn take_shell_work() {
    use std::sync::atomic::Ordering::SeqCst;
    let mut unk = None;
    unsafe {
        if SHCreateThreadRef(SHELL_REFS.as_ptr(), &mut unk).is_ok() {
            if let Some(unk) = unk {
                SHSetInstanceExplorer(&unk);
                SHELL_REFS_IDLE.store(SHELL_REFS.load(SeqCst), SeqCst);
                // kept for the whole run: at a count of 0 the object frees itself
                std::mem::forget(unk);
            }
        }
    }
}

/// Whether a shell thread in this process still holds the process reference (see `take_shell_work`).
pub fn shell_working() -> bool {
    use std::sync::atomic::Ordering::SeqCst;
    SHELL_REFS.load(SeqCst) > SHELL_REFS_IDLE.load(SeqCst)
}

/// Puts the calling thread in Windows's background mode for the rest of its life: lowest CPU, disk
/// and memory priority, for work nobody waits for.
pub fn background_thread() {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

/// The window's taskbar and title icons from the EXE's own icon (resource 1, see build.rs), set
/// at once: eframe sets them only once the window is first active, which a start minimized or in
/// the tray (after an update) may never be, and the taskbar then shows a blank icon.
pub fn set_window_icons(hwnd: isize) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, LoadImageW, SendMessageW, SetClassLongPtrW, GCLP_HICON, GCLP_HICONSM, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, SM_CXICON, SM_CXSMICON,
        WM_SETICON,
    };
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return };
        for (which, metric, class) in [(ICON_BIG, SM_CXICON, GCLP_HICON), (ICON_SMALL, SM_CXSMICON, GCLP_HICONSM)] {
            let size = GetSystemMetrics(metric);
            // no resource (a GNU build): eframe's own icon comes once the window is active
            if let Ok(icon) = LoadImageW(Some(module.into()), PCWSTR(1 as _), IMAGE_ICON, size, size, LR_DEFAULTCOLOR) {
                SendMessageW(HWND(hwnd as _), WM_SETICON, Some(WPARAM(which as usize)), Some(LPARAM(icon.0 as isize)));
                // the class's too, a fallback the taskbar can read without asking the window (winit
                // registers its class with none); whether that was what left the button blank is not proven
                SetClassLongPtrW(HWND(hwnd as _), class, icon.0 as isize);
            }
        }
    }
}

/// The same path through the share a network drive letter maps (`Z:\a` → `\\serwer\ala\a`);
/// None for a local drive.
pub fn drive_unc(path: &Path) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::NetworkManagement::WNet::WNetGetConnectionW;
    let s = path.to_string_lossy();
    let letter = s.get(..2).filter(|d| d.ends_with(':'))?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let r = unsafe { WNetGetConnectionW(&HSTRING::from(letter), Some(PWSTR(buf.as_mut_ptr())), &mut len) };
    // 1201: a remembered connection not connected at the moment; the name is still given
    (r.0 == 0 || r.0 == 1201).then(|| format!("{}{}", from_wide(&buf), &s[2..]))
}

/// A window not shown yet goes straight to minimized, without a frame on screen; `hide`: then
/// hidden as well (in the tray), which makes it minimized and invisible.
pub fn start_minimized(hwnd: isize, hide: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOWMINNOACTIVE};
    let h = HWND(hwnd as _);
    window_animations(hwnd, false);
    unsafe {
        let _ = ShowWindow(h, SW_SHOWMINNOACTIVE);
        if hide {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
    // a hidden window gets its animation back when it is shown (tray_and_close)
    if !hide {
        window_animations(hwnd, true);
    }
}

/// Drawn as usual but never seen (tests run on a machine in use); an ex-style would not stick, winit rewrites it.
pub fn cloak_window(hwnd: isize) {
    use windows::core::BOOL;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
    let on = BOOL::from(true);
    unsafe {
        let _ = DwmSetWindowAttribute(HWND(hwnd as _), DWMWA_CLOAK, &on as *const BOOL as *const _, std::mem::size_of::<BOOL>() as u32);
    }
}

/// Turns the window's minimize/restore animation off or on (off while it goes to the tray, where
/// it is minimized and hidden at once: the animation flashed).
pub fn window_animations(hwnd: isize, on: bool) {
    use windows::core::BOOL;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};
    let off = BOOL::from(!on);
    unsafe {
        let _ = DwmSetWindowAttribute(HWND(hwnd as _), DWMWA_TRANSITIONS_FORCEDISABLED, &off as *const BOOL as *const _, std::mem::size_of::<BOOL>() as u32);
    }
}

/// Programs Windows starts at sign-in, per user (no administrator rights needed).
const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");

/// The command Windows runs at sign-in under `name`, if any.
pub fn run_at_sign_in(name: &str) -> Option<String> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let mut buf = [0u16; 1024];
    let mut len = std::mem::size_of_val(&buf) as u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, &HSTRING::from(name), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len)) };
    r.is_ok().then(|| from_wide(&buf))
}

/// Cloud folders the user's Explorer pins to its navigation pane (OneDrive, Dropbox, iCloud…),
/// with the names it gives them: HKCU's Desktop\NameSpace entries that point at a folder.
pub fn cloud_folders() -> Vec<(PathBuf, String)> {
    use windows::Win32::System::Registry::*;
    // CLSID data from HKEY_CLASSES_ROOT: the user's and the machine's entries merged, as Explorer reads them
    let get = |key: &str, value: Option<&str>| -> Option<String> {
        let mut buf = [0u16; 1024];
        let mut len = std::mem::size_of_val(&buf) as u32;
        let value = value.map(HSTRING::from);
        let v = value.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
        // REG_EXPAND_SZ comes back with %variables% expanded
        let r = unsafe { RegGetValueW(HKEY_CLASSES_ROOT, &HSTRING::from(key), v, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len)) };
        r.is_ok().then(|| from_wide(&buf)).filter(|s| !s.is_empty())
    };
    // without the value an entry stays off Explorer's navigation pane
    let pinned = |key: &str| {
        let mut on = 0u32;
        let mut len = 4u32;
        let _ = unsafe { RegGetValueW(HKEY_CLASSES_ROOT, &HSTRING::from(key), w!("System.IsPinnedToNameSpaceTree"), RRF_RT_REG_DWORD, None, Some((&mut on as *mut u32).cast()), Some(&mut len)) };
        on != 0
    };
    let mut ids = vec![];
    let mut key = HKEY::default();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Desktop\NameSpace"), None, KEY_READ, &mut key) }.is_ok() {
        for i in 0.. {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            if unsafe { RegEnumKeyExW(key, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) }.is_err() {
                break;
            }
            ids.push(from_wide(&name));
        }
        let _ = unsafe { RegCloseKey(key) };
    }
    ids.into_iter()
        .filter_map(|id| {
            let clsid = format!(r"CLSID\{id}");
            let bag = format!(r"{clsid}\Instance\InitPropertyBag");
            // OneDrive names a known folder, the others a path
            let path = get(&bag, Some("TargetFolderPath")).map(PathBuf::from).or_else(|| known_folder(&get(&bag, Some("TargetKnownFolder"))?))?;
            (pinned(&clsid) && path.is_dir()).then(|| (path, get(&clsid, None).unwrap_or_else(|| id.clone())))
        })
        .collect()
}

/// The path of a known folder given as "{guid}".
fn known_folder(guid: &str) -> Option<PathBuf> {
    let g = windows::core::GUID::try_from(guid.trim_matches(['{', '}'])).ok()?;
    unsafe {
        let p = SHGetKnownFolderPath(&g, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s.map(PathBuf::from)
    }
}

/// Sets the command run at sign-in under `name` (`None`: removes it).
pub fn set_run_at_sign_in(name: &str, cmd: Option<&str>) -> Result<(), String> {
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows::Win32::System::Registry::{RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    let name = HSTRING::from(name);
    let r = match cmd {
        Some(c) => {
            let data: Vec<u16> = c.encode_utf16().chain([0]).collect();
            unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, &name, REG_SZ.0, Some(data.as_ptr().cast()), (data.len() * 2) as u32) }
        }
        None => match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, &name) } {
            e if e == ERROR_FILE_NOT_FOUND => return Ok(()),
            e => e,
        },
    };
    r.ok().map_err(|e| rx_core::t!("rx_sys.registry", e = e.message()))
}

/// A notification from the app's own icon by the clock (`tray`: tray-icon's window for it), shown
/// by Windows 10/11 as a toast. False when that window has no icon to show it from.
pub fn tray_balloon(tray: *mut std::ffi::c_void, title: &str, text: &str) -> bool {
    fn put<const N: usize>(to: &mut [u16; N], s: &str) {
        for (d, c) in to.iter_mut().zip(s.encode_utf16().take(N - 1)) {
            *d = c;
        }
    }
    let mut nid = NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: HWND(tray), uFlags: NIF_INFO, dwInfoFlags: NIIF_INFO | NIIF_RESPECT_QUIET_TIME, ..Default::default() };
    put(&mut nid.szInfoTitle, title);
    put(&mut nid.szInfo, text);
    // shortcut: tray-icon keeps its icon's id private (a counter from 1); its window holds that
    // one icon only, so the first id the shell takes is it. Upgrade if tray-icon exposes the id.
    (1..=32).any(|id| {
        nid.uID = id;
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool() }
    })
}

/// A secret kept for this user by Windows (Menedżer poświadczeń), not in the app's files.
pub fn secret(name: &str) -> Option<String> {
    use windows::Win32::Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC};
    let mut c: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        CredReadW(&HSTRING::from(name), CRED_TYPE_GENERIC, None, &mut c).ok()?;
        let n = (*c).CredentialBlobSize as usize;
        // an empty secret may come with no buffer at all
        let blob = if n == 0 { vec![] } else { std::slice::from_raw_parts((*c).CredentialBlob, n).to_vec() };
        CredFree(c as *const _);
        String::from_utf8(blob).ok()
    }
}

/// Keeps `value` as the secret `name` (`None`: removes it).
pub fn set_secret(name: &str, value: Option<&str>) -> Result<(), String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{CredDeleteW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC};
    let target = HSTRING::from(name);
    let r = match value {
        Some(v) => {
            let mut blob = v.as_bytes().to_vec();
            let c = CREDENTIALW {
                Type: CRED_TYPE_GENERIC,
                TargetName: PWSTR(target.as_ptr() as *mut u16),
                CredentialBlobSize: blob.len() as u32,
                CredentialBlob: blob.as_mut_ptr(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                ..Default::default()
            };
            unsafe { CredWriteW(&c, 0) }
        }
        None => match unsafe { CredDeleteW(&target, CRED_TYPE_GENERIC, None) } {
            Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
            r => r,
        },
    };
    r.map_err(|e| rx_core::t!("rx_sys.credentials", e = e.message()))
}

/// Window `hwnd` on screen: visible, not minimized (not hidden in the tray either) and on a monitor.
pub fn window_on_screen(hwnd: isize) -> bool {
    use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONULL};
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
    let h = HWND(hwnd as _);
    unsafe { IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() && !MonitorFromWindow(h, MONITOR_DEFAULTTONULL).is_invalid() }
}

/// Moves a window shown off every monitor to the top-left corner of the main one's work area and
/// returns true (a corner, not the middle: the window may still grow there, when its DPI changes
/// or it gets its start size). A place stored while the window was minimized (Windows parks it at
/// -32000) brought it back out there after an update: shown, yet invisible, and it seemed it
/// could not be maximized.
pub fn bring_onto_screen(hwnd: isize) -> bool {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY};
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER};
    let h = HWND(hwnd as _);
    unsafe {
        if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() || !MonitorFromWindow(h, MONITOR_DEFAULTTONULL).is_invalid() {
            return false;
        }
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTOPRIMARY), &mut info).as_bool() {
            return false;
        }
        SetWindowPos(h, None, info.rcWork.left, info.rcWork.top, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE).is_ok()
    }
}

/// The window's inner size in pixels, as Windows has it now (egui's lags a frame after a restore).
pub fn client_size(hwnd: isize) -> (i32, i32) {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    let mut r = RECT::default();
    let _ = unsafe { GetClientRect(HWND(hwnd as _), &mut r) };
    (r.right, r.bottom)
}

/// Window `hwnd` is the foreground window, the one the keys go to.
pub fn is_foreground(hwnd: isize) -> bool {
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() == HWND(hwnd as _) }
}

/// Puts the window in front, also when another program has the foreground (a click on the tray
/// icon goes to Explorer first): Windows lets a thread attached to the foreground one take it.
pub fn bring_to_front(hwnd: isize) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow};
    let h = HWND(hwnd as _);
    unsafe {
        let fg = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let me = GetCurrentThreadId();
        let attach = fg != 0 && fg != me && AttachThreadInput(me, fg, true).as_bool();
        let _ = BringWindowToTop(h);
        let _ = SetForegroundWindow(h);
        if attach {
            let _ = AttachThreadInput(me, fg, false);
        }
    }
}

/// One running copy per installation (per EXE path) and user session, through a named event. Returns true when this process is
/// that copy: `on_show` then runs (on a thread of its own) each time a later start asks for the
/// window. Returns false when a copy already runs: it has been asked to show itself and allowed
/// to take the foreground, so this process should end.
pub fn single_instance(on_show: impl Fn() + Send + 'static) -> bool {
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};
    unsafe {
        // no event (should not happen): run as before, as a copy of its own
        // per EXE path, so a test copy elsewhere runs beside the installed one
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_lowercase().replace('\\', "/")).unwrap_or_default();
        let Ok(h) = CreateEventW(None, false, false, &HSTRING::from(format!("Local\\RustExplorer.show:{exe}"))) else { return true };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = AllowSetForegroundWindow(ASFW_ANY);
            let _ = SetEvent(h);
            let _ = CloseHandle(h);
            return false;
        }
        // the handle stays open for the process's life: it is what makes this the running copy
        let raw = h.0 as usize;
        std::thread::spawn(move || {
            while WaitForSingleObject(HANDLE(raw as *mut _), INFINITE) == WAIT_OBJECT_0 {
                on_show();
            }
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rx_core::undo::Undo;

    /// How long `file_op`'s size count waits before it starts (unless the operation ends first):
    /// a slow network share, made on purpose.
    static HOLD_COUNT_MS: AtomicU64 = AtomicU64::new(0);

    pub(super) fn hold_count(stop: &AtomicBool) {
        let t = std::time::Instant::now();
        while t.elapsed().as_millis() < HOLD_COUNT_MS.load(Relaxed) as u128 && !stop.load(Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn open_reports_why_windows_could_not() {
        let gone = std::env::current_exe().unwrap().parent().unwrap().join("rx-test-brak.pdf");
        assert!(open(&gone).is_err_and(|e| !e.trim().is_empty()));
    }

    #[test]
    fn a_tree_reports_before_its_size_is_known_and_never_waits_for_it() {
        let base = std::env::current_exe().unwrap().parent().unwrap().join(format!("rx-test-tree-{}", std::process::id()));
        let tree = base.join("drzewo");
        for d in ["a", "b"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        for i in 0..4 {
            std::fs::create_dir_all(tree.join(format!("sub{i}"))).unwrap();
            for j in 0..100 {
                std::fs::write(tree.join(format!("sub{i}")).join(format!("f{j}.txt")), [b'x'; 1024]).unwrap();
            }
        }
        let size = 400 * 1024;
        init_com();

        // the count in time: bytes, ending at done == total == the tree's size
        let mut seen = vec![];
        let (ok, _, _) = copy_into_with(std::slice::from_ref(&tree), &base.join("a"), false, &mut |d, t| seen.push((d, t)));
        eprintln!("in time: {} reports: {:?} .. {:?}", seen.len(), &seen[..seen.len().min(3)], &seen[seen.len().saturating_sub(2)..]);
        assert!(ok && seen.windows(2).all(|w| w[0].0 <= w[1].0) && seen.last() == Some(&(size, size)), "{seen:?}");
        assert_eq!(std::fs::read_dir(base.join("a").join("drzewo").join("sub3")).unwrap().count(), 100);

        // the count held up: reports come anyway, as unknown (0, 0), and the copy does not wait for it
        HOLD_COUNT_MS.store(5000, Relaxed);
        let mut seen = vec![];
        let t = std::time::Instant::now();
        let (ok, _, _) = copy_into_with(std::slice::from_ref(&tree), &base.join("b"), false, &mut |d, t| seen.push((d, t)));
        let took = t.elapsed();
        HOLD_COUNT_MS.store(0, Relaxed);
        eprintln!("held: {} reports in {took:?}: {:?} ..", seen.len(), &seen[..seen.len().min(3)]);
        assert!(ok && !seen.is_empty() && seen.iter().all(|&r| r == (0, 0)) && took.as_millis() < 5000, "{seen:?} {took:?}");
        assert_eq!(std::fs::read_dir(base.join("b").join("drzewo").join("sub3")).unwrap().count(), 100);

        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn the_own_copy_keeps_progress_attributes_and_undo() {
        let base = std::env::current_exe().unwrap().parent().unwrap().join(format!("rx-test-own-{}", std::process::id()));
        let (src, dst, mv) = (base.join("src"), base.join("dst"), base.join("mv"));
        for d in [src.join("tree/.git"), dst.clone(), mv.clone()] {
            std::fs::create_dir_all(d).unwrap();
        }
        for i in 0..50 {
            std::fs::write(src.join(format!("tree/.git/f{i}")), [b'x'; 3000]).unwrap();
        }
        let hidden = |p: &Path| unsafe { GetFileAttributesW(&HSTRING::from(p)) } & FILE_ATTRIBUTE_HIDDEN.0 != 0;
        unsafe { SetFileAttributesW(&HSTRING::from(src.join("tree/.git").as_path()), FILE_ATTRIBUTE_HIDDEN) }.unwrap();
        let big = src.join("big.bin");
        std::fs::write(&big, vec![5u8; 20 << 20]).unwrap();
        let total = (20 << 20) + 50 * 3000;

        let mut seen = vec![];
        let (ok, undo, left, note) = transfer(&[src.join("tree"), big.clone()], &dst, false, &Default::default(), &mut |d, t| seen.push((d, t)));
        eprintln!("{} reports, last {:?}; {undo:?}", seen.len(), seen.last());
        assert!(ok && left == 0 && note.is_none() && seen.last() == Some(&(total, total)) && seen.windows(2).all(|w| w[0].0 <= w[1].0));
        assert_eq!(std::fs::read(dst.join("big.bin")).unwrap(), std::fs::read(&big).unwrap());
        assert!(hidden(&dst.join("tree/.git")) && std::fs::read_dir(dst.join("tree/.git")).unwrap().count() == 50);
        assert_eq!(undo, Undo::Created(vec![(dst.join("tree"), rx_core::undo::stamp(&dst.join("tree"))), (dst.join("big.bin"), rx_core::undo::stamp(&dst.join("big.bin")))], "undo.what.copy"));

        // Windows' errors reach the engine with std's kinds (a refusal hands over to Windows' copy)
        let e = rx_core::copy::Fs::copy_file(&WinFs, &big, &dst.join("big.bin"), &mut |_| true).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists, "{e}");
        // cancelled from the panel inside a file: an error, and nothing of it left behind
        let e = rx_core::copy::Fs::copy_file(&WinFs, &big, &dst.join("cut.bin"), &mut |_| false).unwrap_err();
        assert!(!dst.join("cut.bin").exists(), "{e}");

        // past 260 characters: through \\?\ (works with LongPathsEnabled off)
        let deep = (0..6).fold(src.join("gleboki"), |p, i| p.join(format!("{i}{}", "x".repeat(50))));
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("plik.txt"), "rx").unwrap();
        let (ok, _, _, note) = transfer(&[src.join("gleboki")], &dst, false, &Default::default(), &mut |_, _| {});
        let copied = dst.join(deep.strip_prefix(&src).unwrap()).join("plik.txt");
        assert!(ok && note.is_none() && copied.as_os_str().len() > 300 && copied.exists(), "{note:?}");

        // a move on one volume: renamed, and Ctrl+Z puts it back
        let (ok, undo, _, note) = transfer(std::slice::from_ref(&big), &mv, true, &Default::default(), &mut |_, _| {});
        assert!(ok && note.is_none() && !big.exists() && mv.join("big.bin").exists());
        assert_eq!(undo, Undo::Moved(vec![(mv.join("big.bin"), big.clone())]));
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Windows' copy against the app's own on this machine's disk, for one file of RX_BENCH_MB
    /// megabytes or a folder of RX_BENCH_FILES files of 20 KB:
    /// `set RX_BENCH_MB=500` and `rx_sys-….exe bench --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench() {
        let var = |k| std::env::var(k).ok().and_then(|v| v.parse::<usize>().ok());
        let (mb, files) = (var("RX_BENCH_MB"), var("RX_BENCH_FILES"));
        let base = std::env::current_exe().unwrap().parent().unwrap().join(format!("rx-test-bench-{}", std::process::id()));
        let item = base.join("zrodlo");
        // not zeros: nothing on the way may compress or skip them
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut fill = |p: &Path, bytes: usize| {
            let mut w = std::io::BufWriter::new(std::fs::File::create(p).unwrap());
            for _ in 0..bytes / 8 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                std::io::Write::write_all(&mut w, &x.to_le_bytes()).unwrap();
            }
        };
        let total = match (mb, files) {
            (Some(mb), _) => {
                std::fs::create_dir_all(&base).unwrap();
                fill(&item, mb << 20);
                mb << 20
            }
            (_, Some(n)) => {
                for i in 0..n {
                    let d = item.join(format!("d{}", i % 10));
                    std::fs::create_dir_all(&d).unwrap();
                    fill(&d.join(format!("f{i}.txt")), 20 << 10);
                }
                n * (20 << 10)
            }
            _ => return,
        };
        init_com();
        for round in 0..2 {
            for own in [round == 1, round == 0] {
                let dir = base.join(format!("{round}{own}"));
                std::fs::create_dir_all(&dir).unwrap();
                let t = std::time::Instant::now();
                let ok = if own { transfer(std::slice::from_ref(&item), &dir, false, &Default::default(), &mut |_, _| {}).0 } else { copy_into_with(std::slice::from_ref(&item), &dir, false, &mut |_, _| {}).0 };
                let s = t.elapsed().as_secs_f64();
                eprintln!("{} round {round}: {ok} {s:.2} s, {:.0} MB/s", if own { "own" } else { "windows" }, total as f64 / 1048576.0 / s);
                std::fs::remove_dir_all(&dir).unwrap();
            }
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// `copy_into` as it was before IFileOperation: SHFileOperationW and its name mappings.
    fn copy_into_old(paths: &[PathBuf], dir: &Path, cut: bool) -> (bool, Undo, usize) {
        let flags = if paths.iter().any(|p| p.parent() == Some(dir)) && !cut { FOF_RENAMEONCOLLISION.0 } else { 0 };
        let before: Vec<bool> = paths.iter().map(|p| p.file_name().is_some_and(|n| dir.join(n).symlink_metadata().is_ok())).collect();
        let (ok, renamed) = shell_op(if cut { FO_MOVE } else { FO_COPY }, paths, &[dir.to_path_buf()], flags);
        let (undo, left) = rx_core::undo::transferred(paths, dir, cut, &before, &renamed, rx_core::undo::stamp);
        (ok, undo, left)
    }

    #[test]
    fn a_copy_reports_progress_new_names_and_the_same_undo_as_before() {
        let base = std::env::current_exe().unwrap().parent().unwrap().join(format!("rx-test-fileop-{}", std::process::id()));
        let (src, dst, mv) = (base.join("src"), base.join("dst"), base.join("mv"));
        for d in [&src, &dst, &mv] {
            std::fs::create_dir_all(d).unwrap();
        }
        let (a, b) = (src.join("a.bin"), src.join("b.txt"));
        std::fs::write(&a, vec![7u8; 50 << 20]).unwrap();
        std::fs::write(&b, "rx").unwrap();
        init_com();

        // (a) into another folder: the reports grow to done == total
        let mut seen = vec![];
        let (ok, undo, left) = copy_into_with(&[a.clone(), b.clone()], &dst, false, &mut |d, t| seen.push((d, t)));
        eprintln!("(a) {} reports: {:?} .. {:?}", seen.len(), &seen[..seen.len().min(3)], &seen[seen.len().saturating_sub(3)..]);
        assert!(ok && left == 0 && dst.join("a.bin").metadata().unwrap().len() == 50 << 20 && dst.join("b.txt").exists());
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0) && seen.last() == Some(&((50 << 20) + 2, (50 << 20) + 2)), "{seen:?}");
        let Undo::Created(made, _) = undo else { panic!("{undo:?}") };
        assert_eq!(made.into_iter().map(|m| m.0).collect::<Vec<_>>(), [dst.join("a.bin"), dst.join("b.txt")]);

        // (b) in its own folder: a new name, on the renamed list (else `transferred` leaves it out)
        let (ok, new, left) = copy_into(std::slice::from_ref(&b), &src, false);
        let (ok_old, old, left_old) = copy_into_old(std::slice::from_ref(&b), &src, false);
        eprintln!("(b) {new:?}\n(d) old: {old:?}");
        let (Undo::Created(new, _), Undo::Created(old, _)) = (new, old) else { panic!() };
        assert!(ok && ok_old && left == 0 && left_old == 0 && new.len() == 1 && old.len() == 1);
        assert!(new[0].0 != b && new[0].0.exists() && new[0].0.parent() == Some(src.as_path()));
        // the old way, after the new one, names the next copy: "b - Kopia" then "b - Kopia (2)"
        assert!(old[0].0.file_stem().unwrap().to_string_lossy().starts_with(&*new[0].0.file_stem().unwrap().to_string_lossy()), "{new:?} {old:?}");

        // (c) a move: gone from its folder, in the new one; (d) the old way records the same
        let (ok, undo, _) = copy_into_with(std::slice::from_ref(&a), &mv, true, &mut |_, _| {});
        eprintln!("(c) {undo:?}");
        assert!(ok && !a.exists() && mv.join("a.bin").exists());
        assert_eq!(undo, Undo::Moved(vec![(mv.join("a.bin"), a.clone())]));
        let Undo::Moved(items) = &undo else { unreachable!() };
        assert!(move_back(items).is_empty() && a.exists());
        let (ok, old, _) = copy_into_old(std::slice::from_ref(&a), &mv, true);
        assert!(ok);
        assert_eq!(old, undo);

        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn the_sign_in_entry_is_written_read_and_removed() {
        let name = "RustExplorerTest";
        set_run_at_sign_in(name, Some(r#""C:\Program Files\Ża\rx.exe" --tray"#)).unwrap();
        assert_eq!(run_at_sign_in(name).as_deref(), Some(r#""C:\Program Files\Ża\rx.exe" --tray"#));
        set_run_at_sign_in(name, None).unwrap();
        assert_eq!(run_at_sign_in(name), None);
        // removing what is not there is fine
        set_run_at_sign_in(name, None).unwrap();
    }

    #[test]
    fn a_secret_is_kept_read_and_removed() {
        let name = "RustExplorerTest/sekret";
        set_secret(name, Some("klucz-ąę-123")).unwrap();
        assert_eq!(secret(name).as_deref(), Some("klucz-ąę-123"));
        set_secret(name, None).unwrap();
        assert_eq!(secret(name), None);
        set_secret(name, None).unwrap();
    }

    #[test]
    fn eco_mode_goes_on_and_back_to_windows() {
        use windows::Win32::System::Threading::*;
        let state = || unsafe {
            let mut s = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ..Default::default() };
            GetProcessInformation(GetCurrentProcess(), ProcessPowerThrottling, &mut s as *mut _ as *mut _, size_of_val(&s) as u32).unwrap();
            (s.ControlMask, s.StateMask)
        };
        eco_mode(true);
        assert_eq!(state(), (PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_EXECUTION_SPEED));
        eco_mode(false);
        assert_eq!(state(), (0, 0));
    }

    #[test]
    fn only_a_window_a_person_sees_holds_efficiency_mode_off() {
        use windows::core::w;
        use windows::Win32::UI::WindowsAndMessaging::*;
        let make = |ex, w, h| unsafe { CreateWindowExW(ex, w!("STATIC"), None, WS_POPUP | WS_VISIBLE, -32000, -32000, w, h, None, None, None, None).unwrap() };
        assert!(!shows_a_window());
        // as winit's event window, there in every run of the app
        let helper = make(WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_TOOLWINDOW, 0, 0);
        assert!(!shows_a_window());
        // as a Properties sheet (kept off screen here)
        let sheet = make(WS_EX_NOACTIVATE, 1, 1);
        assert!(shows_a_window());
        // minimized, it holds nothing up either
        unsafe { SetWindowLongW(sheet, GWL_STYLE, (WS_POPUP | WS_VISIBLE | WS_MINIMIZE).0 as i32) };
        assert!(!shows_a_window());
        unsafe {
            DestroyWindow(sheet).unwrap();
            DestroyWindow(helper).unwrap();
        }
        assert!(!shows_a_window());
    }

    #[test]
    fn a_shell_thread_with_a_process_reference_counts_as_work() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        static DONE: AtomicBool = AtomicBool::new(false);
        unsafe extern "system" fn work(_: *mut core::ffi::c_void) -> u32 {
            while !DONE.load(SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            0
        }
        take_shell_work();
        assert!(!shell_working());
        // as the shell starts its copy threads: CTF_PROCESS_REF holds the reference while it runs
        unsafe { SHCreateThread(Some(work), None, CTF_PROCESS_REF as u32, None) }.unwrap();
        assert!(shell_working());
        DONE.store(true, SeqCst);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while shell_working() && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!shell_working());
    }

    #[test]
    fn a_thread_enters_background_mode() {
        use windows::Win32::Foundation::ERROR_THREAD_MODE_ALREADY_BACKGROUND;
        use windows::Win32::System::Threading::*;
        std::thread::spawn(|| {
            background_thread();
            // Windows refuses to begin the mode twice, which shows it is on
            let again = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) };
            assert_eq!(again.unwrap_err().code(), ERROR_THREAD_MODE_ALREADY_BACKGROUND.to_hresult());
        })
        .join()
        .unwrap();
    }

    #[test]
    #[ignore = "needs the app's manifest: copy app/rustexplorer.manifest next to the test EXE as <exe>.manifest"]
    fn a_file_past_260_characters_is_made_listed_renamed_and_deleted() {
        let top = std::env::current_exe().unwrap().parent().unwrap().join(format!("rx-test-long-{}", std::process::id()));
        let dir = top.join("a".repeat(100)).join("b".repeat(100)).join("c".repeat(100));
        let (file, renamed) = (dir.join("plik.txt"), dir.join("plik-2.txt"));
        assert!(file.as_os_str().len() > 300, "{}", file.display());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, "rx").unwrap();
        let listed: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        // the only step here that needs the manifest (std::fs adds \\?\ itself): a plain path to
        // Win32 works only in a long-path-aware process; the app's shell operations are not covered
        let moved = rename_new(&file, &renamed);
        let after: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        let removed = std::fs::remove_file(&renamed);
        std::fs::remove_dir_all(&top).unwrap();
        assert_eq!(listed, ["plik.txt"]);
        moved.unwrap();
        assert_eq!(after, ["plik-2.txt"]);
        removed.unwrap();
        assert!(!top.exists());
    }
}
