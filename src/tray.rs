//! The icon next to the clock: a left click shows or hides the window, its menu shows
//! the window or quits. Closing the window hides it there (no taskbar button; minimizes when
//! there is no tray), starting the app again shows it; it ends from the tray menu or
//! "Zakończ" in the Plik menu.

use super::*;

pub(crate) enum TrayMsg {
    Show,
    /// left click on the icon: hides a shown window, shows a hidden or minimized one
    Toggle,
    Quit,
}

pub(crate) fn make_tray(ctx: &egui::Context) -> (Option<tray_icon::TrayIcon>, Receiver<TrayMsg>) {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let (tx, rx) = channel();
    let show = MenuItem::new(format!("Pokaż {APP_NAME}"), true, None);
    let quit = MenuItem::new("Zakończ", true, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[&show, &PredefinedMenuItem::separator(), &quit]);
    let mut builder = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip(APP_NAME).with_menu_on_left_click(false);
    if let Ok(i) = tray_icon::Icon::from_rgba(icon::rgba(32), 32, 32) {
        builder = builder.with_icon(i);
    }
    // no tray (e.g. Explorer not running): the window just minimizes as before
    let tray = builder.build().ok();
    let (quit_id, t2, c2) = (quit.id().clone(), tx.clone(), ctx.clone());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = t2.send(if e.id == quit_id { TrayMsg::Quit } else { TrayMsg::Show });
        c2.request_repaint();
    }));
    let c3 = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            let _ = tx.send(TrayMsg::Toggle);
            c3.request_repaint();
        }
    }));
    (tray, rx)
}

impl App {
    /// Ends the app for good (tray menu, Plik → Zakończ, an update's restart).
    pub(crate) fn quit(&mut self, ctx: &egui::Context) {
        // in update.log, so an app that is gone tells a quit from a crash (panics go there too)
        update::log("app: zakończony (Zakończ lub instalacja aktualizacji)");
        self.quitting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Hides the window to the tray, or minimizes it when there is no tray to bring it back from.
    pub(crate) fn to_tray(&mut self, ctx: &egui::Context) {
        if self._tray.is_some() {
            self.hidden = true;
            sys::window_animations(self.hwnd, false);
            // minimized first: on Windows eframe sees a merely hidden window as visible (winit sends
            // no Occluded there) and would keep drawing it, undoing the memory trim; the order
            // matters, as minimizing a hidden window shows it again
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    }

    /// Tray clicks, a second start of the app, and the window's close (its button, Alt+F4, the
    /// taskbar) turned into hiding to the tray.
    /// Returns whether a tray click was handled (no update is installed in that pass: the person
    /// asked to see the window or to quit, not to have the app restart).
    pub(crate) fn tray_and_close(&mut self, ctx: &egui::Context) -> bool {
        let mut msgs: Vec<TrayMsg> = self.tray_rx.try_iter().collect();
        if SHOW_REQUESTED.swap(false, AO::SeqCst) {
            msgs.push(TrayMsg::Show);
        }
        let clicked = !msgs.is_empty();
        for t in msgs {
            match t {
                TrayMsg::Toggle if !self.hidden && !ctx.input(|i| i.viewport().minimized.unwrap_or(false)) => self.to_tray(ctx),
                TrayMsg::Show | TrayMsg::Toggle => {
                    (self.hidden, self.focus_pending) = (false, true);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayMsg::Quit => self.quit(ctx),
            }
        }
        // a window still minimized cannot take the front, and the restore lands a frame later
        if self.focus_pending && !ctx.input(|i| i.viewport().minimized.unwrap_or(true)) {
            self.focus_pending = false;
            sys::window_animations(self.hwnd, true);
            sys::bring_to_front(self.hwnd);
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.to_tray(ctx);
        }
        clicked
    }

    /// Learns the window's handle once and gives the window its icons straight away.
    pub(crate) fn window_handle(&mut self, frame: &eframe::Frame) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if self.hwnd != 0 {
            return;
        }
        if let Ok(RawWindowHandle::Win32(w)) = frame.window_handle().map(|h| h.as_raw()) {
            self.hwnd = w.hwnd.get();
            sys::set_window_icons(self.hwnd);
            sys::caption::install(self.hwnd, || {
                if let Some(c) = APP_CTX.get() {
                    c.request_repaint();
                }
            });
        }
    }

    /// 3 s after the window is minimized (also to the tray), its pages go back to Windows, so the
    /// memory Task Manager shows drops to about 1 MB; they come back from RAM's standby list when used.
    /// Only while minimized: eframe then draws nothing, whereas a window that is merely unfocused
    /// still paints and would fault the pages straight back in.
    /// From the trim on the process also runs in Windows 11's efficiency mode, until the window is
    /// back or while a job (file operation, packing, index scan) still runs behind it or another
    /// window of the process (Właściwości) is still shown.
    pub(crate) fn trim_when_minimized(&mut self, ctx: &egui::Context) {
        const AFTER: std::time::Duration = std::time::Duration::from_secs(3);
        let away = self.hidden || ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        update::set_hidden(away);
        if !away {
            (self.minimized_since, self.trimmed) = (None, false);
            self.set_eco(false);
            return;
        }
        // a real clock: egui's input time does not move while the window is hidden
        let since = *self.minimized_since.get_or_insert_with(Instant::now);
        if !self.trimmed {
            match AFTER.checked_sub(since.elapsed()) {
                Some(left) if !left.is_zero() => return ctx.request_repaint_after(left),
                _ => {
                    sys::trim_memory();
                    self.trimmed = true;
                }
            }
        }
        // a Properties sheet left open is this process's window too, and it may be counting a folder
        // and a copy or compression picked in Windows' menu runs on the shell's thread here
        let working = self.busy.load(AO::SeqCst) > 0 || INDEX_SCANNING.load(AO::SeqCst) || sys::shows_a_window() || sys::shell_working();
        if working {
            // nothing else wakes a hidden window when a file operation ends or that sheet closes
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        }
        self.set_eco(!working);
    }

    /// eframe stores the layout only after a paint, every 30 s at most, and a window left alone or
    /// out of sight is not painted: after a key press or pointer move the window is painted once
    /// more when that save is due, and a window that leaves the screen (minimized, to the tray)
    /// stores its layout at once. Without it, tabs opened in the last seconds before the window
    /// was left were lost when Windows restarted or the process was ended.
    pub(crate) fn save_soon(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        const DUE: std::time::Duration = std::time::Duration::from_secs(31);
        if ctx.input(|i| !i.events.is_empty()) {
            self.unsaved.get_or_insert_with(Instant::now);
        }
        let Some(since) = self.unsaved else { return };
        if self.hidden || ctx.input(|i| i.viewport().minimized.unwrap_or(false)) {
            // the window's own place is eframe's to store, at its next save
            if let Some(storage) = frame.storage_mut() {
                eframe::App::save(self, storage);
                storage.flush();
            }
            self.unsaved = None;
        } else {
            match DUE.checked_sub(since.elapsed()) {
                Some(left) => ctx.request_repaint_after(left),
                // eframe's save is due at this paint; given up either way, so it never asks twice
                None => self.unsaved = None,
            }
        }
    }

    fn set_eco(&mut self, on: bool) {
        if self.eco != on {
            self.eco = on;
            sys::eco_mode(on);
        }
    }
}

/// Asks for the window from another thread (a second start, a click on the app's note).
pub(crate) fn ask_to_show() {
    SHOW_REQUESTED.store(true, AO::SeqCst);
    wake();
}

/// Has the window's thread run once more, also for a hidden window (from any thread).
pub(crate) fn wake() {
    if let Some(c) = APP_CTX.get() {
        c.request_repaint();
    }
}

/// Set by a second start of the app (see sys::single_instance), which then exits.
pub(crate) static SHOW_REQUESTED: AtomicBool = AtomicBool::new(false);
/// For that start to wake the window, also when it is hidden.
pub(crate) static APP_CTX: OnceLock<egui::Context> = OnceLock::new();

/// eframe's storage key for the window's place (egui-winit WindowSettings)
pub(crate) const WINDOW_KEY: &str = "window";
