use html_style_model::{Display, Float, PositionMode};
use crate::layout_model::ScenePaintItem;
use std::ops::Range;

use super::{FinalizationScratch, FragmentWriter, LayoutReader};

type PaintOrderKey = Vec<(u8, i32, usize)>;

/// A path through real stacking contexts and the float/atomic-inline groups
/// that paint atomically within them. Positioned descendants escape those
/// temporary groups, while descendants of a real context stay inside it.
fn paint_order_key(reader: &LayoutReader<'_>, box_idx: usize, ordinary_layer: u8) -> PaintOrderKey {
    let source_order = |owner| reader.box_dom_element(owner).map_or(owner, |node| node as usize);
    let mut ancestors = Vec::new();
    let mut current = Some(box_idx);
    while let Some(owner) = current {
        ancestors.push((Some(owner), reader.style(owner), source_order(owner)));
        // Block-in-inline generation removes inline boxes from this layout
        // parent chain. Their retained position styles still establish paint
        // contexts. Recover each source identity from the DOM so separate
        // block fragments stay inside the same original inline context.
        let split = reader.split_inline_position_ancestors(owner);
        if !split.is_empty() {
            let mut node = reader.box_dom_element(owner)
                .and_then(|raw| reader.document().node_id_from_raw(raw))
                .and_then(|node| reader.document().node_ref(node))
                .and_then(|node| node.parent());
            for &indices in split.iter().rev() {
                let mut source = source_order(owner);
                while let Some(ancestor) = node {
                    node = reader.document().node_ref(ancestor).and_then(|node| node.parent());
                    if reader.styles().style_for_node(ancestor) == Some(indices) {
                        source = ancestor.raw() as usize;
                        break;
                    }
                }
                ancestors.push((None, reader.used_style(indices), source));
            }
        }
        current = reader.get_parent(owner);
    }
    let contexts: Vec<_> = ancestors.iter().enumerate()
        .filter(|(_, (owner, style, _))| {
            *owner != reader.root_box()
                && style.position() != PositionMode::Static
                && style.z_index().is_some()
        })
        .map(|(index, _)| index).collect();
    let context = contexts.first().copied().or_else(|| {
        reader.root_box().and_then(|root| ancestors.iter().position(|&(owner, _, _)| owner == Some(root)))
    });
    let mut positioned = None;
    let mut groups = Vec::new();
    for (index, &(_, style, _)) in ancestors[..context.unwrap_or(ancestors.len())].iter().enumerate() {
        if style.position() != PositionMode::Static {
            positioned.get_or_insert(index);
        }
        if positioned.is_none() {
            if matches!(style.float(), Float::Left | Float::Right) {
                groups.push((index, 3));
            } else if matches!(style.display(), Display::InlineBlock | Display::InlineTable | Display::InlineFlex | Display::InlineGrid) {
                groups.push((index, 4));
            }
        }
    }
    let mut key = Vec::with_capacity(contexts.len() + groups.len() + usize::from(positioned.is_some()) + 1);
    for &index in contexts.iter().rev() {
        let (_, style, source) = ancestors[index];
        let z = style.z_index().expect("stacking context has z-index");
        let layer = if z < 0 { 1 } else if z == 0 { 5 } else { 6 };
        key.push((layer, z, source));
    }
    if let Some(index) = positioned {
        key.push((5, 0, ancestors[index].2));
    }
    for &(index, layer) in groups.iter().rev() {
        key.push((layer, 0, ancestors[index].2));
    }
    let group_owner = groups.first().map(|&(index, _)| index).or(positioned).or(context);
    let layer = match ordinary_layer {
        1 if group_owner == Some(0) => 0,
        1 => 2,
        4 => 7,
        _ => 4,
    };
    key.push((layer, 0, source_order(box_idx)));
    key
}

pub(super) fn build_scene_paint_order(
    reader: &LayoutReader<'_>,
    fragments: &mut FragmentWriter<'_>,
) {
    let (layout, _, owners) = fragments.finalization_parts();
    let mut items = Vec::with_capacity(
        layout.fragment_output.decorations.len() + layout.line_output.lines.len(),
    );
    for (index, line) in layout.line_output.lines.iter().enumerate() {
        items.push((
            paint_order_key(reader, line.owner_box_idx as usize, 3),
            layout.line_output.paint_order_ranks[index],
            1u8,
            index,
            ScenePaintItem::Line(index as u32),
        ));
    }
    for (index, decoration) in layout.fragment_output.decorations.fragments().iter().enumerate() {
        let foreground = decoration.is_foreground();
        let line = layout.fragment_output.decoration_line_indices.get(index)
            .copied().filter(|&line| line != u32::MAX);
        let (key, rank) = if index < layout.fragment_output.block_decoration_count as usize {
            (
                paint_order_key(reader, owners[index] as usize, if foreground { 4 } else { 1 }),
                0,
            )
        } else if let Some(line) = line {
            (
                paint_order_key(reader, layout.line_output.lines[line as usize].owner_box_idx as usize, 3),
                layout.line_output.paint_order_ranks[line as usize],
            )
        } else {
            continue;
        };
        items.push((key, rank, if foreground { 2 } else { 0 }, index, ScenePaintItem::Decoration(index as u32)));
    }
    // Positioned backgrounds and their content share a context key. Keeping
    // that key across both kinds prevents a later background from being
    // painted before an earlier context's text. Line decorations retain their
    // before/after-text relationship within the same carrier line.
    items.sort_by(|a, b| (&a.0, a.1, a.2, a.3).cmp(&(&b.0, b.1, b.2, b.3)));
    layout.fragment_output.scene_paint_order.clear();
    layout.fragment_output.scene_paint_order.extend(items.into_iter().map(|(_, _, _, _, item)| item));
}

pub(super) fn sort_lines_and_remap_images(
    reader: &LayoutReader<'_>,
    fragments: &mut FragmentWriter<'_>,
    scratch: &mut FinalizationScratch,
) {
    profile_scope!("finalize.sort_lines");
    let (layout, line_owners, _) = fragments.finalization_parts();
    let mut owners = std::mem::take(line_owners);
    let mut lines = std::mem::take(&mut layout.line_output.lines);
    let mut offsets_by_line = std::mem::take(&mut layout.line_output.line_glyph_offsets);
    let mut advances_by_line = std::mem::take(&mut layout.line_output.line_glyph_advances);
    offsets_by_line.freeze();
    advances_by_line.freeze();
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
            offsets_by_line.range(idx),
            advances_by_line.range(idx),
            idx,
        )
    }));
    offsets_by_line.clear_ranges();
    advances_by_line.clear_ranges();

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
    let line_keys: Vec<_> = owners.iter()
        .map(|&owner| paint_order_key(reader, owner as usize, 3)).collect();
    layout
        .line_output
        .paint_order_indices
        .sort_by(|a_idx, b_idx| {
            let a_idx = *a_idx as usize;
            let b_idx = *b_idx as usize;
            let a = &line_pairs[a_idx].0;
            let b = &line_pairs[b_idx].0;
            line_keys[a_idx].cmp(&line_keys[b_idx])
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
    layout.paint_program.remap_lines(&index_map);
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
        offsets_by_line.push_range(offsets);
        advances_by_line.push_range(advances);
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
    groups.sort_by_cached_key(|(owner, range)| (paint_order_key(reader, *owner as usize, 1), range.start));
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
    profile_scope!("finalize.image_index");
    let layout = fragments.state_mut();
    let by_line = &mut layout.fragment_output.image_fragments_by_line;
    by_line.reset_groups(layout.line_output.lines.len());
    for (idx, fragment) in layout.fragment_output.image_fragments.iter().enumerate() {
        if fragment.line_idx < by_line.len() { by_line.push_to(fragment.line_idx, idx); }
    }
    by_line.freeze();
}

pub(super) fn rebuild_decoration_fragments_by_line(fragments: &mut FragmentWriter<'_>) {
    let layout = fragments.state_mut();
    let paint_orders = &layout.fragment_output.decoration_paint_orders;
    let by_line = &mut layout.fragment_output.decoration_fragments_by_line;
    by_line.reset_groups(layout.line_output.lines.len());

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
            if (line_idx as usize) < by_line.len() {
                by_line.push_to(line_idx as usize, decoration_idx);
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
    by_line.freeze();
    for line_idx in 0..by_line.len() {
        let indexes = by_line.get_mut(line_idx).expect("valid line group");
        // `traversal` already carries CSS tree order (ancestor backgrounds
        // before descendant backgrounds). Keep that stable order when an
        // atomic inline assigns the same token paint slot to its whole
        // subtree; using the raw emission index as a tie-breaker reverses
        // parents and children because block backgrounds emit post-order.
        indexes.sort_by_key(|&index| paint_orders.get(index).copied().unwrap_or(u32::MAX));
    }
}

pub(super) fn rebuild_glyph_line_indices(glyph_count: usize, fragments: &mut FragmentWriter<'_>) {
    profile_scope!("finalize.glyph_line_index");
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
                layout.line_output.glyph_line_indices.assign(start..end, line_idx);
            }
        };
        if let Some(range) = line.text_fragment_range(&layout.line_output.line_details) {
            for fragment in &layout.line_output.text_fragments[range.start as usize..range.end as usize] {
                assign(&fragment.glyphs);
            }
        } else {
            assign(&line.glyphs);
        }
    }
}
