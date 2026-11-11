//! The folder tree on the left (quick access, bookmarks, drives).

use super::*;

impl App {
    /// One row; `open`: it shows folders below it (the arrow points down). Returns `open`.
    pub(crate) fn tree_row(&mut self, ui: &mut Ui, id: &Path, label: &str, kind: Kind, depth: usize, expandable: bool, target: Option<&Path>, bookmark: bool, open: bool) -> bool {
        let h = 28.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::CLICK);
        named(&resp, egui::Role::TreeItem, label);
        let current = target.is_some_and(|t| t == self.tabs[self.cur].path);
        let index = self.tree_rows.len();
        self.tree_rows.push((id.to_path_buf(), target.map(Path::to_path_buf), expandable, depth));
        let focused = self.tree_focus.as_ref().is_some_and(|(i, f)| *i == index && f == id);
        let out_of_view = !ui.clip_rect().contains_rect(rect);
        // just arrived here (tree() moves tree_at after drawing): bring the row into view
        if current && self.tree_at != self.tabs[self.cur].path && out_of_view {
            resp.scroll_to_me(Some(Align::Center));
        }
        if focused && std::mem::take(&mut self.tree_scroll) && out_of_view {
            resp.scroll_to_me(None);
        }
        let p = ui.painter();
        let row = rect.shrink2(vec2(4.0, 1.0));
        if current {
            p.rect_filled(row, cr(4.0), pal().nav_sel);
            p.rect_filled(Rect::from_min_size(row.min + vec2(0.0, (h - 16.0) / 2.0), vec2(3.0, 16.0)), cr(1.5), pal().accent);
        } else if resp.hovered() {
            p.rect_filled(row, cr(4.0), pal().row_hover);
        }
        if focused {
            focus_ring(p, row, 4.0);
        }
        // 12px a level, as Windows 11, so deep folders keep room for their names
        let x = rect.left() + 10.0 + depth as f32 * 12.0;
        let chev = Rect::from_min_size(pos2(x, rect.top()), vec2(16.0, h));
        if expandable {
            p.text(chev.center(), Align2::CENTER_CENTER, if open { ic::CHEV_D } else { ic::CHEV_R }, icon_font(10.0), pal().dim);
        }
        paint_icon(p, Rect::from_min_size(pos2(x + 20.0, rect.center().y - 8.0), vec2(16.0, 16.0)), kind);
        let max_w = rect.right() - x - 50.0;
        // one line cut with "…", so every row keeps its 28px; the full name shows on hover
        let color = if self.tree_dim.contains(id) { pal().dim } else { pal().text };
        // depth 0 is only the group headers (Widok główny, Zakładki, Ten komputer)
        let font = if depth == 0 { FontId::new(14.0, FontFamily::Name("semibold".into())) } else { FontId::proportional(14.0) };
        let mut job = egui::text::LayoutJob::simple_singleline(label.to_string(), font, color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(10.0));
        let galley = p.layout_job(job);
        let elided = galley.elided;
        p.galley(pos2(x + 44.0, rect.center().y - galley.size().y / 2.0), galley, color);
        let resp = if elided { resp.on_hover_text(label) } else { resp };
        if resp.clicked() {
            // the arrow keys now move through the tree (F6 hands them back to the list)
            self.tree_focus = Some((index, id.to_path_buf()));
            let on_chev = resp.interact_pointer_pos().is_some_and(|q| q.x < chev.right() + 2.0);
            if expandable && (on_chev || target.is_none()) {
                if !self.tree_open.remove(id) {
                    self.tree_open.insert(id.to_path_buf());
                }
            } else if let Some(t) = target {
                self.pending.push(Act::Goto(t.to_path_buf()));
            }
        }
        if let Some(t) = target {
            let t = t.to_path_buf();
            let pending = &mut self.pending;
            resp.context_menu(|ui| {
                menu_style(ui);
                if menu_item(ui, ic::OPEN, "Otwórz", "") {
                    pending.push(Act::Goto(t.clone()));
                }
                if menu_item(ui, ic::NEW_WINDOW, "Otwórz w nowej karcie", "") {
                    pending.push(Act::NewTab(Some(t.clone())));
                }
                if bookmark {
                    if menu_item(ui, ic::DELETE, "Usuń z zakładek", "") {
                        pending.push(Act::RemoveBookmark(t.clone()));
                    }
                } else if menu_item(ui, ic::STAR, "Dodaj do zakładek", "") {
                    pending.push(Act::AddBookmark(t.clone()));
                }
            });
        }
        open
    }

    /// A folder and, when open, what is below it. `way`: the folder is in the branch that leads to
    /// the current one (under the pin or drive it is reached from). A folder on that way shows only
    /// the next folder of it and a "… jeszcze N folderów" row, so the tree is just the path being
    /// worked in (the owner wants it minimal); that row, its arrow, → or Widok ▸ Pełne drzewo
    /// folderów show all its folders.
    pub(crate) fn tree_dir(&mut self, ui: &mut Ui, path: &Path, label: &str, kind: Kind, depth: usize, bookmark: bool, way: bool) {
        let here = self.tabs[self.cur].path.clone();
        let on_way = way && here != path && here.starts_with(path);
        let all = self.tree_open.contains(path) || (on_way && self.tree_full);
        let known_empty = self.tree_kids.get(path).is_some_and(|k| k.is_empty());
        if !self.tree_row(ui, path, label, kind, depth, !known_empty || on_way, Some(path), bookmark, all || on_way) || depth >= 30 {
            return;
        }
        let next = on_way.then(|| here.ancestors().find(|a| a.parent() == Some(path)).map(Path::to_path_buf)).flatten();
        if all {
            for k in self.kids(path, ui.ctx()) {
                let name = self.tree_label(&k);
                self.tree_dir(ui, &k, &name, Kind::Folder, depth + 1, false, Some(&k) == next.as_ref());
            }
            // a hidden folder on the way is not among the listed ones
            if let Some(n) = next.filter(|n| !self.tree_kids.get(path).is_some_and(|k| k.contains(n))) {
                let name = self.tree_label(&n);
                self.tree_dir(ui, &n, &name, Kind::Folder, depth + 1, false, true);
            }
            return;
        }
        // first among the folders it stands for: drawn after the way's subtree, the rows of all
        // levels piled up at the bottom as a staircase stepping back left
        let rest = self.kids(path, ui.ctx()).iter().filter(|k| Some(*k) != next.as_ref()).count();
        if rest > 0 && self.tree_more(ui, depth + 1, rest) {
            self.tree_open.insert(path.to_path_buf());
        }
        if let Some(n) = &next {
            let name = self.tree_label(n);
            self.tree_dir(ui, n, &name, Kind::Folder, depth + 1, false, true);
        }
    }

    /// The localised name the address bar shows ("Użytkownicy" for C:\Users), cached in `names`;
    /// not on network drives, where a shell call per folder made Z: crawl.
    fn tree_label(&mut self, p: &Path) -> String {
        let net = p.to_string_lossy().starts_with("\\\\") || self.drives.iter().any(|(d, _, n)| *n && p.starts_with(d));
        if net { file_label(p) } else { self.dname(p) }
    }

    /// "… jeszcze 23 foldery": shows all folders of a folder on the way; `depth`: of those folders.
    fn tree_more(&mut self, ui: &mut Ui, depth: usize, n: usize) -> bool {
        let text = format!("… jeszcze {n} {}", foldery(n));
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::CLICK);
        named(&resp, egui::Role::Button, &text);
        let p = ui.painter();
        if resp.hovered() {
            p.rect_filled(rect.shrink2(vec2(4.0, 1.0)), cr(4.0), pal().row_hover);
        }
        // in the label column of the folders it stands for, as one more of them
        let x = rect.left() + 10.0 + depth as f32 * 12.0 + 44.0;
        p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, text, FontId::proportional(14.0), pal().dim);
        resp.on_hover_text("Pokaż wszystkie foldery na tym poziomie").clicked()
    }

    pub(crate) fn tree(&mut self, ui: &mut Ui) {
        // the way to the current folder shows under the deepest pin of Widok główny or Zakładki
        // holding it (Widok główny on a tie), otherwise under its drive in Ten komputer
        let here = self.tabs[self.cur].path.clone();
        let pins = self.bookmarks.iter().map(|p| (1u8, p)).chain(self.known.iter().map(|k| (0u8, &k.0)));
        let pin = pins.filter(|(_, p)| here.starts_with(p)).max_by_key(|(_, p)| p.components().count()).map(|(g, p)| (g, p.clone()));
        // (not before the places of Widok główny are read: the way may lead through one of them)
        let read = self.places_read;
        let way = |g: u8, p: &Path| match &pin {
            _ if !read => false,
            Some((pg, pp)) => *pg == g && pp == p,
            None => g == 2 && here.starts_with(p),
        };
        self.tree_rows.clear();
        // Kosz stays at the foot (one row, the hairline above it and a little air); Ustawienia are in Plik
        let foot = 28.0 + 13.0 + 4.0;
        egui::ScrollArea::vertical().auto_shrink(false).min_scrolled_height(0.0).max_height((ui.available_height() - foot).max(0.0)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(4.0);
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("C:\\"));
            // an id of its own: the home folder's path would also open C:\Users\<name> under its drive
            let wg = PathBuf::from("::home");
            if self.tree_row(ui, &wg, "Widok główny", Kind::Home, 0, true, Some(&home), false, self.tree_open.contains(&wg)) {
                for (p, name, k) in self.known.clone() {
                    self.tree_dir(ui, &p, &name, k, 1, false, way(0, &p));
                }
                // Google Drive's streamed drive ("Google Drive (G:)"), from the live drive list: it
                // mounts a while after sign-in; the way to a folder on it opens under Ten komputer
                for (p, name, _) in self.drives.clone().into_iter().filter(|d| d.1.starts_with("Google Drive")) {
                    self.tree_dir(ui, &p, &name, Kind::Cloud, 1, false, false);
                }
            }
            group_line(ui);
            let bm = PathBuf::from("::zakladki");
            if self.tree_row(ui, &bm, "Zakładki", Kind::Star, 0, true, None, false, self.tree_open.contains(&bm)) {
                for p in self.bookmarks.clone() {
                    let name = self.dname(&p);
                    // a bookmark to Pobrane, Pulpit… keeps that place's icon; one in a cloud folder shows the cloud
                    let k = self.known.iter().find(|k| k.0 == p).map_or(if self.in_cloud(&p) { Kind::Cloud } else { Kind::Folder }, |k| k.2);
                    self.tree_dir(ui, &p, &name, k, 1, true, way(1, &p));
                }
                if self.bookmarks.is_empty() {
                    ui.label(RichText::new("     Prawy klik na folderze → Dodaj do zakładek").small().color(pal().dim));
                }
            }
            group_line(ui);
            let pc = PathBuf::from(PC);
            if self.tree_row(ui, &pc, "Ten komputer", Kind::Computer, 0, true, Some(&pc), false, self.tree_open.contains(&pc)) {
                for (p, name, net) in self.drives.clone() {
                    self.tree_dir(ui, &p, &name, if net { Kind::NetDrive } else { Kind::Drive }, 1, false, way(2, &p));
                }
            }
        });
        group_line(ui);
        ui.spacing_mut().item_spacing.y = 0.0;
        // the shell's own bin: deleted files go there (sys::recycle), restoring them is Explorer's job
        if tree_link(ui, ic::DELETE, "Kosz", "Otwórz Kosz w Eksploratorze").clicked() {
            let _ = std::process::Command::new("explorer").arg("shell:RecycleBinFolder").spawn();
        }
        // not while the folders above it are still being read: once in, they move its row, which
        // is then scrolled to where it stays
        if read && !here.ancestors().skip(1).any(|a| self.tree_asked.get(a).is_some_and(|k| k.waiting())) {
            self.tree_at = here;
        }
    }
}

/// A folder's name for its tree row.
fn file_label(p: &Path) -> String {
    p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A row at the tree's foot that runs a command, aligned with the group headers above it.
fn tree_link(ui: &mut Ui, glyph: &str, label: &str, tip: &str) -> Response {
    // click() (not CLICK) is focusable, so Tab reaches Kosz, which has no other way in
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    named(&resp, egui::Role::Button, label);
    let p = ui.painter();
    let row = rect.shrink2(vec2(4.0, 1.0));
    if resp.hovered() {
        p.rect_filled(row, cr(4.0), pal().row_hover);
    }
    if resp.has_focus() {
        focus_ring(p, row, 4.0);
    }
    // icon and label where a depth-0 tree row puts them
    p.text(pos2(rect.left() + 38.0, rect.center().y), Align2::CENTER_CENTER, glyph, icon_font(16.0), pal().text);
    p.text(pos2(rect.left() + 54.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(14.0), pal().text);
    resp.on_hover_text(tip)
}

/// A hairline between the tree's groups.
fn group_line(ui: &mut Ui) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline((r.left() + 12.0)..=(r.right() - 12.0), r.center().y, Stroke::new(1.0, pal().line));
    // a little more room above a group's header than below its last row
    ui.add_space(8.0);
}
