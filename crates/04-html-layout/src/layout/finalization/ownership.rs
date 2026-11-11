//! Publish height-independent pagination relationships alongside laid-out lines.

use super::{FragmentWriter, LayoutReader};
use html_engine_model::layout_state::{
    BoxPaginationFacts, LineFragmentOwnership, SourceIntervals, TableFragmentSource,
};
use html_engine_model::{BoxGeometry, LayoutMode};
use html_style_model::{BreakBetween, BreakInside, Float, PositionMode};
use kurbo::Rect;

pub(super) fn prepare_ownership(
    reader: &LayoutReader<'_>,
    geometry: &BoxGeometry,
    fragments: &mut FragmentWriter<'_>,
) {
    let count = reader.box_count();
    let state = fragments.state_mut();
    let output = &mut state.line_output;
    let ownership = &mut output.pagination_ownership;
    ownership.clear();
    ownership.box_intervals = SourceIntervals::new((0..count).map(|owner| {
        let top = geometry.point(owner).y;
        (owner as u32, top, top + geometry.size(owner).height)
    }).collect());
    ownership.boxes.resize(count, BoxPaginationFacts::default());
    ownership.box_flows.resize_with(count, Vec::new);
    let mut children = vec![Vec::new(); count];
    let mut roots = Vec::new();
    for owner in 0..count {
        if let Some(parent) = reader.get_parent(owner) {
            children[parent].push(owner);
        } else {
            roots.push(owner);
        }
        let style = reader.style(owner);
        ownership.boxes[owner] = BoxPaginationFacts {
            independent: style.position() == PositionMode::Absolute
                || matches!(style.float(), Float::Left | Float::Right),
            is_block_container: matches!(
                reader.box_layout_mode(owner),
                Some(LayoutMode::Block(_) | LayoutMode::Table(_) | LayoutMode::TableCell(_))
            ),
            forced_break: matches!(
                style.break_before(),
                BreakBetween::Column | BreakBetween::Page
            ) || matches!(
                style.break_after(),
                BreakBetween::Column | BreakBetween::Page
            ),
            avoid_inside: style.break_inside() == BreakInside::Avoid,
            orphans: style.orphans() as usize,
            widows: style.widows() as usize,
            ..Default::default()
        };
    }
    // Explicit DFS works even when generated boxes were appended after siblings.
    let mut traversal: Vec<_> = roots
        .into_iter()
        .rev()
        .map(|owner| (owner, false, false))
        .collect();
    let mut preorder = 0;
    while let Some((owner, exiting, in_table)) = traversal.pop() {
        if exiting {
            ownership.boxes[owner].subtree.end = preorder;
            continue;
        }
        let is_table = matches!(reader.box_layout_mode(owner), Some(LayoutMode::Table(_)));
        let facts = &mut ownership.boxes[owner];
        facts.subtree.start = preorder;
        facts.in_table = in_table || is_table;
        preorder += 1;
        traversal.push((owner, true, in_table));
        traversal.extend(
            children[owner]
                .iter()
                .rev()
                .map(|&child| (child, false, in_table || is_table)),
        );
    }
    // Retain flow membership once, including currently undecorated boxes: a
    // paint-only style update can add a background without changing layout.
    for owner in 0..count {
        let mut ancestor = Some(owner);
        let mut distance = 0;
        while let Some(flow) = ancestor {
            ownership.box_flows[owner].push((flow as u32, distance));
            if ownership.boxes[flow].independent
                && (flow != owner || !output.box_line_ranges[owner].is_empty())
            {
                break;
            }
            ancestor = reader.get_parent(flow);
            distance += 1;
        }
    }
    // Rows containing a spanning cell are handled by the whole-table source.
    // Split/partial tables retain the general projection path.
    let mut sources = vec![None; count];
    for owner in 0..count {
        let eligible = match reader.box_layout_mode(owner) {
            Some(LayoutMode::Table(_)) => true,
            Some(LayoutMode::TableRow(row)) => row.cells.iter().all(|&cell| {
                matches!(reader.box_layout_mode(cell as usize), Some(LayoutMode::TableCell(cell)) if cell.rowspan == 1)
            }),
            _ => false,
        };
        if eligible {
            sources[owner] = Some(ownership.table_fragments.len() as u32);
            ownership.table_fragments.push(TableFragmentSource {
                bounds: geometry.decoration_rect(owner).unwrap_or_else(|| {
                    Rect::from_origin_size(geometry.point(owner), geometry.size(owner))
                }),
                line_count: 0,
            });
        }
    }
    for owner in 0..count {
        let mut ancestor = Some(owner);
        while let Some(current) = ancestor {
            if let Some(source) = sources[current] {
                let bounds = ownership.table_fragments[source as usize].bounds;
                let box_top = geometry.point(owner).y;
                let box_bottom = box_top + geometry.size(owner).height;
                if box_top >= bounds.y0 && box_bottom <= bounds.y1 {
                    ownership.boxes[owner].table_fragment = Some(source);
                }
                break;
            }
            if ownership.boxes[current].independent {
                break;
            }
            ancestor = reader.get_parent(current);
        }
    }
    let mut content_marks = vec![usize::MAX; count];
    let mut flow_marks = vec![usize::MAX; count];
    let mut carriers = Vec::new();
    for (index, line) in output.lines.iter().enumerate() {
        let images = state
            .fragment_output
            .image_fragments_by_line
            .get(index)
            .unwrap_or(&[]);
        let mut line_ownership = LineFragmentOwnership::default();
        carriers.clear();
        carriers.push(line.owner_box_idx as usize);
        let inline = line.inline_box_fragments(&output.line_details);
        carriers.extend(
            output.inline_box_fragments[inline.start as usize..inline.end as usize]
                .iter()
                .map(|fragment| fragment.box_idx as usize),
        );
        carriers.extend(
            images
                .iter()
                .map(|&image| state.fragment_output.image_fragments[image].box_idx as usize),
        );
        for &carrier in &carriers {
            let mut ancestor = Some(carrier);
            while let Some(owner) = ancestor {
                if content_marks[owner] == index {
                    break;
                }
                content_marks[owner] = index;
                if output.box_line_ranges[owner].contains(&(index as u32)) {
                    line_ownership.content.push(owner as u32);
                }
                ancestor = reader.get_parent(owner);
            }
        }
        carriers.clear();
        if line.glyphs.is_empty() && !images.is_empty() {
            carriers.extend(
                images
                    .iter()
                    .map(|&image| state.fragment_output.image_fragments[image].box_idx as usize),
            );
        } else {
            carriers.push(line.owner_box_idx as usize);
        }
        for &carrier in &carriers {
            let mut ancestor = Some(carrier);
            while let Some(owner) = ancestor {
                if flow_marks[owner] == index {
                    break;
                }
                flow_marks[owner] = index;
                line_ownership.flows.push(owner as u32);
                if ownership.boxes[owner].independent {
                    break;
                }
                ancestor = reader.get_parent(owner);
            }
        }
        for &owner in &line_ownership.content {
            if let Some(source) = sources[owner as usize] {
                ownership.table_fragments[source as usize].line_count += 1;
                line_ownership.table_fragments.push(source);
            }
        }
        ownership.lines.push(line_ownership);
    }
}
