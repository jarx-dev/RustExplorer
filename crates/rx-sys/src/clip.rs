//! Files on the Windows clipboard and in a system drag, as Explorer offers them: a list of paths
//! (CF_HDROP) and, on the clipboard, whether it was cut ("Preferred DropEffect").
use std::ffi::OsString;
use std::mem::ManuallyDrop;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::core::{w, Result};
use windows::Win32::Foundation::{GlobalFree, HGLOBAL, HWND, POINT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, STREAM_SEEK_SET, TYMED_HGLOBAL, TYMED_ISTREAM};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{IDropSource, OleFlushClipboard, OleGetClipboard, OleInitialize, OleSetClipboard, OleUninitialize, ReleaseStgMedium, CF_HDROP, CF_UNICODETEXT, DROPEFFECT_COPY, DROPEFFECT_MOVE};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetKeyState, VK_CONTROL, VK_LBUTTON, VK_SHIFT};
use windows::Win32::UI::Shell::{DragQueryFileW, SHCreateDataObject, SHDoDragDrop, DROPFILES, FD_ATTRIBUTES, FD_FILESIZE, FD_WRITESTIME, FILEDESCRIPTORW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetCursorPos, WindowFromPoint, GA_ROOT};

fn effect_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) }
}

/// Adds `bytes` as `format` to `obj`, in the movable global block clipboard data must be.
fn set(obj: &IDataObject, format: u32, bytes: &[u8]) -> Result<()> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return Err(windows::core::Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        let fmt = FORMATETC { cfFormat: format as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
        let medium = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: h }, pUnkForRelease: ManuallyDrop::new(None) };
        // on success the data object owns the block
        obj.SetData(&fmt, &medium, true).inspect_err(|_| {
            let _ = GlobalFree(Some(h));
        })
    }
}

/// The shell's data object with `paths`; `cut` sets the preferred effect a paste reads. Made
/// from the paths alone, without asking the disk, so it is safe on the UI thread.
fn data(paths: &[PathBuf], cut: Option<bool>) -> Result<IDataObject> {
    let obj: IDataObject = unsafe { SHCreateDataObject(None, None, None::<&IDataObject>)? };
    let head = DROPFILES { pFiles: std::mem::size_of::<DROPFILES>() as u32, pt: POINT::default(), fNC: false.into(), fWide: true.into() };
    let mut bytes = unsafe { std::slice::from_raw_parts((&head as *const DROPFILES).cast::<u8>(), std::mem::size_of::<DROPFILES>()) }.to_vec();
    bytes.extend(super::multi_sz(paths).iter().flat_map(|c| c.to_le_bytes()));
    set(&obj, CF_HDROP.0 as u32, &bytes)?;
    if let Some(cut) = cut {
        set(&obj, effect_format(), &rx_core::clip_effect(cut).to_le_bytes())?;
    }
    Ok(obj)
}

/// Puts `paths` on the Windows clipboard as Explorer's Kopiuj or Wytnij does, kept there after
/// this program exits. Called on the window's thread, which the window library made OLE's.
pub fn put(paths: &[PathBuf], cut: bool) -> bool {
    let obj = match data(paths, Some(cut)) {
        Ok(o) => o,
        Err(_) => return false,
    };
    unsafe { OleSetClipboard(&obj).and_then(|_| OleFlushClipboard()).is_ok() }
}

/// Empties the Windows clipboard, so a list copied before (maybe a cut) is not pasted later.
pub fn clear() {
    let _ = unsafe { OleSetClipboard(None::<&IDataObject>) };
}

/// Changes with every new content of the clipboard: one cheap call, no reading.
pub fn seq() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// The clipboard holds a list of files (asked without reading it).
pub fn has_files() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok() }
}

/// The list of files on the clipboard and whether it was cut; None without one, or while another
/// program holds the clipboard open.
pub fn files() -> Option<(Vec<PathBuf>, bool)> {
    unsafe {
        OpenClipboard(None).ok()?;
        let got = (|| {
            let drop = HDROP(GetClipboardData(CF_HDROP.0 as u32).ok()?.0);
            let paths: Vec<PathBuf> = (0..DragQueryFileW(drop, u32::MAX, None))
                .filter_map(|i| {
                    let mut buf = vec![0u16; DragQueryFileW(drop, i, None) as usize + 1];
                    let len = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                    (len > 0).then(|| PathBuf::from(OsString::from_wide(&buf[..len])))
                })
                .collect();
            let effect = GetClipboardData(effect_format()).ok().map_or(DROPEFFECT_COPY.0, |h| {
                let h = HGLOBAL(h.0);
                let p = GlobalLock(h) as *const u32;
                let v = if p.is_null() || GlobalSize(h) < 4 { DROPEFFECT_COPY.0 } else { p.read_unaligned() };
                let _ = GlobalUnlock(h);
                v
            });
            (!paths.is_empty()).then(|| (paths, rx_core::clip_is_cut(effect)))
        })();
        let _ = CloseClipboard();
        got
    }
}

/// After cut files were moved here, as Explorer does: the source is told "Paste Succeeded" (the
/// move was done here, so it deletes nothing) and the clipboard stops offering them, unless
/// something else was copied since (`seq` is the clipboard's number at the paste).
pub fn paste_done(seq: u32) {
    // OLE's clipboard needs a single-threaded apartment; the file-operation threads are not one
    let _ = std::thread::spawn(move || unsafe {
        if OleInitialize(None).is_err() {
            return;
        }
        if GetClipboardSequenceNumber() == seq {
            if let Ok(obj) = OleGetClipboard() {
                let _ = set(&obj, RegisterClipboardFormatW(w!("Paste Succeeded")), &DROPEFFECT_MOVE.0.to_le_bytes());
            }
            let _ = OleSetClipboard(None::<&IDataObject>);
        }
        OleUninitialize();
    })
    .join();
}

/// The clipboard holds files with no path here, only their contents (FileGroupDescriptorW and
/// FileContents): copied in a Remote Desktop session, out of a ZIP in Explorer, a mail's attachments.
pub fn has_contents() -> bool {
    unsafe { IsClipboardFormatAvailable(RegisterClipboardFormatW(w!("FileGroupDescriptorW"))).is_ok() }
}

/// Where each item named on the clipboard (with whether it is a folder) goes in a folder; None: passed by.
pub type Place = fn(&Path, &[(String, bool)]) -> Vec<Option<PathBuf>>;

/// Writes the files the clipboard holds only as contents (`has_contents`) into `dir`, as Explorer
/// pastes them, where `place` says. Returns the items placed in `dir` itself, and how it stopped
/// early: Ok a cancel from the panel, Err an error.
pub fn paste_contents(dir: &Path, place: Place, ctl: &rx_core::ops::Control, progress: &mut dyn FnMut(u64, u64)) -> (Vec<PathBuf>, Option<std::result::Result<String, String>>) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|s| {
        // OLE's clipboard needs a single-threaded apartment; the job threads are not one
        let job = s.spawn(move || unsafe {
            if OleInitialize(None).is_err() {
                return (vec![], Some(Err(rx_core::t!("rx_sys.internal").into())));
            }
            let got = contents_into(dir, place, ctl, &mut |d, t| {
                let _ = tx.send((d, t));
            });
            OleUninitialize();
            got
        });
        for (done, total) in rx {
            progress(done, total);
        }
        job.join().unwrap_or_else(|_| (vec![], Some(Err(rx_core::t!("rx_sys.internal").into()))))
    })
}

unsafe fn get(obj: &IDataObject, format: windows::core::PCWSTR, index: i32, tymed: i32) -> Result<STGMEDIUM> {
    let fmt = FORMATETC { cfFormat: RegisterClipboardFormatW(format) as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: index, tymed: tymed as u32 };
    obj.GetData(&fmt)
}

unsafe fn contents_into(dir: &Path, place: Place, ctl: &rx_core::ops::Control, progress: &mut dyn FnMut(u64, u64)) -> (Vec<PathBuf>, Option<std::result::Result<String, String>>) {
    let err = |e: &dyn std::fmt::Display| (vec![], Some(Err(e.to_string())));
    let obj = match OleGetClipboard() {
        Ok(o) => o,
        Err(e) => return err(&e.message()),
    };
    let mut fgd = match get(&obj, w!("FileGroupDescriptorW"), -1, TYMED_HGLOBAL.0) {
        Ok(m) => m,
        Err(e) => return err(&e.message()),
    };
    let h = fgd.u.hGlobal;
    let p = GlobalLock(h) as *const u8;
    let each = std::mem::size_of::<FILEDESCRIPTORW>();
    let descs: Option<Vec<FILEDESCRIPTORW>> = (!p.is_null() && GlobalSize(h) >= 4).then(|| {
        // the count comes from another program: never read past the block, and a list cut short is an error
        let n = (p as *const u32).read_unaligned() as usize;
        (n <= (GlobalSize(h) - 4) / each).then(|| (0..n).map(|i| (p.add(4 + i * each) as *const FILEDESCRIPTORW).read_unaligned()).collect())
    }).flatten();
    let _ = GlobalUnlock(h);
    ReleaseStgMedium(&mut fgd);
    let Some(descs) = descs else { return err(&rx_core::t!("rx_sys.incomplete")) };
    let folder = |d: &FILEDESCRIPTORW| d.dwFlags & FD_ATTRIBUTES.0 as u32 != 0 && d.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
    // None: the source does not say
    let size = |d: &FILEDESCRIPTORW| (d.dwFlags & FD_FILESIZE.0 as u32 != 0).then_some(((d.nFileSizeHigh as u64) << 32) | d.nFileSizeLow as u64);
    let named: Vec<(String, bool)> = descs.iter().map(|d| (super::from_wide(&{ d.cFileName }), folder(d))).collect();
    let places = place(dir, &named);
    // only what is pasted: items passed by do not count
    let taken = || descs.iter().zip(&places).filter(|(_, to)| to.is_some()).map(|(d, _)| d);
    let total: u64 = taken().filter_map(size).sum();
    let all = taken().filter(|d| !folder(d)).count();
    let (mut done, mut placed, mut files) = (0u64, vec![], 0usize);
    let mut buf = vec![0u8; 1 << 20];
    ctl.arm();
    for (i, (d, to)) in descs.iter().zip(places).enumerate() {
        let Some(to) = to else { continue };
        if !ctl.go_on() {
            return (placed, Some(Ok(rx_core::t!("copy.cancelled_copy", n = files, total = all))));
        }
        let name = named[i].0.clone();
        let fail = |e: &dyn std::fmt::Display| rx_core::t!("copy.failed", name = name, e = e);
        let top = to.strip_prefix(dir).ok().and_then(|r| r.components().next()).map(|c| dir.join(c)).filter(|t| !placed.contains(t));
        // a top folder is made here, never taken over: one another program made meanwhile is not this paste's to undo
        if let Some(t) = top.as_ref().filter(|t| **t != to || folder(d)) {
            if let Err(e) = std::fs::create_dir(t) {
                return (placed, Some(Err(fail(&e))));
            }
            placed.push(t.clone());
        }
        if folder(d) {
            if let Err(e) = std::fs::create_dir_all(&to) {
                return (placed, Some(Err(fail(&e))));
            }
            continue;
        }
        let opened = to.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::File::create_new(&to));
        let mut f = match opened {
            Ok(f) => f,
            Err(e) => return (placed, Some(Err(fail(&e)))),
        };
        let mut medium = match get(&obj, w!("FileContents"), i as i32, TYMED_ISTREAM.0 | TYMED_HGLOBAL.0) {
            Ok(m) => m,
            Err(e) => {
                drop(f);
                let _ = std::fs::remove_file(&to);
                return (placed, Some(Err(fail(&e.message()))));
            }
        };
        let short = || rx_core::t!("rx_sys.incomplete").to_string();
        let wrote: std::result::Result<bool, String> = (|| {
            use std::io::Write;
            // a cancel during a slow GetData
            if !ctl.go_on() {
                return Ok(false);
            }
            if medium.tymed == TYMED_HGLOBAL.0 as u32 {
                let h = medium.u.hGlobal;
                // the block may be larger than the file, never smaller
                let n = match size(d) {
                    Some(s) if s as usize > GlobalSize(h) => return Err(short()),
                    Some(s) => s as usize,
                    None => GlobalSize(h),
                };
                if n > 0 {
                    let p = GlobalLock(h) as *const u8;
                    if p.is_null() {
                        return Err(rx_core::t!("rx_sys.internal").into());
                    }
                    let r = f.write_all(std::slice::from_raw_parts(p, n)).map_err(|e| e.to_string());
                    let _ = GlobalUnlock(h);
                    r?;
                }
                done += n as u64;
                progress(done, total);
                return Ok(true);
            }
            let stm = match (medium.tymed == TYMED_ISTREAM.0 as u32).then(|| (*medium.u.pstm).as_ref()).flatten() {
                Some(s) => s,
                None => return Err(rx_core::t!("rx_sys.internal").into()),
            };
            let mut written = 0u64;
            // a source may hand out the same stream again (a second paste), left at its end
            let _ = stm.Seek(0, STREAM_SEEK_SET, None);
            loop {
                if !ctl.go_on() {
                    return Ok(false);
                }
                let mut got = 0u32;
                stm.Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut got as *mut u32)).ok().map_err(|e| e.message())?;
                if got == 0 {
                    return if size(d).is_some_and(|s| written < s) { Err(short()) } else { Ok(true) };
                }
                f.write_all(&buf[..got as usize]).map_err(|e| e.to_string())?;
                written += got as u64;
                done += got as u64;
                progress(done, total);
            }
        })();
        ReleaseStgMedium(&mut medium);
        match wrote {
            Ok(true) => {
                files += 1;
                if top.as_ref() == Some(&to) {
                    placed.push(to.clone());
                }
                if d.dwFlags & FD_WRITESTIME.0 as u32 != 0 {
                    // 100 ns steps since 1601, as Explorer keeps the copied file's date
                    let ft = d.ftLastWriteTime;
                    let t = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
                    if let Some(us) = (t / 10).checked_sub(11_644_473_600_000_000) {
                        let _ = f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_micros(us));
                    }
                }
            }
            stop => {
                drop(f);
                let _ = std::fs::remove_file(&to);
                return (placed, Some(stop.map(|_| rx_core::t!("copy.cancelled_copy", n = files, total = all)).map_err(|e| fail(&e))));
            }
        }
    }
    (placed, None)
}

/// A system drag of `paths` out of window `hwnd`, as Explorer starts one: copy or move, as the
/// place dropped on and its Shift/Ctrl rules decide. It runs its own message loop until the
/// button is let go; the files are never deleted here (the target moves them itself).
pub fn drag_out(hwnd: isize, paths: &[PathBuf]) {
    // with the button already up the drag would drop at once on whatever is under the pointer
    if unsafe { GetKeyState(VK_LBUTTON.0 as i32) } >= 0 {
        return;
    }
    if let Ok(obj) = data(paths, None) {
        let _ = unsafe { SHDoDragDrop(Some(HWND(hwnd as _)), &obj, None::<&IDropSource>, DROPEFFECT_COPY | DROPEFFECT_MOVE) };
    }
}

/// Where the mouse pointer is in `hwnd`'s client area, in pixels, and whether it is inside it
/// with no other program's window over that spot.
pub fn pointer(hwnd: isize) -> Option<(i32, i32, bool)> {
    let mut p = POINT::default();
    let (w, h) = super::client_size(hwnd);
    unsafe {
        GetCursorPos(&mut p).ok()?;
        // a mail or Explorer window lying over this one is where a drag there is meant to go
        let ours = GetAncestor(WindowFromPoint(p), GA_ROOT) == HWND(hwnd as _);
        ScreenToClient(HWND(hwnd as _), &mut p).as_bool().then_some((p.x, p.y, ours && (0..w).contains(&p.x) && (0..h).contains(&p.y)))
    }
}

/// Shift and Ctrl are held now, also while another program has the keyboard (a drop from it).
pub fn shift_ctrl() -> (bool, bool) {
    unsafe { (GetAsyncKeyState(VK_SHIFT.0 as i32) < 0, GetAsyncKeyState(VK_CONTROL.0 as i32) < 0) }
}

/// The clipboard's text, for a text field's paste; None without text, or while another program
/// holds the clipboard open.
pub fn text() -> Option<String> {
    unsafe {
        OpenClipboard(None).ok()?;
        let got = (|| {
            let h = HGLOBAL(GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?.0);
            let p = GlobalLock(h) as *const u16;
            if p.is_null() {
                return None;
            }
            let all = std::slice::from_raw_parts(p, GlobalSize(h) / 2);
            let s = String::from_utf16_lossy(&all[..all.iter().position(|&c| c == 0).unwrap_or(all.len())]);
            let _ = GlobalUnlock(h);
            Some(s)
        })();
        let _ = CloseClipboard();
        got
    }
}

#[cfg(test)]
mod tests {
    // ignored: it replaces what the owner has on the clipboard when run in his session
    #[test]
    #[ignore]
    fn clipboard_round_trip() {
        let p = vec![std::env::temp_dir().join("rx-test-clip.txt")];
        unsafe { windows::Win32::System::Ole::OleInitialize(None).unwrap() };
        assert!(super::put(&p, true) && super::has_files());
        assert_eq!(super::files(), Some((p, true)));
        super::paste_done(super::seq());
        assert!(!super::has_files());
    }
}
