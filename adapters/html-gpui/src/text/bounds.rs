use kurbo::Rect;

/// Account for font coordinate conventions, overhangs and shaping offsets.
pub(super) fn conservative_glyph_bounds(font: Rect, (x, y): (f64, f64)) -> Option<Rect> {
    if ![font.x0, font.y0, font.x1, font.y1, x, y].into_iter().all(f64::is_finite) {
        return None;
    }
    let width = font.x0.abs().max(font.x1.abs());
    let height = font.y0.abs().max(font.y1.abs());
    Some(Rect::new(x - width, y - height, x + width, y + height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_overhangs_and_positioned_combining_marks() {
        let bounds = conservative_glyph_bounds(Rect::new(-7.0, -25.0, 19.0, 8.0), (-12.0, -9.0)).unwrap();
        assert_eq!(bounds, Rect::new(-31.0, -34.0, 7.0, 16.0));
    }

    #[test]
    fn covers_both_font_vertical_coordinate_conventions() {
        let up = conservative_glyph_bounds(Rect::new(-7.0, -8.0, 19.0, 25.0), (0.0, 0.0));
        let down = conservative_glyph_bounds(Rect::new(-7.0, -25.0, 19.0, 8.0), (0.0, 0.0));
        assert_eq!(up, down);
    }

    #[test]
    fn invalid_metrics_disable_culling() {
        assert!(conservative_glyph_bounds(Rect::new(0.0, f64::NAN, 1.0, 2.0), (0.0, 0.0)).is_none());
        assert!(conservative_glyph_bounds(Rect::ZERO, (f64::INFINITY, 0.0)).is_none());
    }
}
