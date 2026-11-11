//! Files on the Windows clipboard and in a system drag, as Explorer offers them: a list of paths
//! (CF_HDROP) and, on the clipboard, whether it was cut ("Preferred DropEffect").
use std::ffi::OsString;
use std::mem::ManuallyDrop;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::core::{w, Result};
use windows::Win32::Foundation::{GlobalFree, HGLOBAL, HWND, POINT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{IDropSource, OleFlushClipboard, OleGetClipboard, OleInitialize, OleSetClipboard, OleUninitialize, CF_HDROP, CF_UNICODETEXT, DROPEFFECT_COPY, DROPEFFECT_MOVE};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetKeyState, VK_CONTROL, VK_LBUTTON, VK_SHIFT};
use windows::Win32::UI::Shell::{DragQueryFileW, SHCreateDataObject, SHDoDragDrop, DROPFILES, HDROP};
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
