//! The app icon, drawn in code: a blue Windows 11 tile with a yellow folder holding two panes
//! (the split view), one row green like a new file and one blue like a selection.

/// Signed distance (in pixels) from (x, y) to a rounded rectangle; negative inside.
fn rounded_rect(x: f32, y: f32, r: [f32; 4], radius: f32) -> f32 {
    let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    let (hx, hy) = ((r[2] - r[0]) / 2.0 - radius, (r[3] - r[1]) / 2.0 - radius);
    let (dx, dy) = ((x - cx).abs() - hx, (y - cy).abs() - hy);
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    outside + dx.max(dy).min(0.0) - radius
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn hex(c: u32) -> [f32; 3] {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]
}

/// RGBA pixels of the icon at `n` × `n`.
pub fn rgba(n: u32) -> Vec<u8> {
    let s = n as f32;
    let mut out = Vec::with_capacity((n * n * 4) as usize);
    for py in 0..n {
        for px in 0..n {
            let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
            let v = y / s;
            let mut col = [0.0f32; 3];
            let mut alpha = 0.0f32;
            // paint one shape: unit-space rectangle, unit radius, colour, opacity
            let mut paint = |r: [f32; 4], rad: f32, c: [f32; 3], op: f32, soft: f32| {
                let d = rounded_rect(x, y, r.map(|k| k * s), rad * s);
                let cov = (0.5 - d / soft.max(1.0)).clamp(0.0, 1.0) * op;
                for k in 0..3 {
                    col[k] = col[k] * (1.0 - cov) + c[k] * cov;
                }
                alpha = alpha + cov * (1.0 - alpha);
            };
            // tile with a top-to-bottom blue gradient
            paint([0.04, 0.04, 0.96, 0.96], 0.21, mix(hex(0x47A6F0), hex(0x0A56A8), v), 1.0, 1.0);
            // soft shadow under the folder
            paint([0.17, 0.30, 0.85, 0.84], 0.07, [0.0, 0.0, 0.0], 0.28, s * 0.04);
            // folder back with its tab
            let back = hex(0xE09B12);
            paint([0.18, 0.22, 0.45, 0.34], 0.04, back, 1.0, 1.0);
            paint([0.18, 0.28, 0.82, 0.78], 0.05, back, 1.0, 1.0);
            // folder front, light at the top
            paint([0.15, 0.37, 0.85, 0.80], 0.06, mix(hex(0xFFD86A), hex(0xF4AE00), (v - 0.37) / 0.43), 1.0, 1.0);
            // two panes
            let white = hex(0xFFFFFF);
            for (x0, x1) in [(0.22, 0.485), (0.515, 0.78)] {
                paint([x0, 0.45, x1, 0.74], 0.03, white, 0.95, 1.0);
                for (k, yy) in [0.505, 0.57, 0.635, 0.70].iter().enumerate() {
                    let c = match (x0 < 0.3, k) {
                        (true, 1) => hex(0x2EA043),  // a new file, highlighted green
                        (false, 0) => hex(0x0067C0), // the selected row
                        _ => hex(0xB4BEC8),
                    };
                    let w = if k == 3 { 0.6 } else { 1.0 };
                    paint([x0 + 0.03, yy - 0.014, x0 + 0.03 + (x1 - x0 - 0.06) * w, yy + 0.014], 0.014, c, 1.0, 1.0);
                }
            }
            let a = alpha.clamp(0.0, 1.0);
            // un-premultiply: col was blended over transparent black
            let c = if a > 0.0 { col.map(|k| (k / a).clamp(0.0, 1.0)) } else { [0.0; 3] };
            out.extend([c[0], c[1], c[2], a].map(|k| (k * 255.0).round() as u8));
        }
    }
    out
}

/// Sizes drawn for the small pictures of the .ico (the 256 one is the PNG given to [`ico`]).
const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 128];

/// A Windows .ico with every size the shell asks for (16…128 as 32-bit bitmaps, 256 as the given
/// PNG): with only the 256 picture the taskbar could end up with the blank icon.
pub fn ico(png: &[u8]) -> Vec<u8> {
    let mut pics: Vec<(u32, Vec<u8>)> = ICO_SIZES.iter().map(|&n| (n, dib(n))).collect();
    pics.push((256, png.to_vec()));
    let mut v = vec![0, 0, 1, 0];
    v.extend((pics.len() as u16).to_le_bytes());
    let mut at = 6 + 16 * pics.len() as u32;
    for (n, data) in &pics {
        let side = if *n >= 256 { 0 } else { *n as u8 }; // 0 means 256
        v.extend([side, side, 0, 0, 1, 0, 32, 0]); // no palette, 1 plane, 32 bpp
        v.extend((data.len() as u32).to_le_bytes());
        v.extend(at.to_le_bytes());
        at += data.len() as u32;
    }
    for (_, data) in pics {
        v.extend(data);
    }
    v
}

/// The icon at `n` px as an .ico bitmap: header (height doubled for the mask), BGRA rows bottom
/// up, then the 1-bit mask, set where the picture is see-through (for programs that skip alpha).
fn dib(n: u32) -> Vec<u8> {
    let px = rgba(n);
    let mask_row = n.div_ceil(32) * 4;
    let mut v = Vec::with_capacity((40 + n * n * 4 + mask_row * n) as usize);
    v.extend(40u32.to_le_bytes());
    v.extend(n.to_le_bytes());
    v.extend((2 * n).to_le_bytes());
    v.extend(1u16.to_le_bytes());
    v.extend(32u16.to_le_bytes());
    v.extend([0u8; 24]); // no compression, sizes and colours left to Windows
    for y in (0..n).rev() {
        for x in 0..n {
            let i = ((y * n + x) * 4) as usize;
            v.extend([px[i + 2], px[i + 1], px[i], px[i + 3]]);
        }
    }
    for y in (0..n).rev() {
        let mut row = vec![0u8; mask_row as usize];
        for x in 0..n {
            if px[((y * n + x) * 4 + 3) as usize] < 128 {
                row[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        v.extend(row);
    }
    v
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_ico_lists_every_size_where_its_picture_is() {
        let png = b"\x89PNG fake".to_vec();
        let v = super::ico(&png);
        let count = u16::from_le_bytes([v[4], v[5]]) as usize;
        assert_eq!(count, 9);
        let u32_at = |i: usize| u32::from_le_bytes([v[i], v[i + 1], v[i + 2], v[i + 3]]);
        for k in 0..count {
            let e = 6 + 16 * k;
            let (size, at) = (u32_at(e + 8) as usize, u32_at(e + 12) as usize);
            assert!(at + size <= v.len());
            if k + 1 == count {
                assert_eq!((v[e], &v[at..at + size]), (0, &png[..]));
            } else {
                let n = v[e] as u32;
                assert_eq!(u32_at(at + 4), n); // the bitmap's width
                assert_eq!(u32_at(at + 8), 2 * n); // its height counts the mask too
                assert_eq!(size as u32, 40 + n * n * 4 + n.div_ceil(32) * 4 * n);
                // the top-left corner is see-through: its mask bit (last mask row, first bit) is set
                assert_eq!(v[at + size - (n.div_ceil(32) * 4) as usize] & 0x80, 0x80);
            }
        }
    }

    #[test]
    fn icon_is_opaque_in_the_middle_and_clear_in_the_corner() {
        let n = 64;
        let px = super::rgba(n);
        assert_eq!(px.len(), (n * n * 4) as usize);
        let at = |x: u32, y: u32| px[((y * n + x) * 4 + 3) as usize];
        assert_eq!(at(32, 32), 255);
        assert_eq!(at(0, 0), 0);
    }
}
