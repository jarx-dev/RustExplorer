//! The window's two sets of colours (Jasny, Ciemny) as plain RGB, and the WCAG contrast rule they
//! are tested against; the window turns them into egui colours.

/// One set of the window's colours. Generic so the window can hold the same set as egui colours.
#[derive(Clone, Copy, Debug)]
pub struct Palette<C> {
    /// title and status bars
    pub bar: C,
    /// the file list
    pub content: C,
    /// tree, preview, tabs strip, address bar
    pub pane: C,
    /// hairlines between parts of the window
    pub line: C,
    pub text: C,
    /// labels, hints, inactive tabs
    pub dim: C,
    pub accent: C,
    /// the primary (accent) button hovered and pressed, and text on any accent fill
    pub accent_hover: C,
    pub accent_press: C,
    pub on_accent: C,
    /// a selected row, and a selected row of the inactive pane or the tree's current folder
    pub sel: C,
    pub nav_sel: C,
    /// a tree row under the pointer
    pub row_hover: C,
    /// warnings and errors
    pub red: C,
    /// VirusTotal: suspicious
    pub warn: C,
    /// hidden files once shown
    pub faded: C,
    /// files created or changed in the last 3 hours
    pub new_text: C,
    /// text fields, buttons of dialogs, cards, menus and dialogs
    pub field: C,
    /// egui's own widget fill (check boxes, radio buttons)
    pub widget: C,
    /// the empty part of a fill bar (disk use)
    pub track: C,
}

impl<C: Copy> Palette<C> {
    pub fn map<D>(&self, f: impl Fn(C) -> D) -> Palette<D> {
        Palette {
            bar: f(self.bar),
            content: f(self.content),
            pane: f(self.pane),
            line: f(self.line),
            text: f(self.text),
            dim: f(self.dim),
            accent: f(self.accent),
            accent_hover: f(self.accent_hover),
            accent_press: f(self.accent_press),
            on_accent: f(self.on_accent),
            sel: f(self.sel),
            nav_sel: f(self.nav_sel),
            row_hover: f(self.row_hover),
            red: f(self.red),
            warn: f(self.warn),
            faded: f(self.faded),
            new_text: f(self.new_text),
            field: f(self.field),
            widget: f(self.widget),
            track: f(self.track),
        }
    }
}

/// Windows 11's light look (the colours the app always had).
pub const LIGHT: Palette<[u8; 3]> = Palette {
    bar: [0xF3, 0xF3, 0xF3],
    content: [0xFF, 0xFF, 0xFF],
    pane: [0xFA, 0xFA, 0xFA],
    line: [0xE5, 0xE5, 0xE5],
    text: [0x1A, 0x1A, 0x1A],
    dim: [0x5F, 0x5F, 0x5F],
    accent: [0x00, 0x67, 0xC0],
    accent_hover: [0x00, 0x5A, 0xA8],
    accent_press: [0x00, 0x4E, 0x92],
    on_accent: [0xFF, 0xFF, 0xFF],
    sel: [0xCC, 0xE8, 0xFF],
    nav_sel: [0xE6, 0xE6, 0xE6],
    row_hover: [0xEE, 0xEE, 0xEE],
    red: [0xC4, 0x2B, 0x1C],
    warn: [0xC2, 0x5E, 0x00],
    faded: [0x8A, 0x8A, 0x8A],
    new_text: [0x0B, 0x6A, 0x0B],
    field: [0xFF, 0xFF, 0xFF],
    widget: [0xF6, 0xF6, 0xF6],
    track: [0xE0, 0xE0, 0xE0],
};

/// Windows 11's dark look: lighter surfaces sit higher, text colours lightened for the dark ground.
pub const DARK: Palette<[u8; 3]> = Palette {
    bar: [0x20, 0x20, 0x20],
    content: [0x1C, 0x1C, 0x1C],
    pane: [0x27, 0x27, 0x27],
    line: [0x3D, 0x3D, 0x3D],
    text: [0xF2, 0xF2, 0xF2],
    dim: [0xC5, 0xC5, 0xC5],
    accent: [0x60, 0xCD, 0xFF],
    accent_hover: [0x5A, 0xB9, 0xE6],
    accent_press: [0x4F, 0xA4, 0xCC],
    on_accent: [0x00, 0x00, 0x00],
    sel: [0x12, 0x40, 0x5F],
    nav_sel: [0x3A, 0x3A, 0x3A],
    row_hover: [0x2F, 0x2F, 0x2F],
    red: [0xFF, 0x99, 0xA4],
    warn: [0xFF, 0xB4, 0x5C],
    faded: [0xA8, 0xA8, 0xA8],
    new_text: [0x6C, 0xCB, 0x5F],
    field: [0x2D, 0x2D, 0x2D],
    widget: [0x3A, 0x3A, 0x3A],
    track: [0x3D, 0x3D, 0x3D],
};

/// WCAG 2 contrast ratio of two opaque colours (1 to 21).
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let lum = |c: [u8; 3]| {
        let ch = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wcag_formula() {
        assert!((contrast([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 0.01);
        assert!((contrast([0x77, 0x77, 0x77], [255, 255, 255]) - 4.48).abs() < 0.01);
    }

    /// `top` at opacity `a` over `bg`, as egui blends (on sRGB values).
    fn over(bg: [u8; 3], top: [u8; 3], a: f32) -> [u8; 3] {
        std::array::from_fn(|i| (bg[i] as f32 * (1.0 - a) + top[i] as f32 * a).round() as u8)
    }

    #[test]
    fn blend_as_egui() {
        // the share bar on a selected dark row, as measured on a screenshot
        assert_eq!(over(DARK.sel, DARK.accent, 0.16), [0x1E, 0x57, 0x79]);
    }

    /// Every text colour against each background it is drawn on, as the window uses them.
    #[test]
    fn text_contrast_at_least_4_5() {
        for (name, p) in [("jasny", LIGHT), ("ciemny", DARK)] {
            let pairs: [(&str, [u8; 3], &[(&str, [u8; 3])]); 11] = [
                ("content", p.content, &[("text", p.text), ("dim", p.dim), ("new_text", p.new_text), ("faded", p.faded), ("red", p.red), ("accent", p.accent)]),
                // "Analiza miejsca": the size on its share bar (a selected or hovered dark row gets an outline instead)
                ("content+bar", over(p.content, p.accent, 0.16), &[("text", p.text), ("dim", p.dim), ("new_text", p.new_text), ("faded", p.faded)]),
                ("sel", p.sel, &[("text", p.text), ("dim", p.dim), ("new_text", p.new_text), ("faded", p.faded), ("accent", p.accent)]),
                // also the Operacje button while hovered or open: its error text in red
                ("nav_sel", p.nav_sel, &[("text", p.text), ("dim", p.dim), ("new_text", p.new_text), ("faded", p.faded), ("red", p.red)]),
                ("pane", p.pane, &[("text", p.text), ("dim", p.dim), ("accent", p.accent), ("red", p.red), ("new_text", p.new_text)]),
                ("row_hover", p.row_hover, &[("text", p.text), ("dim", p.dim)]),
                // the status bar: the Operacje button's check mark in new_text
                ("bar", p.bar, &[("text", p.text), ("dim", p.dim), ("red", p.red), ("accent", p.accent), ("new_text", p.new_text)]),
                ("field", p.field, &[("text", p.text), ("dim", p.dim), ("red", p.red), ("new_text", p.new_text), ("warn", p.warn), ("accent", p.accent)]),
                ("accent", p.accent, &[("on_accent", p.on_accent)]),
                ("accent_hover", p.accent_hover, &[("on_accent", p.on_accent)]),
                ("accent_press", p.accent_press, &[("on_accent", p.on_accent)]),
            ];
            for (bg_name, bg, texts) in pairs {
                for &(t_name, t) in texts {
                    // the light look stays as it was, with three known shortfalls: Explorer's grey of
                    // hidden files (3.4:1 on white, 2.7:1 selected), VirusTotal's orange (4.3:1) and
                    // the accent on a selected toggle (4.47:1)
                    if name == "jasny" && (t_name == "faded" || t_name == "warn" || (t_name, bg_name) == ("accent", "sel")) {
                        continue;
                    }
                    let c = contrast(t, bg);
                    assert!(c >= 4.5, "{name}: {t_name} na {bg_name} ma {c:.2}:1");
                }
            }
        }
    }
}
