//! "Analiza miejsca": the treemap under the list, as WizTree draws it. Every file of the current
//! folder at any depth is a tile sized by its bytes and coloured by its kind, inside the tiles of
//! its folders, so the biggest things show at a glance. A click goes there in the list; the
//! context menu also sends a tile to the Recycle Bin.

use super::*;
use rx_core::treemap::squarify;

/// One tile: where it is, which index entry it shows, how deep under the shown folder.
pub(crate) struct Tile {
    r: Rect,
    id: u32,
    depth: u8,
}

/// The layout for one folder, index, deletion count and panel rectangle, drawn as one mesh.
pub(crate) struct TreeMap {
    key: (PathBuf, u64, u64, [i32; 4]),
    tiles: Vec<Tile>,
    mesh: Arc<egui::Mesh>,
    /// bytes in the folder shown, None when the index does not hold it
    total: Option<u64>,
    /// the folder shown (None: Ten komputer, every drive)
    top: Option<u32>,
}

/// More tiles than this are not drawn (they would be below a pixel anyway on usual screens).
const MAX_TILES: usize = 60_000;

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// The kinds' colours from the file icons, a little lighter, so tiles read as the icons do; other
/// files get a colour of their extension (as WizTree), so a heap of .dll reads apart from .dat.
fn tint(name: &str) -> Color32 {
    let c = match kind_of(name, false) {
        Kind::Zip => Color32::from_rgb(0xE5, 0xA3, 0x1E),
        Kind::Sheet => Color32::from_rgb(0x1E, 0x7E, 0x45),
        Kind::Data => Color32::from_rgb(0x1F, 0xA8, 0x74),
        Kind::Doc => Color32::from_rgb(0x2A, 0x64, 0xC8),
        Kind::Slides => Color32::from_rgb(0xE0, 0x63, 0x1C),
        Kind::Pdf => Color32::from_rgb(0xD8, 0x2C, 0x20),
        Kind::Image => Color32::from_rgb(0x4C, 0xA8, 0xEC),
        Kind::Audio => Color32::from_rgb(0xD6, 0x33, 0x84),
        Kind::Video => Color32::from_rgb(0x7B, 0x3F, 0xC4),
        Kind::Exe => Color32::from_rgb(0x55, 0x55, 0x55),
        Kind::Text | Kind::Markdown => Color32::from_rgb(0x4A, 0x6A, 0x96),
        Kind::Code | Kind::Script => Color32::from_rgb(0x0E, 0x76, 0x8A),
        Kind::Backup => Color32::from_rgb(0x5E, 0x6E, 0x82),
        _ => {
            const OTHER: [Color32; 8] = [
                Color32::from_rgb(0x2A, 0x9D, 0x8F),
                Color32::from_rgb(0xD9, 0xA4, 0x41),
                Color32::from_rgb(0xC8, 0x5A, 0x3E),
                Color32::from_rgb(0x5C, 0x6B, 0xC0),
                Color32::from_rgb(0x7A, 0xA0, 0x6B),
                Color32::from_rgb(0x9C, 0x6A, 0xDE),
                Color32::from_rgb(0xB0, 0x8A, 0x62),
                Color32::from_rgb(0x6C, 0x8E, 0xA4),
            ];
            let e = ext_of(name);
            if e.is_empty() {
                Color32::from_rgb(0x8C, 0x96, 0xA4)
            } else {
                // a fixed spread of the extension's letters, the same colour in every run
                OTHER[e.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) as usize % OTHER.len()]
            }
        }
    };
    mix(c, Color32::WHITE, 0.2)
}

/// A rectangle lit from the top left (WizTree's cushions, simplified): lighter in that corner,
/// darker in the opposite one.
fn cushion(m: &mut egui::Mesh, r: Rect, c: Color32) {
    let i = m.vertices.len() as u32;
    m.colored_vertex(r.left_top(), mix(c, Color32::WHITE, 0.35));
    m.colored_vertex(r.right_top(), c);
    m.colored_vertex(r.left_bottom(), c);
    m.colored_vertex(r.right_bottom(), mix(c, Color32::BLACK, 0.25));
    m.add_triangle(i, i + 1, i + 2);
    m.add_triangle(i + 2, i + 1, i + 3);
}

/// Lays `ids` (entries of one folder) into `r`, biggest first, and each folder's own entries
/// inside its tile, a pixel in from its edge so folders show as outlines. A whole level is laid
/// before going deeper, so when MAX_TILES runs out the deepest, smallest tiles are the ones left
/// out, never a big folder next to one that took them all.
fn lay(ix: &rx_core::index::Index, d: &SpaceData, top: &[u32], r: Rect, out: &mut Vec<Tile>) {
    // breadth first: a folder's own tiles are laid in the queue's turn of its level
    let mut queue = std::collections::VecDeque::from([(top, r, 0u8)]);
    while let Some((ids, r, depth)) = queue.pop_front() {
        let mut items: Vec<(u32, u64)> = ids.iter().filter(|k| !d.gone.contains(k)).map(|&k| (k, d.totals[k as usize])).filter(|x| x.1 > 0).collect();
        items.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        let sizes: Vec<u64> = items.iter().map(|x| x.1).collect();
        for (&(id, _), q) in items.iter().zip(squarify(&sizes, [r.min.x, r.min.y, r.width(), r.height()])) {
            if out.len() >= MAX_TILES {
                return;
            }
            let tr = Rect::from_min_size(pos2(q[0], q[1]), vec2(q[2], q[3]));
            if tr.width() < 1.0 || tr.height() < 1.0 {
                continue;
            }
            out.push(Tile { r: tr, id, depth });
            let inner = tr.shrink(1.0);
            if ix.is_dir(id) && inner.width() >= 3.0 && inner.height() >= 3.0 {
                queue.push_back((d.kids.of(id), inner, depth.saturating_add(1)));
            }
        }
    }
}

impl App {
    pub(crate) fn treemap_ui(&mut self, ui: &mut Ui) {
        let path = self.tabs[self.cur].path.clone();
        let pc = is_pc(&path);
        let Some(ix) = self.index.clone() else {
            ui.centered_and_justified(|ui| ui.label(RichText::new("Wczytuję indeks dysków…").color(pal().dim)));
            return;
        };
        // a caption: what the map shows and how to use it
        let head = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new("Mapa").family(FontFamily::Name("semibold".into())));
            let name = if pc { "Ten komputer".to_string() } else { self.dname(&path) };
            ui.label(RichText::new(name).color(pal().dim));
            if let Some(total) = self.tmap.as_ref().and_then(|m| m.total) {
                ui.label(RichText::new(format!("· {}", fmt_size(total))).color(pal().dim));
            }
            ui.label(RichText::new("· kliknij kafelek, aby tam przejść; prawy przycisk: usuń do Kosza").color(pal().dim).small());
        });
        let rect = Rect::from_min_max(pos2(ui.min_rect().left(), head.response.rect.bottom() + 4.0), ui.max_rect().max);
        let resp = ui.allocate_rect(rect, Sense::click());
        let key = (path.clone(), self.index_gen, self.space_ver, [rect.min.x, rect.min.y, rect.max.x, rect.max.y].map(|v| v.round() as i32));
        // while a panel or the window is being dragged the old layout stays (clipped), and the
        // new one is laid once the button is let go, not on every frame of the drag
        let only_moved = self.tmap.as_ref().is_some_and(|m| (&m.key.0, m.key.1, m.key.2) == (&key.0, key.1, key.2));
        let dragging = ui.input(|i| i.pointer.any_down());
        if self.tmap.as_ref().is_none_or(|m| m.key != key) && !(only_moved && dragging) {
            self.space_data();
            let Some((_, d)) = &self.space_data else { return };
            let top = if pc { None } else { ix.find_in(&d.kids, &path) };
            let shown: Option<Vec<u32>> = if pc { Some(ix.roots().collect()) } else { top.map(|id| d.kids.of(id).to_vec()) };
            let mut tiles = vec![];
            let mut mesh = egui::Mesh::default();
            if let Some(ids) = &shown {
                lay(&ix, d, ids, rect, &mut tiles);
                for t in &tiles {
                    if ix.is_dir(t.id) {
                        mesh.add_colored_rect(t.r, Color32::from_gray(0x9C));
                    } else if t.r.width() < 4.0 || t.r.height() < 4.0 {
                        // too small for a cushion and its edge: one quad
                        mesh.add_colored_rect(t.r, tint(ix.name(t.id as usize)));
                    } else {
                        let c = tint(ix.name(t.id as usize));
                        mesh.add_colored_rect(t.r, mix(c, Color32::BLACK, 0.35));
                        cushion(&mut mesh, t.r.shrink(0.5), c);
                    }
                }
            }
            let total = shown.map(|ids| ids.iter().filter(|k| !d.gone.contains(k)).map(|&k| d.totals[k as usize]).sum());
            self.tmap = Some(TreeMap { key, tiles, mesh: Arc::new(mesh), total, top });
        }
        let Some(m) = &self.tmap else { return };
        let p = ui.painter_at(rect);
        let note = match m.total {
            None => Some("Tego folderu nie ma w indeksie dysków (dysk sieciowy albo folder nowszy niż indeks)"),
            Some(_) if m.tiles.is_empty() => Some("Ten folder nie zajmuje miejsca"),
            _ => None,
        };
        if let Some(n) = note {
            p.text(rect.center(), Align2::CENTER_CENTER, n, FontId::proportional(14.0), pal().dim);
            return;
        }
        p.add(egui::Shape::Mesh(m.mesh.clone()));
        // the names of the bigger top-level tiles, on a pill of the ground so they read on any colour
        let font = FontId::proportional(12.0);
        let pill = if dark() { Color32::from_black_alpha(210) } else { Color32::from_white_alpha(210) };
        for t in m.tiles.iter().filter(|t| t.depth == 0 && t.r.width() > 70.0 && t.r.height() > 20.0) {
            let g = p.layout_no_wrap(ix.name(t.id as usize).to_string(), font.clone(), pal().text);
            let at = t.r.left_top() + vec2(4.0, 3.0);
            let tp = p.with_clip_rect(t.r.shrink(1.0).intersect(rect));
            tp.rect_filled(Rect::from_min_size(at, g.size()).expand(2.0), cr(3.0), pill);
            tp.galley(at, g, pal().text);
        }
        // what is selected in the list is outlined here too (the folder's own entries)
        let tab = &self.tabs[self.cur];
        if !tab.found && !tab.sel.is_empty() {
            let names: HashSet<String> = tab.sel.iter().filter_map(|s| s.file_name()).map(|n| n.to_string_lossy().to_lowercase()).collect();
            for t in m.tiles.iter().filter(|t| t.depth == 0 && names.contains(&ix.name(t.id as usize).to_lowercase())) {
                p.rect_stroke(t.r, 0.0, Stroke::new(2.0, pal().accent), StrokeKind::Inside);
            }
        }
        // the deepest tile under the pointer (children come after their folder)
        let hover = resp.hover_pos().and_then(|at| m.tiles.iter().rev().find(|t| t.r.contains(at))).map(|t| (t.id, t.r));
        let total = m.total.unwrap_or(0).max(1);
        if let Some((id, r)) = hover {
            p.rect_stroke(r, 0.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Inside);
            let bytes = self.space_data.as_ref().map_or(0, |d| d.1.totals[id as usize]);
            let what = if ix.is_dir(id) { "folder" } else { "plik" };
            let tip = format!("{}\n{} · {} · {:.1} % tego, co tu widać", ix.path(id as usize).display(), what, fmt_size(bytes), bytes as f64 * 100.0 / total as f64);
            resp.clone().on_hover_text_at_pointer(tip);
        }
        if resp.clicked() {
            if let Some((id, _)) = hover {
                let path = ix.path(id as usize);
                self.pending.push(if ix.is_dir(id) { Act::Goto(path) } else { Act::Reveal(path) });
            }
        }
        if resp.secondary_clicked() {
            // the item and every folder above it up to the folder shown, as paths: the index may
            // be swapped by a rescan while the menu is open
            let top = m.top;
            self.tmap_menu.clear();
            let mut at = hover.map(|h| h.0);
            while let Some(id) = at.filter(|&id| Some(id) != top) {
                self.tmap_menu.push((ix.path(id as usize), ix.is_dir(id)));
                at = ix.parent_of(id);
            }
        }
        resp.context_menu(|ui| {
            menu_style(ui);
            let Some((path, dir)) = self.tmap_menu.first().cloned() else {
                ui.close();
                return;
            };
            let name = |p: &Path| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned());
            ui.label(RichText::new(name(&path)).color(pal().dim));
            if menu_item(ui, ic::OPEN, "Otwórz", "") {
                self.pending.push(if dir { Act::Goto(path.clone()) } else { Act::OpenPath(path.clone()) });
            }
            if menu_item(ui, ic::CHEV_R, "Pokaż na liście", "") {
                self.pending.push(Act::Reveal(path.clone()));
            }
            if menu_item(ui, ic::LINK, "Kopiuj ścieżkę", "") {
                ui.ctx().copy_text(path.display().to_string());
            }
            // a drive itself (Ten komputer) is no thing to recycle
            if path.parent().is_some() && menu_item(ui, ic::DELETE, "Usuń do Kosza", "") {
                self.pending.push(Act::Recycle(vec![path.clone()]));
            }
            // the folders the pointer is also over, nearest first, each with the same commands
            let above: Vec<PathBuf> = self.tmap_menu.iter().skip(1).map(|(p, _)| p.clone()).collect();
            if !above.is_empty() {
                ui.separator();
                ui.label(RichText::new("Foldery nad nim").color(pal().dim).small());
            }
            for p in above {
                ui.menu_button(format!("{}  {}", ic::CHEV_R, name(&p)), |ui| {
                    menu_style(ui);
                    if menu_item(ui, ic::OPEN, "Otwórz", "") {
                        self.pending.push(Act::Goto(p.clone()));
                    }
                    if menu_item(ui, ic::CHEV_R, "Pokaż na liście", "") {
                        self.pending.push(Act::Reveal(p.clone()));
                    }
                    if menu_item(ui, ic::LINK, "Kopiuj ścieżkę", "") {
                        ui.ctx().copy_text(p.display().to_string());
                    }
                    if p.parent().is_some() && menu_item(ui, ic::DELETE, "Usuń folder do Kosza", "") {
                        self.pending.push(Act::Recycle(vec![p.clone()]));
                    }
                });
            }
        });
    }
}
