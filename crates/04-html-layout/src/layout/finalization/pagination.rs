//! Resolve explicit attachment membership once, after final line placement.

use super::{FragmentWriter, LayoutReader};
use crate::layout_model::{LineId, LinePaginationAnchor, LinePaginationGroup};
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

pub(super) fn build_pagination_groups(
    reader: &LayoutReader<'_>,
    fragments: &mut FragmentWriter<'_>,
) {
    let state = fragments.state_mut();
    let output = &mut state.line_output;
    output.pagination_anchors.clear();
    output.pagination_groups.clear();
    output.pagination_origins.clear();
    output.pagination_order.clear();
    output.pagination_ranks.clear();
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
    if markers.is_empty() {
        return;
    }

    let mut parents: Vec<_> = (0..output.lines.len()).collect();
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
        let a = root(&mut parents, output.spatial_ranks[marker.index()] as usize);
        let b = root(&mut parents, output.spatial_ranks[source.index()] as usize);
        parents[a.max(b)] = a.min(b);
    }
    output
        .pagination_anchors
        .sort_unstable_by_key(|anchor| anchor.marker);
    for index in 0..parents.len() {
        root(&mut parents, index);
    }

    output.pagination_order.clear();
    output
        .pagination_order
        .extend_from_slice(&output.spatial_order);
    output.pagination_order.sort_unstable_by_key(|id| {
        let rank = output.spatial_ranks[id.index()] as usize;
        (parents[rank], rank)
    });
    output.pagination_ranks.resize(output.lines.len(), 0);
    for (cursor, id) in output.pagination_order.iter().enumerate() {
        output.pagination_ranks[id.index()] = cursor as u32;
    }
    let mut start = 0;
    while start < output.pagination_order.len() {
        let id = output.pagination_order[start];
        let group = parents[output.spatial_ranks[id.index()] as usize];
        let mut end = start + 1;
        while end < output.pagination_order.len()
            && parents[output.spatial_ranks[output.pagination_order[end].index()] as usize] == group
        {
            end += 1;
        }
        output.pagination_origins.push(start as u32);
        if end > start + 1 {
            let first = &output.lines[id.index()];
            let mut top = first.point.y;
            let mut bottom = first.point.y + first.height;
            for member in &output.pagination_order[start + 1..end] {
                let line = &output.lines[member.index()];
                top = top.min(line.point.y);
                bottom = bottom.max(line.point.y + line.height);
            }
            output.pagination_groups.push(LinePaginationGroup {
                members: start as u32..end as u32,
                point: Point::new(first.point.x, top),
                bounds: top..bottom,
            });
        }
        start = end;
    }
    for image in &state.fragment_output.image_fragments {
        let Some(&cursor) = output.pagination_ranks.get(image.line_idx) else {
            continue;
        };
        let index = output
            .pagination_groups
            .partition_point(|group| group.members.end <= cursor);
        let Some(group) = output
            .pagination_groups
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
        }
    }
    let origins: Vec<_> = output
        .pagination_origins
        .iter()
        .map(|&cursor| {
            let group = output
                .pagination_groups
                .partition_point(|group| group.members.end <= cursor);
            let y = output
                .pagination_groups
                .get(group)
                .filter(|group| group.members.contains(&cursor))
                .map_or_else(
                    || {
                        output.lines[output.pagination_order[cursor as usize].index()]
                            .point
                            .y
                    },
                    |group| group.point.y,
                );
            (cursor, y)
        })
        .collect();
    let mut origins = origins;
    origins.sort_unstable_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    output.pagination_origins.clear();
    output
        .pagination_origins
        .extend(origins.into_iter().map(|(cursor, _)| cursor));
}
