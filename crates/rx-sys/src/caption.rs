//! Windows 11's snap layouts on the app's own "Maksymalizuj" button: the window tells Windows
//! where that button is (WM_NCHITTEST answers HTMAXBUTTON there), so the mouse over it arrives as
//! non-client messages, passed on to the app as hover and click.
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering::Relaxed};
use std::sync::OnceLock;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

const SUBCLASS: usize = 0x5258_4d58;
// one main window per process, and all of it is touched on that window's thread
static RECT: [AtomicI32; 4] = [const { AtomicI32::new(0) }; 4];
static HOVER: AtomicBool = AtomicBool::new(false);
static PRESSED: AtomicBool = AtomicBool::new(false);
static CLICKED: AtomicBool = AtomicBool::new(false);
static REPAINT: OnceLock<fn()> = OnceLock::new();

/// Hooks the window's messages; `repaint` asks the app for a frame when hover or a click changed.
pub fn install(hwnd: isize, repaint: fn()) {
    let _ = REPAINT.set(repaint);
    let _ = unsafe { SetWindowSubclass(HWND(hwnd as _), Some(caption), SUBCLASS, 0) };
}

/// Where the button is now, in client pixels [left, top, right, bottom).
pub fn set_rect(r: [i32; 4]) {
    for (a, v) in RECT.iter().zip(r) {
        a.store(v, Relaxed);
    }
}

/// The mouse is over the button (egui does not see it there).
pub fn hovered() -> bool {
    HOVER.load(Relaxed)
}

/// The button was clicked with the mouse since the last call.
pub fn take_click() -> bool {
    CLICKED.swap(false, Relaxed)
}

fn inside(h: HWND, lp: LPARAM) -> bool {
    // screen coordinates, signed: a second monitor left of the main one is negative
    let mut pt = POINT { x: lp.0 as i16 as i32, y: (lp.0 >> 16) as i16 as i32 };
    let _ = unsafe { ScreenToClient(h, &mut pt) };
    let r = RECT.each_ref().map(|a| a.load(Relaxed));
    rx_core::caption_hit(r, pt.x, pt.y)
}

fn repaint() {
    if let Some(f) = REPAINT.get() {
        f();
    }
}

fn set_hover(on: bool) {
    if HOVER.swap(on, Relaxed) != on {
        repaint();
    }
}

unsafe extern "system" fn caption(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    let on = wp.0 == HTMAXBUTTON as usize;
    match msg {
        WM_NCHITTEST if inside(h, lp) => return LRESULT(HTMAXBUTTON as isize),
        WM_NCMOUSEMOVE => {
            if on {
                // re-armed at every move, or a fast exit is missed
                let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: h, dwHoverTime: 0 };
                let _ = unsafe { TrackMouseEvent(&mut t) };
            } else {
                PRESSED.store(false, Relaxed);
            }
            set_hover(on);
        }
        WM_NCMOUSELEAVE => {
            PRESSED.store(false, Relaxed);
            set_hover(false);
        }
        // never to DefWindowProc, which would track and draw its own old-style button
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if on => {
            PRESSED.store(true, Relaxed);
            return LRESULT(0);
        }
        WM_NCLBUTTONUP if on => {
            if PRESSED.swap(false, Relaxed) {
                CLICKED.store(true, Relaxed);
                repaint();
            }
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            let _ = unsafe { RemoveWindowSubclass(h, Some(caption), SUBCLASS) };
        }
        _ => {}
    }
    unsafe { DefSubclassProc(h, msg, wp, lp) }
}
