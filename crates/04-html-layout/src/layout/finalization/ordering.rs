use crate::layout_model::LineId;
use crate::layout_model::ScenePaintItem;
use html_style_model::{Float, PositionMode, UsedStyleView};
use std::ops::Range;

use super::{FinalizationScratch, FragmentWriter, LayoutReader};

type PaintOrderKey = Box<[(u8, i32, usize)]>;

/// A path through real stacking contexts and the float/atomic-inline groups
/// that paint atomically within them. Positioned descendants escape those
/// temporary groups, while descendants of a real context stay inside it.
fn paint_order_key(reader: &LayoutReader<'_>, box_idx: usize, ordinary_layer: u8) -> PaintOrderKey {
    let source_order = |owner| {
        reader
            .box_dom_element(owner)
            .map_or(owner, |node| node as usize)
    };
    let root = reader.root_box();
    let mut key = Vec::new();
    let mut collect_groups = true;
    let mut owns_group = false;
    let mut append_ancestor = |owner: Option<usize>, style: UsedStyleView<'_>, source| {
        let is_root = owner.is_some() && owner == root;
        let is_context =
            owner != root && style.position() != PositionMode::Static && style.z_index().is_some();
        let group = if is_context {
            let z = style.z_index().expect("stacking context has z-index");
            Some((
                if z < 0 {
                    1
                } else if z == 0 {
                    5
                } else {
                    6
                },
                z,
                source,
            ))
        } else if !collect_groups || is_root {
            None
        } else if style.position() != PositionMode::Static {
            collect_groups = false;
            Some((5, 0, source))
        } else if matches!(style.float(), Float::Left | Float::Right) {
            Some((3, 0, source))
        } else if style.display().is_atomic_inline() {
            Some((4, 0, source))
        } else {
            None
        };
        if let Some(group) = group {
            // Leave room for the final item layer alongside the first group.
            if key.is_empty() {
                key.reserve_exact(2);
            }
            key.push(group);
            owns_group |= owner == Some(box_idx);
        }
        if is_context || is_root {
            collect_groups = false;
            owns_group |= owner == Some(box_idx);
        }
    };
    let mut current = Some(box_idx);
    while let Some(owner) = current {
        append_ancestor(Some(owner), reader.style(owner), source_order(owner));
        // Block-in-inline generation removes inline boxes from this layout
        // parent chain. Their retained position styles still establish paint
        // contexts. Recover each source identity from the DOM so separate
        // block fragments stay inside the same original inline context.
        let split = reader.split_inline_position_ancestors(owner);
        if !split.is_empty() {
            let mut node = reader
                .box_dom_element(owner)
                .and_then(|raw| reader.document().node_id_from_raw(raw))
                .and_then(|node| reader.document().node_ref(node))
                .and_then(|node| node.parent());
            for &indices in split.iter().rev() {
                let mut source = source_order(owner);
                while let Some(ancestor) = node {
                    node = reader
                        .document()
                        .node_ref(ancestor)
                        .and_then(|node| node.parent());
                    if reader.styles().style_for_node(ancestor) == Some(indices) {
                        source = ancestor.raw() as usize;
                        break;
                    }
                }
                append_ancestor(None, reader.used_style(indices), source);
            }
        }
        current = reader.get_parent(owner);
    }
    // The upward walk emits inner groups first; the comparison key starts
    // with outer stacking contexts, then temporary groups, then this item.
    key.reverse();
    let layer = match ordinary_layer {
        1 if owns_group => 0,
        1 => 2,
        4 => 7,
        _ => 4,
    };
    key.reserve_exact(1);
    key.push((layer, 0, source_order(box_idx)));
    key.into_boxed_slice()
}

pub(in crate::layout) fn build_scene_paint_order(
    reader: &LayoutReader<'_>,
    fragments: &mut FragmentWriter<'_>,
) {
    let (layout, owners) = fragments.decoration_parts();
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
    for (index, decoration) in layout
        .fragment_output
        .decorations
        .fragments()
        .iter()
        .enumerate()
    {
        let foreground = decoration.is_foreground();
        let line = layout
            .fragment_output
            .decoration_line_indices
            .get(index)
            .copied()
            .filter(|&line| line != u32::MAX);
        let (key, rank) = if index < layout.fragment_output.block_decoration_count as usize {
            (
                paint_order_key(
                    reader,
                    owners[index] as usize,
                    if foreground { 4 } else { 1 },
                ),
                0,
            )
        } else if let Some(line) = line {
            (
                paint_order_key(
                    reader,
                    layout.line_output.lines[line as usize].owner_box_idx as usize,
                    3,
                ),
                layout.line_output.paint_order_ranks[line as usize],
            )
        } else {
            continue;
        };
        items.push((
            key,
            rank,
            if foreground { 2 } else { 0 },
            index,
            ScenePaintItem::Decoration(index as u32),
        ));
    }
    // Positioned backgrounds and their content share a context key. Keeping
    // that key across both kinds prevents a later background from being
    // painted before an earlier context's text. Line decorations retain their
    // before/after-text relationship within the same carrier line.
    items.sort_by(|a, b| (&a.0, a.1, a.2, a.3).cmp(&(&b.0, b.1, b.2, b.3)));
    layout.fragment_output.scene_paint_order.clear();
    layout
        .fragment_output
        .scene_paint_order
        .extend(items.into_iter().map(|(_, _, _, _, item)| item));
    let output = &mut layout.fragment_output;
    output.scene_line_ranks.clear();
    output.scene_line_ranks.resize(layout.line_output.lines.len(), u32::MAX);
    output.scene_decoration_ranks.clear();
    output.scene_decoration_ranks.resize(output.decorations.len(), u32::MAX);
    for (rank, item) in output.scene_paint_order.iter().enumerate() {
        match *item {
            ScenePaintItem::Line(line) => output.scene_line_ranks[line as usize] = rank as u32,
            ScenePaintItem::Decoration(index) => output.scene_decoration_ranks[index as usize] = rank as u32,
        }
    }
}

/// Build independent traversals over stable emission-order line storage.
pub(super) fn build_line_traversals(reader: &LayoutReader<'_>, fragments: &mut FragmentWriter<'_>) {
    profile_scope!("finalize.line_traversals");
    let output = &mut fragments.state_mut().line_output;
    output.line_glyph_offsets.freeze();
    output.line_glyph_advances.freeze();
    output.spatial_order.clear();
    output
        .spatial_order
        .extend((0..output.lines.len()).map(LineId::new));
    output.spatial_order.sort_unstable_by(|a, b| {
        let left = &output.lines[a.index()];
        let right = &output.lines[b.index()];
        (left.point.y + left.height)
            .total_cmp(&(right.point.y + right.height))
            .then_with(|| left.point.y.total_cmp(&right.point.y))
            .then_with(|| left.point.x.total_cmp(&right.point.x))
            .then_with(|| a.cmp(b))
    });
    output.spatial_ranks.resize(output.lines.len(), 0);
    for (rank, &id) in output.spatial_order.iter().enumerate() {
        output.spatial_ranks[id.index()] = rank as u32;
    }
    let keys: Vec<_> = output
        .lines
        .iter()
        .map(|line| paint_order_key(reader, line.owner_box_idx as usize, 3))
        .collect();
    output.paint_order_indices.clear();
    output
        .paint_order_indices
        .extend((0..output.lines.len()).map(LineId::new));
    output.paint_order_indices.sort_by(|a, b| {
        let left = &output.lines[a.index()];
        let right = &output.lines[b.index()];
        keys[a.index()]
            .cmp(&keys[b.index()])
            .then_with(|| left.point.y.total_cmp(&right.point.y))
            .then_with(|| left.point.x.total_cmp(&right.point.x))
    });
    output.paint_order_ranks.resize(output.lines.len(), 0);
    for (rank, &id) in output.paint_order_indices.iter().enumerate() {
        output.paint_order_ranks[id.index()] = rank as u32;
    }
    output
        .ellipsis_fragments
        .sort_unstable_by_key(|fragment| fragment.line_idx);
    output
        .hyphen_fragments
        .sort_unstable_by_key(|fragment| fragment.line_idx);
}

pub(in crate::layout) fn build_block_decoration_traversal(
    reader: &LayoutReader<'_>,
    fragments: &mut FragmentWriter<'_>,
    scratch: &mut FinalizationScratch,
) {
    let (layout, decoration_owners) = fragments.decoration_parts();
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
    groups.sort_by_cached_key(|(owner, range)| {
        (paint_order_key(reader, *owner as usize, 1), range.start)
    });
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

pub(in crate::layout) fn rebuild_decoration_fragments_by_line(fragments: &mut FragmentWriter<'_>) {
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
            .decoration_paint_layers
            .get(decoration_idx)
            .copied()
            .unwrap_or_default()
            == layout
                .line_output
                .paint_layers
                .get(line_idx as usize)
                .copied()
                .unwrap_or_default();
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
