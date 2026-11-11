//! Title bar with menus and file commands, address bar, tab strip and status bar.

use super::*;

/// A 32px text field with an optional leading `lead` label and a clear button (or `trail` while
/// empty). Returns whether the text changed (typed, cleared, Esc) and the field's response. `id`
/// lets a shortcut or button focus it.
fn text_field(ui: &mut Ui, text: &mut String, width: f32, hint: String, lead: Option<&str>, trail: Option<RichText>, id: &str) -> (bool, Response) {
    let (mut changed, mut focus) = (false, false);
    let fr = field_frame().show(ui, |ui| {
        ui.set_width(width - 22.0);
        ui.set_min_height(20.0);
        ui.horizontal(|ui| {
            if let Some(l) = lead {
                ui.label(RichText::new(l).color(pal().dim));
            }
            let r = ui.add(
                egui::TextEdit::singleline(text)
                    .id(egui::Id::unique(id))
                    .frame(egui::Frame::NONE)
                    .margin(Margin::symmetric(2, 0))
                    .hint_text(hint)
                    .desired_width(ui.available_width() - 30.0),
            );
            changed = r.changed();
            focus = r.has_focus();
            // Esc makes the TextEdit give up focus in the same frame, so lost_focus counts too
            if (r.has_focus() || r.lost_focus()) && !text.is_empty() && ui.input(|i| i.key_pressed(Key::Escape)) {
                text.clear();
                changed = true;
            }
            if !text.is_empty() {
                // 20px tall so the field stays 32px
                let b = egui::Button::new(icon(ic::CLOSE, 10.0).color(pal().dim)).frame(false).min_size(vec2(24.0, 20.0));
                if ui.add(b).on_hover_text("Wyczyść (Esc)").clicked() {
                    text.clear();
                    changed = true;
                }
            } else if let Some(t) = trail {
                ui.add_sized(vec2(24.0, 20.0), egui::Label::new(t));
            }
        });
    });
    let rect = fr.response.rect;
    if focus {
        ui.painter().line_segment([rect.left_bottom() + vec2(2.0, -1.0), rect.right_bottom() + vec2(-2.0, -1.0)], Stroke::new(2.0, pal().accent));
    }
    (changed, fr.response)
}

/// A quiet status-bar text that switches a state off when clicked.
fn chip<'a>(ui: &mut Ui, atoms: impl egui::IntoAtoms<'a>, tip: &str) -> bool {
    ui.add(egui::Button::new(atoms).frame(false)).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip).clicked()
}

impl App {
    pub(crate) fn menu_bar(&mut self, ui: &mut Ui) {
        // a title bar of Windows 11 height: menus centred in it, caption buttons fill it
        ui.set_min_height(TITLE_H);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // the caption buttons take their place first, so nothing is ever drawn under them
            let caps = Rect::everything_right_of(ui.max_rect().right() - CAPTION_W).intersect(ui.max_rect());
            let mut cui = ui.new_child(UiBuilder::new().max_rect(caps).layout(Layout::right_to_left(Align::Center)));
            cui.spacing_mut().item_spacing.x = 0.0;
            caption_buttons(&mut cui);
            ui.set_max_width(ui.max_rect().width() - CAPTION_W);
            ui.set_clip_rect(ui.clip_rect().intersect(Rect::everything_left_of(caps.left())));
            ui.add_space(8.0);
            ui.add(egui::Image::new((self.logo.id(), vec2(18.0, 18.0))));
            ui.add_space(8.0);
            let dir = self.tabs[self.cur].path.clone();
            let cur = self.cur;
            let p = &mut self.pending;
            // every top-level entry is a menu_item: the Fluent glyph where the context menus use
            // one, an empty 16px column otherwise, so labels start at one edge in every menu
            let r = ui.menu_button("Plik", |ui| {
                menu_style(ui);
                let groups: [&[(&str, &str, &str, Act)]; 4] = [
                    &[(ic::ADD, "Nowa karta", "Ctrl+T", Act::NewTab(None)), (ic::CLOSE, "Zamknij kartę", "Ctrl+W", Act::CloseTab(cur))],
                    &[(ic::NEW_FOLDER, "Nowy folder", "Ctrl+Shift+N", Act::NewFolder)],
                    &[(ic::STAR, "Dodaj do zakładek", "", Act::AddBookmark(dir.clone())), (ic::TERMINAL, "Otwórz terminal tutaj", "", Act::Terminal)],
                    &[(ic::INFO, "Właściwości", "Alt+Enter", Act::Properties)],
                ];
                for (g, items) in groups.iter().enumerate() {
                    if g > 0 {
                        ui.separator();
                    }
                    for (glyph, l, k, a) in items.iter() {
                        if menu_item(ui, glyph, l, k) {
                            p.push(a.clone());
                        }
                    }
                    // next to the bookmark and terminal entries, which also act on this folder
                    if g == 2 && menu_item(ui, ic::NEW_WINDOW, "Otwórz w Eksploratorze", "") {
                        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                    }
                }
                ui.separator();
                // preferences kept between starts; what changes the view on screen is in Widok
                let title = (egui::AtomExt::atom_size(icon(ic::SETTINGS, 14.0).color(pal().dim), vec2(16.0, 16.0)), RichText::new("Ustawienia"));
                ui.menu_button(title, |ui| {
                    menu_style(ui);
                    if check_item(ui, ic::CHECK, self.tree_full, "Pełne drzewo folderów", "") {
                        self.tree_full = !self.tree_full;
                    }
                    if check_item(ui, ic::CHECK, self.lock_format, "Blokada formatu (zachowaj sortowanie)", "") {
                        self.lock_format = !self.lock_format;
                    }
                    // at sign-in it starts in the tray; an update swaps the EXE in place, so the entry stays right
                    let cmd = std::env::current_exe().ok().map(|e| format!("\"{}\" {}", e.display(), update::TRAY_ARG));
                    let on = cmd.as_ref().is_some_and(|c| sys::run_at_sign_in(APP_NAME).is_some_and(|v| v.eq_ignore_ascii_case(c)));
                    if check_item(ui, ic::CHECK, on, "Uruchamiaj razem z Windows", "") {
                        if let Err(e) = sys::set_run_at_sign_in(APP_NAME, if on { None } else { cmd.as_deref() }) {
                            self.msg = Some(format!("Uruchamianie z Windows: {e}"));
                        }
                    }
                    if menu_item(ui, ic::SHIELD, "Klucz VirusTotal…", "") {
                        p.push(Act::VtKey);
                    }
                });
                if menu_item(ui, ic::HELP, "O programie", "") {
                    self.about = true;
                }
                if menu_entry(ui, ic::POWER, "Zakończ", "").on_hover_text("Kończy działanie programu (zamknięcie okna tylko je minimalizuje)").clicked() {
                    p.push(Act::Exit);
                }
            });
            self.menus[0] = r.response.id;
            let r = ui.menu_button("Edycja", |ui| {
                menu_style(ui);
                // enabled only with something to take back, named after it
                let undo = self.undo.top().map(|u| u.label());
                if ui.add_enabled_ui(undo.is_some(), |ui| menu_item(ui, ic::UNDO, undo.as_deref().unwrap_or("Cofnij"), "Ctrl+Z")).inner {
                    p.push(Act::Undo);
                }
                ui.separator();
                let groups: [&[(&str, &str, &str, Act)]; 4] = [
                    &[(ic::CUT, "Wytnij", "Ctrl+X", Act::Copy(true)), (ic::COPY, "Kopiuj", "Ctrl+C", Act::Copy(false)), (ic::PASTE, "Wklej", "Ctrl+V", Act::Paste)],
                    &[(ic::RENAME, "Zmień nazwę", "F2", Act::Rename), (ic::DELETE, "Usuń", "Delete", Act::Delete)],
                    &[(ic::LINK, "Kopiuj ścieżkę", "Ctrl+Shift+C", Act::CopyPath)],
                    &[(ic::SELECT_ALL, "Zaznacz wszystko", "Ctrl+A", Act::SelectAll), (ic::INVERT, "Odwróć zaznaczenie", "", Act::Invert)],
                ];
                for (g, items) in groups.iter().enumerate() {
                    if g > 0 {
                        ui.separator();
                    }
                    for (glyph, l, k, a) in items.iter() {
                        if menu_item(ui, glyph, l, k) {
                            p.push(a.clone());
                        }
                    }
                }
            });
            self.menus[1] = r.response.id;
            let r = ui.menu_button("Widok", |ui| {
                menu_style(ui);
                // WinUI menu: a bullet marks the view mode, a check mark each switch that is on
                for (m, l) in [(ViewMode::Details, "Szczegóły"), (ViewMode::Thumbs, "Miniatury"), (ViewMode::DetailsThumbs, "Szczegóły z miniaturami")] {
                    if check_item(ui, ic::BULLET, self.view == m, l, "") {
                        self.view = m;
                    }
                }
                ui.separator();
                if check_item(ui, ic::CHECK, self.show_preview, "Panel podglądu", "Alt+P") {
                    self.show_preview = !self.show_preview;
                }
                if check_item(ui, ic::CHECK, self.split, "Widok z podziałem", "F3") {
                    p.push(Act::ToggleSplit);
                }
                if ui.add_enabled_ui(self.split, |ui| menu_item(ui, ic::SWITCH, "Przełącz panel", "Tab")).inner {
                    p.push(Act::SwitchPane);
                }
                if check_item(ui, ic::CHECK, self.tabs[cur].flat, "Widok płaski", "") {
                    p.push(Act::ToggleFlat);
                }
                if check_item(ui, ic::CHECK, self.show_hidden, "Pokaż ukryte pliki", "Ctrl+H") {
                    p.push(Act::ToggleHidden);
                }
                ui.separator();
                if menu_item(ui, ic::FILTER, "Filtr nazw…", "Ctrl+Shift+F") {
                    p.push(Act::FilterNames);
                }
                if menu_item(ui, ic::REFRESH, "Odśwież", "F5") {
                    p.push(Act::Refresh);
                }
                ui.separator();
                if menu_item(ui, ic::ZOOM_IN, "Powiększ", "Ctrl+Plus") {
                    egui::gui_zoom::zoom_in(ui.ctx());
                }
                if menu_item(ui, ic::ZOOM_OUT, "Pomniejsz", "Ctrl+Minus") {
                    egui::gui_zoom::zoom_out(ui.ctx());
                }
                if menu_item(ui, ic::ZOOM_RESET, "Rzeczywisty rozmiar", "Ctrl+0") {
                    ui.ctx().set_zoom_factor(1.0);
                }
                ui.separator();
                let title = (egui::AtomExt::atom_size(icon(ic::THEME, 14.0).color(pal().dim), vec2(16.0, 16.0)), RichText::new("Motyw"));
                ui.menu_button(title, |ui| {
                    menu_style(ui);
                    use egui::ThemePreference as T;
                    for (t, l) in [(T::System, "Systemowy"), (T::Light, "Jasny"), (T::Dark, "Ciemny")] {
                        if check_item(ui, ic::BULLET, self.theme == t, l, "") {
                            self.theme = t;
                            ui.ctx().set_theme(t);
                        }
                    }
                });
            });
            self.menus[2] = r.response.id;
            let r = ui.menu_button("Narzędzia", |ui| {
                menu_style(ui);
                // as WizTree: what takes the disk, biggest first, with the map of tiles below;
                // asks for the scope: the whole drive or the folder on screen
                // the same 16px icon column as menu_item
                let title = (egui::AtomExt::atom_size(icon(ic::PIE, 14.0).color(pal().dim), vec2(16.0, 16.0)), RichText::new("Analiza miejsca (co zajmuje dysk)"));
                ui.menu_button(title, |ui| {
                    menu_style(ui);
                    let here = self.tabs[cur].path.clone();
                    let pc = is_pc(&here);
                    let drive = here.ancestors().last().map(Path::to_path_buf).filter(|_| !pc);
                    // this folder's drive first (unless the folder is that drive), then every other
                    // local drive: the index covers them all, network drives are left out
                    let same = |a: &Path, b: &Path| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy());
                    let others = self.drives.iter().filter(|d| !d.2 && !drive.as_ref().is_some_and(|c| same(c, &d.0))).map(|d| d.0.clone());
                    let drives: Vec<PathBuf> = drive.clone().filter(|d| !same(d, &here)).into_iter().chain(others).collect();
                    for d in drives {
                        let name = d.to_string_lossy().trim_end_matches('\\').to_string();
                        if menu_item(ui, ic::DRIVE, &format!("Cały dysk {name}"), "") {
                            p.push(Act::Goto(d));
                            if !self.space {
                                p.push(Act::ToggleSpace);
                            }
                        }
                    }
                    let label = if pc {
                        "Wszystkie dyski (Ten komputer)".to_string()
                    } else {
                        format!("Bieżący folder: {}", here.file_name().map_or_else(|| here.to_string_lossy(), |n| n.to_string_lossy()))
                    };
                    if menu_item(ui, ic::PIE, &label, "") && !self.space {
                        p.push(Act::ToggleSpace);
                    }
                    if self.space {
                        ui.separator();
                        if menu_item(ui, ic::CLOSE, "Wyłącz analizę miejsca", "") {
                            p.push(Act::ToggleSpace);
                        }
                    }
                });
                ui.separator();
                for (g, l, k, a) in [
                    (ic::CALC, "Oblicz wielkości folderów", "", Act::CalcSizes),
                    (ic::BAK, "Utwórz kopię .bak", "Ctrl+B", Act::MakeBak),
                    (ic::EXTRACT, "Wypakuj archiwum (ZIP, 7z, RAR)", "", Act::Extract),
                    (ic::PACK, "Spakuj… (ZIP lub 7z)", "", Act::Pack),
                ] {
                    if menu_item(ui, g, l, k) {
                        p.push(a);
                    }
                }
            });
            self.menus[3] = r.response.id;
            // the everyday file commands, in order, each only while it fits whole before the caption
            // buttons (all of them are in the menus and the right-click menu too)
            let label = egui::WidgetText::from("Nowy folder").into_galley(ui, None, f32::INFINITY, egui::TextStyle::Button).size().x;
            // 20 + separator (8 with the spacing) + 4 before it, then the 16 pt glyph, gap, label and padding
            let mut room = ui.available_width() >= 32.0 + 16.0 + ui.spacing().icon_spacing + label + 2.0 * ui.spacing().button_padding.x;
            // on Ten komputer there are no files to act on
            let t = &self.tabs[cur];
            let files = !is_pc(&t.path);
            // search results have no folder of their own to create in or paste into (a folder of
            // an archive has: archive.rs writes it)
            let here = files && !t.found;
            let (has_sel, has_clip) = (files && !t.sel.is_empty(), here && (!self.clip.is_empty() || self.clip_files));
            if room {
                ui.add_space(20.0);
                ui.add(egui::Separator::default().shrink(8.0));
                ui.add_space(4.0);
                // the main command keeps a text label, as Explorer's "Nowy"
                let nb = egui::Button::new((icon(ic::NEW_FOLDER, 16.0).color(pal().text), RichText::new("Nowy folder"))).min_size(vec2(0.0, 32.0));
                if ui.add_enabled(here, nb).on_hover_text("Nowy folder (Ctrl+Shift+N)").clicked() {
                    self.pending.push(Act::NewFolder);
                }
            }
            // clusters (clipboard, then rename and delete) so the icons do not read as one strip
            let groups: [&[(&str, bool, &str, Act)]; 2] = [
                &[
                    (ic::CUT, has_sel, "Wytnij (Ctrl+X)", Act::Copy(true)),
                    (ic::COPY, has_sel, "Kopiuj (Ctrl+C)", Act::Copy(false)),
                    (ic::PASTE, has_clip, "Wklej (Ctrl+V)", Act::Paste),
                ],
                // the Edit pencil: Fluent's Rename glyph (E8AC) reads as "A]" at 16px
                &[(ic::RENAME, has_sel, "Zmień nazwę (F2)", Act::Rename), (ic::DELETE, has_sel, "Usuń (Delete)", Act::Delete)],
            ];
            for items in groups {
                // the separator (12 with the spacing) with the first 32 pt button, then each button
                room &= ui.available_width() >= 12.0 + 32.0;
                if !room {
                    break;
                }
                ui.add_space(2.0);
                ui.add(egui::Separator::default().shrink(10.0));
                ui.add_space(2.0);
                for (g, on, tip, a) in items {
                    room &= ui.available_width() >= 32.0;
                    if !room {
                        break;
                    }
                    // disabled in dim (egui fades it to disabled_alpha), so it reads as off, not as lighter ink
                    let b = egui::Button::new(icon(g, 16.0).color(if *on { pal().text } else { pal().dim })).min_size(vec2(32.0, 32.0));
                    let r = ui.add_enabled(*on, b);
                    named(&r, egui::Role::Button, tip);
                    if r.on_hover_text(*tip).clicked() {
                        self.pending.push(a.clone());
                    }
                }
            }
            // Sortuj and Wyświetl drop-downs, where Windows 11 Explorer has them; left out when the
            // room left before the caption buttons is under ~215 px (sorting stays on the column
            // headers, the view on the status bar and the Widok menu)
            if room && ui.available_width() >= 215.0 + 16.0 {
                ui.add_space(2.0);
                ui.add(egui::Separator::default().shrink(10.0));
                ui.add_space(2.0);
                let drop = |glyph: &str, label: &str| {
                    egui::Button::new((icon(glyph, 16.0).color(pal().text), RichText::new(label), icon(ic::CHEV_D, 10.0).color(pal().dim))).min_size(vec2(0.0, 32.0))
                };
                ui.add_enabled_ui(files, |ui| {
                    let (r, _) = egui::containers::menu::MenuButton::from_button(drop(ic::SORT, "Sortuj")).ui(ui, |ui| {
                        menu_style(ui);
                        let t = &mut self.tabs[cur];
                        let was = (t.sort, t.asc);
                        for (c, l) in [(Col::Name, "Nazwa"), (Col::Size, "Rozmiar"), (Col::Type, "Typ"), (Col::Modified, "Zmodyfikowany")] {
                            if check_item(ui, ic::BULLET, t.sort == c, l, "") && t.sort != c {
                                // the direction a column header click starts with
                                t.asc = c == Col::Name || c == Col::Type;
                                t.sort = c;
                                t.dirty = true;
                            }
                        }
                        ui.separator();
                        for (asc, l) in [(true, "Rosnąco"), (false, "Malejąco")] {
                            if check_item(ui, ic::BULLET, t.asc == asc, l, "") {
                                t.asc = asc;
                                t.dirty = true;
                            }
                        }
                        if (t.sort, t.asc) != was {
                            self.remember_sort();
                        }
                    });
                    named(&r, egui::Role::Button, "Sortuj");
                });
                let (r, _) = egui::containers::menu::MenuButton::from_button(drop(ic::GRID, "Wyświetl")).ui(ui, |ui| {
                    menu_style(ui);
                    // Ten komputer has one layout, as the status bar's view buttons show
                    ui.add_enabled_ui(files, |ui| {
                        for (m, l) in [(ViewMode::Details, "Szczegóły"), (ViewMode::Thumbs, "Miniatury"), (ViewMode::DetailsThumbs, "Szczegóły z miniaturami")] {
                            if check_item(ui, ic::BULLET, self.view == m, l, "") {
                                self.view = m;
                            }
                        }
                    });
                    ui.separator();
                    if check_item(ui, ic::CHECK, self.show_preview, "Panel podglądu", "Alt+P") {
                        self.show_preview = !self.show_preview;
                    }
                    if check_item(ui, ic::CHECK, self.split, "Widok z podziałem", "F3") {
                        self.pending.push(Act::ToggleSplit);
                    }
                });
                named(&r, egui::Role::Button, "Wyświetl");
            }
        });
    }

    pub(crate) fn nav_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let t = &self.tabs[self.cur];
            let (b, f, u) = (!t.back.is_empty(), !t.fwd.is_empty(), !is_pc(&t.path));
            if icon_button(ui, ic::BACK, b, "Wstecz (Alt+←)").clicked() {
                self.pending.push(Act::Back);
            }
            if icon_button(ui, ic::FORWARD, f, "Dalej (Alt+→)").clicked() {
                self.pending.push(Act::Forward);
            }
            if icon_button(ui, ic::UP, u, "W górę (Backspace)").clicked() {
                self.pending.push(Act::Up);
            }
            ui.add_space(8.0);
            // the search box, then the funnel that switches it to the name filter (kept across
            // folders, shown as a status-bar chip while set)
            // in a narrow window the box shrinks to 160 pt, then is left out, so the address keeps 200 pt
            let search_w = (ui.available_width() - 82.0 - 200.0).min(300.0);
            let search = search_w >= 160.0;
            let w = if search { ui.available_width() - search_w - 82.0 } else { ui.available_width() };
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
            let edit_mode = self.addr_edit.is_some();
            ui.painter().rect_filled(rect, cr(4.0), pal().field);
            ui.painter().rect_stroke(rect, cr(4.0), Stroke::new(1.0, pal().line), StrokeKind::Inside);
            if edit_mode {
                ui.painter().line_segment([rect.left_bottom() + vec2(2.0, -1.0), rect.right_bottom() + vec2(-2.0, -1.0)], Stroke::new(2.0, pal().accent));
            }
            // refresh at the box's right end, where Windows 11 Explorer has it
            let rr = Rect::from_center_size(pos2(rect.right() - 18.0, rect.center().y), vec2(28.0, 28.0));
            let mut rui = ui.new_child(UiBuilder::new().max_rect(rr));
            let r = rui.add(egui::Button::new(icon(ic::REFRESH, 14.0).color(pal().text)).min_size(vec2(28.0, 28.0)));
            named(&r, egui::Role::Button, "Odśwież");
            if r.on_hover_text("Odśwież (F5)").clicked() {
                self.pending.push(Act::Refresh);
            }
            let body = Rect::from_min_max(rect.min + vec2(8.0, 2.0), pos2(rr.left() - 2.0, rect.bottom() - 2.0));
            // a path longer than the box (as wide as last frame's) starts that much further left, so its
            // end (the folder on screen) stays in view, and is cut at the box
            let wid = ui.scope_id().with("addr_w");
            let over = if edit_mode { 0.0 } else { (ui.data(|d| d.get_temp::<f32>(wid)).unwrap_or(0.0) - body.width()).max(0.0) };
            let mut inner = ui.new_child(UiBuilder::new().max_rect(body.with_min_x(body.min.x - over)).layout(Layout::left_to_right(Align::Center)));
            inner.set_clip_rect(body.intersect(ui.clip_rect()));
            // one path, not scattered words: no gap between segments, 6px padding, 16px chevrons
            inner.spacing_mut().item_spacing.x = 0.0;
            inner.spacing_mut().button_padding = vec2(6.0, 2.0);
            if let Some(text) = &mut self.addr_edit {
                let r = inner.add(egui::TextEdit::singleline(text).frame(egui::Frame::NONE).desired_width(f32::INFINITY));
                // lost_focus first: asked for focus before it, the box took it back from every click
                // elsewhere, so editing never ended and no other field could be typed in
                if r.lost_focus() {
                    if inner.input(|i| i.key_pressed(Key::Enter)) {
                        self.pending.push(Act::Goto(PathBuf::from(text.trim())));
                    }
                    self.addr_edit = None;
                } else {
                    r.request_focus();
                }
            } else {
                let path = self.tabs[self.cur].path.clone();
                let mut chain: Vec<PathBuf> = if is_pc(&path) { vec![] } else { path.ancestors().map(Path::to_path_buf).collect() };
                chain.reverse();
                let (ir, iresp) = inner.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                paint_icon(inner.painter(), ir.shrink(1.0), Kind::Computer);
                if iresp.on_hover_text("Ten komputer").clicked() {
                    self.pending.push(Act::Goto(PathBuf::from(PC)));
                }
                let chev = |ui: &mut Ui| {
                    ui.add_sized(vec2(16.0, 24.0), egui::Label::new(icon(ic::CHEV_R, 12.0).color(pal().dim)));
                };
                // the current folder in semibold, ancestors regular, so "where am I" leads the path
                let here = |s: String| RichText::new(s).family(FontFamily::Name("semibold".into())).color(pal().text);
                if is_pc(&path) {
                    chev(&mut inner);
                    inner.add_space(6.0);
                    inner.label(here("Ten komputer".into()));
                }
                if !chain.is_empty() {
                    chev(&mut inner);
                }
                let last = chain.len().saturating_sub(1);
                for (k, p) in chain.into_iter().enumerate() {
                    let name = self.dname(&p);
                    let text = if k == last { here(name) } else { RichText::new(name).color(pal().text) };
                    let r = inner.add(egui::Button::new(text).min_size(vec2(0.0, 24.0)).sense(Sense::click_and_drag()));
                    // dragged onto Zakładki in the tree, the folder becomes a bookmark (as a browser's address)
                    if r.drag_started_by(egui::PointerButton::Primary) {
                        r.dnd_set_drag_payload(DraggedPath(p.clone()));
                    }
                    if r.clicked() {
                        self.pending.push(Act::Goto(p.clone()));
                    }
                    // the separator after a folder lists its subfolders, as in Explorer; a 16x24
                    // target framed only on hover
                    let b = egui::Button::new(icon(ic::CHEV_R, 12.0).color(pal().dim)).min_size(vec2(16.0, 24.0)).frame_when_inactive(false);
                    inner.spacing_mut().button_padding = vec2(2.0, 2.0);
                    let (r, _) = egui::containers::menu::MenuButton::from_button(b).ui(&mut inner, |ui| {
                        menu_style(ui);
                        let kids = self.kids(&p, ui.ctx());
                        // read on another thread: the menu fills when they come
                        if kids.is_empty() {
                            let read = self.tree_kids.contains_key(&p);
                            ui.add_enabled(false, egui::Button::new(if read { "Brak podfolderów" } else { "Wczytuję…" }));
                        }
                        egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                            for k in kids {
                                let name = self.dname(&k);
                                if cmd(ui, &name, "") {
                                    self.pending.push(Act::Goto(k));
                                }
                            }
                        });
                    });
                    inner.spacing_mut().button_padding = vec2(6.0, 2.0);
                    named(&r, egui::Role::Button, "Podfoldery");
                }
                let used = inner.min_rect().width();
                if ui.data(|d| d.get_temp::<f32>(wid)) != Some(used) {
                    ui.data_mut(|d| d.insert_temp(wid, used));
                    ui.ctx().request_repaint();
                }
                if resp.on_hover_text("Kliknij, aby wpisać ścieżkę (Ctrl+L)").clicked() {
                    self.addr_edit = Some(path.display().to_string());
                }
            }
            if !search {
                return;
            }
            ui.add_space(6.0);
            let path = self.tabs[self.cur].path.clone();
            let name = self.dname(&path);
            // Ten komputer has no files to search or filter
            ui.add_enabled_ui(!is_pc(&path), |ui| {
                let (mode, on, all) = (self.filter_open, !self.tabs[self.cur].filter.is_empty(), self.tabs[self.cur].search_all);
                // the mode shows as a "Filtr:" label inside the box, never by the placeholder alone
                let (changed, r) = if mode {
                    text_field(ui, &mut self.tabs[self.cur].filter, search_w, "*.pdf;*.jpg".into(), Some("Filtr:"), None, "filter")
                } else {
                    let trail = icon(ic::SEARCH, 14.0).color(pal().dim);
                    let hint = if all { "Szukaj na wszystkich dyskach".into() } else { format!("Przeszukaj: {name}") };
                    text_field(ui, &mut self.tabs[self.cur].search, search_w, hint, None, Some(trail), "search")
                };
                let tip = match (mode, all) {
                    (true, _) => "Filtr nazw zostaje przy zmianie folderu",
                    (false, true) => "Nazwy plików na wszystkich dyskach lokalnych; przy kilku słowach muszą wystąpić wszystkie, *.pdf szuka po rozszerzeniu",
                    (false, false) => "Szukaj w bieżącym folderze (Ctrl+F)",
                };
                r.on_hover_text(tip);
                if changed {
                    self.tabs[self.cur].dirty = true;
                }
                // Everything-like search of every local drive; the index is read only while it is on
                let ab = egui::Button::new(icon(ic::DRIVE, 16.0).color(if all { pal().accent } else { pal().text })).min_size(vec2(32.0, 32.0)).selected(all);
                let ar = ui.add(ab);
                ar.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ar.enabled(), all, "Szukaj wszędzie"));
                let atip = if all { "Wróć do szukania w bieżącym folderze" } else { "Szukaj wszędzie: na wszystkich dyskach lokalnych, jak Everything" };
                if ar.on_hover_text(atip).clicked() {
                    let t = &mut self.tabs[self.cur];
                    (t.search_all, t.dirty) = (!all, true);
                    if !all {
                        // read the index now, so results are ready by the time a word is typed
                        self.filter_open = false;
                        self.want_index(ui.ctx());
                    }
                    ui.memory_mut(|m| m.request_focus(egui::Id::unique("search")));
                }
                // a 32px toggle of its own; accent while a filter is set, even with the box on search
                let tip = if mode { "Wróć do wyszukiwania w bieżącym folderze (Ctrl+F)" } else { "Filtr nazw, np. *.pdf;*.jpg; zostaje przy zmianie folderu (Ctrl+Shift+F)" };
                let fb = egui::Button::new(icon(ic::FILTER, 16.0).color(if on { pal().accent } else { pal().text })).min_size(vec2(32.0, 32.0)).selected(mode);
                let fr = ui.add(fb);
                fr.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, fr.enabled(), mode, "Filtr nazw"));
                if fr.on_hover_text(tip).clicked() {
                    self.filter_open = !mode;
                    let id = if mode { "search" } else { "filter" };
                    ui.memory_mut(|m| m.request_focus(egui::Id::unique(id)));
                }
            });
        });
    }

    /// A tab strip. `side` limits it to one pane's tabs; `shown` is the tab that pane shows.
    /// Returns the clicked tab and whether "+" was clicked; the caller decides what that means.
    pub(crate) fn tabs_bar(&mut self, ui: &mut Ui, side: Option<u8>, shown: usize) -> (Option<usize>, bool) {
        let mut clicked = None;
        let mut plus = false;
        let tab_h = 32.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // the hairline under the strip; both callers inset the strip by 8px
            let strip = ui.max_rect().expand2(vec2(8.0, 0.0));
            ui.painter().hline(strip.x_range(), strip.top() + tab_h - 0.5, Stroke::new(1.0, pal().line));
            let ids: Vec<usize> = (0..self.tabs.len()).filter(|&i| side.is_none_or(|s| self.tabs[i].side == s)).collect();
            let n = self.tabs.len();
            for &i in &ids {
                let label = self.tab_label(i);
                let full = self.display_path(&self.tabs[i].path.clone());
                let is_shown = i == shown;
                // the inactive pane's tab is only outlined, so the white card marks the active pane
                let card = if self.inactive_pane { pal().pane } else { pal().content };
                let active = is_shown && !self.inactive_pane;
                // one active-tab look in both layouts: semibold accent label + accent underline
                let (col, font) = if active {
                    (pal().accent, FontId::new(14.0, FontFamily::Name("semibold".into())))
                } else {
                    (pal().dim, FontId::proportional(14.0))
                };
                // Act::CloseTab keeps the app's last tab; a pane's last tab closes (the pane shows another)
                let closable = n > 1;
                let galley = ui.painter().layout_no_wrap(label, font, col);
                // room for × always, so a lone tab looks like a tab and keeps its width
                let close_w = 26.0;
                let size = vec2(galley.size().x + 40.0 + close_w, tab_h);
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click() | Sense::DRAG);
                // dragged by its header, the tab can go to the other pane (or open the split)
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    resp.dnd_set_drag_payload(DraggedTab(i));
                }
                let resp = resp.on_hover_text(if closable { format!("{full}\nŚrodkowy klik zamyka kartę") } else { full });
                let p = ui.painter();
                if is_shown {
                    let top = CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 };
                    p.rect_filled(rect, top, card);
                    p.rect_stroke(rect, top, Stroke::new(1.0, pal().line), StrokeKind::Inside);
                    // open at the bottom, so the tab flows into the list
                    p.hline((rect.left() + 1.0)..=(rect.right() - 1.0), rect.bottom() - 0.5, Stroke::new(1.0, card));
                    // the tab commands go to: the single-pane tab or the active pane's tab
                    if !self.inactive_pane {
                        p.hline((rect.left() + 8.0)..=(rect.right() - 8.0), rect.bottom() - 1.0, Stroke::new(2.0, pal().accent));
                    }
                } else if resp.hovered() {
                    p.rect_filled(rect.shrink(2.0), cr(4.0), shade(10));
                }
                let kind = if is_pc(&self.tabs[i].path) { Kind::Computer } else { Kind::Folder };
                paint_icon(p, Rect::from_min_size(pos2(rect.left() + 10.0, rect.center().y - 8.0), vec2(16.0, 16.0)), kind);
                p.galley(pos2(rect.left() + 32.0, rect.center().y - galley.size().y / 2.0), galley, col);
                let mut closed = false;
                if closable && (active || resp.hovered()) {
                    let c = Rect::from_center_size(pos2(rect.right() - 17.0, rect.center().y), vec2(24.0, 24.0));
                    let hover = ui.rect_contains_pointer(c);
                    if hover {
                        p.rect_filled(c, cr(4.0), shade(14));
                    }
                    p.text(c.center(), Align2::CENTER_CENTER, ic::CLOSE, icon_font(10.0), pal().dim);
                    if resp.clicked() && hover {
                        self.pending.push(Act::CloseTab(i));
                        closed = true;
                    }
                }
                if resp.clicked() && !closed {
                    clicked = Some(i);
                }
                if resp.middle_clicked() {
                    self.pending.push(Act::CloseTab(i));
                }
                let pending = &mut self.pending;
                resp.context_menu(|ui| {
                    menu_style(ui);
                    // through `plus`, so the caller puts the tab in the right pane
                    if menu_item(ui, ic::ADD, "Nowa karta", "Ctrl+T") {
                        plus = true;
                    }
                    if closable && menu_item(ui, ic::CLOSE, "Zamknij kartę", "Ctrl+W") {
                        pending.push(Act::CloseTab(i));
                    }
                });
            }
            let r = ui.add(egui::Button::new(icon(ic::ADD, 12.0).color(pal().dim)).min_size(vec2(32.0, 32.0)));
            named(&r, egui::Role::Button, "Nowa karta");
            if r.on_hover_text("Nowa karta (Ctrl+T)").clicked() {
                plus = true;
            }
        });
        (clicked, plus)
    }

    pub(crate) fn status_bar(&mut self, ui: &mut Ui) {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Small);
        let t = &self.tabs[self.cur];
        let pc = is_pc(&t.path);
        // split view: which pane the commands, the address and this bar belong to
        let pane = if !self.split {
            ""
        } else if t.side == 0 {
            "Lewy panel · "
        } else {
            "Prawy panel · "
        };
        let (mut dirs, mut files, mut sd, mut sf, mut sb, mut tb) = (0, 0, 0, 0, 0u64, 0u64);
        for &i in &t.view {
            let e = &t.all[i];
            let s = t.sel.contains(&e.path);
            if s {
                sb += e.size.unwrap_or(0);
                if e.dir { sd += 1 } else { sf += 1 }
            }
            // the folder's own items; rows of an expanded folder (Analiza miejsca) are inside them
            if e.depth > 0 {
                continue;
            }
            if e.dir {
                dirs += 1;
            } else {
                files += 1;
            }
            tb += e.size.unwrap_or(0);
        }
        let (hidden, flat) = (t.hidden, t.flat);
        let (found, shown, listed, matched, biggest) = (t.found, t.view.len(), t.all.len(), t.found_total, t.biggest);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // the row the counts are centred in, before the taller controls on the right grow it
            let row = ui.max_rect();
            // labels in dim, values in text, as the preview's label/value rows
            let font = egui::TextStyle::Small.resolve(ui.style());
            let mut job = egui::text::LayoutJob::default();
            let mut put = |s: &str, c: Color32| job.append(s, 0.0, egui::TextFormat::simple(font.clone(), c));
            put(pane, pal().dim);
            if pc {
                let drives = self.pc.clone().unwrap_or_default();
                let (free, total) = drives.iter().filter(|d| d.2 != 4).fold((0, 0), |a, d| (a.0 + d.3, a.1 + d.4));
                // the same wording as the folder view's free-space label
                put(&drives.len().to_string(), pal().text);
                put(&format!(" {} · ", dyski(drives.len())), pal().dim);
                put(&fmt_size(free), pal().text);
                put(" wolne z ", pal().dim);
                put(&fmt_size(total), pal().text);
                put(" (lokalnie)", pal().dim);
            } else if found {
                // results everywhere: how many show and how many matched in all; the biggest files:
                // of how many files under the folder
                put(if biggest { "Największe pliki: " } else { "Wyniki: " }, pal().dim);
                put(&group(shown as u64), pal().text);
                if matched > listed {
                    put(" z ", pal().dim);
                    put(&group(matched as u64), pal().text);
                    if biggest {
                        put(" w tym folderze", pal().dim);
                    }
                }
            } else {
                put("Foldery: ", pal().dim);
                put(&dirs.to_string(), pal().text);
                put(" · Pliki: ", pal().dim);
                put(&files.to_string(), pal().text);
                if sd + sf > 0 {
                    put(" · Zaznaczono: ", pal().dim);
                    put(&format!("{} ({})", sd + sf, fmt_size(sb)), pal().text);
                } else {
                    put(" · Razem: ", pal().dim);
                    put(&fmt_size(tb), pal().text);
                }
            }
            // always on screen (the owner's request): how many hidden files are here and one click to
            // show or hide them; red while some are hidden, as in the original design
            let hid = (!pc).then(|| {
                if self.show_hidden {
                    (format!("ukryte widoczne: {hidden}"), pal().accent, "Kliknij, aby znów ukryć ukryte pliki (Ctrl+H)")
                } else if hidden > 0 {
                    (format!("ukrytych: {hidden}"), pal().red, "Kliknij, aby pokazać ukryte pliki (Ctrl+H)")
                } else {
                    ("ukrytych: 0".to_string(), pal().dim, "Brak ukrytych plików; kliknij, aby pokazywać je zawsze (Ctrl+H)")
                }
            });
            // the room the counts and the counter take (10 + spacing before the counter, a frameless button)
            let text_w = ui.painter().layout_job(job.clone()).size().x;
            let hid_w = hid.as_ref().map_or(0.0, |h| 16.0 + ui.painter().layout_no_wrap(h.0.clone(), font.clone(), h.1).size().x);
            // the controls on the right take their place first; the counts get what is left
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                // every view control in one cluster (and in Widok): Explorer's two view buttons
                // (right to left here, so Szczegóły shows first), then the panes. Szczegóły z
                // miniaturami stays in Widok and the list's menu. Kept, greyed, on Ten komputer, so
                // the bar does not jump between views.
                for (g, l, m) in [(ic::GRID, "Miniatury", ViewMode::Thumbs), (ic::DETAILS, "Szczegóły", ViewMode::Details)] {
                    if toggle_icon(ui, g, self.view == m && !pc, !pc, l, l) {
                        self.view = m;
                    }
                }
                // a wide gap, so the pane toggles do not read as more view buttons
                ui.add_space(10.0);
                ui.add(egui::Separator::default().shrink(6.0));
                ui.add_space(10.0);
                // the same filled pill as the view modes; both panes work on Ten komputer too
                if toggle_icon(ui, ic::PREVIEW, self.show_preview, true, "Panel podglądu", "Panel podglądu (Alt+P)") {
                    self.show_preview = !self.show_preview;
                }
                if toggle_icon(ui, ic::SPLIT, self.split, true, "Widok z podziałem", "Widok z podziałem (F3)") {
                    self.pending.push(Act::ToggleSplit);
                }
                ui.add_space(12.0);
                ui.spacing_mut().item_spacing.x = 6.0;
                if let Some(&(_, free, total)) = self.disk.as_ref().filter(|_| !pc) {
                    let label = format!("{} wolne z {}", fmt_size(free), fmt_size(total));
                    // the free space gives way first, before the counts are cut (bar 80, two spacings)
                    let need = 92.0 + ui.painter().layout_no_wrap(label.clone(), font.clone(), pal().text).size().x + text_w + hid_w;
                    if total > 0 && ui.available_width() >= need {
                        let (r, _) = ui.allocate_exact_size(vec2(80.0, 6.0), Sense::hover());
                        let used = 1.0 - free as f32 / total as f32;
                        let p = ui.painter();
                        p.rect_filled(r, cr(3.0), pal().track);
                        p.rect_filled(Rect::from_min_size(r.min, vec2(r.width() * used, r.height())), cr(3.0), list::drive_fill(used));
                        ui.label(label);
                    }
                }
                // states that change the listing, each switched off with one click
                let x = || icon(ic::CLOSE, 8.0).color(pal().dim);
                let t = &mut self.tabs[self.cur];
                if !t.filter.is_empty() && chip(ui, (RichText::new(format!("Filtr: {}", t.filter)).color(pal().dim), x()), "Kliknij, aby wyczyścić filtr") {
                    t.filter.clear();
                    t.dirty = true;
                }
                if self.lock_format && chip(ui, (RichText::new("Blokada formatu").color(pal().dim), x()), "Kliknij, aby wyłączyć blokadę formatu") {
                    self.lock_format = false;
                }
                if flat && chip(ui, (RichText::new("Widok płaski").color(pal().dim), x()), "Kliknij, aby wyłączyć widok płaski") {
                    self.pending.push(Act::ToggleFlat);
                }
                let left = row.with_max_x(ui.available_rect_before_wrap().right());
                ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    // what does not fit is cut here, never drawn under the controls on the right
                    ui.set_clip_rect(ui.clip_rect().intersect(Rect::everything_left_of(left.right())));
                    // the counts end in an ellipsis before the counter gives way
                    job.wrap = egui::text::TextWrapping::truncate_at_width((ui.available_width() - hid_w).max(0.0));
                    ui.label(ui.painter().layout_job(job));
                    if let Some((text, col, tip)) = hid {
                        ui.add_space(10.0);
                        if chip(ui, RichText::new(text).color(col), tip) {
                            self.pending.push(Act::ToggleHidden);
                        }
                    }
                    // file operations: what runs and how far; a click opens the Operacje panel
                    self.ops_indicator(ui);
                    if self.index_busy && self.tabs[self.cur].search_all && !self.space {
                        ui.add_space(16.0);
                        ui.add(egui::Spinner::new().size(12.0));
                        ui.label("Odświeżam indeks dysków…").on_hover_text("Wyniki pochodzą z poprzedniego indeksowania; nowe pliki dojdą po jego zakończeniu");
                    }
                    if let Some(m) = &self.msg {
                        ui.add_space(16.0);
                        ui.add(egui::Label::new(RichText::new(m).color(pal().text)).truncate());
                    }
                });
            });
        });
    }
}
