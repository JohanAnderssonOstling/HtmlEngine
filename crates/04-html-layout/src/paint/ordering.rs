use html_engine_model::{LayoutMode, ScenePaintItem};
use html_style_model::{BorderCollapseMode, Float, PositionMode, UsedStyleView};

use crate::FragmentWriter;
use crate::engine::StyleReader;

use rustc_data_structures::fx::FxHashMap;
use std::iter::once;
use std::sync::Arc;

type PaintOrderKey = Arc<[(u8, i32, (usize, usize))]>;
type GroupPaths = FxHashMap<(usize, bool, bool), (PaintOrderKey, bool)>;

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub(crate) enum PaintItem {
    Background,
    Content,
    Foreground,
    CollapsedBorder,
}

#[repr(u8)]
enum GroupRank {
    Negative = 1,
    Table = 2,
    Float = 3,
    AtomicInline = 4,
    Positioned = 5,
    Positive = 6,
}
#[repr(u8)]
enum ItemRank {
    OwnBackground = 0,
    Background = 2,
    Content = 4,
    Foreground = 7,
}

#[derive(Default)]
pub(crate) struct PaintOrderCache {
    keys: FxHashMap<(usize, PaintItem), PaintOrderKey>,
    sources: FxHashMap<usize, Option<usize>>,
    paths: GroupPaths,
}

impl PaintOrderCache {
    pub(crate) fn clear(&mut self) {
        self.keys.clear();
        self.sources.clear();
        self.paths.clear();
    }
    pub(crate) fn key(
        &mut self,
        reader: &StyleReader<'_>,
        owner: usize,
        item: PaintItem,
    ) -> PaintOrderKey {
        self.keys
            .entry((owner, item))
            .or_insert_with(|| {
                paint_order_key(reader, owner, item, &mut self.sources, &mut self.paths)
            })
            .clone()
    }
}

struct KeyBuilder {
    key: Vec<(u8, i32, (usize, usize))>,
    collect_groups: bool,
    collect_table_groups: bool,
    owns_group: bool,
}

/// A path through real stacking contexts and the float/atomic-inline groups
/// that paint atomically within them. Positioned descendants escape those
/// temporary groups, while descendants of a real context stay inside it.
fn paint_order_key(
    reader: &StyleReader<'_>,
    box_idx: usize,
    item: PaintItem,
    sources: &mut FxHashMap<usize, Option<usize>>,
    paths: &mut GroupPaths,
) -> PaintOrderKey {
    let mut missing_sources = Vec::new();
    let mut source_order = |owner| {
        missing_sources.clear();
        let mut current = Some(owner);
        let node = loop {
            let Some(index) = current else { break None };
            if let Some(&node) = sources.get(&index) {
                break node;
            }
            missing_sources.push(index);
            if let Some(element) = reader.box_dom_element(index) {
                break Some(element.index());
            }
            current = reader.get_parent(index);
        };
        for index in missing_sources.drain(..) {
            sources.insert(index, node);
        }
        // Generated boxes share DOM identity but retain their own box order.
        (node.unwrap_or(owner), owner)
    };
    let mut pending = Vec::new();
    let mut collect_groups = true;
    let mut collect_table_groups =
        matches!(item, PaintItem::Background | PaintItem::CollapsedBorder);
    let mut path: Option<(PaintOrderKey, bool)> = None;
    for owner in once(box_idx).chain(reader.layout_tree().ancestors(box_idx)) {
        let state = (owner, collect_groups, collect_table_groups);
        if let Some(cached) = paths.get(&state) {
            path = Some(cached.clone());
            break;
        }
        let mut builder = KeyBuilder {
            key: Vec::new(),
            collect_groups,
            collect_table_groups,
            owns_group: false,
        };
        builder.append_ancestor(
            reader,
            Some(owner),
            reader.style(owner),
            source_order(owner),
        );
        // Block-in-inline generation removes inline boxes from this layout
        // parent chain. Their retained position styles still establish paint
        // contexts. Recover each source identity from the DOM so separate
        // block fragments stay inside the same original inline context.
        let split = reader.split_inline_position_ancestors(owner);
        if !split.is_empty() {
            let mut node = reader
                .box_dom_element(owner)
                .and_then(|node| reader.document().node_ref(node).parent());
            for &indices in split.iter().rev() {
                let mut source = source_order(owner);
                while let Some(ancestor) = node {
                    node = reader.document().node_ref(ancestor).parent();
                    if reader.styles().style_for_node(ancestor) == Some(indices) {
                        source = (ancestor.index(), owner);
                        break;
                    }
                }
                builder.append_ancestor(reader, None, reader.used_style(indices), source);
            }
        }
        collect_groups = builder.collect_groups;
        collect_table_groups = builder.collect_table_groups;
        pending.push((state, builder.key, builder.owns_group));
    }
    let (mut path, mut owns_group) = path.unwrap_or_else(|| (Arc::from([]), false));
    // Complete missing paths parent-first. Boxes without a group share their
    // parent's allocation; escaped temporary groups have their own cache state.
    for (state, groups, owns) in pending.into_iter().rev() {
        if !groups.is_empty() {
            let mut joined = path.to_vec();
            joined.extend(groups.into_iter().rev());
            path = joined.into();
        }
        owns_group = owns;
        paths.insert(state, (path.clone(), owns));
    }
    let mut key = path.to_vec();
    let layer = match item {
        PaintItem::Background if owns_group => ItemRank::OwnBackground,
        PaintItem::Background | PaintItem::CollapsedBorder => ItemRank::Background,
        PaintItem::Foreground => ItemRank::Foreground,
        PaintItem::Content => ItemRank::Content,
    };
    let source = if item == PaintItem::CollapsedBorder {
        (usize::MAX, usize::MAX)
    } else {
        source_order(box_idx)
    };
    key.push((layer as u8, 0, source));
    key.into()
}

impl KeyBuilder {
    fn append_ancestor(
        &mut self,
        reader: &StyleReader<'_>,
        owner: Option<usize>,
        style: UsedStyleView<'_>,
        source: (usize, usize),
    ) {
        let root = reader.root_box();

        let is_root = owner.is_some() && owner == root;
        let is_context =
            owner != root && style.position() != PositionMode::Static && style.z_index().is_some();
        let group = if is_context {
            let z = style.z_index().expect("stacking context has z-index");
            Some((
                if z < 0 {
                    GroupRank::Negative
                } else if z == 0 {
                    GroupRank::Positioned
                } else {
                    GroupRank::Positive
                } as u8,
                z,
                source,
            ))
        } else if !self.collect_groups || is_root {
            None
        } else if style.position() != PositionMode::Static {
            self.collect_groups = false;
            Some((GroupRank::Positioned as u8, 0, source))
        } else if owner.map_or_else(
            || style.float() != Float::None,
            |owner| reader.box_facts(owner).float != Float::None,
        ) {
            Some((GroupRank::Float as u8, 0, source))
        } else if owner.map_or_else(
            || style.display().is_atomic_inline(),
            |owner| reader.box_facts(owner).atomic_inline,
        ) {
            Some((GroupRank::AtomicInline as u8, 0, source))
        } else if self.collect_table_groups
            && style.border_collapse() == BorderCollapseMode::Collapse
            && owner.is_some_and(|owner| {
                matches!(reader.box_layout_mode(owner), Some(LayoutMode::Table(_)))
            })
        {
            // Keep ordinary descendant backgrounds inside the table's block
            // paint phase. Its collapsed grid follows them, before floats,
            // inline content, and positioned descendants.
            Some((GroupRank::Table as u8, 0, source))
        } else {
            None
        };
        if let Some(group) = group {
            // Leave room for the final item layer alongside the first group.
            if self.key.is_empty() {
                self.key.reserve_exact(2);
            }
            self.collect_table_groups &= group.0 == GroupRank::Table as u8;
            self.key.push(group);
        }
        self.collect_groups &= !is_context && !is_root;
        self.owns_group |= (group.is_some() || is_root) && owner.is_some();
    }
}

pub(super) fn build_scene_paint_order(
    reader: &StyleReader<'_>,
    fragments: &mut FragmentWriter<'_>,
    cache: &mut PaintOrderCache,
) {
    let layout = fragments.state_mut();
    let owners = &layout.fragment_output.block_decoration_owners;
    let mut items = Vec::with_capacity(
        layout.fragment_output.decorations.len() + layout.line_output.lines.len(),
    );
    for (index, line) in layout.line_output.lines.iter().enumerate() {
        items.push((
            cache.key(reader, line.owner_box_idx as usize, PaintItem::Content),
            layout.line_output.paint_order_indices.ranks()[index],
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
        let collapsed_border =
            layout.fragment_output.decoration_paint_layers[index].is_collapsed_border();
        let line = layout.fragment_output.decoration_line_indices[index];
        let line = (line != u32::MAX).then_some(line);
        let (key, rank) = if index < layout.fragment_output.block_decoration_count as usize {
            (
                cache.key(
                    reader,
                    owners[index] as usize,
                    if collapsed_border {
                        PaintItem::CollapsedBorder
                    } else if foreground {
                        PaintItem::Foreground
                    } else {
                        PaintItem::Background
                    },
                ),
                0,
            )
        } else if let Some(line) = line {
            (
                cache.key(
                    reader,
                    layout.line_output.lines[line as usize].owner_box_idx as usize,
                    PaintItem::Content,
                ),
                layout.line_output.paint_order_indices.ranks()[line as usize],
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
    let output = &mut layout.fragment_output;
    output.scene_paint_order.rebuild(
        items.into_iter().map(|(_, _, _, _, item)| item),
        layout.line_output.lines.len(),
        output.decorations.len(),
    );
}

pub(super) fn build_block_decoration_traversal(
    reader: &StyleReader<'_>,
    fragments: &mut FragmentWriter<'_>,
    cache: &mut PaintOrderCache,
) {
    let layout = fragments.state_mut();
    let owners = &layout.fragment_output.block_decoration_owners;
    assert_eq!(
        owners.len(),
        layout.fragment_output.decorations.len(),
        "every block decoration must carry a paint-order owner"
    );
    layout.fragment_output.block_decoration_count = u32::try_from(owners.len())
        .expect("block decoration count exceeds traversal index capacity");
    layout.fragment_output.block_paint_ranges.clear();

    let mut start = 0usize;
    while start < owners.len() {
        let owner = owners[start];
        let collapsed_border =
            layout.fragment_output.decoration_paint_layers[start].is_collapsed_border();
        let mut end = start + 1;
        while end < owners.len()
            && owners[end] == owner
            && layout.fragment_output.decoration_paint_layers[end].is_collapsed_border()
                == collapsed_border
        {
            end += 1;
        }
        layout.fragment_output.block_paint_ranges.push(
            u32::try_from(start).expect("decoration index exceeds traversal capacity")
                ..u32::try_from(end).expect("decoration index exceeds traversal capacity"),
        );
        start = end;
    }
    // Topology IDs are preorder. Sorting compact semantic groups retains
    // edge order within an owner while leaving fragment storage untouched.
    layout
        .fragment_output
        .block_paint_ranges
        .sort_by_cached_key(|range| {
            let layer = if layout.fragment_output.decoration_paint_layers[range.start as usize]
                .is_collapsed_border()
            {
                PaintItem::CollapsedBorder
            } else {
                PaintItem::Background
            };
            (
                cache.key(reader, owners[range.start as usize] as usize, layer),
                range.start,
            )
        });

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
        let line_idx = layout.fragment_output.decoration_line_indices[decoration_idx];
        if line_idx == u32::MAX {
            continue;
        }
        let same_paint_layer = layout.fragment_output.decoration_paint_layers[decoration_idx]
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
        } else {
            // A decoration cannot be replayed with a line from another CSS
            // paint layer. Publish it as line-independent so the global pass
            // paints it exactly once in its own positioned layer.
            layout.fragment_output.decoration_line_indices[decoration_idx] = u32::MAX;
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
        indexes.sort_by_key(|&index| paint_orders[index]);
    }
}
