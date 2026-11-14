//! The file list: details and thumbnail views, column header, selection, renaming, Ten komputer and split panes.

use super::*;

/// The "Ten komputer" rows of these drives: size, free space and file system asked of each.
pub(crate) fn pc_rows(drives: &[(PathBuf, String, bool)]) -> Vec<PcRow> {
    drives
        .iter()
        .map(|(root, name, _)| {
            let (free, total) = sys::disk_space(root).unwrap_or((0, 0));
            (root.clone(), name.clone(), sys::drive_type(root), free, total, sys::fs_name(root))
        })
        .collect()
}

impl App {
    pub(crate) fn header(&mut self, ui: &mut Ui, x0: f32, name_w: f32, cw: [f32; 4]) {
        let h = 32.0;
        // CLICK without FOCUSABLE here and on rows: Tab must not park focus in the list, or the
        // arrow keys stop moving the selection
        let (rect, bg) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::CLICK);
        columns_menu(&mut self.widths, &bg);
        // the active split pane's labels at full ink, the inactive pane's quiet
        let (ink, mark) = if self.inactive_pane { (pal().dim, pal().dim) } else if self.split { (pal().text, pal().accent) } else { (pal().dim, pal().accent) };
        let cols = [(Col::Name, t!("list.col.name")), (Col::Size, t!("list.col.size")), (Col::Type, t!("list.col.type")), (Col::Modified, t!("list.col.modified")), (Col::Attr, t!("list.col.attrs"))];
        let mut x = x0;
        for (k, (col, label)) in cols.iter().enumerate() {
            let w = if k == 0 { name_w } else { cw[k - 1] };
            if w == 0.0 {
                continue;
            }
            let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(w, h));
            let resp = ui.interact(cell, ui.scope_id().with(("hdr", k)), Sense::CLICK).on_hover_text(t!("list.col.tip"));
            columns_menu(&mut self.widths, &resp);
            let p = ui.painter();
            if resp.hovered() {
                p.rect_filled(cell.shrink(2.0), cr(4.0), shade(8));
            }
            let right = *col == Col::Size;
            let t = &self.tabs[self.cur];
            // Nazwa starts where the names do (after the icon), Rozmiar ends where the sizes do
            let indent = if k > 0 { 8.0 } else if self.view == ViewMode::DetailsThumbs { 54.0 } else { 40.0 };
            let pos = if right { pos2(cell.right() - 14.0, cell.center().y) } else { pos2(cell.left() + indent, cell.center().y) };
            let r = p.text(pos, if right { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER }, label, FontId::proportional(12.0), ink);
            if t.sort == *col {
                // a filled 8x5 triangle: the thin chevron glyph was easy to miss
                let (ax, cy) = (if right { r.left() - 10.0 } else { r.right() + 10.0 }, cell.center().y);
                let (tip, base) = if t.asc { (cy - 2.5, cy + 2.5) } else { (cy + 2.5, cy - 2.5) };
                p.add(egui::Shape::convex_polygon(vec![pos2(ax - 4.0, base), pos2(ax + 4.0, base), pos2(ax, tip)], mark, Stroke::NONE));
            }
            // a 16px divider at every border, as in Explorer, so resizing can be found
            p.line_segment([pos2(cell.right(), cell.center().y - 8.0), pos2(cell.right(), cell.center().y + 8.0)], Stroke::new(1.0, pal().line));
            if resp.clicked() {
                let t = self.tab();
                if t.sort == *col {
                    t.asc = !t.asc;
                } else {
                    t.sort = *col;
                    t.asc = *col == Col::Name || *col == Col::Type;
                }
                t.dirty = true;
                self.remember_sort();
            }
            // every border is a drag handle (Nazwa's too); double-click on Nazwa's lets it fit again
            let handle = Rect::from_center_size(pos2(cell.right(), cell.center().y), vec2(10.0, h));
            let hr = ui.interact(handle, ui.scope_id().with(("resize", k)), Sense::CLICK | Sense::DRAG);
            if hr.hovered() || hr.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if hr.dragged() {
                let dx = hr.drag_delta().x;
                if k == 0 {
                    let from = if self.name_width > 0.0 { self.name_width } else { name_w };
                    self.name_width = (from + dx).max(MIN_NAME_W);
                } else {
                    // the columns fill the pane, so a border only moves if the next shown column
                    // gives or takes the width; the last column just grows
                    let i = k - 1;
                    match (k..4).find(|&j| cw[j] > 0.0) {
                        Some(j) => {
                            let (wi, wj) = (self.widths[i], self.widths[j]);
                            let (lo, hi) = ((50.0 - wi).max(wj - 600.0), (600.0 - wi).min(wj - 50.0));
                            if lo <= hi {
                                let d = dx.clamp(lo, hi);
                                self.widths[i] += d;
                                self.widths[j] -= d;
                            }
                        }
                        None => self.widths[i] = (self.widths[i] + dx).clamp(50.0, 600.0),
                    }
                }
            }
            if k == 0 && hr.double_clicked() {
                self.name_width = 0.0;
            }
            x += w;
        }
        ui.painter().hline(x0..=x, rect.bottom() - 0.5, Stroke::new(1.0, pal().line));
    }

    /// Text width of the longest name in this listing at the list's 14px font, measured once per
    /// listing (a 10 000-file folder would cost a pass every frame).
    fn name_fit(&self, ui: &Ui) -> f32 {
        let t = &self.tabs[self.cur];
        let id = egui::Id::unique(("name_fit", &t.path, t.view.len(), t.all.len()));
        if let Some(w) = ui.data(|d| d.get_temp(id)) {
            return w;
        }
        let font = FontId::proportional(14.0);
        let w = ui.ctx().fonts_mut(|f| {
            let mut w = 0.0f32;
            for &i in &t.view {
                w = w.max(t.all[i].name.chars().map(|c| f.glyph_width(&font, c)).sum());
                // past the column's cap nothing longer matters
                if w >= 520.0 {
                    break;
                }
            }
            w
        });
        ui.data_mut(|d| d.insert_temp(id, w));
        w
    }

    /// Click, double-click and context menu handling shared by list rows and grid tiles.
    pub(crate) fn item_interact(&mut self, resp: &Response, vi: usize, any_clicked: &mut bool) {
        let mods = resp.ctx.input(|i| i.modifiers);
        // the entry's own flag: path.is_dir() here ran for every row in every frame, a server round trip each on a network drive
        let (path, is_dir, attrs) = {
            let t = &self.tabs[self.cur];
            let e = &t.all[t.view[vi]];
            (e.path.clone(), e.dir, e.attrs)
        };
        if resp.clicked() {
            *any_clicked = true;
            self.kbd_focus = false;
            self.tree_focus = None;
            if self.rename.as_ref().is_some_and(|r| r.path != path) {
                self.commit_rename();
            }
            select_at(self.tab(), vi, mods.shift, mods.ctrl);
        }
        if resp.double_clicked() {
            self.pending.push(Act::OpenPath(path.clone()));
        }
        if resp.secondary_clicked() {
            *any_clicked = true;
            let t = self.tab();
            if !t.sel.contains(&path) {
                select_at(t, vi, false, false);
            }
        }
        let is_archive = rx_archive::can_extract(&path);
        // the cloud's own sharing link, for OneDrive and Google Drive (others have no way in)
        let link = self.cloud_link(&path).filter(|_| !archive::is_inside(&path));
        // anything in a cloud folder can be kept here or freed; a folder is offered both, a file
        // what it is not yet. Windows hides the placeholder's reparse flag from this program, so
        // the folder is all there is to go by
        let placeholder = self.in_cloud(&path) && !archive::is_inside(&path);
        let state = (!is_dir).then(|| cloud_of(attrs));
        let kbd = self.want_menu.as_ref() == Some(&path);
        // Shift+right click: Windows' own menu straight away, as Explorer's classic one (an entry of
        // an archive is no file the shell knows; asked only on the click, as is_inside asks the disk)
        if mods.shift && resp.secondary_clicked() && !archive::is_inside(&path) {
            egui::Popup::close_id(&resp.ctx, egui::Popup::default_response_id(resp));
            self.pending.push(Act::ShellMenu(path, resp.interact_pointer_pos()));
            return;
        }
        let popup = self.popup_for(resp, kbd, resp.rect.left_bottom() + vec2(32.0, 0.0));
        // opened from the keyboard: Windows' menu comes where this one is, not at a pointer far away
        let kbd_at = self.menu_at.filter(|m| m.0 == egui::Popup::default_response_id(resp)).map(|m| m.1);
        let pending = &mut self.pending;
        let has_clip = !self.clip.is_empty() || self.clip_files;
        popup.show(|ui| {
            menu_style(ui);
            // only while the menu is open: is_inside asks the disk
            let own = !archive::is_inside(&path);
            let mut item = |ui: &mut Ui, glyph: &str, label: &str, key: &str, a: Act| {
                if menu_item(ui, glyph, label, key) {
                    pending.push(a);
                }
            };
            item(ui, ic::OPEN, t!("bars.cmd.open"), "Enter", Act::Open);
            // an entry of an archive has no file of its own for another program to open
            if !is_dir && own {
                item(ui, ic::OPEN_WITH, t!("list.menu.open_with"), "", Act::OpenWith(path.clone()));
            }
            if is_dir {
                item(ui, ic::NEW_WINDOW, t!("bars.cmd.open_new_tab"), "", Act::NewTab(Some(path.clone())));
                item(ui, ic::STAR, t!("bars.cmd.add_bookmark"), "", Act::AddBookmark(path.clone()));
            }
            ui.separator();
            item(ui, ic::CUT, t!("bars.cmd.cut"), "Ctrl+X", Act::Copy(true));
            item(ui, ic::COPY, t!("bars.cmd.copy"), "Ctrl+C", Act::Copy(false));
            if has_clip {
                item(ui, ic::PASTE, t!("bars.cmd.paste"), "Ctrl+V", Act::Paste);
            }
            item(ui, ic::RENAME, t!("bars.cmd.rename"), "F2", Act::Rename);
            item(ui, ic::DELETE, t!("bars.cmd.delete"), "Delete", Act::Delete);
            ui.separator();
            if !is_dir {
                item(ui, ic::BAK, t!("bars.cmd.make_bak"), "Ctrl+B", Act::MakeBak);
                if own {
                    // shown greyed out without a key, so people learn the check exists
                    let has_key = sys::secret(crate::scan::KEY_NAME).is_some();
                    ui.add_enabled_ui(has_key, |ui| item(ui, ic::SHIELD, t!("scan.check"), "", Act::VtCheck(path.clone()))).response.on_disabled_hover_text(t!("scan.no_key"));
                }
            }
            if is_archive {
                item(ui, ic::EXTRACT, t!("list.menu.extract_to"), "", Act::Extract);
                // inside an archive both go next to it; here only beside the archive on disk
                if !self.tabs[self.cur].in_archive() {
                    item(ui, ic::EXTRACT, t!("list.menu.extract_here"), "", Act::ExtractHere);
                }
            }
            // search results have no folder of their own to put the archive in
            if !self.tabs[self.cur].found {
                item(ui, ic::PACK, t!("list.menu.pack"), "", Act::Pack);
            }
            item(ui, ic::LINK, t!("bars.cmd.copy_path"), "Ctrl+Shift+C", Act::CopyPath);
            // the cloud's commands in a submenu of their own, named after the service where it is known
            if link.is_some() || placeholder {
                let title = (egui::AtomExt::atom_size(icon(ic::CLOUD, 14.0).color(pal().dim), vec2(16.0, 16.0)), RichText::new(link.unwrap_or(t!("list.menu.cloud"))));
                ui.menu_button(title, |ui| {
                    menu_style(ui);
                    if link.is_some() {
                        item(ui, ic::LINK, t!("list.menu.copy_link"), "", Act::CloudLink(path.clone()));
                    }
                    if placeholder && state != Some(Cloud::Pinned) {
                        item(ui, ic::CHECK, t!("list.menu.keep"), "", Act::CloudKeep(true));
                    }
                    if placeholder && state != Some(Cloud::Online) {
                        item(ui, ic::CLOUD, t!("list.menu.free"), "", Act::CloudKeep(false));
                    }
                });
            }
            item(ui, ic::INFO, t!("bars.cmd.properties"), "Alt+Enter", Act::Properties);
            // an entry of an archive is no file the shell knows
            if own {
                ui.separator();
                let at = kbd_at.or_else(|| ui.ctx().pointer_latest_pos());
                item(ui, ic::MORE, t!("list.menu.more"), t!("list.menu.more_key"), Act::ShellMenu(path.clone(), at));
            }
        });
    }

    /// A right-click menu for `resp` that Shift+F10 or the Menu key can also open (`open`), then
    /// shown at `at`, since a menu opened from the keyboard has no pointer position to sit at.
    pub(crate) fn popup_for(&mut self, resp: &Response, open: bool, at: egui::Pos2) -> egui::Popup<'static> {
        let id = egui::Popup::default_response_id(resp);
        if open {
            self.menu_at = Some((id, at));
            egui::Popup::open_id(&resp.ctx, id);
        }
        let mut popup = egui::Popup::context_menu(resp);
        if let Some((m, at)) = self.menu_at.filter(|(m, _)| *m == id) {
            if egui::Popup::is_id_open(&resp.ctx, m) && !resp.secondary_clicked() {
                popup = popup.at_position(at);
            } else {
                self.menu_at = None;
            }
        }
        popup
    }

    /// Right-click on empty list space: what Explorer offers there.
    fn list_menu(&mut self, bg: &Response) {
        let has_clip = !self.clip.is_empty() || self.clip_files;
        let dir = self.tabs[self.cur].path.clone();
        let kbd = self.want_menu.as_ref() == Some(&dir);
        let popup = self.popup_for(bg, kbd, bg.rect.left_top() + vec2(24.0, 40.0));
        let pending = &mut self.pending;
        let mut view = None;
        let cur_view = self.view;
        popup.show(|ui| {
            menu_style(ui);
            let mut item = |ui: &mut Ui, glyph: &str, label: &str, key: &str, a: Act| {
                if menu_item(ui, glyph, label, key) {
                    pending.push(a);
                }
            };
            item(ui, ic::NEW_FOLDER, t!("bars.cmd.new_folder"), "Ctrl+Shift+N", Act::NewFolder);
            if has_clip {
                item(ui, ic::PASTE, t!("bars.cmd.paste"), "Ctrl+V", Act::Paste);
            }
            item(ui, ic::REFRESH, t!("bars.cmd.refresh"), "F5", Act::Refresh);
            item(ui, ic::SELECT_ALL, t!("bars.cmd.select_all"), "Ctrl+A", Act::SelectAll);
            item(ui, ic::FILTER, t!("bars.cmd.name_filter"), "Ctrl+Shift+F", Act::FilterNames);
            ui.separator();
            item(ui, ic::STAR, t!("bars.cmd.add_bookmark"), "", Act::AddBookmark(dir.clone()));
            item(ui, ic::TERMINAL, t!("bars.cmd.terminal"), "", Act::Terminal);
            ui.separator();
            for (m, l) in [(ViewMode::Details, t!("bars.view.details")), (ViewMode::Thumbs, t!("bars.view.thumbs")), (ViewMode::DetailsThumbs, t!("bars.view.details_thumbs"))] {
                if check_item(ui, ic::BULLET, cur_view == m, l, "") {
                    view = Some(m);
                }
            }
        });
        if let Some(m) = view {
            self.view = m;
        }
    }

    /// "Analiza miejsca" above the list: what the folder holds in all, whether sizes are still being
    /// counted, the biggest files (or the folder again) and a way out of the mode.
    fn space_strip(&mut self, ui: &mut Ui) {
        let t = &self.tabs[self.cur];
        // the folder's own items: the tree's deeper rows are inside them already
        let total: u64 = t.view.iter().map(|&i| &t.all[i]).filter(|e| e.depth == 0).map(|e| e.size.unwrap_or(0)).sum();
        // hidden items count in the map, not in this list unless shown (pagefile.sys can be GBs)
        let hidden: u64 = if self.show_hidden { 0 } else { t.all.iter().filter(|e| e.depth == 0 && e.attrs & 2 != 0).map(|e| e.size.unwrap_or(0)).sum() };
        let (biggest, found) = (t.biggest, t.found);
        let counting = self.index_busy || !self.sizing.is_empty();
        let strip = egui::Frame::new().fill(pal().pane).inner_margin(Margin { left: 12, right: 8, top: 4, bottom: 4 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(icon(ic::PIE, 16.0).color(pal().accent));
                ui.label(RichText::new(t!("list.space.title")).family(FontFamily::Name("semibold".into())));
                if biggest {
                    ui.label(RichText::new(t!("list.space.biggest_here")).color(pal().dim));
                } else if !found {
                    ui.label(RichText::new(t!("list.space.this_folder")).color(pal().dim));
                    ui.label(fmt_size(total));
                    if hidden >= 1 << 20 {
                        let r = ui.add(egui::Button::new(RichText::new(t!("list.space.hidden", size = fmt_size(hidden))).color(pal().red)).frame(false));
                        if r.on_hover_text(t!("list.space.hidden_tip")).clicked() {
                            self.pending.push(Act::ToggleHidden);
                        }
                    }
                }
                if counting {
                    // no spinner: it would redraw the window (and the map) every frame of a rescan
                    ui.label(icon(ic::REFRESH, 12.0).color(pal().dim));
                    ui.label(RichText::new(t!("list.space.counting")).color(pal().dim)).on_hover_text(t!("list.space.counting_tip"));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let r = ui.add(egui::Button::new(icon(ic::CLOSE, 12.0).color(pal().text)).min_size(vec2(28.0, 28.0)));
                    named(&r, egui::Role::Button, t!("bars.cmd.space_off"));
                    if r.on_hover_text(t!("bars.cmd.space_off")).clicked() {
                        self.pending.push(Act::ToggleSpace);
                    }
                    // search results are no folder to look into
                    if biggest || !found {
                        let (glyph, label, tip) = if biggest {
                            (ic::BACK, t!("list.space.back"), t!("list.space.back_tip"))
                        } else {
                            (ic::SORT, t!("list.space.biggest"), t!("list.space.biggest_tip"))
                        };
                        let b = egui::Button::new((icon(glyph, 14.0).color(pal().text), RichText::new(label))).fill(pal().field).stroke(Stroke::new(1.0, pal().line)).min_size(vec2(0.0, 28.0));
                        if ui.add(b).on_hover_text(tip).clicked() {
                            self.pending.push(Act::Biggest);
                        }
                    }
                });
            });
        });
        let r = strip.response.rect;
        ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, pal().line));
    }

    pub(crate) fn file_list(&mut self, ui: &mut Ui) {
        if is_pc(&self.tabs[self.cur].path) {
            self.computer_view(ui);
            return;
        }
        if let Some(e) = &self.tabs[self.cur].error {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(t!("list.cant_open", error = e)).color(pal().red)));
            return;
        }
        if self.space {
            self.space_strip(ui);
        }
        let list_rect = ui.available_rect_before_wrap();
        // registered before the rows, so the rows win the hit test
        let bg = ui.interact(list_rect, ui.scope_id().with(("list_bg", self.cur)), Sense::CLICK);
        self.list_menu(&bg);
        let mut any_clicked = false;
        self.drop_on_row = false;
        match self.view {
            ViewMode::Thumbs => self.grid(ui, &mut any_clicked),
            _ => self.details(ui, &mut any_clicked),
        }
        // dropped on the other pane's list but not on a folder row: into the folder that pane shows
        // (its own list is no target: in flat view a slip would move nested files up), unless the
        // items are already there
        // files from another program take the same way, by where the pointer is
        let ext = self.ext.as_ref().filter(|x| list_rect.contains(x.0)).cloned();
        if let Some(d) = bg.dnd_hover_payload::<Dragged>().or_else(|| ext.as_ref().map(|x| x.1.clone())).filter(|d| !self.drop_on_row && d.1 != self.cur) {
            let t = &self.tabs[self.cur];
            let into = t.path.clone();
            // shortcut: no drop into an archive or a results list; Kopiuj and Wklej do those
            if !t.found && !t.in_archive() && d.0.iter().all(|p| p.parent() != Some(into.as_path()) && !into.starts_with(p)) {
                let mv = drop_moves_now(&d, &into, ext.is_some(), ui.input(|i| i.modifiers));
                ui.painter().rect(list_rect.shrink(1.0), cr(4.0), pal().accent.gamma_multiply(0.08), Stroke::new(2.0, pal().accent), StrokeKind::Inside);
                self.drop_target = Some((into.clone(), mv));
                if let Some(d) = bg.dnd_release_payload::<Dragged>().or_else(|| ext.filter(|x| x.2).map(|x| x.1)) {
                    self.ext = None;
                    self.pending.push(Act::Drop(d.0.clone(), into, self.cur, mv));
                }
            }
        }
        // clicking empty space clears the selection; a click on a dialog above the list (its
        // layer on top there) is not one, or Spakuj's own button emptied what it was to pack
        let layer = ui.layer_id();
        // the layer is asked outside ui.input(): another Context call inside its closure deadlocks
        let click = ui.input(|i| if i.pointer.primary_clicked() { i.pointer.interact_pos() } else { None });
        let empty_click = click.is_some_and(|p| list_rect.contains(p) && ui.ctx().layer_id_at(p).is_none_or(|l| l == layer));
        if empty_click && !any_clicked && !ui.ctx().any_popup_open() {
            self.tree_focus = None;
            let t = self.tab();
            t.sel.clear();
            t.focus = None;
            if self.rename.is_some() {
                self.commit_rename();
            }
        }
        if self.tabs[self.cur].view.is_empty() {
            let t = &self.tabs[self.cur];
            // said only after a moment, and by one repaint asked for then: a folder on this
            // computer is read before anyone could see the words flash
            let wait = std::time::Duration::from_millis(300).saturating_sub(t.asked_at.elapsed());
            if t.loading() && !wait.is_zero() {
                ui.ctx().request_repaint_after(wait);
                return;
            }
            let text = if t.loading() {
                t!("bars.cmd.loading")
            } else if t.found && self.index.is_none() && self.index_busy {
                t!("list.indexing")
            } else if t.search.is_empty() && t.filter.is_empty() {
                t!("list.empty")
            } else {
                t!("list.no_match")
            };
            ui.painter().text(list_rect.center_top() + vec2(0.0, 80.0), Align2::CENTER_CENTER, text, FontId::proportional(14.0), pal().dim);
        }
    }

    /// "Ten komputer": drives grouped by kind with size, free space, a fill bar and the file system.
    pub(crate) fn computer_view(&mut self, ui: &mut Ui) {
        // sizes and free space are asked on a thread of its own, this computer's drives first (a
        // network drive that does not answer takes seconds); the rows read before stay until
        // the new ones come (Msg::Pc)
        if !std::mem::replace(&mut self.pc_asked, true) {
            let (drives, tx, ctx) = (self.drives.clone(), self.tx.clone(), ui.ctx().clone());
            std::thread::spawn(move || {
                let (net, local): (Vec<_>, Vec<_>) = drives.into_iter().partition(|d| d.2);
                for part in [local, net] {
                    let _ = tx.send(Msg::Pc(pc_rows(&part)));
                    ctx.request_repaint();
                }
            });
        }
        let drives = self.pc.clone().unwrap_or_default();
        // column widths: name, size, type, free, fill bar, file system; Nazwa takes the rest, as in
        // the details view, so the header and the group lines end at the same right edge
        let x0 = ui.min_rect().left() + 4.0;
        let rest = [80.0, 130.0, 100.0, 170.0, 100.0];
        let name_w = (ui.available_width() - 8.0 - rest.iter().sum::<f32>()).max(200.0);
        let w = [name_w, rest[0], rest[1], rest[2], rest[3], rest[4]];
        let right_edge = x0 + w.iter().sum::<f32>();
        let h = 32.0;
        let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        let ink = if self.split && !self.inactive_pane { pal().text } else { pal().dim };
        let mut x = x0;
        for (k, label) in [t!("list.col.name"), t!("preview.capacity"), t!("list.col.type"), t!("list.pc.free"), t!("list.pc.used"), t!("list.pc.fs")].iter().enumerate() {
            let cell = Rect::from_min_size(pos2(x, hr.top()), vec2(w[k], h));
            let right = k == 1 || k == 3 || k == 5;
            // Nazwa starts where the names do, the bar column's label where the bars do; the
            // capacity ends 14px short, as Rozmiar does in the file list
            let indent = match k {
                0 => 40.0,
                4 => 12.0,
                _ => 8.0,
            };
            let pad_r = if k == 1 { 14.0 } else { 8.0 };
            let pos = if right { pos2(cell.right() - pad_r, cell.center().y) } else { pos2(cell.left() + indent, cell.center().y) };
            // no column borders: this header neither sorts nor resizes, so it must not look like it does
            ui.painter().text(pos, if right { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER }, *label, FontId::proportional(12.0), ink);
            x += w[k];
        }
        ui.painter().hline(x0..=x, hr.bottom() - 0.5, Stroke::new(1.0, pal().line));
        let groups: [(&[u32], &str, &str); 4] = [
            (&[3], t!("list.pc.hard"), t!("list.pc.hard_type")),
            (&[2], t!("list.pc.removable"), t!("list.pc.removable_type")),
            (&[5], t!("list.pc.optical"), t!("list.pc.optical_type")),
            (&[4], t!("list.pc.network"), t!("list.pc.network_type")),
        ];
        egui::ScrollArea::vertical().id_salt(("pc", self.cur)).auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (types, title, type_label) in groups {
                let rows: Vec<_> = drives.iter().filter(|d| types.contains(&d.2)).collect();
                if rows.is_empty() {
                    continue;
                }
                ui.add_space(8.0);
                let (gr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
                let p = ui.painter();
                // a plain group title: a chevron would promise collapsing that is not there
                let r = p.text(pos2(x0 + 8.0, gr.center().y), Align2::LEFT_CENTER, title, FontId::new(14.0, FontFamily::Name("semibold".into())), pal().text);
                let r = p.text(pos2(r.right() + 6.0, gr.center().y), Align2::LEFT_CENTER, format!("({})", rows.len()), FontId::proportional(14.0), pal().dim);
                p.line_segment([pos2(r.right() + 8.0, gr.center().y), pos2(right_edge, gr.center().y)], Stroke::new(1.0, pal().line));
                for (root, name, _, free, total, fs) in rows {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::CLICK);
                    named(&resp, egui::Role::ListItem, name);
                    let p = ui.painter();
                    let row = Rect::from_min_max(pos2(x0, rect.top() + 1.0), pos2(right_edge, rect.bottom() - 1.0));
                    // one click selects a drive (Enter or Alt+Enter then act on it), as in the file list
                    let t = &self.tabs[self.cur];
                    if t.focus.as_ref() == Some(root) {
                        let (fill, mark) = if self.inactive_pane { (pal().nav_sel, pal().dim) } else { (pal().sel, pal().accent) };
                        p.rect_filled(row, cr(4.0), fill);
                        sel_pill(p, row, mark);
                    } else if resp.hovered() {
                        p.rect_filled(row, cr(4.0), shade(10));
                    }
                    if resp.clicked() || resp.secondary_clicked() {
                        let t = self.tab();
                        t.sel = [root.clone()].into();
                        t.focus = Some(root.clone());
                    }
                    let cy = rect.center().y;
                    let kind = if types.contains(&4) { Kind::NetDrive } else { Kind::Drive };
                    // icon and name at the file list's x
                    paint_icon(p, Rect::from_min_size(pos2(x0 + 14.0, cy - 10.0), vec2(20.0, 20.0)), kind);
                    let font = FontId::proportional(14.0);
                    let clip = p.with_clip_rect(Rect::from_min_max(pos2(x0 + 40.0, rect.top()), pos2(x0 + w[0] - 6.0, rect.bottom())));
                    clip.text(pos2(x0 + 40.0, cy), Align2::LEFT_CENTER, name, font.clone(), pal().text);
                    let mut x = x0 + w[0];
                    // the name leads; the rest is secondary (dim), as in the details view
                    let size_txt = if *total > 0 { fmt_size(*total) } else { String::new() };
                    p.text(pos2(x + w[1] - 14.0, cy), Align2::RIGHT_CENTER, size_txt, font.clone(), pal().dim);
                    x += w[1];
                    p.text(pos2(x + 8.0, cy), Align2::LEFT_CENTER, type_label, font.clone(), pal().dim);
                    x += w[2];
                    if *total > 0 {
                        p.text(pos2(x + w[3] - 8.0, cy), Align2::RIGHT_CENTER, fmt_size(*free), font.clone(), pal().dim);
                        // a thin bar like the status bar's, 12px clear of the free space, with the share as text
                        let bar = Rect::from_min_size(pos2(x + w[3] + 12.0, cy - 2.0), vec2(w[4] - 60.0, 4.0));
                        let used = 1.0 - *free as f32 / *total as f32;
                        p.rect_filled(bar, cr(2.0), pal().track);
                        p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * used, bar.height())), cr(2.0), drive_fill(used));
                        p.text(pos2(x + w[3] + w[4] - 8.0, cy), Align2::RIGHT_CENTER, format!("{:.0}\u{00A0}%", used * 100.0), font.clone(), pal().dim);
                    }
                    x += w[3] + w[4];
                    p.text(pos2(x + w[5] - 8.0, cy), Align2::RIGHT_CENTER, fs, font.clone(), pal().dim);
                    if resp.double_clicked() {
                        self.pending.push(Act::Goto(root.clone()));
                    }
                    let r = root.clone();
                    let kbd = self.want_menu.as_ref() == Some(root);
                    let popup = self.popup_for(&resp, kbd, rect.left_bottom() + vec2(32.0, 0.0));
                    let pending = &mut self.pending;
                    popup.show(|ui| {
                        menu_style(ui);
                        if menu_item(ui, ic::OPEN, t!("bars.cmd.open"), "") {
                            pending.push(Act::Goto(r.clone()));
                        }
                        if menu_item(ui, ic::NEW_WINDOW, t!("bars.cmd.open_new_tab"), "") {
                            pending.push(Act::NewTab(Some(r.clone())));
                        }
                    });
                }
            }
        });
    }

    /// The scroll offset that shows the focused item (else item `scroll_to`) in a viewport of
    /// `height`, measured in this frame: the least move for a nearby target (arrow keys), centred
    /// and snapped to whole rows for a jump (start with --select, Up to the folder just left).
    /// `scroll_to` waits while the viewport has no height yet or the listing is not ready.
    /// `row_h` must be the pitch show_rows uses, so callers zero item_spacing.y first.
    pub(crate) fn scroll_offset(&mut self, row_h: f32, per_row: usize, height: f32) -> Option<f32> {
        let t = &mut self.tabs[self.cur];
        if height <= 0.0 || t.dirty || (t.focus.is_some() && t.view.is_empty()) {
            return None;
        }
        let missing = t.focus.as_ref().is_some_and(|f| !t.view.iter().any(|&k| &t.all[k].path == f));
        // Analiza miejsca: a revealed file deep in the tree gets its row once the sizes are in
        if self.space && t.sized.is_none() && missing {
            return None;
        }
        let i = t.scroll_to.take()?;
        // by path, so a view sorted or filtered since the index was set still finds the row
        let i = t.focus.as_ref().and_then(|f| t.view.iter().position(|&k| &t.all[k].path == f)).unwrap_or(i);
        let top = (i / per_row.max(1)) as f32 * row_h;
        let (off, seen) = t.scroll;
        if seen > 0.0 && top >= off && top + row_h <= off + height {
            return None;
        }
        if seen > 0.0 && top + row_h > off - height && top < off + 2.0 * height {
            return Some(if top < off { top } else { top + row_h - height });
        }
        Some(((top - (height - row_h) / 2.0) / row_h).round().max(0.0) * row_h)
    }

    pub(crate) fn details(&mut self, ui: &mut Ui, any_clicked: &mut bool) {
        let thumbs = self.view == ViewMode::DetailsThumbs;
        let row_h = if thumbs { 40.0 } else { 28.0 };
        let x0 = ui.min_rect().left() + 4.0;
        let pane = ui.available_width() - 18.0;
        let (mut cw, fill) = col_widths(self.widths, pane);
        // by default Nazwa fits its longest name (icon + text + 48px), so sizes sit near the names,
        // and the spare width goes 60/40 to Nazwa (at most 60% of the pane) and Typ, so the columns
        // still end at the pane edge; split panes give it all to Nazwa, so both panes' columns line
        // up. A width set by hand wins (the spare then all goes to Typ); never wider than the space left
        let fit = (self.name_fit(ui) + if thumbs { 54.0 } else { 40.0 } + 48.0).clamp(280.0, 520.0);
        // search results show their folder after the name, so Nazwa takes the room
        let auto = if self.split || self.tabs[self.cur].found { fill } else { fit };
        let mut name_w = if self.name_width > 0.0 { self.name_width } else { auto }.min(fill);
        let spare = fill - name_w;
        let to_name = if self.name_width > 0.0 {
            0.0
        } else if cw[1] == 0.0 {
            spare
        } else {
            (spare * 0.6).min((pane * 0.6 - name_w).max(0.0))
        };
        name_w += to_name;
        if cw[1] > 0.0 {
            cw[1] += spare - to_name;
        }
        self.header(ui, x0, name_w, cw);
        let n = self.tabs[self.cur].view.len();
        // show_rows steps by row_h + item_spacing.y; with 0 its rows land where scroll_offset expects
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut area = egui::ScrollArea::vertical().id_salt(("list", self.cur)).auto_shrink(false);
        if let Some(o) = self.scroll_offset(row_h, 1, ui.available_height()) {
            area = area.vertical_scroll_offset(o);
        }
        let (sel_fill, sel_mark) = if self.inactive_pane { (pal().nav_sel, pal().dim) } else { (pal().sel, pal().accent) };
        let caret0 = self.caret_at_top(ui.ctx());
        // "Analiza miejsca": the folder's total and its biggest item, for the rows' share bars (a
        // folder's listing, not a results list), counted on the folder's own items (the tree's
        // deeper rows are inside them)
        let tree = self.space && !self.tabs[self.cur].found;
        let space_total = {
            let t = &self.tabs[self.cur];
            let sizes = || t.view.iter().map(|&i| &t.all[i]).filter(|e| e.depth == 0).map(|e| e.size.unwrap_or(0));
            tree.then(|| (sizes().sum::<u64>(), sizes().max().unwrap_or(0))).filter(|&(s, m)| s > 0 && m > 0)
        };
        let out = area.show_rows(ui, row_h, n, |ui, range| {
            for vi in range {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::CLICK | Sense::DRAG);
                self.item_interact(&resp, vi, any_clicked);
                let t = &self.tabs[self.cur];
                let e = t.all[t.view[vi]].clone();
                let selected = t.sel.contains(&e.path);
                let cloud = self.cloud_mark(&e);
                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListItem, true, selected, item_label(&e, cloud)));
                let focused = t.focus.as_ref() == Some(&e.path);
                let cut = self.cut && self.clip.contains(&e.path);
                let p = ui.painter().clone();
                let row = Rect::from_min_max(pos2(x0, rect.top() + 1.0), pos2(x0 + name_w + cw.iter().sum::<f32>(), rect.bottom() - 1.0));
                if selected {
                    p.rect_filled(row, cr(4.0), sel_fill);
                    sel_pill(&p, row, sel_mark);
                } else if resp.hovered() {
                    p.rect_filled(row, cr(4.0), shade(10));
                }
                if focused || (caret0 && vi == 0) {
                    self.caret_ring(&p, row, 4.0);
                }
                self.drag_drop_row(&resp, vi, &e, &p, row);
                let isz = if thumbs { 34.0 } else { 20.0 };
                // "Analiza miejsca": the tree's indent, and an arrow that opens a folder in place
                let indent = if tree { 18.0 + e.depth as f32 * 18.0 } else { 0.0 };
                if tree && e.dir {
                    let chev = Rect::from_center_size(pos2(x0 + 12.0 + e.depth as f32 * 18.0, rect.center().y), vec2(18.0, 18.0));
                    let open = self.tabs[self.cur].expanded.contains(&e.path);
                    let hot = resp.hover_pos().is_some_and(|q| chev.contains(q));
                    if hot {
                        p.rect_filled(chev, cr(3.0), shade(14));
                    }
                    p.text(chev.center(), Align2::CENTER_CENTER, if open { ic::CHEV_D } else { ic::CHEV_R }, icon_font(10.0), if hot { pal().text } else { pal().dim });
                    if hot && resp.clicked() {
                        self.pending.push(Act::ToggleExpand(e.path.clone()));
                    }
                }
                let irect = Rect::from_min_size(pos2(x0 + 14.0 + indent, rect.center().y - isz / 2.0), vec2(isz, isz));
                let own = !e.dir && own_icon(&e.name);
                // in a thumbnail's 128 pixels also for a 20 pt icon: asked for in 35 or 48, the
                // shell's small pictures lost their 1 px outlines (a white page on a white row)
                if !((thumbs || own) && !e.dir && self.draw_thumb(&p, irect, &e)) {
                    paint_icon(&p, irect, e.kind);
                }
                veil_hidden(&p, &e, irect, if selected { sel_fill } else { pal().content });
                let dim_text = item_color(&e, cut);
                let font = FontId::proportional(14.0);
                let mut x = x0 + name_w;
                let cy = rect.center().y;
                // name (or inline rename box)
                let name_rect = Rect::from_min_max(pos2(irect.right() + 6.0, rect.top() + 1.0), pos2(x0 + name_w - 6.0, rect.bottom() - 1.0));
                if self.rename.as_ref().is_some_and(|r| r.path == e.path) {
                    self.rename_box(ui, name_rect);
                } else {
                    let g = p.layout(e.name.clone(), font.clone(), dim_text, f32::INFINITY);
                    let clip = p.with_clip_rect(name_rect.intersect(p.clip_rect()));
                    let mut after = name_rect.left() + g.size().x;
                    // where a cloud file is: one quiet column at the right end of Nazwa, as Explorer's "Stan"
                    let right = name_rect.right() - cloud.map_or(0.0, |_| 22.0);
                    // VirusTotal's verdict, right after the name; a long name gives way to both
                    // shortcut: 30 px is room for a 3-digit ring at this size
                    let vt = self.vt_report(&e.path, e.modified);
                    let vt_x = vt.as_ref().map(|_| (after + 8.0).min(right - 30.0));
                    let name_end = vt_x.map_or(right, |x| x - 4.0);
                    let name_clip = p.with_clip_rect(Rect::from_min_max(name_rect.min, pos2(name_end, name_rect.bottom())).intersect(p.clip_rect()));
                    name_clip.galley(pos2(name_rect.left(), cy - g.size().y / 2.0), g, dim_text);
                    if let Some(c) = cloud {
                        paint_cloud(&clip, pos2(right + 4.0, cy), c, 18.0);
                    }
                    if let (Some(x), Some(r)) = (vt_x, vt) {
                        after = x + paint_badge(&clip, pos2(x, cy), &r, 20.0);
                    }
                    // a search result also tells where it is: its folder, muted, after the name
                    // (short of the cloud mark)
                    if self.tabs[self.cur].found {
                        if let Some(dir) = e.path.parent() {
                            let d = p.layout(dir.display().to_string(), font.clone(), pal().dim, f32::INFINITY);
                            let dir_clip = p.with_clip_rect(Rect::from_min_max(name_rect.min, pos2(right, name_rect.bottom())).intersect(p.clip_rect()));
                            dir_clip.galley(pos2(after + 12.0, cy - d.size().y / 2.0), d, pal().dim);
                        }
                    }
                }
                let cells = [
                    (
                        match e.size {
                            Some(s) => fmt_size(s),
                            None if self.sizing.contains(&e.path) => "…".into(),
                            None => String::new(),
                        },
                        true,
                    ),
                    (e.type_name.to_string(), false),
                    // a fresh change reads "40 min temu" (the exact time is in the preview); one repaint a minute keeps it true
                    match ago(e.modified) {
                        Some(a) => {
                            ui.ctx().request_repaint_after(std::time::Duration::from_secs(60));
                            (a, false)
                        }
                        None => (fmt_time(e.modified), false),
                    },
                    // only the letters of attributes that are set; "-------" on every row is noise
                    (fmt_attrs(e.attrs).replace('-', ""), false),
                ];
                for (k, (text, right)) in cells.into_iter().enumerate() {
                    let w = cw[k];
                    if w == 0.0 {
                        continue;
                    }
                    // Rozmiar ends 14px short, so a right-aligned size and the type after it read apart
                    let pad_r = if k == 0 { 14.0 } else { 8.0 };
                    let cell = Rect::from_min_max(pos2(x + 8.0, rect.top()), pos2(x + w - pad_r, rect.top() + row_h));
                    let clip = p.with_clip_rect(cell.intersect(p.clip_rect()));
                    let font = if k == 3 { FontId::monospace(13.0) } else { font.clone() };
                    // the name leads; recent (green) and hidden or cut rows keep one colour across the row
                    let col = if dim_text == pal().text { pal().dim } else { dim_text };
                    // "Analiza miejsca": a bar against the biggest item (so the order reads at a glance)
                    // and the share of the folder in per cent where the column has room for it
                    if let (0, Some((tot, max)), Some(s)) = (k, space_total, e.size) {
                        let bar = Rect::from_min_size(pos2(x + 4.0, rect.top() + 5.0), vec2((w - 8.0) * (s as f64 / max as f64).clamp(0.0, 1.0) as f32, row_h - 10.0));
                        // dark: the tint over a selected or hovered row takes green and grey text below 4.5:1
                        if dark() && (selected || resp.hovered()) {
                            clip.rect_stroke(bar, cr(3.0), Stroke::new(1.0, pal().accent.gamma_multiply(0.5)), StrokeKind::Inside);
                        } else {
                            clip.rect_filled(bar, cr(3.0), pal().accent.gamma_multiply(0.16));
                        }
                        let share = s as f64 * 100.0 / tot as f64;
                        if w >= 96.0 && share >= 1.0 {
                            clip.text(pos2(cell.left(), cy), Align2::LEFT_CENTER, format!("{share:.0} %"), FontId::proportional(11.0), pal().dim);
                        }
                    }
                    if right {
                        clip.text(pos2(cell.right(), cy), Align2::RIGHT_CENTER, text, font, col);
                    } else {
                        clip.text(pos2(cell.left(), cy), Align2::LEFT_CENTER, text, font, col);
                    }
                    x += w;
                }
            }
        });
        self.tabs[self.cur].scroll = (out.state.clamped_offset().y, out.inner_rect.height());
    }

    /// Drag and drop of a tab: `zone` takes it into pane `side` (outside split view, opening it).
    pub(crate) fn tab_drop_zone(&mut self, ui: &Ui, zone: Rect, side: u8) {
        let Some(t) = egui::DragAndDrop::payload::<DraggedTab>(ui.ctx()) else { return };
        // in split view a tab already in this pane stays put
        if (self.split && self.tabs.get(t.0).is_none_or(|x| x.side == side)) || !ui.rect_contains_pointer(zone) {
            return;
        }
        ui.painter().rect(zone.shrink(2.0), cr(6.0), pal().accent.gamma_multiply(0.10), Stroke::new(2.0, pal().accent), StrokeKind::Inside);
        self.tab_target = Some(if self.split { t!("list.tab.move_here") } else { t!("list.tab.open_split") });
        if ui.input(|i| i.pointer.any_released()) {
            if let Some(t) = egui::DragAndDrop::take_payload::<DraggedTab>(ui.ctx()) {
                self.pending.push(Act::TabToSide(t.0, side));
            }
        }
    }

    /// A row's drag carries the selection (the row alone when it is not selected); a folder row
    /// takes a drop of other items into itself, in either pane.
    fn drag_drop_row(&mut self, resp: &Response, vi: usize, e: &Entry, p: &Painter, r: Rect) {
        // the left button only: Explorer's right-drag asks first, here it does nothing
        if resp.drag_started_by(egui::PointerButton::Primary) {
            let from = self.cur;
            let t = self.tab();
            if !t.sel.contains(&e.path) {
                select_at(t, vi, false, false);
            }
            // the grabbed item first: its name and icon go on the card under the pointer
            let mut paths = t.selected();
            paths.sort_by_key(|p| p != &e.path);
            resp.dnd_set_drag_payload(Dragged(paths, from, e.kind));
        }
        if !e.dir || archive::is_inside(&e.path) {
            return;
        }
        // files from another program: by where the pointer is (egui does not see it move then)
        let ext = self.ext.as_ref().filter(|x| resp.interact_rect.contains(x.0)).cloned();
        let Some(d) = resp.dnd_hover_payload::<Dragged>().or_else(|| ext.as_ref().map(|x| x.1.clone())) else { return };
        // a folder row is never a way through to the pane behind it, also when it cannot take the drop
        self.drop_on_row = true;
        // not into a dragged folder or below it, nor where the items already are
        if d.0.iter().any(|x| e.path.starts_with(x) || x.parent() == Some(e.path.as_path())) {
            return;
        }
        let mv = drop_moves_now(&d, &e.path, ext.is_some(), resp.ctx.input(|i| i.modifiers));
        p.rect_filled(r, cr(4.0), pal().sel);
        p.rect_stroke(r, cr(4.0), Stroke::new(2.0, pal().accent), StrokeKind::Inside);
        self.drop_target = Some((e.path.clone(), mv));
        if let Some(d) = resp.dnd_release_payload::<Dragged>().or_else(|| ext.filter(|x| x.2).map(|x| x.1)) {
            self.ext = None;
            self.pending.push(Act::Drop(d.0.clone(), e.path.clone(), self.cur, mv));
        }
    }

    /// No focused item yet, but the arrows would start at item 0: the keys reach this pane's list
    /// (the same gates as `keys`), so item 0 is the caret.
    fn caret_at_top(&self, ctx: &egui::Context) -> bool {
        self.tabs[self.cur].focus.is_none()
            && !self.inactive_pane
            && self.tree_focus.is_none()
            && !self.about
            && self.ask_pw.is_none()
            && self.ask_pack.is_none()
            && self.ask.is_none()
            && !ctx.text_edit_focused()
            && ctx.memory(|m| m.focused().is_none())
            && !crate::ops::popup_blocks(ctx)
    }

    /// The keyboard caret, drawn over the selection fill: the Fluent focus ring after arrow keys,
    /// else a 1px outline, accent in the active pane and grey in the inactive one.
    fn caret_ring(&self, p: &Painter, r: Rect, radius: f32) {
        if self.inactive_pane {
            p.rect_stroke(r, cr(radius), Stroke::new(1.0, pal().dim), StrokeKind::Inside);
        } else if self.kbd_focus {
            focus_ring(p, r, radius);
        } else {
            p.rect_stroke(r, cr(radius), Stroke::new(1.0, pal().accent), StrokeKind::Inside);
        }
    }

    pub(crate) fn rename_box(&mut self, ui: &mut Ui, rect: Rect) {
        let Some(r) = &mut self.rename else { return };
        let resp = ui.put(rect, egui::TextEdit::singleline(&mut r.text).margin(Margin::symmetric(4, 2)));
        ui.painter().rect_stroke(rect, cr(3.0), Stroke::new(1.0, pal().accent), StrokeKind::Inside);
        if r.fresh {
            resp.request_focus();
            // select the name without its extension, like Explorer
            let end = Path::new(&r.text).file_stem().map(|s| s.to_string_lossy().chars().count()).unwrap_or(0);
            let end = if r.path.is_dir() { r.text.chars().count() } else { end };
            if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
                st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(end))));
                st.store(ui.ctx(), resp.id);
            }
            r.fresh = false;
        } else if ui.input(|i| i.key_pressed(Key::Escape)) {
            self.rename = None;
        } else if resp.lost_focus() {
            self.commit_rename();
        }
    }

    /// Draws the shell's picture of the file; false until it has come.
    pub(crate) fn draw_thumb(&mut self, p: &Painter, rect: Rect, e: &Entry) -> bool {
        // a program's or a shortcut's picture is in its content, which is not read (downloaded)
        // for a file that is only in the cloud; thumbnails the shell has without reading it
        if own_icon(&e.name) && rx_core::only_in_cloud(e.attrs) {
            return false;
        }
        let path = e.path.as_path();
        let seen = p.ctx().cumulative_pass_nr();
        match self.thumbs.get_mut(path) {
            Some(t) => t.seen = seen,
            None => {
                self.thumbs.insert(path.to_path_buf(), Thumb { tex: None, seen });
                self.thumb_req.push(path.to_path_buf());
            }
        }
        let Some(tex) = self.thumbs.get(path).and_then(|t| t.tex.as_ref()) else { return false };
        let s = tex.size_vec2();
        let scale = (rect.width() / s.x).min(rect.height() / s.y);
        let r = Rect::from_center_size(rect.center(), s * scale);
        p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        true
    }

    pub(crate) fn grid(&mut self, ui: &mut Ui, any_clicked: &mut bool) {
        ui.add_space(6.0);
        let (tw, th) = (120.0, 132.0);
        let cols = ((ui.available_width() - 12.0) / tw).floor().max(1.0) as usize;
        let n = self.tabs[self.cur].view.len();
        let rows = n.div_ceil(cols);
        // the same row pitch for show_rows and scroll_offset (see details)
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut area = egui::ScrollArea::vertical().id_salt(("grid", self.cur)).auto_shrink(false);
        if let Some(o) = self.scroll_offset(th, cols, ui.available_height()) {
            area = area.vertical_scroll_offset(o);
        }
        let (sel_fill, sel_mark) = if self.inactive_pane { (pal().nav_sel, pal().dim) } else { (pal().sel, pal().accent) };
        let caret0 = self.caret_at_top(ui.ctx());
        let out = area.show_rows(ui, th, rows, |ui, range| {
            for r in range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(6.0);
                    for c in 0..cols {
                        let vi = r * cols + c;
                        if vi >= n {
                            break;
                        }
                        let (rect, resp) = ui.allocate_exact_size(vec2(tw, th), Sense::CLICK | Sense::DRAG);
                        self.item_interact(&resp, vi, any_clicked);
                        let t = &self.tabs[self.cur];
                        let e = t.all[t.view[vi]].clone();
                        let selected = t.sel.contains(&e.path);
                        let cloud = self.cloud_mark(&e);
                        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListItem, true, selected, item_label(&e, cloud)));
                        let focused = t.focus.as_ref() == Some(&e.path);
                        let p = ui.painter().clone();
                        let tile = rect.shrink(3.0);
                        if selected {
                            p.rect_filled(tile, cr(6.0), sel_fill);
                            sel_pill(&p, tile, sel_mark);
                        } else if resp.hovered() {
                            p.rect_filled(tile, cr(6.0), shade(10));
                        }
                        if focused || (caret0 && vi == 0) {
                            self.caret_ring(&p, tile, 6.0);
                        }
                        self.drag_drop_row(&resp, vi, &e, &p, tile);
                        let irect = Rect::from_center_size(pos2(tile.center().x, tile.top() + 46.0), vec2(80.0, 80.0));
                        if e.dir || !self.draw_thumb(&p, irect, &e) {
                            paint_icon(&p, irect.shrink(8.0), e.kind);
                        }
                        veil_hidden(&p, &e, irect, if selected { sel_fill } else { pal().content });
                        // VirusTotal's verdict on the picture's top-right corner
                        if let Some(r) = self.vt_report(&e.path, e.modified) {
                            paint_badge(&p, pos2(irect.right() - 18.0, irect.top() + 10.0), &r, 20.0);
                        }
                        // and where a cloud file is on its bottom-left one
                        if let Some(c) = cloud {
                            paint_cloud(&p, pos2(irect.left() + 4.0, irect.bottom() - 12.0), c, 20.0);
                        }
                        let col = item_color(&e, self.cut && self.clip.contains(&e.path));
                        let g = p.layout(e.name.clone(), FontId::proportional(13.0), col, tw - 14.0);
                        let clip = p.with_clip_rect(Rect::from_min_max(pos2(tile.left(), tile.top() + 92.0), tile.max).intersect(p.clip_rect()));
                        clip.galley(pos2(tile.center().x - g.size().x / 2.0, tile.top() + 92.0), g, col);
                    }
                });
            }
        });
        self.tabs[self.cur].scroll = (out.state.clamped_offset().y, out.inner_rect.height());
    }

    /// Two panes side by side, each with its own tabs (Tab::side); the pane clicked last is active.
    pub(crate) fn split_panes(&mut self, ui: &mut Ui) {
        self.fix_panes();
        if !self.split {
            self.file_list(ui);
            return;
        }
        let full = ui.available_rect_before_wrap();
        let half = (full.width() - 4.0) / 2.0;
        let rects = [
            Rect::from_min_size(full.min, vec2(half, full.height())),
            Rect::from_min_size(pos2(full.left() + half + 4.0, full.top()), vec2(half, full.height())),
        ];
        let (active, other) = (self.cur, self.other);
        let pressed = ui.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None });
        let mut next = active;
        let mut picked: Option<(u8, usize)> = None;
        let mut new_in: Option<u8> = None;
        ui.painter().vline(rects[0].right() + 2.0, full.y_range(), Stroke::new(1.0, pal().line));
        for (side, rect) in [(0u8, rects[0]), (1u8, rects[1])] {
            let idx = if self.tabs[active].side == side { active } else { other };
            let is_active = idx == active;
            self.cur = idx;
            ui.scope_builder(UiBuilder::new().id_salt(("pane", side)).max_rect(rect).layout(Layout::top_down(Align::Min)), |ui| {
                ui.set_clip_rect(rect.intersect(ui.clip_rect()));
                ui.spacing_mut().item_spacing.y = 0.0;
                // tab strip
                // the same 4px top and 8px sides as the single-pane strip
                let strip = Rect::from_min_size(rect.min, vec2(rect.width(), 36.0));
                // where commands go: the active pane's tab strip washed with accent under a 3px
                // accent bar across the whole pane (filled, not a stroke, so it stays 3px at any scale), its tab the white
                // card with an accent underline and its header labels at full ink; the inactive
                // pane's tab only outlined, tab text, header and selection grey, its rows at full
                // contrast so it does not look disabled (the status bar names the pane)
                let p = ui.painter();
                p.rect_filled(strip, cr(0.0), pal().pane);
                if is_active {
                    p.rect_filled(strip, cr(0.0), pal().accent.gamma_multiply(0.07));
                    p.rect_filled(Rect::from_min_size(rect.min, vec2(rect.width(), 3.0)), cr(0.0), pal().accent);
                }
                let tabs = Rect::from_min_max(strip.min + vec2(8.0, 4.0), strip.max - vec2(8.0, 0.0));
                self.inactive_pane = !is_active;
                let (c, plus) = ui.scope_builder(UiBuilder::new().max_rect(tabs), |ui| self.tabs_bar(ui, Some(side), idx)).inner;
                ui.advance_cursor_after_rect(strip);
                if let Some(c) = c {
                    picked = Some((side, c));
                }
                if plus {
                    new_in = Some(side);
                }
                self.file_list(ui);
                self.tab_drop_zone(ui, rect, side);
                self.inactive_pane = false;
            });
            if pressed.is_some_and(|q| rect.contains(q)) {
                next = idx;
            }
        }
        self.cur = next;
        self.other = if next == active { other } else { active };
        // a tab picked in a pane becomes that pane's tab and makes the pane active
        if let Some((side, i)) = picked {
            if self.tabs[self.cur].side != side {
                self.other = self.cur;
            }
            self.cur = i;
        }
        if let Some(side) = new_in {
            if self.tabs[self.cur].side != side {
                std::mem::swap(&mut self.cur, &mut self.other);
            }
            self.pending.push(Act::NewTab(None));
        }
    }
}

/// Header right-click: which columns show, as in Explorer. A width of 0 hides a column, so the
/// choice is saved with the widths; Nazwa always shows.
fn columns_menu(widths: &mut [f32; 4], resp: &Response) {
    resp.context_menu(|ui| {
        menu_style(ui);
        for (k, label) in [t!("list.col.size"), t!("list.col.type"), t!("list.col.modified"), t!("list.col.attrs")].into_iter().enumerate() {
            let on = widths[k] > 0.0;
            if check_item(ui, ic::CHECK, on, label, "") {
                widths[k] = if on { 0.0 } else { DEFAULT_W[k] };
            }
        }
    });
}

/// The 3x16px accent bar at a selected row's left edge, as the tree marks its current folder:
/// selection that does not rest on the pale fill alone.
fn sel_pill(p: &Painter, row: Rect, col: Color32) {
    p.rect_filled(Rect::from_center_size(pos2(row.left() + 1.5, row.center().y), vec2(3.0, 16.0)), cr(1.5), col);
}

/// A drive's fill bar: accent, amber above 80 % used, red above DRIVE_RED.
pub(crate) fn drive_fill(used: f32) -> Color32 {
    if used > DRIVE_RED {
        pal().red
    } else if used > 0.8 {
        Color32::from_rgb(0xC7, 0x77, 0x00)
    } else {
        pal().accent
    }
}

/// What a screen reader reads for a list item; the green text it cannot see becomes a word.
fn item_label(e: &Entry, cloud: Option<Cloud>) -> String {
    let mut s = if e.recent { t!("list.a11y.new", name = e.name) } else { e.name.clone() };
    s += match cloud {
        Some(Cloud::Online) => t!("list.a11y.online"),
        Some(Cloud::Local) => t!("list.a11y.local"),
        Some(Cloud::Pinned) => t!("list.a11y.pinned"),
        None => "",
    };
    s
}

/// Text colour of a list item: green when created or changed in the last 3 hours (as Directory
/// Opus colours recent files; the owner relies on it), light grey when hidden, grey when cut.
fn item_color(e: &Entry, cut: bool) -> Color32 {
    if e.attrs & 2 != 0 {
        pal().faded
    } else if cut {
        pal().dim
    } else if e.recent {
        pal().new_text
    } else {
        pal().text
    }
}

/// Shortcuts, programs and icons carry a picture of their own: Windows' one, as in Explorer.
fn own_icon(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "lnk" | "url" | "exe" | "ico" | "msi" | "appref-ms")
}

/// A hidden item's icon or thumbnail shows through a veil of the row's own background, half
/// see-through, as Explorer draws hidden files.
fn veil_hidden(p: &Painter, e: &Entry, r: Rect, bg: Color32) {
    if e.attrs & 2 != 0 {
        p.rect_filled(r.expand(1.0), cr(0.0), bg.gamma_multiply(0.55));
    }
}

/// Where a cloud file is, `h` across with its left edge at `at`, as Explorer marks it: a blue
/// cloud when it is only in the cloud, a green tick in a green ring when it is on this device,
/// a white tick on green when it is kept here always. Returns the mark's width.
fn paint_cloud(p: &Painter, at: egui::Pos2, c: Cloud, h: f32) -> f32 {
    let m = pos2(at.x + h / 2.0, at.y);
    // the tick's circle is as wide as the cloud glyph, not as its white disc, so the two weigh the same
    let r = h * 0.36;
    // filled inside, as the VirusTotal ring, so it also reads on a selected row and a thumbnail
    let k = pal();
    match c {
        Cloud::Online => {
            p.circle_filled(m, h / 2.0, k.field);
            p.text(m, Align2::CENTER_CENTER, ic::CLOUD, icon_font(h * 0.7), k.accent);
        }
        Cloud::Local => {
            p.circle(m, r - 0.5, k.field, Stroke::new(1.0, k.new_text));
            p.text(m, Align2::CENTER_CENTER, ic::CHECK, icon_font(h * 0.4), k.new_text);
        }
        Cloud::Pinned => {
            p.circle_filled(m, r, k.new_text);
            p.text(m, Align2::CENTER_CENTER, ic::CHECK, icon_font(h * 0.4), k.on_accent);
        }
    }
    h
}
