//! Shared page placements with retained content and enclosing-flow membership.

use super::PagePositions;
use crate::layout::{BoxId, RenderView};
use html_engine_model::page_placements::{
    self, BoxProjectionBand, FlowPlacement, PlacementMembership, PlacementMemberships,
};
use html_engine_model::{LineId, LineScreen, layout_state};
use kurbo::{Point, Rect};
use std::borrow::Cow;
use std::collections::{HashMap, hash_map::Entry};

pub(super) fn compose_box_fragments(doc: RenderView<'_>, page: &mut PagePositions) {
    page.flow_placements.clear();
    let boxes = doc.boxes();
    let ownership = boxes.pagination_ownership();
    let mut positioned = Vec::new();
    for (order, (index, screen)) in page.positions.iter().enumerate() {
        let Some(line) = doc.text().line(index) else { continue };
        if ownership.lines.get(layout_state::line_id_index(index)).is_none() {
            continue;
        }
        let Some(direct) = projection_band(page, screen, line.point(), 0) else { continue };
        let enclosing = enclosing_band(page, direct, 1);
        let placements = &mut page.flow_placements;
        let same = placements.spans.last().is_some_and(|previous| {
            same_mapping(previous.direct, direct)
                && match (previous.enclosing, enclosing) {
                    (Some(a), Some(b)) => same_mapping(a, b),
                    (None, None) => true,
                    _ => false,
                }
        });
        if !same {
            placements.spans.push(FlowPlacement { direct, enclosing });
        }
        positioned.push((index, PlacementMembership {
            placement: placements.spans.len() - 1,
            order,
            source_y: line.point().y,
        }));
    }
    if page.flow_placements.spans.len() != 1 {
        let mut flow_cursor = 0;
        for &(index, entry) in &positioned {
            let membership = &ownership.lines[layout_state::line_id_index(index)];
            for &owner in &membership.content {
                if ownership.boxes[owner as usize].table_fragment
                    .is_some_and(|id| page.table_fragments.contains_key(&id))
                {
                    continue;
                }
                retain_membership(page.flow_placements.content.entry(owner), entry);
            }
            for &flow in ownership.flow_line_index.sources_for_line(
                layout_state::line_id_index(index) as u32, &mut flow_cursor,
            ) {
                retain_membership(page.flow_placements.flows.entry(flow), entry);
            }
        }
    }
    page.flow_placements.prepared = true;
    // Resolve ownership once while composing the page. Paint-time projection
    // must not choose a new mapping based on the particular border strip or
    // background rectangle that happens to be requested.
    let mut ranges: Vec<(f64, f64)> = page.flow_placements.spans.iter()
        .flat_map(|placement| [Some(placement.direct), placement.enclosing])
        .flatten()
        .chain(page.table_fragments.values().flat_map(|placed| placed.bands.iter().copied()))
        .map(|band| (band.top, band.bottom))
        .collect();
    ranges.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (top, bottom) in ranges {
        if let Some(previous) = merged.last_mut() && top <= previous.1 {
            previous.1 = previous.1.max(bottom);
        } else {
            merged.push((top, bottom));
        }
    }
    let mut candidates = Vec::new();
    for (top, bottom) in merged {
        ownership.box_intervals.intersecting(top, bottom, &mut candidates);
    }
    candidates.sort_unstable();
    candidates.dedup();
    if let [placement] = page.flow_placements.spans.as_slice() {
        let placement = *placement;
        compose_single_placement(doc, page, &candidates, &positioned, placement);
        return;
    }
    for index in candidates.into_iter().map(|index| index as usize) {
        let owner = layout_state::box_id_new(index);
        let Some(bounds) = boxes.bounds(owner) else { continue };
        let candidates = if let Some(bands) = super::table_fragments::bands(doc, page, owner) {
            bands.to_vec()
        } else {
            inferred_box_bands(doc, page, owner).into_owned()
        };
        let fragments = resolve_box_bands(candidates, bounds, page.geometry.col_count);
        if !fragments.is_empty() {
            page.flow_placements.boxes.insert(index as u32, fragments);
        }
    }
}

/// A page-local closure of candidates and direct carriers. Masks are in one
/// flat arena; no document-sized scratch or per-line ancestor lists are built.
struct ProjectionNode {
    owner: BoxId,
    parent: Option<usize>,
    flow: bool,
    inherited_flow: bool,
    candidate: bool,
}

fn compose_single_placement(
    doc: RenderView<'_>,
    page: &mut PagePositions,
    candidates: &[u32],
    positioned: &[(LineId, PlacementMembership)],
    placement: FlowPlacement,
) {
    let boxes = doc.boxes();
    let facts = &boxes.pagination_ownership().boxes;
    // Only presence matters for one shared mapping. Stable-ID order makes a
    // box's content range a contiguous interval of bits, even for continuations.
    let mut lines: Vec<_> = positioned.iter().map(|&(line, _)| line).collect();
    lines.sort_unstable();
    lines.dedup();
    let words = lines.len().div_ceil(64);
    let mut indices = HashMap::with_capacity(candidates.len());
    let mut nodes: Vec<ProjectionNode> = Vec::with_capacity(candidates.len());
    let mut masks = Vec::new();
    let mut pending = Vec::new();
    let mut add = |owner: BoxId, seed: Option<(usize, bool)>| {
        let mut current = Some(owner);
        let mut parent = None;
        while let Some(owner) = current {
            if let Some(&index) = indices.get(&owner) {
                parent = Some(index);
                break;
            }
            pending.push(owner);
            current = boxes.parent(owner);
        }
        // Insert ancestors first. Arena order is then already topological:
        // reverse order aggregates children, forward order inherits providers.
        while let Some(owner) = pending.pop() {
            let index = nodes.len();
            indices.insert(owner, index);
            nodes.push(ProjectionNode {
                owner, parent, flow: false, inherited_flow: false, candidate: false,
            });
            masks.resize(masks.len() + words, 0u64);
            parent = Some(index);
        }
        let index = parent.expect("a carrier or candidate has a box");
        if let Some((bit, flow)) = seed {
            masks[index * words + bit / 64] |= 1 << (bit % 64);
            nodes[index].flow |= flow;
        } else {
            nodes[index].candidate = true;
        }
    };
    for (bit, &line) in lines.iter().enumerate() {
        doc.text().visit_line_carriers(line, |owner, flow| {
            add(owner, Some((bit, flow)));
        });
    }
    for &owner in candidates {
        add(layout_state::box_id_new(owner as usize), None);
    }
    for index in (0..nodes.len()).rev() {
        if let Some(parent) = nodes[index].parent {
            for word in 0..words {
                masks[parent * words + word] |= masks[index * words + word];
            }
            if !facts[layout_state::box_id_index(nodes[index].owner)].independent {
                nodes[parent].flow |= nodes[index].flow;
            }
        }
    }
    for index in 0..nodes.len() {
        let node = &nodes[index];
        let owner = node.owner;
        let independent = facts[layout_state::box_id_index(owner)].independent;
        let range = boxes.content_line_range(owner);
        let incoming = node.parent.is_some_and(|parent| nodes[parent].inherited_flow);
        let direct_flow = node.flow;
        // Empty independent boxes may inherit for themselves, but do not pass
        // that inherited provider to descendants.
        let inherited = incoming && (!independent || range.is_empty());
        let candidate = node.candidate;
        nodes[index].inherited_flow = direct_flow || (incoming && !independent);
        if !candidate { continue }
        let Some(bounds) = boxes.bounds(owner) else { continue };
        let fragments = if let Some(bands) = super::table_fragments::bands(doc, page, owner) {
            resolve_box_bands(bands.to_vec(), bounds, page.geometry.col_count)
        } else {
            let start = lines.partition_point(|&line| line < range.start);
            let end = lines.partition_point(|&line| line < range.end);
            let content = (start / 64..end.div_ceil(64)).any(|word| {
                let lo = start.saturating_sub(word * 64);
                let hi = (end - word * 64).min(64);
                let mask = (u64::MAX << lo) & (u64::MAX >> (64 - hi));
                masks[index * words + word] & mask != 0
            });
            let band = if content || direct_flow {
                Some(placement.direct)
            } else if inherited {
                placement.enclosing
            } else {
                None
            };
            // A single band has no competing distance or overlap to resolve.
            band.filter(|band| bounds.y1 >= band.top && bounds.y0 <= band.bottom
                && band.bottom > band.top).into_iter().collect()
        };
        if !fragments.is_empty() {
            page.flow_placements.boxes.insert(layout_state::box_id_index(owner) as u32, fragments);
        }
    }
}

fn same_mapping(a: BoxProjectionBand, b: BoxProjectionBand) -> bool {
    a.column == b.column && a.translation == b.translation && a.top == b.top && a.bottom == b.bottom
}

fn retain_membership(slot: Entry<'_, u32, PlacementMemberships>, entry: PlacementMembership) {
    match slot {
        Entry::Occupied(mut occupied) => occupied.get_mut().retain(entry),
        Entry::Vacant(vacant) => {
            vacant.insert(PlacementMemberships {
                first: entry,
                additional: Vec::new(),
            });
        }
    }
}

/// Resolve a box through shared placements, without scanning positioned lines
/// or expanding each line's flow membership into all its descendant boxes.
fn shared_box_bands(
    doc: RenderView<'_>,
    page: &PagePositions,
    owner: BoxId,
) -> Vec<BoxProjectionBand> {
    let placements = &page.flow_placements;
    let boxes = doc.boxes();
    let owner = layout_state::box_id_index(owner);
    let mut selected: Vec<(usize, usize, BoxProjectionBand)> = Vec::new();
    let mut add = |entries: &PlacementMemberships, distance: usize| {
        for entry in entries.iter() {
            let placement = &placements.spans[entry.placement];
            let band = if distance == 0 {
                Some(placement.direct)
            } else {
                placement.enclosing
            };
            let Some(mut band) = band else { continue };
            band.distance = distance;
            band.source_y = entry.source_y;
            match selected.binary_search_by_key(&entry.placement, |&(index, _, _)| index) {
                Ok(index) => {
                    // Match the first nearest carrier in pagination order,
                    // including ties between direct content and its own flow.
                    let (_, previous_order, previous) = &mut selected[index];
                    if distance < previous.distance
                        || (distance == previous.distance && entry.order < *previous_order)
                    {
                        *previous_order = entry.order;
                        *previous = band;
                    }
                }
                Err(index) => selected.insert(index, (entry.placement, entry.order, band)),
            }
        }
        selected.len() == placements.spans.len()
    };
    let mut complete = false;
    if let Some(entries) = placements.content.get(&(owner as u32)) {
        complete = add(entries, 0);
    }
    for (flow, distance) in boxes.flow_ancestors(layout_state::box_id_new(owner)) {
        // Distances increase along the ancestor chain. Still visit distance
        // zero to resolve source-order ties with content.
        if complete && distance > 0 {
            break;
        }
        if let Some(entries) = placements.flows.get(&flow) {
            complete = add(entries, distance);
        }
    }
    selected.into_iter().map(|(_, _, band)| band).collect()
}

/// Preserve the general fragmentation rules while preparing their result.
/// Adjacent equal mappings collapse to one band, so an unfragmented cell does
/// not carry a separate mapping for every text line during painting.
fn inferred_box_bands<'a>(
    doc: RenderView<'_>,
    page: &'a PagePositions,
    owner: BoxId,
) -> Cow<'a, [BoxProjectionBand]> {
    let placements = &page.flow_placements;
    let explicit_table = doc
        .boxes()
        .pagination_facts(owner)
        .and_then(|facts| facts.table_fragment)
        .is_some_and(|id| page.table_fragments.contains_key(&id));
    if placements.prepared && !explicit_table {
        return Cow::Owned(shared_box_bands(doc, page, owner));
    }
    let boxes = doc.boxes();
    let mut bands = Vec::new();
    for (index, screen) in page.positions.iter() {
        let Some(distance) = boxes.fragmentation_distance(owner, index) else {
            continue;
        };
        let Some(line) = doc.text().line(index) else {
            continue;
        };
        if let Some(band) = projection_band(page, screen, line.point(), distance) {
            bands.push(band);
        }
    }
    compact_bands(&mut bands);
    Cow::Owned(bands)
}

pub(super) fn box_projection_bands<'a>(
    page: &'a PagePositions,
    owner: BoxId,
) -> &'a [BoxProjectionBand] {
    page.flow_placements.boxes.get(&(layout_state::box_id_index(owner) as u32))
        .map_or(&[], Vec::as_slice)
}

pub(super) fn resolve_box_bands(
    mut bands: Vec<BoxProjectionBand>,
    bounds: Rect,
    columns: f64,
) -> Vec<BoxProjectionBand> {
    let mut distances = vec![usize::MAX; columns.ceil().max(0.0) as usize];
    for band in &bands {
        if bounds.y1 >= band.top && bounds.y0 <= band.bottom {
            distances[band.column as usize] = distances[band.column as usize].min(band.distance);
        }
    }
    bands.retain(|band| band.distance == distances[band.column as usize]
        && bounds.y1 >= band.top && bounds.y0 <= band.bottom);
    bands.sort_by(|a, b| a.column.cmp(&b.column).then_with(|| a.source_y.total_cmp(&b.source_y)));
    let mut fragments: Vec<BoxProjectionBand> = Vec::with_capacity(bands.len());
    for mut band in bands {
        if let Some(previous) = fragments.last_mut()
            && previous.column == band.column
        {
            if previous.translation == band.translation
                && previous.top <= band.bottom && band.top <= previous.bottom
            {
                previous.top = previous.top.min(band.top);
                previous.bottom = previous.bottom.max(band.bottom);
                continue;
            }
            if previous.top < band.bottom && band.top < previous.bottom {
                previous.bottom = previous.bottom.min(band.source_y);
                band.top = band.top.max(band.source_y);
            }
        }
        fragments.push(band);
    }
    fragments.retain(|band| band.bottom > band.top);
    fragments
}

fn projection_band(
    page: &PagePositions,
    screen: LineScreen,
    point: Point,
    distance: usize,
) -> Option<BoxProjectionBand> {
    if screen.col_index < 0
        || screen.col_index as usize >= page.geometry.col_count.ceil().max(0.0) as usize
    {
        return None;
    }
    let mut translation = screen.point - point;
    let top = screen.visible_top() - translation.y;
    let bottom = screen.visible_bottom(page.geometry.size.height) - translation.y;
    if bottom <= top {
        return None;
    }
    translation.x -= page_placements::col_x(&page.geometry, f64::from(screen.col_index), 0.0);
    let band = BoxProjectionBand {
        column: screen.col_index,
        translation,
        source_y: point.y,
        top,
        bottom,
        distance,
    };
    // Direct content can carry an entire decorated box, not only its own
    // line. A continued caption must not project the earlier image region
    // through its new translation, so direct and enclosing carriers share
    // the same source-fragment limits.
    clip_to_source_fragment(page, band)
}

fn enclosing_band(
    page: &PagePositions,
    mut band: BoxProjectionBand,
    distance: usize,
) -> Option<BoxProjectionBand> {
    band.distance = distance;
    clip_to_source_fragment(page, band)
}

/// Decorations must stop where pagination changes the source translation,
/// including when the following content was deferred to another column.
pub(super) fn clip_to_source_fragment(
    page: &PagePositions,
    mut band: BoxProjectionBand,
) -> Option<BoxProjectionBand> {
    // A fresh page does not retain the preceding page's offset breaks.
    // Its source start is still a hard boundary for continued decorations.
    band.top = band.top.max(page.paint_start_offset_y);
    // A split carrier can originate above its current visible fragment.
    let source_y = band.source_y.max(band.top);
    let index = page
        .offset_breaks
        .partition_point(|(at, _)| *at <= source_y);
    if let Some(previous) = index.checked_sub(1) {
        band.top = band.top.max(page.offset_breaks[previous].0);
    }
    if let Some((next, _)) = page.offset_breaks.get(index) {
        band.bottom = band.bottom.min(*next);
    }
    (band.bottom > band.top).then_some(band)
}

fn compact_bands(bands: &mut Vec<BoxProjectionBand>) {
    // With one translation, duplicate line mappings have no effect on overlap
    // clipping. Keep the full sequence for fragmented flows: source_y matters
    // when adjacent, different translations divide overlapping source bands.
    if bands.first().is_some_and(|first| {
        bands
            .iter()
            .all(|band| band.column == first.column && band.translation == first.translation)
    }) {
        if let Some(nearest) = bands.iter().min_by_key(|band| band.distance).copied()
            && bands
                .iter()
                .all(|band| nearest.top <= band.top && nearest.bottom >= band.bottom)
        {
            // The ordinary unfragmented box needs exactly one translation and
            // clip: its own content covers every enclosing-flow candidate.
            bands.clear();
            bands.push(nearest);
            return;
        }
        bands.dedup_by(|right, left| {
            right.distance == left.distance && right.top == left.top && right.bottom == left.bottom
        });
    }
}
