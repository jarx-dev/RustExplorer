//! A small note of the app's own in the screen's corner, above the clock: a window of Windows'
//! own on a thread of its own, so it shows while the app's window is hidden in the tray and eframe draws nothing.

use std::sync::Mutex;
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// The note on screen now.
struct Shown {
    title: String,
    text: String,
    on_click: Box<dyn Fn() + Send>,
    hwnd: isize,
    scale: f32,
    /// the app's icon from the EXE (0 without one)
    icon: isize,
    /// ground, line, text, dimmed text: LIGHT or DARK
    colors: [u32; 4],
}

static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);

const fn hex(c: [u8; 3]) -> u32 {
    (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}
use rx_core::palette::{DARK as D, LIGHT as L};
// the app's colours: its field, a line, text, dimmed text (the light note's line stays as it was)
const LIGHT: [u32; 4] = [hex(L.field), 0xD9DCE0, hex(L.text), hex(L.dim)];
const DARK: [u32; 4] = [hex(D.field), hex(D.line), hex(D.text), hex(D.dim)];
const W: f32 = 360.0;
const H: f32 = 62.0;
const CLOSE_TIMER: usize = 1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// 0xRRGGBB → Windows' 0x00BBGGRR
fn rgb(c: u32) -> COLORREF {
    ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
}

unsafe fn text(hdc: HDC, s: &str, size: f32, bold: bool, color: u32, r: RECT, scale: f32) {
    let face = wide("Segoe UI");
    let font = CreateFontW(-(size * scale).round() as i32, 0, 0, 0, if bold { 600 } else { 400 }, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr());
    let old = SelectObject(hdc, font);
    SetTextColor(hdc, rgb(color));
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut r = r;
    DrawTextW(hdc, w.as_ptr(), w.len() as i32, &mut r, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX | DT_VCENTER);
    SelectObject(hdc, old);
    DeleteObject(font);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            if let Some(s) = SHOWN.lock().ok().as_ref().and_then(|g| g.as_ref()) {
                let k = s.scale;
                let [bg, line, ink, dim] = s.colors;
                let px = |v: f32| (v * k).round() as i32;
                let mut rc: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut rc);
                let b = CreateSolidBrush(rgb(bg));
                FillRect(hdc, &rc, b);
                DeleteObject(b);
                let pen = CreatePen(PS_SOLID, 1, rgb(line));
                let op = SelectObject(hdc, pen);
                let ob = SelectObject(hdc, GetStockObject(NULL_BRUSH));
                Rectangle(hdc, 0, 0, rc.right, rc.bottom);
                SelectObject(hdc, op);
                SelectObject(hdc, ob);
                DeleteObject(pen);
                let mut left = px(14.0);
                if s.icon != 0 {
                    let d = px(28.0);
                    DrawIconEx(hdc, left, (rc.bottom - d) / 2, s.icon as HICON, d, d, 0, std::ptr::null_mut(), DI_NORMAL);
                    left += d + px(12.0);
                }
                SetBkMode(hdc, TRANSPARENT as i32);
                let right = rc.right - px(30.0);
                text(hdc, &s.title, 13.5, true, ink, RECT { left, top: px(9.0), right, bottom: px(30.0) }, k);
                text(hdc, &s.text, 12.0, false, dim, RECT { left, top: px(31.0), right, bottom: px(52.0) }, k);
                text(hdc, "✕", 11.0, false, dim, RECT { left: rc.right - px(24.0), top: px(4.0), right: rc.right - px(6.0), bottom: px(24.0) }, k);
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_LBUTTONUP => {
            // a click on the note (not on its ✕) brings the app back
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            if let Some(s) = SHOWN.lock().ok().as_ref().and_then(|g| g.as_ref()) {
                let close = x > rc.right - (30.0 * s.scale) as i32 && y < (30.0 * s.scale) as i32;
                if !close {
                    (s.on_click)();
                }
            }
            DestroyWindow(hwnd);
            0
        }
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_HAND));
            1
        }
        WM_TIMER if wp == CLOSE_TIMER => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            if let Ok(mut g) = SHOWN.lock() {
                if g.as_ref().is_some_and(|s| s.hwnd == hwnd as isize) {
                    *g = None;
                }
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Shows the note for `seconds` in the app's light or dark look; a click on it runs `on_click`.
/// A note still on screen is replaced.
pub fn show(title: &str, text: &str, seconds: u32, dark: bool, on_click: impl Fn() + Send + 'static) {
    if let Some(s) = SHOWN.lock().ok().as_ref().and_then(|g| g.as_ref()) {
        unsafe { PostMessageW(s.hwnd as HWND, WM_CLOSE, 0, 0) };
    }
    let (title, text) = (title.to_string(), text.to_string());
    std::thread::spawn(move || unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let class = wide("RustExplorerNote");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_DROPSHADOW,
            lpfnWndProc: Some(wndproc),
            hInstance: inst,
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_HAND),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc); // a second time it fails harmlessly: the class is already there
        let scale = GetDpiForSystem().max(96) as f32 / 96.0;
        let (w, h) = ((W * scale).round() as i32, (H * scale).round() as i32);
        let mut work: RECT = std::mem::zeroed();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work as *mut RECT as *mut std::ffi::c_void, 0);
        let margin = (12.0 * scale) as i32;
        let name = wide(&title);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            name.as_ptr(),
            WS_POPUP,
            work.right - w - margin,
            work.bottom - h - margin,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            inst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        // the icon group build.rs puts into the EXE has the number 1
        let d = (28.0 * scale).round() as i32;
        let icon = LoadImageW(inst, 1 as _, IMAGE_ICON, d, d, 0) as isize;
        if let Ok(mut g) = SHOWN.lock() {
            *g = Some(Shown { title, text, on_click: Box::new(on_click), hwnd: hwnd as isize, scale, icon, colors: if dark { DARK } else { LIGHT } });
        }
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        UpdateWindow(hwnd);
        SetTimer(hwnd, CLOSE_TIMER, seconds * 1000, None);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}
