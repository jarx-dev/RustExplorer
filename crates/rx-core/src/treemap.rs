//! Squarified treemap layout (Bruls, Huizing, van Wijk 2000), as WizTree draws a folder: each item
//! gets a share of a rectangle in proportion to its size, in tiles kept close to squares.

/// Lays `sizes` (biggest first) into the rectangle `[x, y, w, h]`; one `[x, y, w, h]` per size, in
/// the same order. Rows run along the shorter side and grow while their worst aspect ratio improves.
pub fn squarify(sizes: &[u64], r: [f32; 4]) -> Vec<[f32; 4]> {
    let total: f64 = sizes.iter().map(|&s| s as f64).sum();
    if total <= 0.0 || r[2] <= 0.0 || r[3] <= 0.0 {
        return vec![[r[0], r[1], 0.0, 0.0]; sizes.len()];
    }
    let scale = r[2] as f64 * r[3] as f64 / total;
    let areas: Vec<f64> = sizes.iter().map(|&s| s as f64 * scale).collect();
    let (mut x, mut y, mut w, mut h) = (r[0] as f64, r[1] as f64, r[2] as f64, r[3] as f64);
    let mut out = Vec::with_capacity(sizes.len());
    let mut i = 0;
    while i < areas.len() {
        let side = w.min(h);
        // the worst width:height of a row holding `sum` with its biggest and smallest item
        let worst = |sum: f64, big: f64, small: f64| {
            let (s2, sum2) = (side * side, sum * sum);
            (s2 * big / sum2).max(sum2 / (s2 * small))
        };
        let (mut j, mut sum) = (i + 1, areas[i]);
        while j < areas.len() && worst(sum + areas[j], areas[i], areas[j]) <= worst(sum, areas[i], areas[j - 1]) {
            sum += areas[j];
            j += 1;
        }
        // the row's thickness across the shorter side; an empty row (zero sizes) is a line
        let thick = if side > 0.0 { sum / side } else { 0.0 };
        let mut at = 0.0;
        for &a in &areas[i..j] {
            let len = if thick > 0.0 { a / thick } else { 0.0 };
            out.push(if w >= h { [x, y + at, thick, len] } else { [x + at, y, len, thick] }.map(|v| v as f32));
            at += len;
        }
        if w >= h {
            (x, w) = (x + thick, (w - thick).max(0.0));
        } else {
            (y, h) = (y + thick, (h - thick).max(0.0));
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(r: &[f32; 4]) -> f64 {
        r[2] as f64 * r[3] as f64
    }

    #[test]
    fn fills_the_rectangle_in_proportion() {
        let sizes = [600, 300, 60, 25, 10, 4, 1];
        let rs = squarify(&sizes, [10.0, 20.0, 400.0, 150.0]);
        assert_eq!(rs.len(), sizes.len());
        let total: f64 = rs.iter().map(area).sum();
        assert!((total - 400.0 * 150.0).abs() < 1.0, "{total}");
        for (r, &s) in rs.iter().zip(&sizes) {
            assert!((area(r) - 60_000.0 * s as f64 / 1000.0).abs() < 1.0);
            assert!(r[0] >= 10.0 - 0.01 && r[1] >= 20.0 - 0.01 && r[0] + r[2] <= 410.01 && r[1] + r[3] <= 170.01, "{r:?}");
        }
    }

    #[test]
    fn equal_items_come_out_square() {
        let rs = squarify(&[1, 1, 1, 1], [0.0, 0.0, 100.0, 100.0]);
        for r in rs {
            assert!((r[2] - 50.0).abs() < 0.01 && (r[3] - 50.0).abs() < 0.01, "{r:?}");
        }
    }

    #[test]
    fn nothing_to_lay_out_is_not_a_crash() {
        assert_eq!(squarify(&[], [0.0, 0.0, 10.0, 10.0]).len(), 0);
        assert_eq!(squarify(&[0, 0], [0.0, 0.0, 10.0, 10.0]), vec![[0.0, 0.0, 0.0, 0.0]; 2]);
        assert_eq!(squarify(&[5], [1.0, 1.0, 0.0, 10.0]).len(), 1);
        // zero sizes after real ones take no room and do not stall the loop
        let rs = squarify(&[5, 0, 0], [0.0, 0.0, 10.0, 10.0]);
        assert_eq!(rs.len(), 3);
        assert!((area(&rs[0]) - 100.0).abs() < 0.01);
    }
}
