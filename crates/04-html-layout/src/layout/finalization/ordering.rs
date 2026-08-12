use html_style_model::{Display, Float, PositionMode};
use std::ops::Range;

use super::{FinalizationScratch, FragmentWriter, LayoutReader};
use crate::layout::PlacementState;

type PaintOrderKey = (u8, i32, usize, u8, usize);

/// Produces a compact CSS2 paint-order path for one fragment owner.
///
/// The first three fields identify the outer paint layer/context. The last
/// two retain ordering *inside* an atomic inline formatting root. Keeping the
/// atomic root in the key is important: an inline-block's backgrounds and
/// descendants paint together in the inline layer instead of being flattened
/// into the ordinary block-background layer.
fn paint_order_key(reader: &LayoutReader<'_>, box_idx: usize, ordinary_layer: u8) -> PaintOrderKey {
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
        if matches!(
            style.display(),
            Display::InlineBlock | Display::InlineTable | Display::InlineFlex | Display::InlineGrid
        ) {
            outer_atomic_inline = Some(idx);
        }
        current = reader.get_parent(idx);
    }
    if let Some(context) = outer_stacking_context.or(nearest_positioned) {
        let style = reader.style(context);
        let z_index = style.z_index().unwrap_or(0);
        // Out-of-flow boxes may be materialized after later in-flow siblings,
        // so layout-box allocation order is not a valid CSS source order.
        let source_order = reader
            .box_at(context)
            .and_then(|layout_box| layout_box.dom_element())
            .map_or(context, |node| node as usize);
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
        let inner_layer = if outer_float.is_some() {
            2
        } else {
            ordinary_layer
        };
        (3, 0, atomic, inner_layer, box_idx)
    } else if let Some(float) = outer_float {
        (2, 0, float, ordinary_layer, box_idx)
    } else {
        (ordinary_layer, 0, box_idx, ordinary_layer, box_idx)
    }
}

pub(super) fn sort_lines_and_remap_images(
    reader: &LayoutReader<'_>,
    fragments: &mut FragmentWriter<'_>,
    placement: &mut PlacementState,
    scratch: &mut FinalizationScratch,
) {
    let (layout, line_owners, _) = fragments.finalization_parts();
    let mut owners = std::mem::take(line_owners);
    let mut lines = std::mem::take(&mut layout.line_output.lines);
    let mut offsets_by_line = std::mem::take(&mut layout.line_output.line_glyph_offsets);
    let mut advances_by_line = std::mem::take(&mut layout.line_output.line_glyph_advances);
    assert_eq!(
        owners.len(),
        lines.len(),
        "every line must carry a formatting-context owner"
    );
    let mut line_pairs = std::mem::take(&mut scratch.line_pairs);
    line_pairs.clear();
    line_pairs.extend(lines.drain(..).enumerate().map(|(idx, line)| {
        (
            line,
            offsets_by_line
                .get_mut(idx)
                .map(std::mem::take)
                .unwrap_or_default(),
            advances_by_line
                .get_mut(idx)
                .map(std::mem::take)
                .unwrap_or_default(),
            idx,
        )
    }));
    offsets_by_line.clear();
    advances_by_line.clear();

    // CSS paint order and document geometry are different orderings. Keep a
    // compact paint traversal, but store the lines themselves top-to-bottom;
    // pagination, hit testing, and viewport binary searches require monotonic
    // vertical geometry.
    layout.line_output.paint_order_indices.clear();
    layout.line_output.paint_order_indices.extend(
        (0..line_pairs.len()).map(|index| {
            u32::try_from(index).expect("line index exceeds paint traversal capacity")
        }),
    );
    layout
        .line_output
        .paint_order_indices
        .sort_by(|a_idx, b_idx| {
            let a_idx = *a_idx as usize;
            let b_idx = *b_idx as usize;
            let a = &line_pairs[a_idx].0;
            let b = &line_pairs[b_idx].0;
            let a_owner = owners.get(a_idx).copied().unwrap_or_default() as usize;
            let b_owner = owners.get(b_idx).copied().unwrap_or_default() as usize;
            paint_order_key(reader, a_owner, 3)
                .cmp(&paint_order_key(reader, b_owner, 3))
                .then_with(|| {
                    a.point
                        .y
                        .partial_cmp(&b.point.y)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    a.point
                        .x
                        .partial_cmp(&b.point.x)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });

    line_pairs.sort_by(|(a, _, _, a_idx), (b, _, _, b_idx)| {
        (a.point.y + a.height)
            .total_cmp(&(b.point.y + b.height))
            .then_with(|| a.point.y.total_cmp(&b.point.y))
            .then_with(|| a.point.x.total_cmp(&b.point.x))
            .then_with(|| a_idx.cmp(b_idx))
    });

    let mut index_map = std::mem::take(&mut scratch.index_map);
    index_map.resize(line_pairs.len(), 0);
    for (new_idx, (_, _, _, old_idx)) in line_pairs.iter().enumerate() {
        index_map[*old_idx] = new_idx;
    }
    placement.remap_lines(&index_map);
    for line_idx in &mut layout.line_output.paint_order_indices {
        *line_idx = u32::try_from(index_map[*line_idx as usize])
            .expect("line index exceeds paint traversal capacity");
    }
    layout.line_output.paint_order_ranks.clear();
    layout
        .line_output
        .paint_order_ranks
        .resize(layout.line_output.paint_order_indices.len(), 0);
    for (rank, &line_idx) in layout.line_output.paint_order_indices.iter().enumerate() {
        layout.line_output.paint_order_ranks[line_idx as usize] =
            u32::try_from(rank).expect("paint rank exceeds traversal capacity");
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
            *line_idx =
                u32::try_from(new_idx).expect("line index exceeds decoration traversal capacity");
        }
    }
    for frag in &mut layout.line_output.ellipsis_fragments {
        if let Some(&new_idx) = index_map.get(frag.line_idx) {
            frag.line_idx = new_idx;
        }
    }
    layout
        .line_output
        .ellipsis_fragments
        .sort_unstable_by_key(|fragment| fragment.line_idx);
    for frag in &mut layout.line_output.hyphen_fragments {
        if let Some(&new_idx) = index_map.get(frag.line_idx) {
            frag.line_idx = new_idx;
        }
    }
    layout
        .line_output
        .hyphen_fragments
        .sort_unstable_by_key(|fragment| fragment.line_idx);
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

pub(super) fn build_block_decoration_traversal(
    reader: &LayoutReader<'_>,
    fragments: &mut FragmentWriter<'_>,
    scratch: &mut FinalizationScratch,
) {
    let (layout, _, decoration_owners) = fragments.finalization_parts();
    let owners = std::mem::take(decoration_owners);
    assert_eq!(
        owners.len(),
        layout.fragment_output.decorations.len(),
        "every block decoration must carry a paint-order owner"
    );
    layout.fragment_output.block_decoration_count = u32::try_from(owners.len())
        .expect("block decoration count exceeds traversal index capacity");
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
        groups.push((
            owner,
            u32::try_from(start).expect("decoration index exceeds traversal capacity")
                ..u32::try_from(end).expect("decoration index exceeds traversal capacity"),
        ));
        start = end;
    }
    // Topology IDs are preorder. Sorting compact semantic groups retains
    // edge order within an owner while leaving fragment storage untouched.
    groups.sort_by_key(|(owner, range)| (paint_order_key(reader, *owner as usize, 1), range.start));
    layout
        .fragment_output
        .block_paint_ranges
        .extend(groups.drain(..).map(|(_, range)| range));

    let block_count = owners.len();
    layout.fragment_output.block_decoration_paint_ranks.clear();
    layout
        .fragment_output
        .block_decoration_paint_ranks
        .resize(block_count, 0);
    for (rank, index) in layout
        .fragment_output
        .block_paint_ranges
        .iter()
        .flat_map(|range| range.clone())
        .enumerate()
    {
        layout.fragment_output.block_decoration_paint_ranks[index as usize] =
            u32::try_from(rank).expect("decoration paint rank exceeds traversal capacity");
    }

    layout.fragment_output.block_decoration_indices_by_y.clear();
    layout.fragment_output.block_decoration_indices_by_y.extend(
        (0..block_count).map(|index| {
            u32::try_from(index).expect("decoration index exceeds traversal capacity")
        }),
    );
    let decoration_fragments = layout.fragment_output.decorations.fragments();
    layout
        .fragment_output
        .block_decoration_indices_by_y
        .sort_unstable_by(|&left, &right| {
            decoration_fragments[left as usize]
                .rect
                .y0
                .total_cmp(&decoration_fragments[right as usize].rect.y0)
                .then_with(|| left.cmp(&right))
        });
    layout
        .fragment_output
        .block_decoration_prefix_max_y1
        .clear();
    let mut maximum_y1 = f64::NEG_INFINITY;
    for &index in &layout.fragment_output.block_decoration_indices_by_y {
        maximum_y1 = maximum_y1.max(decoration_fragments[index as usize].rect.y1);
        layout
            .fragment_output
            .block_decoration_prefix_max_y1
            .push(maximum_y1);
    }
    scratch.block_groups = groups;
    *decoration_owners = owners;
}

pub(super) fn rebuild_image_fragments_by_line(fragments: &mut FragmentWriter<'_>) {
    let layout = fragments.state_mut();
    layout
        .fragment_output
        .image_fragments_by_line
        .resize_with(layout.line_output.lines.len(), Vec::new);
    layout
        .fragment_output
        .image_fragments_by_line
        .truncate(layout.line_output.lines.len());
    for (idx, frag) in layout.fragment_output.image_fragments.iter().enumerate() {
        if let Some(line) = layout
            .fragment_output
            .image_fragments_by_line
            .get_mut(frag.line_idx)
        {
            line.push(idx);
        }
    }
}

pub(super) fn rebuild_decoration_fragments_by_line(fragments: &mut FragmentWriter<'_>) {
    let layout = fragments.state_mut();
    let paint_orders = &layout.fragment_output.decoration_paint_orders;
    let by_line = &mut layout.fragment_output.decoration_fragments_by_line;
    by_line.resize_with(layout.line_output.lines.len(), Vec::new);
    by_line.truncate(layout.line_output.lines.len());
    for indexes in by_line.iter_mut() {
        indexes.clear();
    }

    let block_count = layout.fragment_output.block_decoration_count as usize;
    let traversal = layout
        .fragment_output
        .block_paint_ranges
        .iter()
        .flat_map(|range| range.clone().map(|index| index as usize))
        .chain(block_count..layout.fragment_output.decorations.len());
    for decoration_idx in traversal {
        let Some(&line_idx) = layout
            .fragment_output
            .decoration_line_indices
            .get(decoration_idx)
        else {
            continue;
        };
        if line_idx == u32::MAX {
            continue;
        }
        let same_paint_layer = layout
            .fragment_output
            .decoration_positioned_layers
            .get(decoration_idx)
            .copied()
            .unwrap_or(false)
            == layout
                .line_output
                .positioned_layers
                .get(line_idx as usize)
                .copied()
                .unwrap_or(false)
            && layout
                .fragment_output
                .decoration_negative_positioned_layers
                .get(decoration_idx)
                .copied()
                .unwrap_or(false)
                == layout
                    .line_output
                    .negative_positioned_layers
                    .get(line_idx as usize)
                    .copied()
                    .unwrap_or(false)
            && layout
                .fragment_output
                .decoration_independent_positioned_layers
                .get(decoration_idx)
                .copied()
                .unwrap_or(false)
                == layout
                    .line_output
                    .independent_positioned_layers
                    .get(line_idx as usize)
                    .copied()
                    .unwrap_or(false);
        if same_paint_layer {
            if let Some(line) = by_line.get_mut(line_idx as usize) {
                line.push(decoration_idx);
            }
        } else if let Some(owner) = layout
            .fragment_output
            .decoration_line_indices
            .get_mut(decoration_idx)
        {
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

pub(super) fn rebuild_glyph_line_indices(glyph_count: usize, fragments: &mut FragmentWriter<'_>) {
    const NO_LINE: u32 = u32::MAX;
    let layout = fragments.state_mut();
    layout
        .line_output
        .glyph_line_indices
        .resize(glyph_count, NO_LINE);
    layout.line_output.glyph_line_indices.fill(NO_LINE);

    for (line_idx, line) in layout.line_output.lines.iter().enumerate() {
        let line_idx =
            u32::try_from(line_idx).expect("layout line count exceeds glyph-line index capacity");
        let mut assign = |range: &Range<u32>| {
            let start = usize::try_from(range.start)
                .unwrap_or(usize::MAX)
                .min(glyph_count);
            let end = usize::try_from(range.end)
                .unwrap_or(usize::MAX)
                .min(glyph_count);
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
