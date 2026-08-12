use kurbo::Rect;

use super::{FinalizationScratch, FragmentWriter, GeometryWriter, LayoutReader};

pub(super) fn publish_inline_box_geometry(
    reader: &LayoutReader<'_>,
    geometry: &mut GeometryWriter<'_>,
    fragments: &FragmentWriter<'_>,
    scratch: &mut FinalizationScratch,
) {
    let mut bounds = std::mem::take(&mut scratch.inline_bounds);
    bounds.resize(reader.box_count(), None);
    bounds.fill(None);
    let state = fragments.state();
    for line in &state.line_output.lines {
        let range =
            line.inline_box_fragments.start as usize..line.inline_box_fragments.end as usize;
        for fragment in state
            .line_output
            .inline_box_fragments
            .get(range)
            .unwrap_or_default()
        {
            let box_idx = fragment.box_idx as usize;
            // Replaced inline fragments already publish their independently
            // resolved border-box geometry during line placement.
            if fragment.flags & crate::layout_model::LineInlineBoxFragment::BORDER_BOX_BOUNDS != 0
                || !matches!(
                    reader.box_layout_mode(box_idx),
                    Some(crate::layout_model::LayoutMode::Inline(_))
                )
            {
                continue;
            }
            let style = reader.style(box_idx);
            let containing_width = reader
                .get_parent(box_idx)
                .map_or(0.0, |parent| geometry.size(parent).width);
            let inline_start =
                fragment.flags & crate::layout_model::LineInlineBoxFragment::INLINE_START != 0;
            let inline_end =
                fragment.flags & crate::layout_model::LineInlineBoxFragment::INLINE_END != 0;
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
    scratch.inline_bounds = bounds;
}
