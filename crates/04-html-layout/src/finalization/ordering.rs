//! Build indexes over finalized lines, glyphs, and image fragments.
use crate::paint::{PaintItem, PaintOrderCache};
use crate::{FragmentWriter, LayoutReader};
use html_engine_model::GroupedArena;
use html_engine_model::layout_state;
use std::ops::Range;

/// Build independent traversals over stable emission-order line storage.
pub(super) fn build_line_traversals(
    reader: &LayoutReader<'_>,
    fragments: &mut FragmentWriter<'_>,
    cache: &mut PaintOrderCache,
) {
    let output = &mut fragments.state_mut().line_output;
    output.line_glyph_offsets.freeze();
    output.line_glyph_advances.freeze();
    output
        .spatial_order
        .rebuild_unstable(output.lines.len(), |a, b| {
            let left = &output.lines[layout_state::line_id_index(*a)];
            let right = &output.lines[layout_state::line_id_index(*b)];
            (left.point.y + left.height)
                .total_cmp(&(right.point.y + right.height))
                .then_with(|| left.point.y.total_cmp(&right.point.y))
                .then_with(|| left.point.x.total_cmp(&right.point.x))
                .then_with(|| a.cmp(b))
        });
    let keys: Vec<_> = output
        .lines
        .iter()
        .map(|line| cache.key(reader, line.owner_box_idx as usize, PaintItem::Content))
        .collect();
    output
        .paint_order_indices
        .rebuild(output.lines.len(), |a, b| {
            let left = &output.lines[layout_state::line_id_index(*a)];
            let right = &output.lines[layout_state::line_id_index(*b)];
            keys[layout_state::line_id_index(*a)]
                .cmp(&keys[layout_state::line_id_index(*b)])
                .then_with(|| left.point.y.total_cmp(&right.point.y))
                .then_with(|| left.point.x.total_cmp(&right.point.x))
        });
    output
        .ellipsis_fragments
        .sort_unstable_by_key(|fragment| fragment.line_idx);
    output
        .hyphen_fragments
        .sort_unstable_by_key(|fragment| fragment.line_idx);
}

pub(super) fn prepare_line_metadata(
    fragments: &mut FragmentWriter<'_>,
    bounds: &mut Vec<Range<f64>>,
    carriers: &mut GroupedArena<usize>,
) {
    let layout = fragments.state_mut();
    bounds.clear();
    layout.line_output.hit_test_above = 0.0;
    layout.line_output.hit_test_below = 0.0;
    carriers.reset_groups(layout.line_output.lines.len());
    for (index, line) in layout.line_output.lines.iter().enumerate() {
        bounds.push(line.point.y..line.point.y + line.height);
        layout.line_output.hit_test_above = layout.line_output.hit_test_above.max(line.height);
        carriers.push_to(index, line.owner_box_idx as usize);
        let inline = line.inline_box_fragments(&layout.line_output.line_details);
        for fragment in
            &layout.line_output.inline_box_fragments[inline.start as usize..inline.end as usize]
        {
            carriers.push_to(index, fragment.box_idx as usize);
        }
    }
    let by_line = &mut layout.fragment_output.image_fragments_by_line;
    by_line.reset_groups(layout.line_output.lines.len());
    for (idx, fragment) in layout.fragment_output.image_fragments.iter().enumerate() {
        if fragment.line_idx < by_line.len() {
            by_line.push_to(fragment.line_idx, idx);
            carriers.push_to(fragment.line_idx, fragment.box_idx as usize);
            let visible = kurbo::Rect::from_origin_size(fragment.offset, fragment.size)
                .intersect(fragment.clip);
            let height = layout.line_output.lines[fragment.line_idx].height;
            if visible.width() > 0.0 && visible.height() > 0.0 {
                let y = layout.line_output.lines[fragment.line_idx].point.y;
                bounds[fragment.line_idx].start =
                    bounds[fragment.line_idx].start.min(y + visible.y0);
                bounds[fragment.line_idx].end = bounds[fragment.line_idx].end.max(y + visible.y1);
                layout.line_output.hit_test_above =
                    layout.line_output.hit_test_above.max(height - visible.y0);
                layout.line_output.hit_test_below =
                    layout.line_output.hit_test_below.max(visible.y1 - height);
            }
        }
    }
    by_line.freeze();
    carriers.freeze();
}

pub(super) fn rebuild_glyph_line_indices(glyph_count: usize, fragments: &mut FragmentWriter<'_>) {
    let layout = fragments.state_mut();
    layout
        .line_output
        .glyph_line_indices
        .reset(glyph_count, layout.line_output.lines.len());

    for (line_idx, line) in layout.line_output.lines.iter().enumerate() {
        let mut assign = |range: &Range<u32>| {
            let start = usize::try_from(range.start)
                .unwrap_or(usize::MAX)
                .min(glyph_count);
            let end = usize::try_from(range.end)
                .unwrap_or(usize::MAX)
                .min(glyph_count);
            if start < end {
                layout
                    .line_output
                    .glyph_line_indices
                    .assign(start..end, line_idx);
            }
        };
        if line.owns_glyph_range {
            assign(&line.glyphs);
            continue;
        }
        let Some(range) = line.text_fragment_range(&layout.line_output.line_details) else {
            assign(&line.glyphs);
            continue;
        };
        layout
            .line_output
            .text_fragments
            .for_each_glyph_span(range, |glyphs| assign(&glyphs));
    }
}
