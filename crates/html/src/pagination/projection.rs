//! Shared page placements with retained content and enclosing-flow membership.

use super::PagePositions;
use crate::layout::{BoxId, RenderView};
use html_engine_model::page_placements::{
    self, BoxProjectionBand, FlowPlacement, PlacementMembership, PlacementMemberships,
};
use html_engine_model::{LineScreen, layout_state};
use kurbo::{Point, Rect};
use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::slice;

pub(super) fn compose_box_fragments(doc: RenderView<'_>, page: &mut PagePositions) {
    page.flow_placements.clear();
    let boxes = doc.boxes();
    let ownership = boxes.pagination_ownership();
    for (order, (index, screen)) in page.positions.iter().enumerate() {
        let Some(line) = doc.text().line(index) else {
            continue;
        };
        let Some(membership) = ownership.lines.get(layout_state::line_id_index(index)) else {
            continue;
        };
        let Some(direct) = projection_band(page, screen, line.point(), 0) else {
            continue;
        };
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
        let placement = placements.spans.len() - 1;
        let entry = PlacementMembership {
            placement,
            order,
            source_y: line.point().y,
        };
        for &owner in &membership.content {
            if ownership.boxes[owner as usize]
                .table_fragment
                .is_some_and(|id| page.table_fragments.contains_key(&id))
            {
                continue;
            }
            retain_membership(placements.content.entry(owner), entry);
        }
        for &flow in &membership.flows {
            retain_membership(placements.flows.entry(flow), entry);
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
    let ownership = boxes.pagination_ownership();
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
    if let Some(flows) = ownership.box_flows.get(owner) {
        for &(flow, distance) in flows {
            // Distances increase along the retained ancestor chain. Still
            // visit distance zero to resolve source-order ties with content.
            if complete && distance > 0 {
                break;
            }
            if let Some(entries) = placements.flows.get(&flow) {
                complete = add(entries, distance);
            }
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
        // An unfragmented page shares its placement directly: no per-box
        // mapping allocation, sorting, or compaction is needed during paint.
        if let [placement] = placements.spans.as_slice() {
            let index = layout_state::box_id_index(owner);
            if placements.content.contains_key(&(index as u32)) {
                return Cow::Borrowed(slice::from_ref(&placement.direct));
            }
            if let Some(flows) = doc.boxes().pagination_ownership().box_flows.get(index) {
                for &(flow, distance) in flows {
                    if placements.flows.contains_key(&flow) {
                        let band = if distance == 0 {
                            Some(&placement.direct)
                        } else {
                            placement.enclosing.as_ref()
                        };
                        return Cow::Borrowed(band.map_or(&[], slice::from_ref));
                    }
                }
            }
            return Cow::Borrowed(&[]);
        }
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
    if distance == 0 {
        Some(band)
    } else {
        enclosing_band(page, band, distance)
    }
}

fn enclosing_band(
    page: &PagePositions,
    mut band: BoxProjectionBand,
    distance: usize,
) -> Option<BoxProjectionBand> {
    let index = page
        .offset_breaks
        .partition_point(|(at, _)| *at <= band.source_y);
    if let Some(previous) = index.checked_sub(1) {
        band.top = band.top.max(page.offset_breaks[previous].0);
    }
    if let Some((next, _)) = page.offset_breaks.get(index) {
        band.bottom = band.bottom.min(*next);
    }
    band.distance = distance;
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
