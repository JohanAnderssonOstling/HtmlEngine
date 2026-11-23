use html_engine_model::{BoxGeometry, LineInlineBoxFragment};
use kurbo::Rect;

use super::{FinalizationScratch, FragmentWriter, LayoutReader};

pub(super) fn publish_inline_box_geometry(
    reader: &LayoutReader<'_>,
    geometry: &mut BoxGeometry,
    fragments: &FragmentWriter<'_>,
    scratch: &mut FinalizationScratch,
) {
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
            let rect = crate::decorations::inline_border_rect(
                reader.style(box_idx),
                line.point,
                fragment,
                line.details(&state.line_output.line_details)
                    .map_or(0.0, |details| details.inline_containing_width),
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
