//! Explorer's own context menu for files ("Wyślij do", 7-Zip, Git…): the shell and its extensions
//! fill it, this window shows it and runs what was picked there.
use std::path::PathBuf;

use windows::core::{Interface, Result, HSTRING, PCSTR, PCWSTR};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_SHIFT};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// the first command number; 0 is what TrackPopupMenuEx gives back when nothing was picked
const FIRST: u32 = 1;
const SUBCLASS: usize = 0x5258_4d4e;
// CMIC_MASK_UNICODE (the same bit as SEE_MASK_UNICODE), which windows-rs does not define
const CMIC_MASK_UNICODE: u32 = 0x4000;

/// The shell's own names (ITEMIDLISTs) of the items a menu is for: plain memory, tied to no
/// thread, so they are made where a slow disk may hold things up and used on the window's thread.
pub struct Items(Vec<*mut ITEMIDLIST>);

// CoTaskMem blocks owned only by this value
unsafe impl Send for Items {}

impl Drop for Items {
    fn drop(&mut self) {
        for &p in &self.0 {
            unsafe { CoTaskMemFree(Some(p as _)) };
        }
    }
}

/// Asks the disk for each of `paths` (a share that stopped answering holds the caller, so not the
/// window's thread), which need COM initialized.
pub fn items(paths: &[PathBuf]) -> std::result::Result<Items, String> {
    let mut items = Items(Vec::with_capacity(paths.len()));
    for p in paths {
        let mut pidl = std::ptr::null_mut();
        unsafe { SHParseDisplayName(&HSTRING::from(p.as_path()), None::<&IBindCtx>, &mut pidl, 0, None) }.map_err(|e| e.message())?;
        items.0.push(pidl);
    }
    Ok(items)
}

/// The shell's menu for `items`, all in one folder; `extended`: with what Explorer adds when Shift
/// is held.
fn build(owner: HWND, items: &Items, extended: bool) -> Result<(IContextMenu, HMENU)> {
    unsafe {
        let folder: IShellFolder = SHBindToParent(items.0[0], None)?;
        let children: Vec<*const ITEMIDLIST> = items.0.iter().map(|&p| ILFindLastID(p) as *const _).collect();
        let menu: IContextMenu = folder.GetUIObjectOf(owner, &children, None)?;
        let hmenu = CreatePopupMenu()?;
        let flags = CMF_NORMAL | if extended { CMF_EXTENDEDVERBS } else { 0 };
        if let Err(e) = menu.QueryContextMenu(hmenu, 0, FIRST, 0x7FFF, flags).ok() {
            let _ = DestroyMenu(hmenu);
            return Err(e);
        }
        Ok((menu, hmenu))
    }
}

/// Shows the shell's menu for `items` (all in one folder) at `at`, a point in `hwnd`'s client
/// area in pixels (None: at the mouse pointer), and runs the command picked there with the window
/// as owner. Ok(true): a command was picked, so files may have changed. The menu runs its own
/// message loop; it must be called on the window's thread, a single-threaded COM one as shell
/// extensions need.
pub fn shell_menu(hwnd: isize, at: Option<(i32, i32)>, items: &Items) -> std::result::Result<bool, String> {
    if items.0.is_empty() || hwnd == 0 {
        return Ok(false);
    }
    let h = HWND(hwnd as _);
    let fail = |e: windows::core::Error| rx_core::t!("rx_sys.menu", e = e.message());
    let extended = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    let (menu, hmenu) = build(h, items, extended).map_err(fail)?;
    let mut pt = POINT::default();
    unsafe {
        match at {
            Some((x, y)) => {
                pt = POINT { x, y };
                let _ = ClientToScreen(h, &mut pt);
            }
            None => {
                let _ = GetCursorPos(&mut pt);
            }
        }
        // "Wyślij do", "Otwórz za pomocą" and other submenus are filled only when they open,
        // through messages to the menu's owner window: they go to the shell while it is open
        let _ = SetWindowSubclass(h, Some(forward), SUBCLASS, &menu as *const IContextMenu as usize);
        let cmd = TrackPopupMenuEx(hmenu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, pt.x, pt.y, h, None).0 as u32;
        let _ = RemoveWindowSubclass(h, Some(forward), SUBCLASS);
        let ran = if cmd >= FIRST {
            let verb = (cmd - FIRST) as usize;
            // no Shift or Ctrl flag: "Usuń" from this menu always goes to the Recycle Bin
            let info = CMINVOKECOMMANDINFOEX {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                fMask: CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE,
                hwnd: h,
                lpVerb: PCSTR(verb as *const u8),
                lpVerbW: PCWSTR(verb as *const u16),
                nShow: SW_SHOWNORMAL.0,
                ptInvoke: pt,
                ..Default::default()
            };
            match menu.InvokeCommand(&info as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO) {
                Err(e) if e.code() != ERROR_CANCELLED.to_hresult() => Err(fail(e)),
                _ => Ok(true),
            }
        } else {
            Ok(false)
        };
        let _ = DestroyMenu(hmenu);
        ran
    }
}

/// The menu messages a shell extension draws or fills its items with, while the menu is open.
unsafe extern "system" fn forward(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, menu: usize) -> LRESULT {
    if matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        let menu = unsafe { &*(menu as *const IContextMenu) };
        let mut r = LRESULT(0);
        if let Ok(m3) = menu.cast::<IContextMenu3>() {
            if unsafe { m3.HandleMenuMsg2(msg, wp, lp, Some(&mut r)) }.is_ok() {
                return r;
            }
        } else if let Ok(m2) = menu.cast::<IContextMenu2>() {
            if unsafe { m2.HandleMenuMsg(msg, wp, lp) }.is_ok() {
                return r;
            }
        }
    }
    unsafe { DefSubclassProc(h, msg, wp, lp) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_shell_builds_a_menu_for_two_files() {
        use windows::Win32::UI::WindowsAndMessaging::{DestroyMenu, GetMenuItemCount};
        // next to the test EXE: the test folder, not the user's own
        let exe = std::env::current_exe().unwrap();
        let dir = exe.parent().unwrap();
        let paths: Vec<_> = ["rx-test-menu-a.txt", "rx-test-menu-b.txt"].iter().map(|n| dir.join(n)).collect();
        for p in &paths {
            std::fs::write(p, "x").unwrap();
        }
        unsafe { windows::Win32::System::Ole::OleInitialize(None).unwrap() };
        let built = super::items(&paths).map(|i| super::build(Default::default(), &i, false).map_err(|e| e.message()));
        for p in &paths {
            let _ = std::fs::remove_file(p);
        }
        let (_menu, hmenu) = built.unwrap().unwrap();
        // at least Explorer's own Kopiuj, Usuń and Właściwości
        assert!(unsafe { GetMenuItemCount(Some(hmenu)) } >= 3);
        unsafe { DestroyMenu(hmenu).unwrap() };
    }
}
