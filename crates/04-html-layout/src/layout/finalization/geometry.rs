use crate::layout_model::{BoxGeometry, LineInlineBoxFragment};
use kurbo::Rect;

use super::{FinalizationScratch, FragmentWriter, LayoutReader};

pub(super) fn publish_inline_box_geometry(
    reader: &LayoutReader<'_>,
    geometry: &mut BoxGeometry,
    fragments: &FragmentWriter<'_>,
    scratch: &mut FinalizationScratch,
) {
    profile_scope!("finalize.inline_geometry");
    let bounds = &mut scratch.inline_bounds;
    bounds.clear();
    bounds.resize(reader.box_count(), None);
    let state = fragments.state();
    for line in &state.line_output.lines {
        let inline_range = line.inline_box_fragments(&state.line_output.line_details);
        let range = inline_range.start as usize..inline_range.end as usize;
        for fragment in state
            .line_output
            .inline_box_fragments
            .get(range)
            .unwrap_or_default()
        {
            let box_idx = fragment.box_idx as usize;
            // Replaced inline fragments already publish their independently
            // resolved border-box geometry during line placement.
            if fragment.flags & LineInlineBoxFragment::BORDER_BOX_BOUNDS != 0
                || !reader.is_inline_box(box_idx)
            {
                continue;
            }
            let style = reader.style(box_idx);
            let containing_width = reader
                .get_parent(box_idx)
                .map_or(0.0, |parent| geometry.size(parent).width);
            let inline_start = fragment.flags & LineInlineBoxFragment::INLINE_START != 0;
            let inline_end = fragment.flags & LineInlineBoxFragment::INLINE_END != 0;
            let left = if inline_start {
                style.padding_left().resolve(containing_width) + style.border_left_width() as f64
            } else {
                0.0
            };
            let right = if inline_end {
                style.padding_right().resolve(containing_width) + style.border_right_width() as f64
            } else {
                0.0
            };
            let top =
                style.padding_top().resolve(containing_width) + style.border_top_width() as f64;
            let bottom = style.padding_bottom().resolve(containing_width)
                + style.border_bottom_width() as f64;
            let rect = Rect::new(
                line.point.x + fragment.start_x as f64 - left,
                line.point.y + fragment.top as f64 - top,
                line.point.x + fragment.end_x as f64 + right,
                line.point.y + fragment.bottom as f64 + bottom,
            );
            bounds[box_idx] = Some(bounds[box_idx].map_or(rect, |current| current.union(rect)));
        }
    }
    for (box_idx, rect) in bounds.iter().copied().enumerate() {
        if let Some(rect) = rect {
            geometry.set_point(box_idx, rect.origin());
            geometry.set_size(box_idx, rect.size());
        }
    }
}
