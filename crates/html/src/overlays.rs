//! Styled source ranges painted through caller-supplied line placements.
use crate::geometry::Geometry;
use crate::layout::RenderView;
use crate::render::{LinePlacement, Painter};
use kurbo::Rect;
use peniko::Color;
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayStyle {
    Highlight,
    Underline,
    Squiggly,
    Strikethrough,
}

/// Appearance and logical range supplied by the host. The engine chooses
/// intrinsic clipping and draws the resulting projected rectangles.
pub struct TextOverlay {
    pub range: Range<u32>,
    pub style: OverlayStyle,
    pub color: Color,
}

#[derive(Default)]
pub struct OverlayPaintScratch {
    rects: Vec<((OverlayStyle, Color), Rect)>,
}

pub(crate) fn paint_overlays(
    view: RenderView<'_>,
    overlays: impl IntoIterator<Item = TextOverlay>,
    placements: impl IntoIterator<Item = LinePlacement>,
    painter: &mut impl Painter,
    scratch: &mut OverlayPaintScratch,
) {
    scratch.rects.clear();
    Geometry::new(view).highlights(
        overlays
            .into_iter()
            .map(|overlay| ((overlay.style, overlay.color), overlay.range)),
        placements,
        &mut scratch.rects,
    );
    for &((style, color), bounds) in &scratch.rects {
        paint_overlay(painter, bounds, style, color);
    }
}

fn paint_overlay(painter: &mut impl Painter, bounds: Rect, style: OverlayStyle, color: Color) {
    let thickness = 2.0;
    let rect = match style {
        OverlayStyle::Highlight => bounds,
        OverlayStyle::Squiggly => {
            for rect in squiggly_rects(bounds.x0, bounds.x1, bounds.y1) {
                painter.fill_rect(rect, color);
            }
            return;
        }
        OverlayStyle::Underline => {
            Rect::new(bounds.x0, bounds.y1 - thickness, bounds.x1, bounds.y1)
        }
        OverlayStyle::Strikethrough => Rect::new(
            bounds.x0,
            (bounds.y0 + bounds.y1) / 2.0,
            bounds.x1,
            (bounds.y0 + bounds.y1) / 2.0 + thickness,
        ),
    };
    painter.fill_rect(rect, color);
}

/// Subpixel strips trace a wave using the portable painter's rectangle
/// primitive. Keep the whole stroke inside the line's lower edge.
fn squiggly_rects(x0: f64, x1: f64, bottom: f64) -> impl Iterator<Item = Rect> {
    const STEP: f64 = 0.5;
    const AMPLITUDE: f64 = 1.5;
    const THICKNESS: f64 = 1.5;
    const WAVELENGTH: f64 = 6.0;
    let count = ((x1 - x0).max(0.0) / STEP).ceil() as usize;
    (0..count).map(move |index| {
        let left = x0 + index as f64 * STEP;
        let right = (left + STEP).min(x1);
        let phase = ((left + right) / 2.0 - x0) * std::f64::consts::TAU / WAVELENGTH;
        let center = bottom - AMPLITUDE - THICKNESS / 2.0 + AMPLITUDE * phase.sin();
        Rect::new(
            left,
            center - THICKNESS / 2.0,
            right,
            center + THICKNESS / 2.0,
        )
    })
}

#[cfg(test)]
mod render_tests {
    use super::*;

    #[test]
    fn squiggly_stroke_waves_and_stays_inside_its_text_range() {
        let rects: Vec<_> = squiggly_rects(10.25, 28.4, 40.0).collect();
        assert_eq!(rects.first().unwrap().x0, 10.25);
        assert_eq!(rects.last().unwrap().x1, 28.4);
        assert!(rects.iter().all(|rect| rect.y0 >= 35.5 && rect.y1 <= 40.0));
        assert!(rects.windows(2).all(|pair| pair[0].x1 == pair[1].x0));
        assert!(rects.iter().any(|rect| rect.y0 > 38.0));
        assert!(rects.iter().any(|rect| rect.y0 < 36.0));
        assert!(squiggly_rects(10.0, 10.0, 40.0).next().is_none());
    }
}
