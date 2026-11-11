//! The preview pane: what a file shows (picture, PDF page, text, folder summary) and drawing it.

use super::*;

pub(crate) fn to_color_image(img: image::DynamicImage) -> ColorImage {
    let rgba = img.to_rgba8();
    ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
}

/// `px`: the preview card's width in screen pixels; a PDF page is rendered that wide, because the
/// GPU side scales without mipmaps and a much larger page would shimmer when shrunk. `read`: the
/// content is read also when the file's attributes say it is only in the cloud.
pub(crate) fn build_preview(path: &Path, px: u32, read: bool) -> Raw {
    if archive::is_inside(path) {
        return archive::preview(path, px);
    }
    if path.is_dir() {
        let (mut d, mut f, mut b) = (0, 0, 0);
        for e in std::fs::read_dir(path).into_iter().flatten().flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => d += 1,
                Ok(m) => {
                    f += 1;
                    b += m.len();
                }
                _ => {}
            }
        }
        return Raw::Folder(d, f, b);
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = ext_of(&name);
    // only in the cloud: reading the content would download the file, so until that is asked for
    // (Act::CloudFetch, then `read`) it shows the shell's thumbnail; the attributes are read fresh, recalling nothing
    use std::os::windows::fs::MetadataExt;
    if !read && std::fs::symlink_metadata(path).is_ok_and(|m| rx_core::only_in_cloud(m.file_attributes())) {
        let img = sys::shell_thumbnail(path, 512, true).map(|(w, h, px)| ColorImage::from_rgba_premultiplied([w, h], &px));
        return Raw::Cloud(img, IMAGE_EXT.contains(&ext.as_str()) || TEXT_EXT.contains(&ext.as_str()) || ext == "pdf");
    }
    if IMAGE_EXT.contains(&ext.as_str()) {
        if let Ok(img) = image::open(path) {
            let info = format!("{} × {} px", img.width(), img.height());
            return Raw::Image(to_color_image(img.thumbnail(1600, 1600)), info);
        }
    }
    if ext == "pdf" {
        if let Ok((png, pages)) = sys::pdf_first_page(path, px) {
            if let Ok(img) = image::load_from_memory(&png) {
                return Raw::Image(to_color_image(img), format!("Strona 1 z {pages}"));
            }
        }
    }
    if TEXT_EXT.contains(&ext.as_str()) {
        if let Ok(bytes) = read_head(path, 64 * 1024) {
            if !bytes.contains(&0) {
                return Raw::Text(String::from_utf8_lossy(&bytes).into_owned());
            }
        }
    }
    match sys::shell_thumbnail(path, 512, false) {
        Some((w, h, px)) => Raw::Image(ColorImage::from_rgba_premultiplied([w, h], &px), String::new()),
        None => Raw::Nothing("Brak podglądu dla tego pliku".into()),
    }
}

pub(crate) fn read_head(path: &Path, n: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut buf = vec![];
    std::fs::File::open(path)?.take(n as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Readable names of the set attributes; empty when only the archive bit (the usual case) is set.
fn attr_words(a: u32) -> String {
    if a & !0x20 & (0x1 | 0x2 | 0x4 | 0x800 | 0x1000 | 0x4000) == 0 {
        return String::new();
    }
    let words = [(0x1, "Tylko do odczytu"), (0x2, "Ukryty"), (0x4, "Systemowy"), (0x20, "Archiwalny"), (0x800, "Skompresowany"), (0x4000, "Zaszyfrowany"), (0x1000, "Offline")];
    words.iter().filter(|&&(m, _)| a & m != 0).map(|&(_, w)| w).collect::<Vec<_>>().join(", ")
}

/// Heading, type line and label/value rows, laid out the same in every preview state.
fn details(ui: &mut Ui, title: &str, kind: &str, rows: Vec<(&str, String)>) {
    // one step below Heading for the narrow pane; wraps so long names keep their extension visible
    ui.add(egui::Label::new(RichText::new(title).size(17.0).family(FontFamily::Name("semibold".into()))).wrap());
    ui.label(RichText::new(kind).color(pal().dim));
    ui.add_space(4.0);
    // one fixed label column (fits "Zmodyfikowany") so values start at the same x in every state; values wrap
    egui::Grid::new("meta").num_columns(2).min_col_width(108.0).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
        for (k, v) in rows {
            ui.label(RichText::new(k).color(pal().dim));
            // a "value\nnote" row shows the note muted after the value on the same line (e.g. the exact
            // byte count after the size), so every label keeps one baseline
            match v.split_once('\n') {
                Some((v, note)) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(v);
                        // whole: in a narrow pane "(42 min temu)" moves to the next line instead of splitting
                        ui.add(egui::Label::new(RichText::new(format!("({note})")).color(pal().dim)).wrap_mode(egui::TextWrapMode::Extend));
                    });
                }
                None => {
                    ui.add(egui::Label::new(v).wrap());
                }
            }
            ui.end_row();
        }
    });
}

impl App {
    pub(crate) fn preview_ui(&mut self, ui: &mut Ui) {
        // with the panel's 8 px margin every row (header, card, details, buttons) sits 16 px from both edges
        egui::Frame::new().inner_margin(Margin::symmetric(8, 0)).show(ui, |ui| self.preview_body(ui));
    }

    fn preview_body(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Podgląd").size(14.0).family(FontFamily::Name("semibold".into())).color(pal().dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let r = ui.add(egui::Button::new(icon(ic::CLOSE, 12.0).color(pal().text)).min_size(vec2(32.0, 32.0)));
                named(&r, egui::Role::Button, "Ukryj podgląd");
                if r.on_hover_text("Ukryj podgląd (Alt+P)").clicked() {
                    self.show_preview = false;
                }
            });
        });
        ui.add_space(6.0);
        // the card's inner width in pixels, for the next PDF page to render at (see build_preview)
        self.preview_px = ((ui.available_width() - 14.0) * ui.ctx().pixels_per_point()).max(0.0) as u32;
        let Some((path, shown)) = &self.preview else {
            let t = &self.tabs[self.cur];
            if is_pc(&t.path) {
                let drives = self.pc.as_deref().unwrap_or_default();
                let free = drives.iter().map(|d| d.3).sum();
                let total = drives.iter().map(|d| d.4).sum();
                details(ui, "Ten komputer", "Folder systemowy", vec![("Dyski", group(drives.len() as u64)), ("Wolne", fmt_size(free)), ("Pojemność", fmt_size(total))]);
                ui.add_space(10.0);
                ui.add(egui::Label::new(RichText::new("Zaznacz dysk, aby zobaczyć szczegóły").color(pal().dim)).wrap());
                return;
            }
            // the current folder in the file metadata layout; counted like the status bar, from the listing
            let (mut dirs, mut files, mut bytes) = (0, 0, 0u64);
            for &i in &t.view {
                let e = &t.all[i];
                if e.dir { dirs += 1 } else { files += 1 }
                bytes += e.size.unwrap_or(0);
            }
            let name = t.path.file_name().map_or_else(|| t.path.to_string_lossy(), |n| n.to_string_lossy());
            // the list holds search results (or the biggest files), not this folder
            let (name, kind) = if t.biggest {
                ("Największe pliki".into(), "W tym folderze i podfolderach")
            } else if t.found {
                ("Wyniki wyszukiwania".into(), "Wszystkie dyski lokalne")
            } else {
                (name, "Folder plików")
            };
            details(ui, &name, kind, vec![("Foldery", group(dirs)), ("Pliki", group(files)), ("Rozmiar", fmt_size(bytes))]);
            ui.add_space(10.0);
            ui.add(egui::Label::new(RichText::new("Zaznacz plik, aby zobaczyć podgląd").color(pal().dim)).wrap());
            return;
        };
        let path = path.clone();
        let entry = self.tabs[self.cur].all.iter().find(|e| e.path == path).cloned();
        // a cloud file's line (two rows in a narrow pane) and its button go under the card
        let under = match shown {
            Shown::Cloud(_, true) => 100.0,
            Shown::Cloud(..) => 56.0,
            _ => 0.0,
        };
        let mut box_h = (ui.available_height() - 250.0 - under).max(160.0);
        let w = ui.available_width();
        if let Shown::Image(tex, _) | Shown::Cloud(Some(tex), _) = shown {
            // the card takes the picture's shape, at most 60% of the pane, so the details move up
            let s = tex.size_vec2();
            let cap = (ui.available_height() * 0.6 - under).max(160.0);
            let scale = ((w - 34.0) / s.x).min((cap - 16.0) / s.y).min(if s.x < 300.0 { 2.0 } else { 1.0 });
            box_h = (s.y * scale + 16.0).max(120.0);
        }
        let mut extra = String::new();
        let card = egui::Frame::new().fill(pal().field).stroke(Stroke::new(1.0, pal().line)).corner_radius(cr(8.0)).inner_margin(Margin::same(6));
        let pdf = ext_of(&path.to_string_lossy()) == "pdf";
        if let Shown::Text(t) = shown {
            // the card fits the text up to ~58% of the pane and scrolls beyond, so the details stay close
            let cap = (ui.available_height() * 0.58).max(160.0);
            let prose = matches!(ext_of(&path.to_string_lossy()).as_str(), "txt" | "md");
            let text = RichText::new(t).size(13.0).line_height(Some(19.0));
            let text = if prose { text } else { text.monospace() };
            card.show(ui, |ui| {
                ui.set_width(w - 14.0);
                egui::ScrollArea::vertical().max_height(cap).auto_shrink([false, true]).show(ui, |ui| {
                    egui::Frame::new().inner_margin(Margin::same(3)).show(ui, |ui| {
                        ui.add(egui::Label::new(text).wrap());
                    });
                });
            });
        } else if let (true, Shown::Image(tex, info)) = (pdf && matches!(shown, Shown::Image(_, i) if !i.is_empty()), shown) {
            // a page fills the card's width so its text reads; a longer page scrolls inside the card,
            // which still leaves the details and buttons below in view (an unreadable PDF's shell
            // icon, with no page info, stays a centred icon below)
            extra = info.clone();
            let iw = w - 14.0;
            let s = tex.size_vec2();
            let cap = (ui.available_height() - self.preview_details_h - 30.0).max(160.0);
            card.show(ui, |ui| {
                ui.set_width(iw);
                // per file, so the next document starts at its top
                egui::ScrollArea::vertical().id_salt(&path).max_height(cap).auto_shrink([false, true]).show(ui, |ui| {
                    ui.add(egui::Image::new((tex.id(), vec2(iw, s.y * iw / s.x.max(1.0)))));
                });
            });
        } else {
            let zoom = if self.preview_zoom.0 == path { self.preview_zoom.1 } else { 1.0 };
            let card_rect = card.show(ui, |ui| {
                ui.set_min_size(vec2(w - 14.0, box_h));
                ui.set_max_size(vec2(w - 14.0, box_h));
                ui.centered_and_justified(|ui| match shown {
                    Shown::Loading => {
                        ui.spinner();
                    }
                    Shown::Image(tex, info) => {
                        extra = info.clone();
                        let s = tex.size_vec2();
                        let scale = ((w - 34.0) / s.x).min((box_h - 16.0) / s.y).min(if s.x < 300.0 { 2.0 } else { 1.0 });
                        let id = ui.make_persistent_id(("preview-zoom", &path));
                        if zoom > 1.0 {
                            // zoomed: the picture scrolls (wheel or drag) inside the card, which keeps its size
                            let (img, view) = (s * scale * zoom, ui.available_size());
                            let content = img.max(view);
                            let mut area = egui::ScrollArea::both().id_salt(&path);
                            // a new zoom keeps the spot that was in the middle of the card there
                            let before: Option<(f32, egui::Vec2, egui::Vec2)> = ui.data(|d| d.get_temp(id));
                            if before.is_none_or(|b| b.0 != zoom) {
                                let mid = before.map_or(vec2(0.5, 0.5), |(_, off, size)| (off + view / 2.0) / size);
                                area = area.scroll_offset(mid * content - view / 2.0);
                            }
                            let out = area.show(ui, |ui| {
                                let (r, _) = ui.allocate_exact_size(content, Sense::hover());
                                egui::Image::new((tex.id(), img)).paint_at(ui, egui::Rect::from_center_size(r.center(), img));
                            });
                            ui.data_mut(|d| d.insert_temp(id, (zoom, out.state.clamped_offset(), content)));
                        } else {
                            ui.data_mut(|d| d.remove::<(f32, egui::Vec2, egui::Vec2)>(id));
                            ui.add(egui::Image::new((tex.id(), s * scale)));
                        }
                    }
                    Shown::Text(_) => {}
                    Shown::Folder(d, f, b) => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(box_h / 2.0 - 70.0);
                            let (r, _) = ui.allocate_exact_size(vec2(96.0, 96.0), Sense::hover());
                            paint_icon(ui.painter(), r, Kind::Folder);
                            ui.label(format!("Folderów: {d}   Plików: {f}"));
                            ui.label(RichText::new(format!("Pliki w tym folderze: {}", fmt_size(*b))).color(pal().dim));
                        });
                    }
                    Shown::Cloud(Some(tex), _) => {
                        let s = tex.size_vec2();
                        let scale = ((w - 34.0) / s.x).min((box_h - 16.0) / s.y).min(if s.x < 300.0 { 2.0 } else { 1.0 });
                        ui.add(egui::Image::new((tex.id(), s * scale)));
                    }
                    Shown::Nothing(_) | Shown::Cloud(None, _) => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(box_h / 2.0 - 70.0);
                            let (r, _) = ui.allocate_exact_size(vec2(96.0, 96.0), Sense::hover());
                            paint_icon(ui.painter(), r, entry.as_ref().map_or(Kind::File, |e| e.kind));
                            if let Shown::Nothing(m) = shown {
                                ui.label(RichText::new(m).color(pal().dim));
                            }
                        });
                    }
                });
            })
            .response
            .rect;
            // − and + over the card's top-right corner, for a picture (a shell icon has no size line)
            if let Shown::Image(_, info) = shown {
                if !info.is_empty() {
                    // up to 4×: the picture is a thumbnail of at most 1600 px, more only blows up its pixels
                    const STEPS: [f32; 5] = [1.0, 1.5, 2.0, 3.0, 4.0];
                    let up = STEPS.into_iter().find(|&z| z > zoom);
                    let down = STEPS.into_iter().rev().find(|&z| z < zoom);
                    let mut x = card_rect.right() - 8.0;
                    for (glyph, name, to) in [(ic::ZOOM_IN, "Powiększ obraz", up), (ic::ZOOM_OUT, "Pomniejsz obraz", down)] {
                        x -= 30.0;
                        let at = egui::Rect::from_min_size(egui::pos2(x, card_rect.top() + 8.0), vec2(30.0, 30.0));
                        let b = egui::Button::new(icon(glyph, 14.0).color(pal().text)).fill(pal().field).stroke(Stroke::new(1.0, pal().line));
                        // a child that takes no room, or the details below would move up over the card
                        let r = ui.new_child(egui::UiBuilder::new().max_rect(at)).add_enabled(to.is_some(), b.min_size(at.size()));
                        named(&r, egui::Role::Button, name);
                        if r.on_hover_text(name).on_disabled_hover_text(name).clicked() {
                            self.preview_zoom = (path.clone(), to.unwrap_or(zoom));
                        }
                        x -= 4.0;
                    }
                }
            }
        }
        ui.add_space(8.0);
        // Fluent standard buttons (white, hairline border), so they read as buttons, not links
        let button = |glyph: &str, label: &str| {
            egui::Button::new((icon(glyph, 16.0).color(pal().text), RichText::new(label))).fill(pal().field).stroke(Stroke::new(1.0, pal().line)).min_size(vec2(0.0, 32.0))
        };
        if let Shown::Cloud(_, more) = shown {
            ui.add(egui::Label::new(RichText::new("Plik jest tylko w chmurze, więc podgląd go nie pobiera.").color(pal().dim)).wrap());
            // only where the content would show: a picture, a PDF, text
            if *more && ui.add(button(ic::CLOUD, "Pobierz, aby zobaczyć podgląd")).clicked() {
                self.pending.push(Act::CloudFetch(path.clone()));
            }
            ui.add_space(8.0);
        }
        if let Some(e) = entry {
            let top = ui.cursor().top();
            let mut rows = vec![];
            if let Some(s) = e.size {
                // the exact count only adds something once the short form is rounded (kB and up)
                rows.push(("Rozmiar", if s < 1024 { fmt_size(s) } else { format!("{}\n{} B", fmt_size(s), group(s)) }));
            }
            let when = fmt_time(e.modified);
            let rel = ago(e.modified);
            if rel.is_some() {
                // the list may not repaint (Miniatury, recent rows scrolled away), so the preview asks too
                ui.ctx().request_repaint_after(std::time::Duration::from_secs(60));
            }
            rows.push(("Zmodyfikowany", rel.map_or(when.clone(), |a| format!("{when}\n{a}"))));
            let attrs = attr_words(e.attrs);
            if !attrs.is_empty() {
                rows.push(("Atrybuty", attrs));
            }
            if let Some(c) = self.cloud_mark(&e) {
                let at = match c {
                    Cloud::Online => "Tylko w chmurze\npobierze się przy otwarciu",
                    Cloud::Local => "Na tym urządzeniu",
                    Cloud::Pinned => "Zawsze na tym urządzeniu",
                };
                rows.push(("Chmura", at.into()));
            }
            if !extra.is_empty() {
                rows.push((if e.kind == Kind::Pdf { "Dokument" } else { "Wymiary" }, extra));
            }
            details(ui, &e.name, &e.type_name, rows);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // primary action: accent fill; set per state because Button::fill would also freeze hover/pressed
                let open = ui.scope(|ui| {
                    let w = &mut ui.visuals_mut().widgets;
                    for (v, c) in [(&mut w.inactive, pal().accent), (&mut w.hovered, pal().accent_hover), (&mut w.active, pal().accent_press)] {
                        v.weak_bg_fill = c;
                        v.bg_stroke = Stroke::new(1.0, c);
                    }
                    let ink = pal().on_accent;
                    ui.add(egui::Button::new((icon(ic::OPEN, 16.0).color(ink), RichText::new("Otwórz").color(ink))).min_size(vec2(0.0, 32.0)))
                });
                if open.inner.clicked() {
                    self.pending.push(Act::OpenPath(e.path.clone()));
                }
                if ui.add(button(ic::INFO, "Właściwości")).clicked() {
                    self.pending.push(Act::Properties);
                }
            });
            // a file on disk can be checked in VirusTotal (an entry of an archive has no file of its own)
            if !e.dir && !archive::is_inside(&e.path) {
                ui.add_space(10.0);
                self.vt_ui(ui, &e.path, e.modified);
            }
            self.preview_details_h = ui.cursor().top() - top;
        }
    }
}
