//! Geometry-dependent decoration recording and shared border descriptions.
//! Layout records retained commands here; paint resolves their appearance later.

mod borders;
mod recording;

pub(crate) use borders::{PhysicalBorderEdge, border_pattern, physical_borders};
pub(crate) use recording::{
    record_block_background, record_block_border_and_outline, record_block_decorations,
    record_bound_background, record_bound_border,
};

use html_engine_model::LineInlineBoxFragment;
use html_style_model::UsedStyleView;
use kurbo::{Point, Rect};

/// Use the layout-time percentage basis for both published bounds and paint.
pub(crate) fn inline_border_rect(
    style: UsedStyleView<'_>,
    point: Point,
    fragment: &LineInlineBoxFragment,
    containing_width: f64,
) -> Rect {
    let mut rect = Rect::new(
        point.x + f64::from(fragment.start_x),
        point.y + f64::from(fragment.top),
        point.x + f64::from(fragment.end_x),
        point.y + f64::from(fragment.bottom),
    );
    if fragment.flags & LineInlineBoxFragment::BORDER_BOX_BOUNDS == 0 {
        if fragment.flags & LineInlineBoxFragment::INLINE_START != 0 {
            rect.x0 -= style.padding_left().resolve(containing_width)
                + f64::from(style.border_left_width());
        }
        if fragment.flags & LineInlineBoxFragment::INLINE_END != 0 {
            rect.x1 += style.padding_right().resolve(containing_width)
                + f64::from(style.border_right_width());
        }
        rect.y0 -=
            style.padding_top().resolve(containing_width) + f64::from(style.border_top_width());
        rect.y1 += style.padding_bottom().resolve(containing_width)
            + f64::from(style.border_bottom_width());
    }
    rect
}
