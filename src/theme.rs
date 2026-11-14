//! Look of the window: Segoe Fluent glyphs, painted file-kind icons, egui style, caption buttons and resize edges.

use super::*;

/// Segoe Fluent Icons code points (the Windows 11 icon font).
pub(crate) mod ic {
    pub const BACK: &str = "\u{E72B}";
    pub const FORWARD: &str = "\u{E72A}";
    pub const UP: &str = "\u{E74A}";
    pub const REFRESH: &str = "\u{E72C}";
    pub const SEARCH: &str = "\u{E721}";
    pub const NEW_FOLDER: &str = "\u{E8F4}";
    pub const COPY: &str = "\u{E8C8}";
    pub const CUT: &str = "\u{E8C6}";
    pub const PASTE: &str = "\u{E77F}";
    pub const DELETE: &str = "\u{E74D}";
    // the Edit pencil: Fluent's own Rename glyph (E8AC) reads as "A]" at 16 px
    pub const RENAME: &str = "\u{E70F}";
    pub const INFO: &str = "\u{E946}";
    pub const DETAILS: &str = "\u{E8FD}";
    pub const GRID: &str = "\u{E8A9}";
    pub const SORT: &str = "\u{E8CB}";
    pub const PIE: &str = "\u{EB05}";
    /// Calculator, for "Oblicz wielkości folderów"
    pub const CALC: &str = "\u{E8EF}";
    /// Package (a box), for "Spakuj…"
    pub const PACK: &str = "\u{E7B8}";
    /// Lock, Show (an eye) and Hide (an eye struck through), for the archive password
    pub const LOCK: &str = "\u{E72E}";
    pub const EYE: &str = "\u{E890}";
    pub const EYE_OFF: &str = "\u{ED1A}";
    pub const SETTINGS: &str = "\u{E713}";
    /// Globe, for Ustawienia ▸ Language / Język
    pub const GLOBE: &str = "\u{E774}";
    pub const DRIVE: &str = "\u{EDA2}";
    pub const CHECK: &str = "\u{E73E}";
    pub const BULLET: &str = "\u{E915}";
    pub const LINK: &str = "\u{E71B}";
    pub const NEW_WINDOW: &str = "\u{E8A7}";
    pub const PREVIEW: &str = "\u{E8A1}";
    pub const CHEV_R: &str = "\u{E76C}";
    pub const CHEV_D: &str = "\u{E70D}";
    pub const CHEV_U: &str = "\u{E70E}";
    pub const ADD: &str = "\u{E710}";
    pub const CLOSE: &str = "\u{E711}";
    pub const PLAY: &str = "\u{E768}";
    pub const PAUSE: &str = "\u{E769}";
    pub const FILTER: &str = "\u{E71C}";
    pub const BAK: &str = "\u{E8F1}";
    /// Download (an arrow into a tray), for unpacking an archive
    pub const EXTRACT: &str = "\u{E896}";
    pub const STAR: &str = "\u{E734}";
    pub const STAR_FILL: &str = "\u{E735}";
    /// a tab pinned to its folder
    pub const PIN: &str = "\u{E718}";
    pub const UNPIN: &str = "\u{E77A}";
    pub const SELECT_ALL: &str = "\u{E8B3}";
    pub const TERMINAL: &str = "\u{E756}";
    pub const OPEN: &str = "\u{E8E5}";
    pub const SPLIT: &str = "\u{E89F}";
    pub const UNDO: &str = "\u{E7A7}";
    pub const HELP: &str = "\u{E897}";
    pub const POWER: &str = "\u{E7E8}";
    pub const SWITCH: &str = "\u{E8AB}";
    pub const INVERT: &str = "\u{E8E6}";
    pub const ZOOM_IN: &str = "\u{E8A3}";
    pub const ZOOM_OUT: &str = "\u{E71F}";
    pub const ZOOM_RESET: &str = "\u{E9A6}";
    /// Brightness, for Widok ▸ Motyw
    pub const THEME: &str = "\u{E706}";
    /// VirusTotal
    pub const SHIELD: &str = "\u{EA18}";
    pub const CLOUD: &str = "\u{E753}";
    /// Otwórz za pomocą (an app's window)
    pub const OPEN_WITH: &str = "\u{E7AC}";
    /// Więcej opcji Windows (three dots)
    pub const MORE: &str = "\u{E712}";
    pub const WIN_MIN: &str = "\u{E921}";
    pub const WIN_MAX: &str = "\u{E922}";
    pub const WIN_RESTORE: &str = "\u{E923}";
    pub const WIN_CLOSE: &str = "\u{E8BB}";
}

pub(crate) type Pal = rx_core::palette::Palette<Color32>;

static DARK: AtomicBool = AtomicBool::new(false);
static PALS: std::sync::LazyLock<[Pal; 2]> =
    std::sync::LazyLock::new(|| [rx_core::palette::LIGHT, rx_core::palette::DARK].map(|p| p.map(|c| Color32::from_rgb(c[0], c[1], c[2]))));

/// The colours of the look on screen: free functions draw too, so one global set, which
/// follow_theme() switches with egui's theme.
pub(crate) fn pal() -> &'static Pal {
    &PALS[DARK.load(AO::Relaxed) as usize]
}

pub(crate) fn dark() -> bool {
    DARK.load(AO::Relaxed)
}

/// A hover or press fill over any surface: black on the light look, white (a little stronger,
/// as Windows 11's subtle fills) on the dark one.
pub(crate) fn shade(a: u8) -> Color32 {
    shade_of(dark(), a)
}

fn shade_of(dark: bool, a: u8) -> Color32 {
    if dark { Color32::from_white_alpha((a as u16 * 3 / 2) as u8) } else { Color32::from_black_alpha(a) }
}

/// Takes the colours of egui's theme now: Systemowy follows Windows (winit reports a change of
/// its app mode as an event, so nothing polls), Jasny and Ciemny are fixed. Called every frame.
pub(crate) fn follow_theme(ctx: &egui::Context) {
    DARK.store(ctx.theme() == egui::Theme::Dark, AO::Relaxed);
    // winit forgets a fixed Jasny/Ciemny at the next WM_SETTINGCHANGE and gives the border Windows' look back
    if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(true)))) {
        let t = match ctx.options(|o| o.theme_preference) {
            egui::ThemePreference::Light => egui::SystemTheme::Light,
            egui::ThemePreference::Dark => egui::SystemTheme::Dark,
            egui::ThemePreference::System => return,
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(t));
    }
}

pub(crate) fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icons".into()))
}

pub(crate) fn icon(code: &str, size: f32) -> RichText {
    RichText::new(code).font(icon_font(size))
}

// ---------- painted file-kind icons ----------
pub(crate) fn cr(r: f32) -> CornerRadius {
    CornerRadius::same(r.round().clamp(0.0, 255.0) as u8)
}

/// Drag and drop: the card that follows the pointer, with the dragged item's icon and name and,
/// under it, what letting go would do (in accent once it would do something); several items get
/// their count on the icon.
pub(crate) fn drag_card(ctx: &egui::Context, kind: Kind, name: &str, count: usize, action: &str, hot: bool) {
    let Some(at) = ctx.pointer_latest_pos() else { return };
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("przeciaganie")));
    // two lines of the name at most, so a long one does not hide the place to drop on
    let mut job = egui::text::LayoutJob::simple(name.to_string(), FontId::new(14.0, FontFamily::Name("semibold".into())), pal().text, 340.0);
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = true;
    let title = p.layout_job(job);
    let act = p.layout(action.to_string(), FontId::proportional(12.5), if hot { pal().accent } else { pal().dim }, 340.0);
    let text_h = title.size().y + 2.0 + act.size().y;
    // 54 px to the text on the left, 20 px clear of the border on the right
    let size = vec2(title.size().x.max(act.size().x) + 74.0, (text_h + 20.0).max(56.0));
    // right of and below the pointer, or left of / above it near the window's edge (where the
    // right pane takes drops), so the card stays whole
    let screen = ctx.content_rect();
    let x = if at.x + 18.0 + size.x > screen.right() { at.x - 18.0 - size.x } else { at.x + 18.0 };
    let y = (at.y + 14.0).min(screen.bottom() - size.y - 4.0);
    let r = Rect::from_min_size(pos2(x, y), size);
    p.add(egui::epaint::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(50) }.as_shape(r, cr(8.0)));
    p.rect(r, cr(8.0), pal().field, Stroke::new(1.5, pal().accent), StrokeKind::Inside);
    let icon = Rect::from_min_size(pos2(r.left() + 12.0, r.center().y - 16.0), vec2(32.0, 32.0));
    paint_icon(&p, icon, kind);
    if count > 1 {
        let c = icon.right_top() + vec2(0.0, 2.0);
        let n = if count > 99 { "99+".to_string() } else { count.to_string() };
        let g = p.layout_no_wrap(n, FontId::new(11.0, FontFamily::Name("semibold".into())), pal().on_accent);
        let bubble = Rect::from_center_size(c, vec2((g.size().x + 8.0).max(18.0), 18.0));
        p.rect(bubble, cr(9.0), pal().accent, Stroke::new(1.5, pal().field), StrokeKind::Outside);
        p.galley(bubble.center() - g.size() / 2.0, g, pal().on_accent);
    }
    let top = r.center().y - text_h / 2.0;
    let x = r.left() + 54.0;
    let th = title.size().y;
    p.galley(pos2(x, top), title, pal().text);
    p.galley(pos2(x, top + th + 2.0), act, pal().dim);
}

/// Flat, colourful icons in the spirit of Windows 11 (own drawings, no copied artwork).
/// Every document kind is the same page (or the same landscape tile) with one stroke weight, so
/// the set reads as one family; the colour and the white mark tell the kinds apart.
pub(crate) fn paint_icon(p: &Painter, r: Rect, kind: Kind) {
    let ppp = p.pixels_per_point();
    // edges on whole physical pixels keep 16-20 px glyphs crisp at 100 % and 175 %
    let snap = |v: f32| (v * ppp).round() / ppp;
    let s = snap(r.width().min(r.height()));
    let o = pos2(snap(r.center().x - s / 2.0), snap(r.center().y - s / 2.0));
    let at = |x: f32, y: f32| pos2(snap(o.x + x * s), snap(o.y + y * s));
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(at(x0, y0), at(x1, y1));
    // the one stroke weight; straight marks are filled bars, sharper than anti-aliased lines
    let sw = snap((s * 0.07).max(1.0));
    let hbar = |x0: f32, x1: f32, y: f32, c: Color32| {
        p.rect_filled(Rect::from_min_size(at(x0, y), vec2(at(x1, y).x - at(x0, y).x, sw)), CornerRadius::ZERO, c)
    };
    let vbar = |x: f32, y0: f32, y1: f32, c: Color32| {
        p.rect_filled(Rect::from_min_size(at(x, y0), vec2(sw, at(x, y1).y - at(x, y0).y)), CornerRadius::ZERO, c)
    };
    let poly = |pts: &[(f32, f32)], c: Color32| {
        p.add(egui::Shape::convex_polygon(pts.iter().map(|&(x, y)| at(x, y)).collect(), c, Stroke::NONE));
    };
    // a page with a folded top-right corner; a white page gets an outline in `edge`
    let page = |fill: Color32, edge: Color32| {
        let (a, b, f, i) = (at(0.16, 0.04), at(0.84, 0.96), snap(s * 0.26), snap((s * 0.05).max(1.0)));
        let shape = |i: f32, c: Color32| {
            let pts = vec![pos2(a.x + i, a.y + i), pos2(b.x - f, a.y + i), pos2(b.x - i, a.y + f), pos2(b.x - i, b.y - i), pos2(a.x + i, b.y - i)];
            p.add(egui::Shape::convex_polygon(pts, c, Stroke::NONE));
        };
        shape(0.0, edge);
        if fill != edge {
            shape(i, fill);
        }
        let fold = vec![pos2(b.x - f, a.y), pos2(b.x - f, a.y + f), pos2(b.x, a.y + f)];
        p.add(egui::Shape::convex_polygon(fold, edge.lerp_to_gamma(Color32::WHITE, 0.45), Stroke::NONE));
    };
    // the landscape tile of pictures, videos, programs and the desktop
    let tile = |c: Color32| p.rect_filled(rect(0.06, 0.16, 0.94, 0.84), cr(s * 0.1), c);
    let w = Color32::WHITE;
    match kind {
        Kind::Folder | Kind::Zip => {
            let back = Color32::from_rgb(0xE5, 0xA3, 0x1E);
            let front = Color32::from_rgb(0xFF, 0xCA, 0x45);
            p.rect_filled(rect(0.06, 0.16, 0.46, 0.36), cr(s * 0.06), back);
            p.rect_filled(rect(0.06, 0.24, 0.94, 0.86), cr(s * 0.08), back);
            p.rect_filled(rect(0.06, 0.34, 0.94, 0.86), cr(s * 0.08), front);
            if kind == Kind::Zip {
                p.rect_filled(rect(0.46, 0.34, 0.56, 0.86), CornerRadius::ZERO, Color32::from_rgb(0x7A, 0x5B, 0x22));
                for i in 0..4 {
                    let y = 0.40 + i as f32 * 0.11;
                    p.rect_filled(rect(0.43, y, 0.59, y + 0.04), CornerRadius::ZERO, Color32::from_rgb(0xD8, 0xD8, 0xD8));
                }
            }
        }
        Kind::Star => {
            // a bare gold star, so the Zakładki header reads as a section and not one more folder;
            // the font's filled star with its outline in darker gold stays crisp on white
            let m = at(0.5, 0.5);
            p.text(m, Align2::CENTER_CENTER, ic::STAR_FILL, icon_font(s), Color32::from_rgb(0xFF, 0xC1, 0x07));
            p.text(m, Align2::CENTER_CENTER, ic::STAR, icon_font(s), Color32::from_rgb(0xC7, 0x8A, 0x00));
        }
        Kind::Downloads => {
            // a white arrow dropping into a tray, on its own green-teal tile (Pulpit is cyan)
            tile(Color32::from_rgb(0x13, 0x8D, 0x75));
            p.rect_filled(rect(0.44, 0.24, 0.56, 0.46), CornerRadius::ZERO, w);
            poly(&[(0.30, 0.44), (0.70, 0.44), (0.50, 0.64)], w);
            hbar(0.24, 0.76, 0.70, w);
            vbar(0.24, 0.58, 0.70, w);
            vbar(0.69, 0.58, 0.70, w);
        }
        Kind::Drive | Kind::NetDrive => {
            // a chunky outlined silver body with a darker base band, so it never reads as a divider line;
            // a network drive sits higher, on a green cable and connector, so C: and Z: differ at a glance
            let net = kind == Kind::NetDrive;
            let (y0, y1, band) = if net { (0.08, 0.62, 0.42) } else { (0.20, 0.80, 0.56) };
            let green = Color32::from_rgb(0x2E, 0xA0, 0x43);
            let i = snap((s * 0.05).max(1.0));
            let k = s * 0.12;
            let body = rect(0.04, y0, 0.96, y1);
            p.rect_filled(body, cr(k), Color32::from_rgb(0x5E, 0x6E, 0x82));
            let inner = body.shrink(i);
            p.rect_filled(inner, cr(k - i), Color32::from_rgb(0xE6, 0xEA, 0xEE));
            let b = cr(k - i).sw;
            let base = Rect::from_min_max(pos2(inner.min.x, at(0.0, band).y), inner.max);
            p.rect_filled(base, CornerRadius { nw: 0, ne: 0, sw: b, se: b }, Color32::from_rgb(0xA4, 0xB0, 0xBE));
            let ly = (band + y1) / 2.0;
            p.rect_filled(rect(0.68, ly - 0.05, 0.84, ly + 0.05), cr(1.0), if net { green } else { Color32::from_rgb(0x00, 0x67, 0xC0) });
            if net {
                p.rect_filled(rect(0.44, y1, 0.56, 0.80), CornerRadius::ZERO, green);
                p.rect_filled(rect(0.08, 0.82, 0.92, 0.92), CornerRadius::ZERO, green);
                p.rect_filled(rect(0.36, 0.76, 0.64, 0.98), cr(s * 0.04), green);
            }
        }
        Kind::Desktop => {
            // wallpaper, two desktop icons and a taskbar: not the monitor of Ten komputer
            tile(Color32::from_rgb(0x1C, 0xA6, 0xC9));
            let k = cr(s * 0.1).nw;
            let bar = CornerRadius { nw: 0, ne: 0, sw: k, se: k };
            p.rect_filled(rect(0.06, 0.68, 0.94, 0.84), bar, Color32::from_rgb(0x1F, 0x3A, 0x56));
            p.rect_filled(rect(0.16, 0.26, 0.30, 0.38), cr(1.0), w);
            p.rect_filled(rect(0.16, 0.46, 0.30, 0.58), cr(1.0), w);
        }
        Kind::Computer => {
            p.rect_filled(rect(0.04, 0.14, 0.96, 0.72), cr(s * 0.06), Color32::from_rgb(0x30, 0x30, 0x30));
            // the near-black frame vanishes on dark surfaces without an edge
            if dark() {
                p.rect_stroke(rect(0.04, 0.14, 0.96, 0.72), cr(s * 0.06), Stroke::new(1.0, pal().dim), StrokeKind::Inside);
            }
            p.rect_filled(rect(0.09, 0.19, 0.91, 0.66), cr(s * 0.03), Color32::from_rgb(0x2B, 0x8C, 0xE6));
            p.rect_filled(rect(0.42, 0.72, 0.58, 0.84), CornerRadius::ZERO, pal().dim);
            p.rect_filled(rect(0.28, 0.84, 0.72, 0.9), cr(1.0), pal().dim);
        }
        Kind::Cloud => {
            // a blue cloud: three puffs on a flat base
            let c = Color32::from_rgb(0x00, 0x78, 0xD4);
            p.circle_filled(at(0.34, 0.58), s * 0.18, c);
            p.circle_filled(at(0.56, 0.46), s * 0.25, c);
            p.circle_filled(at(0.77, 0.61), s * 0.15, c);
            p.rect_filled(rect(0.34, 0.58, 0.77, 0.76), CornerRadius::ZERO, c);
        }
        Kind::Home => {
            let c = Color32::from_rgb(0xE8, 0x7A, 0x2B);
            p.add(egui::Shape::convex_polygon(vec![at(0.5, 0.08), at(0.95, 0.48), at(0.05, 0.48)], c, Stroke::NONE));
            p.rect_filled(rect(0.18, 0.46, 0.82, 0.92), CornerRadius::ZERO, c);
            p.rect_filled(rect(0.42, 0.6, 0.58, 0.92), CornerRadius::ZERO, Color32::WHITE);
        }
        Kind::Exe => {
            // a program window: dark body, blue title bar
            tile(Color32::from_rgb(0x2B, 0x2B, 0x2B));
            let k = cr(s * 0.1).nw;
            let top = CornerRadius { nw: k, ne: k, sw: 0, se: 0 };
            p.rect_filled(rect(0.06, 0.16, 0.94, 0.34), top, Color32::from_rgb(0x3A, 0x96, 0xDD));
            if dark() {
                p.rect_stroke(rect(0.06, 0.16, 0.94, 0.84), cr(s * 0.1), Stroke::new(1.0, pal().dim), StrokeKind::Inside);
            }
            hbar(0.18, 0.52, 0.48, Color32::from_rgb(0xD0, 0xD0, 0xD0));
            hbar(0.18, 0.70, 0.64, Color32::from_rgb(0x9A, 0x9A, 0x9A));
        }
        Kind::Image => {
            // sky, two hills and a sun
            tile(Color32::from_rgb(0x4C, 0xA8, 0xEC));
            poly(&[(0.10, 0.84), (0.38, 0.42), (0.68, 0.84)], Color32::from_rgb(0x2E, 0x9B, 0x4E));
            poly(&[(0.44, 0.84), (0.66, 0.56), (0.90, 0.84)], Color32::from_rgb(0x1E, 0x7A, 0x3C));
            p.circle_filled(at(0.74, 0.36), s * 0.09, Color32::from_rgb(0xFF, 0xD2, 0x3F));
        }
        Kind::Video => {
            tile(Color32::from_rgb(0x7B, 0x3F, 0xC4));
            poly(&[(0.40, 0.30), (0.66, 0.50), (0.40, 0.70)], w);
        }
        Kind::Audio => {
            // a beamed pair of notes: the only kind drawn as a bare glyph
            let c = Color32::from_rgb(0xD6, 0x33, 0x84);
            p.circle_filled(at(0.28, 0.76), s * 0.14, c);
            p.circle_filled(at(0.72, 0.68), s * 0.14, c);
            p.rect_filled(rect(0.34, 0.24, 0.42, 0.76), CornerRadius::ZERO, c);
            p.rect_filled(rect(0.78, 0.14, 0.86, 0.68), CornerRadius::ZERO, c);
            poly(&[(0.34, 0.20), (0.86, 0.08), (0.86, 0.24), (0.34, 0.36)], c);
        }
        Kind::Sheet => {
            // a white table with green grid lines
            let c = Color32::from_rgb(0x1E, 0x7E, 0x45);
            page(c, c);
            p.rect_filled(rect(0.28, 0.40, 0.72, 0.84), CornerRadius::ZERO, w);
            hbar(0.28, 0.72, 0.54, c);
            hbar(0.28, 0.72, 0.68, c);
            vbar(0.46, 0.40, 0.84, c);
        }
        Kind::Data => {
            // rows of comma-split values, lighter green than Sheet
            let c = Color32::from_rgb(0x1F, 0xA8, 0x74);
            page(c, c);
            for y in [0.44, 0.58, 0.72] {
                hbar(0.28, 0.46, y, w);
                hbar(0.54, 0.72, y, w);
            }
        }
        Kind::Doc => {
            let c = Color32::from_rgb(0x2A, 0x64, 0xC8);
            page(c, c);
            for y in [0.42, 0.54, 0.66] {
                hbar(0.28, 0.72, y, w);
            }
            hbar(0.28, 0.56, 0.78, w);
        }
        Kind::Slides => {
            // a pie chart with one slice cut out
            let c = Color32::from_rgb(0xE0, 0x63, 0x1C);
            page(c, c);
            let (m, rr) = (at(0.5, 0.64), s * 0.18);
            p.circle_filled(m, rr, w);
            p.add(egui::Shape::convex_polygon(vec![m, m + vec2(0.0, -rr), m + vec2(rr, -rr), m + vec2(rr, 0.0)], c, Stroke::NONE));
        }
        Kind::Pdf => {
            let c = Color32::from_rgb(0xD8, 0x2C, 0x20);
            page(c, c);
            hbar(0.28, 0.72, 0.42, w);
            p.rect_filled(rect(0.26, 0.54, 0.74, 0.82), cr(s * 0.04), w);
            // the word only where it can be read (grid view, preview)
            if s >= 32.0 {
                p.text(at(0.5, 0.68), Align2::CENTER_CENTER, "PDF", FontId::proportional(s * 0.17), c);
            }
        }
        Kind::Code => {
            // white </> on teal
            let c = Color32::from_rgb(0x0E, 0x76, 0x8A);
            page(c, c);
            let st = Stroke::new(s * 0.08, w);
            p.add(egui::Shape::line(vec![at(0.38, 0.50), at(0.27, 0.63), at(0.38, 0.76)], st));
            p.add(egui::Shape::line(vec![at(0.62, 0.50), at(0.73, 0.63), at(0.62, 0.76)], st));
            p.line_segment([at(0.55, 0.46), at(0.45, 0.80)], st);
        }
        Kind::Script => {
            // a white >_ prompt on dark navy, a console window in page form
            let c = Color32::from_rgb(0x1F, 0x2D, 0x4A);
            page(c, if dark() { pal().dim } else { c });
            let st = Stroke::new(s * 0.08, w);
            p.add(egui::Shape::line(vec![at(0.28, 0.48), at(0.42, 0.61), at(0.28, 0.74)], st));
            hbar(0.48, 0.72, 0.72, w);
        }
        Kind::Markdown => {
            // a bold white # heading mark on indigo, from bars like the other marks
            let c = Color32::from_rgb(0x4F, 0x46, 0xB8);
            page(c, c);
            for x in [0.38, 0.54] {
                vbar(x, 0.38, 0.82, w);
            }
            for y in [0.50, 0.66] {
                hbar(0.28, 0.72, y, w);
            }
        }
        Kind::Text => {
            let c = Color32::from_rgb(0x4A, 0x6A, 0x96);
            page(w, c);
            for (i, y) in [0.40, 0.52, 0.64, 0.76].into_iter().enumerate() {
                hbar(0.28, if i == 3 { 0.56 } else { 0.72 }, y, c);
            }
        }
        Kind::Backup => {
            // a page with a round clock badge: an earlier state of the file
            let c = Color32::from_rgb(0x5E, 0x6E, 0x82);
            page(w, c);
            let m = at(0.62, 0.70);
            p.circle_filled(m, s * 0.29, w);
            p.circle_filled(m, s * 0.25, c);
            vbar(0.60, 0.54, 0.72, w);
            hbar(0.60, 0.76, 0.70, w);
        }
        _ => page(w, Color32::from_rgb(0x80, 0x8A, 0x96)),
    }
}

// ---------- drawing ----------
pub(crate) fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let load = |f: &str| std::fs::read(format!("C:\\Windows\\Fonts\\{f}")).ok();
    let prop = FontFamily::Proportional;
    // Segoe UI Variable is Windows 11's own UI font (its default instance is Regular); Windows 10 has only Segoe UI
    if let Some(b) = load("SegUIVar.ttf").or_else(|| load("segoeui.ttf")) {
        fonts.font_data.insert("segoe".into(), Arc::new(egui::FontData::from_owned(b)));
        fonts.families.get_mut(&prop).unwrap().insert(0, "segoe".into());
    }
    let mut semibold = fonts.families[&prop].clone();
    if let Some(b) = load("seguisb.ttf") {
        fonts.font_data.insert("segoe_sb".into(), Arc::new(egui::FontData::from_owned(b)));
        semibold.insert(0, "segoe_sb".into());
    }
    fonts.families.insert(FontFamily::Name("semibold".into()), semibold);
    let mut icons = fonts.families[&prop].clone();
    if let Some(b) = load("SegoeIcons.ttf").or_else(|| load("segmdl2.ttf")) {
        fonts.font_data.insert("fluent".into(), Arc::new(egui::FontData::from_owned(b)));
        icons.insert(0, "fluent".into());
    }
    fonts.families.insert(FontFamily::Name("icons".into()), icons);
    ctx.set_fonts(fonts);
    // Widok ▸ Motyw is egui's theme preference (restore() and --theme set it); while Windows
    // has not said yet, Systemowy is light
    ctx.set_theme(egui::ThemePreference::System);
    ctx.options_mut(|o| o.fallback_theme = egui::Theme::Light);
    for (theme, p) in [(egui::Theme::Light, &PALS[0]), (egui::Theme::Dark, &PALS[1])] {
        style_of(ctx, theme, p);
    }
}

fn style_of(ctx: &egui::Context, theme: egui::Theme, p: &Pal) {
    let dark = theme == egui::Theme::Dark;
    ctx.style_mut_of(theme, |s| {
        use egui::TextStyle::*;
        s.text_styles.insert(Body, FontId::proportional(14.0));
        s.text_styles.insert(Button, FontId::proportional(14.0));
        s.text_styles.insert(Small, FontId::proportional(12.0));
        s.text_styles.insert(Heading, FontId::new(20.0, FontFamily::Name("semibold".into())));
        s.text_styles.insert(Monospace, FontId::monospace(12.5));
        s.spacing.item_spacing = vec2(4.0, 2.0);
        s.spacing.button_padding = vec2(8.0, 4.0);
        s.spacing.interact_size.y = 24.0;
        s.spacing.menu_margin = Margin::same(4);
        // a thin bar visible at rest that widens to a 12px target on hover, over the content so
        // every row keeps one width; no edge fade, which made the last row look hidden or cut
        s.spacing.scroll = egui::style::ScrollStyle {
            bar_width: 12.0,
            // 4px at rest stays inside the cells' 8px right padding, so it never covers row text
            floating_width: 4.0,
            floating_allocated_width: 0.0,
            handle_min_length: 32.0,
            // the handle in translucent text colour: thin() paints it in the widgets' near-white bg_fill,
            // which vanished on the white list; mid-grey at rest, darker when hovered
            foreground_color: true,
            dormant_handle_opacity: 0.35,
            active_handle_opacity: 0.45,
            interact_handle_opacity: 0.6,
            dormant_background_opacity: 0.0,
            active_background_opacity: 0.0,
            fade: egui::style::ScrollFadeStyle { strength: 0.0, size: 0.0 },
            ..egui::style::ScrollStyle::thin()
        };
        let v = &mut s.visuals;
        v.panel_fill = p.content;
        v.window_fill = p.field;
        v.extreme_bg_color = p.field;
        v.faint_bg_color = p.pane;
        v.override_text_color = Some(p.text);
        // hints carry instructions, so they keep a readable grey (6.4:1 on white)
        v.weak_text_color = Some(p.dim);
        v.selection.bg_fill = p.sel;
        v.selection.stroke = Stroke::new(1.0, p.accent);
        v.window_corner_radius = cr(8.0);
        v.menu_corner_radius = cr(8.0);
        v.window_stroke = Stroke::new(1.0, p.line);
        v.popup_shadow = egui::Shadow { offset: [0, 8], blur: 16, spread: 0, color: Color32::from_black_alpha(36) };
        // disabled text and icons at about Windows' grey, not near-white
        v.disabled_alpha = 0.4;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
        for (w, fill) in [
            (&mut v.widgets.inactive, Color32::TRANSPARENT),
            (&mut v.widgets.hovered, shade_of(dark, 12)),
            (&mut v.widgets.active, shade_of(dark, 22)),
            (&mut v.widgets.open, shade_of(dark, 12)),
        ] {
            w.weak_bg_fill = fill;
            w.bg_fill = p.widget;
            w.bg_stroke = Stroke::NONE;
            w.fg_stroke = Stroke::new(1.0, p.text);
            w.corner_radius = cr(4.0);
            w.expansion = 0.0;
        }
    });
}

/// Minimise, maximise/restore and close, drawn inside the menu bar (the window has no Windows frame).
/// Height of the custom title bar (menus + caption buttons).
pub(crate) const TITLE_H: f32 = 36.0;
/// Width of the three caption buttons, 46 px each as in Windows 11.
pub(crate) const CAPTION_W: f32 = 3.0 * 46.0;

/// Width of the band at the window's edges that resizes it (resize_edges).
const RESIZE_EDGE: f32 = 4.0;

pub(crate) fn caption_buttons(ui: &mut Ui) {
    let max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    for (glyph, tip, cmd) in [
        (ic::WIN_CLOSE, t!("theme.caption.close"), egui::ViewportCommand::Close),
        (if max { ic::WIN_RESTORE } else { ic::WIN_MAX }, if max { t!("theme.caption.restore") } else { t!("theme.caption.maximize") }, egui::ViewportCommand::Maximized(!max)),
        (ic::WIN_MIN, t!("theme.caption.minimize"), egui::ViewportCommand::Minimized(true)),
    ] {
        let (rect, resp) = ui.allocate_exact_size(vec2(CAPTION_W / 3.0, TITLE_H), Sense::click());
        let close = glyph == ic::WIN_CLOSE;
        // over the maximise button Windows hands the mouse to rx_sys::caption (snap layouts), not egui
        let snap = matches!(cmd, egui::ViewportCommand::Maximized(_));
        // egui never sees that press, so an open modal dialog blocks it here as it blocks the others
        let modal = ui.ctx().memory(|m| m.top_modal_layer()).is_some();
        let snap_hover = snap && !modal && sys::caption::hovered();
        let snap_click = snap && sys::caption::take_click() && !modal;
        if snap_click {
            // what a click elsewhere does: menus close, an edit (rename) ends
            egui::Popup::close_all(ui.ctx());
            ui.ctx().memory_mut(|m| m.stop_text_input());
        }
        if snap {
            sys::caption::set_rect(rx_core::caption_rect_px([rect.min.x, rect.min.y, rect.max.x, rect.max.y], ui.ctx().pixels_per_point(), max, RESIZE_EDGE));
        }
        let (bg, fg) = match (resp.hovered() || snap_hover, close) {
            (true, true) => (Color32::from_rgb(0xC4, 0x2B, 0x1C), Color32::WHITE),
            (true, false) => (shade(14), pal().text),
            _ => (Color32::TRANSPARENT, pal().text),
        };
        ui.painter().rect_filled(rect, cr(0.0), bg);
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, icon_font(10.0), fg);
        named(&resp, egui::Role::Button, tip);
        if snap_hover {
            resp.show_tooltip_text(tip);
        }
        if resp.on_hover_text(tip).clicked() || snap_click {
            ui.ctx().send_viewport_cmd(cmd);
        }
    }
}

/// Lets the frameless window be resized from its edges and corners.
pub(crate) fn resize_edges(ctx: &egui::Context) {
    use egui::{CursorIcon as C, ResizeDirection as D};
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }
    // a scrollbar or column border being dragged near the edge keeps the pointer
    if ctx.dragged_id().is_some() {
        return;
    }
    let r = ctx.content_rect();
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
    let m = RESIZE_EDGE;
    let (l, rt, t, b) = (pos.x - r.left() < m, r.right() - pos.x < m, pos.y - r.top() < m, r.bottom() - pos.y < m);
    let hit = match (l, rt, t, b) {
        (true, _, true, _) => Some((D::NorthWest, C::ResizeNorthWest)),
        (_, true, true, _) => Some((D::NorthEast, C::ResizeNorthEast)),
        (true, _, _, true) => Some((D::SouthWest, C::ResizeSouthWest)),
        (_, true, _, true) => Some((D::SouthEast, C::ResizeSouthEast)),
        (true, ..) => Some((D::West, C::ResizeWest)),
        (_, true, ..) => Some((D::East, C::ResizeEast)),
        (_, _, true, _) => Some((D::North, C::ResizeNorth)),
        (.., true) => Some((D::South, C::ResizeSouth)),
        _ => None,
    };
    if let Some((dir, cursor)) = hit {
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
        }
    }
}

/// An icon-only toggle (32x28, accent while on) read by a screen reader as a pressed button.
pub(crate) fn toggle_icon(ui: &mut Ui, glyph: &str, on: bool, enabled: bool, name: &str, tip: &str) -> bool {
    let b = egui::Button::new(icon(glyph, 16.0).color(if on { pal().accent } else { pal().text })).min_size(vec2(32.0, 28.0)).selected(on);
    let r = ui.add_enabled(enabled, b);
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, enabled, on, name));
    r.on_hover_text(tip).clicked()
}

/// Fluent's keyboard focus: a 2px ring in the text colour with a 1px one of the ground inside it.
pub(crate) fn focus_ring(p: &Painter, r: Rect, radius: f32) {
    p.rect_stroke(r, cr(radius), Stroke::new(2.0, pal().text), StrokeKind::Inside);
    p.rect_stroke(r.shrink(2.0), cr(radius - 2.0), Stroke::new(1.0, pal().content), StrokeKind::Inside);
}

pub(crate) fn icon_button(ui: &mut Ui, glyph: &str, enabled: bool, tip: &str) -> Response {
    let r = ui.add_enabled(enabled, egui::Button::new(icon(glyph, 16.0).color(pal().text)).min_size(vec2(32.0, 32.0)));
    named(&r, egui::Role::Button, tip);
    r.on_hover_text(tip)
}

/// The name a screen reader reads for an icon-only control (the glyph itself is a private-use character).
pub(crate) fn named(r: &Response, role: egui::Role, name: &str) {
    r.widget_info(|| egui::WidgetInfo::labeled(role, r.enabled(), name));
}

/// Windows 11 menu spacing, shared by the menu bar and the context menus.
pub(crate) fn menu_style(ui: &mut Ui) {
    ui.spacing_mut().button_padding = vec2(12.0, 6.0);
    ui.set_min_width(220.0);
}

/// A menu item with its shortcut right-aligned, as in Windows menus.
pub(crate) fn cmd(ui: &mut Ui, label: &str, key: &str) -> bool {
    let b = egui::Button::new(label);
    ui.add(if key.is_empty() { b } else { b.shortcut_text(key) }).clicked()
}

/// A WinUI menu item with a 16px leading column: `mark` (ic::CHECK for a toggle, ic::BULLET for
/// one of a group) shows when `on`; with `on` false the column stays empty, so plain commands in
/// the same menu line up.
pub(crate) fn check_item(ui: &mut Ui, mark: &str, on: bool, label: &str, key: &str) -> bool {
    use egui::AtomExt;
    let col = if on { pal().text } else { Color32::TRANSPARENT };
    let b = egui::Button::new((icon(mark, 12.0).color(col).atom_size(vec2(16.0, 16.0)), RichText::new(label)));
    let r = ui.add(if key.is_empty() { b } else { b.shortcut_text(key) });
    let role = if mark == ic::BULLET { egui::Role::MenuItemRadio } else { egui::Role::MenuItemCheckBox };
    r.widget_info(|| egui::WidgetInfo::selected(role, true, on, label));
    r.clicked()
}

/// A menu item (menu bar and context menus alike): monochrome icon, label and the shortcut
/// right-aligned. An empty `glyph` keeps the 16px icon column, so every label starts at one edge.
pub(crate) fn menu_item(ui: &mut Ui, glyph: &str, label: &str, key: &str) -> bool {
    menu_entry(ui, glyph, label, key).clicked()
}

/// menu_item's response, for an entry that also needs a tooltip.
pub(crate) fn menu_entry(ui: &mut Ui, glyph: &str, label: &str, key: &str) -> Response {
    use egui::AtomExt;
    let b = egui::Button::new((icon(glyph, 14.0).color(pal().dim).atom_size(vec2(16.0, 16.0)), RichText::new(label)));
    ui.add(if key.is_empty() { b } else { b.shortcut_text(key) })
}

/// A 32px text field with a hairline border (Windows 11 TextBox).
pub(crate) fn field_frame() -> egui::Frame {
    egui::Frame::new().fill(pal().field).stroke(Stroke::new(1.0, pal().line)).corner_radius(cr(4.0)).inner_margin(Margin::symmetric(10, 5))
}

/// A full-width TextBox: field_frame() with the accent line under it while typing, and `trail`
/// (a suffix or a button) at its right end. Returns the text edit's response.
pub(crate) fn edit_field(ui: &mut Ui, text: &mut String, hint: &str, secret: bool, id: &str, trail: impl FnOnce(&mut Ui)) -> Response {
    let mut out = None;
    let fr = field_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                trail(ui);
                let e = egui::TextEdit::singleline(text).id(egui::Id::unique(id)).frame(egui::Frame::NONE).password(secret).hint_text(hint).desired_width(ui.available_width());
                out = Some(ui.add(e));
            });
        });
    });
    let r = out.expect("drawn above");
    if r.has_focus() {
        let b = fr.response.rect;
        ui.painter().line_segment([b.left_bottom() + vec2(2.0, -1.0), b.right_bottom() + vec2(-2.0, -1.0)], Stroke::new(2.0, pal().accent));
    }
    r
}

/// Windows 11's ToggleSwitch (40x20): accent with the knob right when on, an outline when off.
pub(crate) fn switch(ui: &mut Ui, on: &mut bool, name: &str) -> Response {
    let (rect, mut r) = ui.allocate_exact_size(vec2(40.0, 20.0), Sense::click());
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    let (enabled, now) = (ui.is_enabled(), *on);
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, enabled, now, name));
    let t = ui.ctx().animate_bool_responsive(r.id, *on);
    let p = ui.painter();
    let hot = r.hovered() && enabled;
    if *on {
        p.rect_filled(rect, cr(10.0), if hot { pal().accent_hover } else { pal().accent });
    } else {
        p.rect(rect, cr(10.0), if hot { shade(10) } else { Color32::TRANSPARENT }, Stroke::new(1.0, pal().dim), StrokeKind::Inside);
    }
    // the knob grows a little under the pointer, as in WinUI
    let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
    p.circle_filled(pos2(x, rect.center().y), if hot { 7.0 } else { 6.0 }, if *on { pal().on_accent } else { pal().dim });
    if r.has_focus() {
        focus_ring(p, rect.expand(3.0), 13.0);
    }
    r
}

/// A row of choices in one track, the picked one raised on a field-coloured pill (WinUI's
/// segmented control). Returns whether the choice changed.
pub(crate) fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, items: &[(T, &str, &str)]) -> bool {
    let mut changed = false;
    egui::Frame::new().fill(pal().widget).corner_radius(cr(6.0)).inner_margin(Margin::same(2)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let n = items.len() as f32;
        let w = (ui.available_width() - 2.0 * (n - 1.0)) / n;
        ui.horizontal(|ui| {
            for (v, label, tip) in items {
                let on = *value == *v;
                let text = RichText::new(*label).size(13.0).color(if on { pal().text } else { pal().dim });
                let mut b = egui::Button::new(text).min_size(vec2(w, 28.0)).corner_radius(cr(4.0));
                if on {
                    b = b.fill(pal().field).stroke(Stroke::new(1.0, pal().line));
                }
                let r = ui.add_sized(vec2(w, 28.0), b);
                let enabled = ui.is_enabled();
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, enabled, on, *label));
                if r.on_hover_text(*tip).clicked() && !on {
                    *value = *v;
                    changed = true;
                }
            }
        });
    });
    changed
}

/// A radio choice drawn as a card: a title with a line under it, the picked one outlined in accent.
pub(crate) fn choice_card(ui: &mut Ui, on: bool, title: &str, sub: &str, width: f32) -> Response {
    let (rect, r) = ui.allocate_exact_size(vec2(width, 58.0), Sense::click());
    let enabled = ui.is_enabled();
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, enabled, on, title));
    let p = ui.painter();
    let fill = if on { pal().accent.gamma_multiply(0.10) } else if r.hovered() { shade(8) } else { pal().field };
    let edge = if on { Stroke::new(1.5, pal().accent) } else { Stroke::new(1.0, pal().line) };
    p.rect(rect, cr(6.0), fill, edge, StrokeKind::Inside);
    // the WinUI radio mark: a ring, filled with a dot of the ground when picked
    let c = pos2(rect.left() + 22.0, rect.center().y);
    if on {
        p.circle_filled(c, 9.0, pal().accent);
        p.circle_filled(c, 4.0, pal().on_accent);
    } else {
        p.circle_stroke(c, 8.5, Stroke::new(1.0, pal().dim));
    }
    let x = rect.left() + 42.0;
    let wrap = rect.right() - x - 10.0;
    let t = p.layout(title.to_string(), FontId::new(14.0, FontFamily::Name("semibold".into())), pal().text, wrap);
    let s = p.layout(sub.to_string(), FontId::proportional(12.5), pal().dim, wrap);
    let top = rect.center().y - (t.size().y + 2.0 + s.size().y) / 2.0;
    let th = t.size().y;
    p.galley(pos2(x, top), t, pal().text);
    p.galley(pos2(x, top + th + 2.0), s, pal().dim);
    if r.has_focus() {
        focus_ring(p, rect.expand(3.0), 9.0);
    }
    r
}
