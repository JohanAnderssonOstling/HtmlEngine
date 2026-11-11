//! Resolve inline-row attachments, markers and float keeps after line placement.

use super::{FragmentWriter, LayoutReader};
use html_engine_model::{
    BoxGeometry, FloatPaginationKeep, ImageFragment, LayoutMode, LineId, LineOutput, LinePaginationAnchor,
    LinePaginationGroup};
use html_engine_model::layout_state;
use html_style_model::{Float, PositionMode};
use kurbo::{Point, Rect};
use std::ops::Range;

fn include_lines(target: &mut Range<u32>, lines: Range<u32>) {
    if !lines.is_empty() {
        if target.start >= target.end {
            *target = lines;
        } else {
            target.start = target.start.min(lines.start);
            target.end = target.end.max(lines.end);
        }
    }
}

fn root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

fn float_flows(reader: &LayoutReader<'_>) -> Vec<(usize, bool)> {
    let mut flows: Vec<(usize, bool)> = Vec::with_capacity(reader.box_count());
    for owner in 0..reader.box_count() {
        let inherited = reader
            .get_parent(owner)
            .map_or((owner, false), |parent| flows[parent]);
        let style = reader.style(owner);
        let floating = matches!(style.float(), Float::Left | Float::Right);
        flows.push(
            if style.position() == PositionMode::Absolute
                || (!floating && !inherited.1 && reader.box_uses_float_context(owner))
            {
                (owner, false)
            } else {
                (inherited.0, inherited.1 || floating)
            },
        );
    }
    flows
}

fn visible_image_rect(image: &ImageFragment, origin: Point) -> Rect {
    Rect::from_origin_size(image.offset, image.size).intersect(image.clip) + origin.to_vec2()
}

/// Positioned descendants inherit the fragment at their position inside the
/// containing block. Attaching to one source carrier preserves independent
/// paint ordering without making the entire containing block indivisible.
fn positioned_attachments(
    reader: &LayoutReader<'_>,
    geometry: &BoxGeometry,
    output: &LineOutput,
    images: &[ImageFragment],
) -> Vec<(LineId, LineId)> {
    let mut bounds: Vec<_> = output.lines.iter()
        .map(|line| line.point.y..line.point.y + line.height).collect();
    for image in images {
        let rect = visible_image_rect(image, output.lines[image.line_idx].point);
        if rect.width() > 0.0 && rect.height() > 0.0 {
            bounds[image.line_idx].start = bounds[image.line_idx].start.min(rect.y0);
            bounds[image.line_idx].end = bounds[image.line_idx].end.max(rect.y1);
        }
    }
    let mut carriers: Vec<Vec<usize>> = vec![Vec::new(); reader.box_count()];
    for (index, line) in output.lines.iter().enumerate() {
        let owner = line.owner_box_idx as usize;
        let container = std::iter::once(owner).chain(reader.layout_tree().ancestors(owner))
            .find(|&ancestor| reader.style(ancestor).position() != PositionMode::Static);
        if let Some(container) = container
            && reader.style(container).position() != PositionMode::Absolute
        {
            carriers[container].push(index);
        }
    }
    for candidates in &mut carriers {
        candidates.sort_unstable_by(|&a, &b| bounds[a].start.total_cmp(&bounds[b].start));
    }
    let mut attachments = Vec::new();
    for owner in 0..reader.box_count() {
        if reader.style(owner).position() != PositionMode::Absolute {
            continue;
        }
        let Some(container) = reader.layout_tree().ancestors(owner)
            .find(|&ancestor| reader.style(ancestor).position() != PositionMode::Static)
        else { continue };
        let candidates = &carriers[container];
        let y = geometry.point(owner).y;
        // Prefer a carrier covering this source position. For padding or an
        // overflowing overlay, use the nearest carrier in the same container.
        let distance = |index: usize| {
            let band = &bounds[index];
            (band.start - y).max(0.0).max(y - band.end)
        };
        let Some(&anchor) = candidates.iter().min_by(|&&a, &&b| {
            distance(a).total_cmp(&distance(b))
                .then_with(|| bounds[a].start.total_cmp(&bounds[b].start))
        }) else { continue };
        for index in output.box_line_ranges[owner].clone() {
            let line_owner = output.lines[index as usize].owner_box_idx as usize;
            if line_owner == owner || reader.layout_tree().ancestors(line_owner).any(|ancestor| ancestor == owner) {
                attachments.push((LineId::new(anchor), LineId::new(index as usize)));
            }
        }
    }
    attachments
}

/// Shared overlap index for column groups and continuous-layout keeps.
struct FloatLineIndex {
    flows: Vec<(usize, bool)>,
    float_roots: Vec<Option<usize>>,
    bounds: Vec<Range<f64>>,
    order: Vec<usize>,
    prefix_bottom: Vec<f64>,
}

impl FloatLineIndex {
    fn new(reader: &LayoutReader<'_>, output: &LineOutput, images: &[ImageFragment]) -> Self {
        // Spatial overlap determines wrapping, not ownership. Only content
        // inside a float belongs to its pagination group; neighbouring normal
        // flow must remain free to fill the space beside that float.
        let mut float_roots = Vec::with_capacity(reader.box_count());
        for owner in 0..reader.box_count() {
            let inherited = reader.get_parent(owner).and_then(|parent| float_roots[parent]);
            let style = reader.style(owner);
            float_roots.push(if style.position() == PositionMode::Absolute {
                None
            } else {
                inherited.or_else(|| {
                    matches!(style.float(), Float::Left | Float::Right).then_some(owner)
                })
            });
        }
        let mut bounds: Vec<_> = output
            .lines
            .iter()
            .map(|line| line.point.y..line.point.y + line.height)
            .collect();
        for image in images {
            let rect = visible_image_rect(image, output.lines[image.line_idx].point);
            if rect.width() > 0.0 && rect.height() > 0.0 {
                let extent = &mut bounds[image.line_idx];
                extent.start = extent.start.min(rect.y0);
                extent.end = extent.end.max(rect.y1);
            }
        }
        let mut order: Vec<_> = (0..output.lines.len()).collect();
        order.sort_unstable_by(|&a, &b| {
            bounds[a]
                .start
                .total_cmp(&bounds[b].start)
                .then_with(|| a.cmp(&b))
        });
        let mut bottom = f64::NEG_INFINITY;
        let prefix_bottom = order
            .iter()
            .map(|&index| {
                bottom = bottom.max(bounds[index].end);
                bottom
            })
            .collect();
        Self {
            flows: float_flows(reader),
            float_roots,
            bounds,
            order,
            prefix_bottom,
        }
    }

    fn overlapping<'a>(
        &'a self,
        output: &'a LineOutput,
        owner: usize,
        top: f64,
        bottom: f64,
    ) -> impl Iterator<Item = usize> + 'a {
        let start = self.prefix_bottom.partition_point(|&end| end <= top + 0.01);
        let end = self
            .order
            .partition_point(|&index| self.bounds[index].start < bottom - 0.01);
        self.order[start..end.max(start)]
            .iter()
            .copied()
            .filter(move |&index| {
                self.float_roots[output.lines[index].owner_box_idx as usize] == Some(owner)
                    && self.bounds[index].end > top + 0.01
            })
    }
}

pub(super) fn build_float_keeps(
    reader: &LayoutReader<'_>,
    geometry: &BoxGeometry,
    fragments: &mut FragmentWriter<'_>,
) {
    let state = fragments.state_mut();
    let output = &mut state.line_output;
    output.pagination_float_keeps.clear();
    if !reader.has_floats() {
        return;
    }
    let index = FloatLineIndex::new(reader, output, &state.fragment_output.image_fragments);
    let ranks = if output.pagination_order.len() == output.lines.len() {
        output.pagination_order.ranks()
    } else {
        output.spatial_order.ranks()
    };

    let mut keeps = Vec::new();
    for owner in 0..reader.box_count() {
        let style = reader.style(owner);
        if style.position() == PositionMode::Absolute
            || !matches!(style.float(), Float::Left | Float::Right)
        {
            continue;
        }
        let top = geometry.point(owner).y;
        let bottom = top + geometry.size(owner).height;
        if bottom <= top + 0.01 {
            continue;
        }
        let mut bounds = top..bottom;
        let mut trigger: Option<u32> = None;
        // Keep the float's own content together. Wrapped paragraph lines
        // have independent break opportunities, even at the same source y.
        for line in index.overlapping(output, owner, top, bottom) {
            let cursor = ranks[line];
            let group = output
                .pagination_groups
                .partition_point(|group| group.members.end <= cursor);
            let group = output
                .pagination_groups
                .get(group)
                .filter(|group| group.members.contains(&cursor));
            let cursor = group.map_or(cursor, |group| group.members.start);
            let extent = group.map_or(&index.bounds[line], |group| &group.bounds);
            bounds.start = bounds.start.min(extent.start);
            bounds.end = bounds.end.max(extent.end);
            trigger = Some(trigger.map_or(cursor, |previous| previous.min(cursor)));
        }
        if let Some(trigger) = trigger {
            keeps.push((
                index.flows[owner].0,
                FloatPaginationKeep { trigger, bounds },
            ));
        }
    }
    keeps.sort_unstable_by(|(a_context, a), (b_context, b)| {
        a_context
            .cmp(b_context)
            .then_with(|| a.bounds.start.total_cmp(&b.bounds.start))
    });
    let mut previous_context = None;
    for (context, keep) in keeps {
        // An overlapping second float must not introduce an offset halfway
        // through the first. Oversized combined keeps can still be split.
        if previous_context == Some(context)
            && let Some(previous) = output.pagination_float_keeps.last_mut()
            && keep.bounds.start < previous.bounds.end - 0.01
        {
            previous.bounds.end = previous.bounds.end.max(keep.bounds.end);
            previous.trigger = previous.trigger.min(keep.trigger);
        } else {
            output.pagination_float_keeps.push(keep);
        }
        previous_context = Some(context);
    }
    output
        .pagination_float_keeps
        .sort_unstable_by_key(|keep| keep.trigger);
}

fn line_group(output: &LineOutput, members: Range<usize>) -> LinePaginationGroup {
    let first = &output.lines[layout_state::line_id_index(output.pagination_order[members.start])];
    let mut bounds = first.point.y..first.point.y + first.height;
    for id in &output.pagination_order[members.clone()] {
        let line = &output.lines[layout_state::line_id_index(*id)];
        bounds.start = bounds.start.min(line.point.y);
        bounds.end = bounds.end.max(line.point.y + line.height);
    }
    LinePaginationGroup {
        members: members.start as u32..members.end as u32,
        point: Point::new(first.point.x, bounds.start),
        bounds,
    }
}

/// Parallel cells share one fragment boundary. Rowspans connect their covered
/// rows; a group larger than a page remains splittable by the compositor.
fn attach_table_rows(
    reader: &LayoutReader<'_>,
    geometry: &BoxGeometry,
    output: &LineOutput,
    parents: &mut [usize],
) -> Vec<(usize, Range<f64>)> {
    let mut bands = Vec::new();
    for table in 0..reader.box_count() {
        let Some(table) = reader.layout_tree().get_box_table(table) else { continue };
        let rows = &table.rows;
        let mut start = 0;
        while start < rows.len() {
            let row_parent = reader.get_parent(rows[start] as usize);
            let group_limit = start + rows[start..].iter()
                .take_while(|&&row| reader.get_parent(row as usize) == row_parent).count();
            let mut end = start + 1;
            let mut scan = start;
            let mut parallel = false;
            while scan < end {
                if let Some(LayoutMode::TableRow(row)) = reader.box_layout_mode(rows[scan] as usize) {
                    parallel |= row.cells.len() > 1;
                    for &cell in &row.cells {
                        if let Some(LayoutMode::TableCell(cell)) = reader.box_layout_mode(cell as usize) {
                            end = end.max(scan.saturating_add(cell.rowspan.max(1)).min(group_limit));
                        }
                    }
                }
                scan += 1;
            }
            if parallel || end > start + 1 {
                let mut first = None;
                let mut bounds = f64::INFINITY..f64::NEG_INFINITY;
                for &row in &rows[start..end] {
                    let row = row as usize;
                    bounds.start = bounds.start.min(geometry.point(row).y);
                    bounds.end = bounds.end.max(geometry.point(row).y + geometry.size(row).height);
                    for index in output.box_line_ranges[row].clone() {
                        let owner = output.lines[index as usize].owner_box_idx as usize;
                        if owner != row && !reader.layout_tree().ancestors(owner).any(|ancestor| ancestor == row) {
                            continue;
                        }
                        let rank = output.spatial_order.ranks()[index as usize] as usize;
                        if let Some(first) = first {
                            let a = root(parents, first);
                            let b = root(parents, rank);
                            parents[a.max(b)] = a.min(b);
                        } else {
                            first = Some(rank);
                        }
                    }
                }
                if let Some(first) = first {
                    bands.push((first, bounds));
                }
            }
            start = end;
        }
    }
    bands
}

pub(super) fn build_pagination_groups(
    reader: &LayoutReader<'_>,
    geometry: &BoxGeometry,
    fragments: &mut FragmentWriter<'_>,
) {
    let state = fragments.state_mut();
    let output = &mut state.line_output;
    output.pagination_anchors.clear();
    output.pagination_groups.clear();
    output.pagination_base_groups.clear();
    output.pagination_origins.clear();
    output.pagination_base_origins.clear();
    output.pagination_order.clear();
    output.box_line_ranges.clear();
    output.box_line_ranges.resize(reader.box_count(), 0..0);
    for (index, line) in output.lines.iter().enumerate() {
        let carrier = index as u32..index as u32 + 1;
        include_lines(
            &mut output.box_line_ranges[line.owner_box_idx as usize],
            carrier.clone(),
        );
        let inline = line.inline_box_fragments(&output.line_details);
        for fragment in &output.inline_box_fragments[inline.start as usize..inline.end as usize] {
            include_lines(
                &mut output.box_line_ranges[fragment.box_idx as usize],
                carrier.clone(),
            );
        }
    }
    for image in &state.fragment_output.image_fragments {
        include_lines(
            &mut output.box_line_ranges[image.box_idx as usize],
            image.line_idx as u32..image.line_idx as u32 + 1,
        );
    }
    for owner in (0..reader.box_count()).rev() {
        if let Some(parent) = reader.get_parent(owner) {
            let lines = output.box_line_ranges[owner].clone();
            include_lines(&mut output.box_line_ranges[parent], lines);
        }
    }
    let markers: Vec<_> = (0..reader.box_count())
        .filter_map(|owner| {
            reader
                .list_marker(owner)
                .map(|marker| (owner, marker.marker_box as usize))
        })
        .collect();
    let overlays = positioned_attachments(
        reader, geometry, output, &state.fragment_output.image_fragments,
    );
    let mut parents: Vec<_> = (0..output.lines.len()).collect();
    for (image, overlay) in overlays {
        let a = root(&mut parents, output.spatial_order.ranks()[layout_state::line_id_index(image)] as usize);
        let b = root(&mut parents, output.spatial_order.ranks()[layout_state::line_id_index(overlay)] as usize);
        parents[a.max(b)] = a.min(b);
    }
    // Nested lines retain their paint identities but paginate with the outer
    // line. Attaching every item to its carrier also keeps an entire gallery
    // row together, without encoding any page height into its coordinates.
    for (carrier, children) in &output.atomic_inline_lines {
        let carrier_rank = output.spatial_order.ranks()[layout_state::line_id_index(*carrier)] as usize;
        for child in children.clone() {
            let a = root(&mut parents, carrier_rank);
            let b = root(&mut parents, output.spatial_order.ranks()[child] as usize);
            parents[a.max(b)] = a.min(b);
        }
    }
    let row_bands = attach_table_rows(reader, geometry, output, &mut parents);
    for (owner, marker_box) in markers {
        let marker_lines = &output.box_line_ranges[marker_box];
        let source_lines = &output.box_line_ranges[owner];
        if marker_lines.is_empty() || source_lines.is_empty() {
            continue;
        }
        let marker = LineId::new(marker_lines.start as usize);
        let source = LineId::new(source_lines.start as usize);
        if marker == source {
            continue;
        }
        output
            .pagination_anchors
            .push(LinePaginationAnchor { marker, source });
        let a = root(
            &mut parents,
            output.spatial_order.ranks()[layout_state::line_id_index(marker)] as usize,
        );
        let b = root(
            &mut parents,
            output.spatial_order.ranks()[layout_state::line_id_index(source)] as usize,
        );
        parents[a.max(b)] = a.min(b);
    }
    output
        .pagination_anchors
        .sort_unstable_by_key(|anchor| anchor.marker);
    for index in 0..parents.len() {
        root(&mut parents, index);
    }
    // An image or positioned descendant can extend above its carrier's
    // nominal baseline. Traverse groups by their actual source top so page
    // composition never discovers an earlier fragment after placing text
    // below it.
    let mut group_tops = vec![f64::INFINITY; parents.len()];
    let mut group_bottoms = vec![f64::NEG_INFINITY; parents.len()];
    for (index, line) in output.lines.iter().enumerate() {
        let rank = output.spatial_order.ranks()[index] as usize;
        let group = parents[rank];
        group_tops[group] = group_tops[group].min(line.point.y);
    }
    for (rank, bounds) in row_bands {
        let group = parents[rank];
        group_tops[group] = group_tops[group].min(bounds.start);
        group_bottoms[group] = group_bottoms[group].max(bounds.end);
    }
    for image in &state.fragment_output.image_fragments {
        let rect = visible_image_rect(image, output.lines[image.line_idx].point);
        if rect.width() > 0.0 && rect.height() > 0.0 {
            let rank = output.spatial_order.ranks()[image.line_idx] as usize;
            let group = parents[rank];
            group_tops[group] = group_tops[group].min(rect.y0);
        }
    }
    output
        .pagination_order
        .rebuild_unstable(output.lines.len(), |a, b| {
            let key = |id: &LineId| {
                let rank = output.spatial_order.ranks()[layout_state::line_id_index(*id)] as usize;
                (
                    parents[rank],
                    rank,
                )
            };
            let a = key(a);
            let b = key(b);
            group_tops[a.0].total_cmp(&group_tops[b.0]).then_with(|| a.cmp(&b))
        });
    let mut start = 0;
    while start < output.pagination_order.len() {
        let id = output.pagination_order[start];
        let group = parents[output.spatial_order.ranks()[layout_state::line_id_index(id)] as usize];
        let mut end = start + 1;
        while end < output.pagination_order.len()
            && parents[output.spatial_order.ranks()[layout_state::line_id_index(output.pagination_order[end])] as usize]
                == group
        {
            end += 1;
        }
        if end > start + 1 || group_tops[group] < output.lines[layout_state::line_id_index(id)].point.y
            || group_bottoms[group].is_finite()
        {
            let mut placement = line_group(output, start..end);
            placement.bounds.start = placement.bounds.start.min(group_tops[group]);
            placement.bounds.end = placement.bounds.end.max(group_bottoms[group]);
            placement.point.y = placement.bounds.start;
            output.pagination_groups.push(placement);
        }
        start = end;
    }
    output.pagination_base_groups.clone_from(&output.pagination_groups);
    for image in &state.fragment_output.image_fragments {
        let Some(&cursor) = output.pagination_order.ranks().get(image.line_idx) else {
            continue;
        };
        for groups in [
            &mut output.pagination_groups,
            &mut output.pagination_base_groups,
        ] {
            let index = groups.partition_point(|group| group.members.end <= cursor);
            let Some(group) = groups
                .get_mut(index)
                .filter(|group| group.members.contains(&cursor))
            else {
                continue;
            };
            let rect = Rect::from_origin_size(image.offset, image.size).intersect(image.clip);
            if rect.width() > 0.0 && rect.height() > 0.0 {
                let y = output.lines[image.line_idx].point.y;
                group.bounds.start = group.bounds.start.min(y + rect.y0);
                group.bounds.end = group.bounds.end.max(y + rect.y1);
                group.point.y = group.bounds.start;
            }
        }
    }
    output.pagination_origins = group_origins(output, &output.pagination_groups);
    output.pagination_base_origins.clone_from(&output.pagination_origins);
}

/// Index every structural group and singleton by source origin.
fn group_origins(output: &LineOutput, groups: &[LinePaginationGroup]) -> Vec<u32> {
    let mut origins = Vec::new();
    let mut cursor = 0;
    let mut group_index = 0;
    while cursor < output.pagination_order.len() {
        let (end, y) = if let Some(group) = groups.get(group_index)
            && group.members.start as usize == cursor
        {
            group_index += 1;
            (group.members.end as usize, group.point.y)
        } else {
            let id = output.pagination_order[cursor];
            (cursor + 1, output.lines[layout_state::line_id_index(id)].point.y)
        };
        origins.push((cursor as u32, y));
        cursor = end;
    }
    origins.sort_unstable_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    origins.into_iter().map(|(cursor, _)| cursor).collect()
}
