use crate::layout_model::{GlyphAdvanceRun, GlyphOffsetRun, Line, OverflowClip};
use html_style_model::{Display, Float, PositionMode};
use kurbo::Rect;
use std::ops::Range;
use std::time::Instant;

use super::engine::LayoutEngine;
use super::fragment_writer::FragmentWriter;
use super::geometry_writer::GeometryWriter;
use super::read_context::LayoutReader;

type PaintOrderKey = (u8, i32, usize, u8, usize);

/// Produces a compact CSS2 paint-order path for one fragment owner.
///
/// The first three fields identify the outer paint layer/context. The last
/// two retain ordering *inside* an atomic inline formatting root. Keeping the
/// atomic root in the key is important: an inline-block's backgrounds and
/// descendants paint together in the inline layer instead of being flattened
/// into the ordinary block-background layer.
fn paint_order_key(reader: &super::read_context::LayoutReader<'_>, box_idx: usize, ordinary_layer: u8) -> PaintOrderKey {
    let mut current = Some(box_idx);
    let mut nearest_positioned = None;
    let mut outer_stacking_context = None;
    let mut outer_float = None;
    let mut outer_atomic_inline = None;
    while let Some(idx) = current {
        let style = reader.style(idx);
        if style.position() != PositionMode::Static {
            nearest_positioned.get_or_insert(idx);
            if style.z_index().is_some() {
                outer_stacking_context = Some(idx);
            }
        }
        if matches!(style.float(), Float::Left | Float::Right) {
            outer_float = Some(idx);
        }
        if matches!(style.display(), Display::InlineBlock | Display::InlineTable | Display::InlineFlex | Display::InlineGrid) {
            outer_atomic_inline = Some(idx);
        }
        current = reader.get_parent(idx);
    }
    if let Some(context) = outer_stacking_context.or(nearest_positioned) {
        let style = reader.style(context);
        let z_index = style.z_index().unwrap_or(0);
        // Out-of-flow boxes may be materialized after later in-flow siblings,
        // so layout-box allocation order is not a valid CSS source order.
        let source_order = reader.box_at(context).and_then(|layout_box| layout_box.dom_element()).map_or(context, |node| node as usize);
        let layer = if z_index < 0 {
            0
        } else if z_index == 0 {
            3
        } else {
            4
        };
        return (layer, z_index, source_order, ordinary_layer, box_idx);
    }
    if let Some(atomic) = outer_atomic_inline {
        let inner_layer = if outer_float.is_some() { 2 } else { ordinary_layer };
        (3, 0, atomic, inner_layer, box_idx)
    } else if let Some(float) = outer_float {
        (2, 0, float, ordinary_layer, box_idx)
    } else {
        (ordinary_layer, 0, box_idx, ordinary_layer, box_idx)
    }
}

#[derive(Default)]
pub(super) struct FinalizationScratch {
    line_pairs: Vec<(Line, Vec<GlyphOffsetRun>, Vec<GlyphAdvanceRun>, usize)>,
    index_map: Vec<usize>,
    sorted_line_owners: Vec<u32>,
    block_groups: Vec<(u32, Range<u32>)>,
    content_clips: Vec<Option<OverflowClip>>,
    inline_bounds: Vec<Option<Rect>>,
}

impl FinalizationScratch {
    #[cfg(test)]
    pub(super) fn allocation_capacities(&self) -> (usize, usize, usize, usize, usize, usize) {
        (self.line_pairs.capacity(), self.index_map.capacity(), self.sorted_line_owners.capacity(), self.block_groups.capacity(), self.content_clips.capacity(), self.inline_bounds.capacity())
    }
}

impl LayoutEngine<'_, '_> {
    pub(super) fn finalize(&mut self, track_overflow_clips: bool) {
        let finalization_started = Instant::now();

        let start = Instant::now();
        sort_lines_and_remap_images(&self.reader, &mut self.fragments, &mut self.finalization);
        self.record_timing(|timings| timings.sort_lines_and_remap_images += start.elapsed());

        rebuild_glyph_line_indices(self.text.glyphs().len(), &mut self.fragments);
        build_block_decoration_traversal(&self.reader, &mut self.fragments, &mut self.finalization);

        let start = Instant::now();
        super::decorations::collect_inline_decorations(self);
        self.record_timing(|timings| timings.collect_inline_decorations += start.elapsed());

        publish_inline_box_geometry(&self.reader, &mut self.geometry, &self.fragments, &mut self.finalization);

        rebuild_decoration_fragments_by_line(&mut self.fragments);

        let start = Instant::now();
        rebuild_image_fragments_by_line(&mut self.fragments);
        self.record_timing(|timings| timings.rebuild_image_fragments_by_line += start.elapsed());

        rebuild_overflow_clips(&self.reader, &self.geometry, &mut self.fragments, &mut self.finalization, track_overflow_clips);
        self.record_timing(|timings| timings.finalize_layout += finalization_started.elapsed());
    }
}

fn publish_inline_box_geometry(reader: &LayoutReader<'_>, geometry: &mut GeometryWriter<'_>, fragments: &FragmentWriter<'_>, scratch: &mut FinalizationScratch) {
    let mut bounds = std::mem::take(&mut scratch.inline_bounds);
    bounds.resize(reader.box_count(), None);
    bounds.fill(None);
    let state = fragments.state();
    for line in &state.line_output.lines {
        let range = line.inline_box_fragments.start as usize..line.inline_box_fragments.end as usize;
        for fragment in state.line_output.inline_box_fragments.get(range).unwrap_or_default() {
            let box_idx = fragment.box_idx as usize;
            // Replaced inline fragments already publish their independently
            // resolved border-box geometry during line placement.
            if fragment.flags & crate::layout_model::LineInlineBoxFragment::BORDER_BOX_BOUNDS != 0
                || !matches!(reader.box_layout_mode(box_idx), Some(crate::layout_model::LayoutMode::Inline(_)))
            {
                continue;
            }
            let style = reader.style(box_idx);
            let containing_width = reader.get_parent(box_idx).map_or(0.0, |parent| geometry.size(parent).width);
            let inline_start = fragment.flags & crate::layout_model::LineInlineBoxFragment::INLINE_START != 0;
            let inline_end = fragment.flags & crate::layout_model::LineInlineBoxFragment::INLINE_END != 0;
            let left = if inline_start { style.padding_left().resolve(containing_width) + style.border_left_width() as f64 } else { 0.0 };
            let right = if inline_end { style.padding_right().resolve(containing_width) + style.border_right_width() as f64 } else { 0.0 };
            let top = style.padding_top().resolve(containing_width) + style.border_top_width() as f64;
            let bottom = style.padding_bottom().resolve(containing_width) + style.border_bottom_width() as f64;
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

fn sort_lines_and_remap_images(reader: &LayoutReader<'_>, fragments: &mut FragmentWriter<'_>, scratch: &mut FinalizationScratch) {
    let (layout, line_owners, _) = fragments.finalization_parts();
    let mut owners = std::mem::take(line_owners);
    let mut lines = std::mem::take(&mut layout.line_output.lines);
    let mut offsets_by_line = std::mem::take(&mut layout.line_output.line_glyph_offsets);
    let mut advances_by_line = std::mem::take(&mut layout.line_output.line_glyph_advances);
    assert_eq!(owners.len(), lines.len(), "every line must carry a formatting-context owner");
    let mut line_pairs = std::mem::take(&mut scratch.line_pairs);
    line_pairs.clear();
    line_pairs.extend(lines.drain(..).enumerate().map(|(idx, line)| (line, offsets_by_line.get_mut(idx).map(std::mem::take).unwrap_or_default(), advances_by_line.get_mut(idx).map(std::mem::take).unwrap_or_default(), idx)));
    offsets_by_line.clear();
    advances_by_line.clear();

    // CSS paint order and document geometry are different orderings. Keep a
    // compact paint traversal, but store the lines themselves top-to-bottom;
    // pagination, hit testing, and viewport binary searches require monotonic
    // vertical geometry.
    layout.line_output.paint_order_indices.clear();
    layout.line_output.paint_order_indices.extend((0..line_pairs.len()).map(|index| u32::try_from(index).expect("line index exceeds paint traversal capacity")));
    layout.line_output.paint_order_indices.sort_by(|a_idx, b_idx| {
        let a_idx = *a_idx as usize;
        let b_idx = *b_idx as usize;
        let a = &line_pairs[a_idx].0;
        let b = &line_pairs[b_idx].0;
        let a_owner = owners.get(a_idx).copied().unwrap_or_default() as usize;
        let b_owner = owners.get(b_idx).copied().unwrap_or_default() as usize;
        paint_order_key(reader, a_owner, 3)
            .cmp(&paint_order_key(reader, b_owner, 3))
            .then_with(|| a.point.y.partial_cmp(&b.point.y).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.point.x.partial_cmp(&b.point.x).unwrap_or(std::cmp::Ordering::Equal))
    });

    line_pairs
        .sort_by(|(a, _, _, a_idx), (b, _, _, b_idx)| (a.point.y + a.height).total_cmp(&(b.point.y + b.height)).then_with(|| a.point.y.total_cmp(&b.point.y)).then_with(|| a.point.x.total_cmp(&b.point.x)).then_with(|| a_idx.cmp(b_idx)));

    let mut index_map = std::mem::take(&mut scratch.index_map);
    index_map.resize(line_pairs.len(), 0);
    for (new_idx, (_, _, _, old_idx)) in line_pairs.iter().enumerate() {
        index_map[*old_idx] = new_idx;
    }
    for line_idx in &mut layout.line_output.paint_order_indices {
        *line_idx = u32::try_from(index_map[*line_idx as usize]).expect("line index exceeds paint traversal capacity");
    }

    let remap_flags = |flags: &mut Vec<bool>| {
        let old = std::mem::take(flags);
        flags.resize(index_map.len(), false);
        for (old_idx, value) in old.into_iter().enumerate().take(index_map.len()) {
            flags[index_map[old_idx]] = value;
        }
    };
    remap_flags(&mut layout.line_output.positioned_layers);
    remap_flags(&mut layout.line_output.negative_positioned_layers);
    remap_flags(&mut layout.line_output.independent_positioned_layers);

    for (line, offsets, advances, _) in line_pairs.drain(..) {
        lines.push(line);
        offsets_by_line.push(offsets);
        advances_by_line.push(advances);
    }
    layout.line_output.lines = lines;
    layout.line_output.line_glyph_offsets = offsets_by_line;
    layout.line_output.line_glyph_advances = advances_by_line;
    for frag in &mut layout.fragment_output.image_fragments {
        if let Some(&new_idx) = index_map.get(frag.line_idx) {
            frag.line_idx = new_idx;
        }
    }
    for line_idx in &mut layout.fragment_output.decoration_line_indices {
        if *line_idx != u32::MAX
            && let Some(&new_idx) = index_map.get(*line_idx as usize)
        {
            *line_idx = u32::try_from(new_idx).expect("line index exceeds decoration traversal capacity");
        }
    }
    for frag in &mut layout.line_output.ellipsis_fragments {
        if let Some(&new_idx) = index_map.get(frag.line_idx) {
            frag.line_idx = new_idx;
        }
    }
    layout.line_output.ellipsis_fragments.sort_unstable_by_key(|fragment| fragment.line_idx);
    for frag in &mut layout.line_output.hyphen_fragments {
        if let Some(&new_idx) = index_map.get(frag.line_idx) {
            frag.line_idx = new_idx;
        }
    }
    layout.line_output.hyphen_fragments.sort_unstable_by_key(|fragment| fragment.line_idx);
    let mut sorted_owners = std::mem::take(&mut scratch.sorted_line_owners);
    sorted_owners.resize(owners.len(), 0);
    for (old_idx, owner) in owners.drain(..).enumerate() {
        sorted_owners[index_map[old_idx]] = owner;
    }
    scratch.line_pairs = line_pairs;
    scratch.index_map = index_map;
    scratch.sorted_line_owners = owners;
    *line_owners = sorted_owners;
}

fn build_block_decoration_traversal(reader: &LayoutReader<'_>, fragments: &mut FragmentWriter<'_>, scratch: &mut FinalizationScratch) {
    let (layout, _, decoration_owners) = fragments.finalization_parts();
    let owners = std::mem::take(decoration_owners);
    assert_eq!(owners.len(), layout.fragment_output.decorations.len(), "every block decoration must carry a paint-order owner");
    layout.fragment_output.block_decoration_count = u32::try_from(owners.len()).expect("block decoration count exceeds traversal index capacity");
    layout.fragment_output.block_paint_ranges.clear();

    let mut groups = std::mem::take(&mut scratch.block_groups);
    groups.clear();
    let mut start = 0usize;
    while start < owners.len() {
        let owner = owners[start];
        let mut end = start + 1;
        while end < owners.len() && owners[end] == owner {
            end += 1;
        }
        groups.push((owner, u32::try_from(start).expect("decoration index exceeds traversal capacity")..u32::try_from(end).expect("decoration index exceeds traversal capacity")));
        start = end;
    }
    // Topology IDs are preorder. Sorting compact semantic groups retains
    // edge order within an owner while leaving fragment storage untouched.
    groups.sort_by_key(|(owner, range)| (paint_order_key(reader, *owner as usize, 1), range.start));
    layout.fragment_output.block_paint_ranges.extend(groups.drain(..).map(|(_, range)| range));
    scratch.block_groups = groups;
    *decoration_owners = owners;
}

/// Resolves overflow ownership once layout geometry is final. The common
/// `overflow: visible` path leaves both output vectors empty; renderers can
/// therefore test one slice and continue without issuing clip commands.
fn rebuild_overflow_clips(reader: &LayoutReader<'_>, geometry: &GeometryWriter<'_>, fragments: &mut FragmentWriter<'_>, scratch: &mut FinalizationScratch, track_overflow_clips: bool) {
    let box_count = reader.box_count();
    if !track_overflow_clips {
        let (layout, line_owners, decoration_owners) = fragments.finalization_parts();
        layout.line_output.line_clips.clear();
        layout.fragment_output.decoration_clips.clear();
        line_owners.clear();
        decoration_owners.clear();
        return;
    }

    fn combine(ancestor: Option<OverflowClip>, own: Option<OverflowClip>) -> Option<OverflowClip> {
        match (ancestor, own) {
            (Some(a), Some(b)) => {
                let x0 = if a.x && b.x {
                    a.rect.x0.max(b.rect.x0)
                } else if a.x {
                    a.rect.x0
                } else {
                    b.rect.x0
                };
                let x1 = if a.x && b.x {
                    a.rect.x1.min(b.rect.x1)
                } else if a.x {
                    a.rect.x1
                } else {
                    b.rect.x1
                };
                let y0 = if a.y && b.y {
                    a.rect.y0.max(b.rect.y0)
                } else if a.y {
                    a.rect.y0
                } else {
                    b.rect.y0
                };
                let y1 = if a.y && b.y {
                    a.rect.y1.min(b.rect.y1)
                } else if a.y {
                    a.rect.y1
                } else {
                    b.rect.y1
                };
                Some(OverflowClip { rect: Rect::new(x0, y0, x1.max(x0), y1.max(y0)), x: a.x || b.x, y: a.y || b.y })
            }
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    // Boxes are built in DOM preorder, so an owning parent's resolved clip
    // is available before every child. This is also the ordering used for
    // decoration paint keys.
    let geometry = geometry.as_ref();
    let (layout, line_owners, decoration_owners) = fragments.finalization_parts();
    let mut content_clips = std::mem::take(&mut scratch.content_clips);
    content_clips.resize(box_count, None);
    content_clips.fill(None);
    for idx in 0..box_count {
        let parent_clip = reader.get_parent(idx).and_then(|parent| {
            debug_assert!(parent < idx, "layout boxes must be stored in parent-before-child order");
            content_clips.get(parent).copied().flatten()
        });
        let style = reader.style(idx);
        let (overflow_x, overflow_y) = reader.effective_overflow_modes(idx);
        let clip_x = overflow_x.clips();
        let clip_y = overflow_y.clips();
        let own_clip = (clip_x || clip_y).then(|| {
            let point = geometry.point(idx);
            let size = geometry.size(idx);
            let left = style.border_left_width() as f64;
            let top = style.border_top_width() as f64;
            let right = style.border_right_width() as f64;
            let bottom = style.border_bottom_width() as f64;
            OverflowClip { rect: Rect::new(point.x + left, point.y + top, (point.x + size.width - right).max(point.x + left), (point.y + size.height - bottom).max(point.y + top)), x: clip_x, y: clip_y }
        });
        content_clips[idx] = combine(parent_clip, own_clip);
    }

    assert_eq!(line_owners.len(), layout.line_output.lines.len(), "every final line must retain its owner until clip resolution");
    assert_eq!(decoration_owners.len(), layout.fragment_output.decorations.len(), "every final decoration must retain its owner until clip resolution");
    let mut line_clips = std::mem::take(&mut layout.line_output.line_clips);
    line_clips.clear();
    line_clips.extend(line_owners.iter().map(|&owner| content_clips.get(owner as usize).copied().flatten()));
    let mut decoration_clips = std::mem::take(&mut layout.fragment_output.decoration_clips);
    decoration_clips.clear();
    decoration_clips.extend(decoration_owners.iter().map(|&owner| reader.get_parent(owner as usize).and_then(|parent| content_clips.get(parent).copied().flatten())));
    layout.line_output.line_clips = line_clips;
    layout.fragment_output.decoration_clips = decoration_clips;
    scratch.content_clips = content_clips;
    line_owners.clear();
    decoration_owners.clear();
}

fn rebuild_image_fragments_by_line(fragments: &mut FragmentWriter<'_>) {
    let layout = fragments.state_mut();
    layout.fragment_output.image_fragments_by_line.resize_with(layout.line_output.lines.len(), Vec::new);
    layout.fragment_output.image_fragments_by_line.truncate(layout.line_output.lines.len());
    for (idx, frag) in layout.fragment_output.image_fragments.iter().enumerate() {
        if let Some(line) = layout.fragment_output.image_fragments_by_line.get_mut(frag.line_idx) {
            line.push(idx);
        }
    }
}

fn rebuild_decoration_fragments_by_line(fragments: &mut FragmentWriter<'_>) {
    let layout = fragments.state_mut();
    let paint_orders = &layout.fragment_output.decoration_paint_orders;
    let by_line = &mut layout.fragment_output.decoration_fragments_by_line;
    by_line.resize_with(layout.line_output.lines.len(), Vec::new);
    by_line.truncate(layout.line_output.lines.len());
    for indexes in by_line.iter_mut() {
        indexes.clear();
    }

    let block_count = layout.fragment_output.block_decoration_count as usize;
    let traversal = layout.fragment_output.block_paint_ranges.iter().flat_map(|range| range.clone().map(|index| index as usize)).chain(block_count..layout.fragment_output.decorations.len());
    for decoration_idx in traversal {
        let Some(&line_idx) = layout.fragment_output.decoration_line_indices.get(decoration_idx) else { continue };
        if line_idx == u32::MAX {
            continue;
        }
        let same_paint_layer = layout.fragment_output.decoration_positioned_layers.get(decoration_idx).copied().unwrap_or(false) == layout.line_output.positioned_layers.get(line_idx as usize).copied().unwrap_or(false)
            && layout.fragment_output.decoration_negative_positioned_layers.get(decoration_idx).copied().unwrap_or(false) == layout.line_output.negative_positioned_layers.get(line_idx as usize).copied().unwrap_or(false)
            && layout.fragment_output.decoration_independent_positioned_layers.get(decoration_idx).copied().unwrap_or(false) == layout.line_output.independent_positioned_layers.get(line_idx as usize).copied().unwrap_or(false);
        if same_paint_layer {
            if let Some(line) = by_line.get_mut(line_idx as usize) {
                line.push(decoration_idx);
            }
        } else if let Some(owner) = layout.fragment_output.decoration_line_indices.get_mut(decoration_idx) {
            // A decoration cannot be replayed with a line from another CSS
            // paint layer. Publish it as line-independent so the global pass
            // paints it exactly once in its own positioned layer.
            *owner = u32::MAX;
        }
    }
    for indexes in by_line.iter_mut() {
        // `traversal` already carries CSS tree order (ancestor backgrounds
        // before descendant backgrounds). Keep that stable order when an
        // atomic inline assigns the same token paint slot to its whole
        // subtree; using the raw emission index as a tie-breaker reverses
        // parents and children because block backgrounds emit post-order.
        indexes.sort_by_key(|&index| paint_orders.get(index).copied().unwrap_or(u32::MAX));
    }
}

fn rebuild_glyph_line_indices(glyph_count: usize, fragments: &mut FragmentWriter<'_>) {
    const NO_LINE: u32 = u32::MAX;
    let layout = fragments.state_mut();
    layout.line_output.glyph_line_indices.resize(glyph_count, NO_LINE);
    layout.line_output.glyph_line_indices.fill(NO_LINE);

    for (line_idx, line) in layout.line_output.lines.iter().enumerate() {
        let line_idx = u32::try_from(line_idx).expect("layout line count exceeds glyph-line index capacity");
        let mut assign = |range: &Range<u32>| {
            let start = usize::try_from(range.start).unwrap_or(usize::MAX).min(glyph_count);
            let end = usize::try_from(range.end).unwrap_or(usize::MAX).min(glyph_count);
            if start < end {
                layout.line_output.glyph_line_indices[start..end].fill(line_idx);
            }
        };
        if let Some(fragments) = &line.text_fragments {
            for fragment in fragments.iter() {
                assign(&fragment.glyphs);
            }
        } else {
            assign(&line.glyphs);
        }
    }
}
