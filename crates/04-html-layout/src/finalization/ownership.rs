//! Publish height-independent pagination relationships alongside laid-out lines.
use html_engine_model::GroupedArena;

use super::{FragmentWriter, LayoutReader};
use html_engine_model::layout_state::{
    BoxPaginationFacts, LineFragmentOwnership, LineMembershipIndex, PaginationOwnership,
    SourceIntervals, TableFragmentSource,
};
use html_engine_model::{BoxGeometry, FragmentOutput, LayoutMode, LineOutput};
use html_style_model::{BreakBetween, BreakInside, Float};
use kurbo::Rect;
use std::iter::once;
use std::ops::Range;

#[derive(Default)]
pub(super) struct OwnershipScratch {
    sources: Vec<Option<u32>>,
    table_spans: GroupedArena<Range<u32>>,
    content_marks: Vec<usize>,
    flow_spans: GroupedArena<Range<u32>>,
    flow_marks: Vec<usize>,
}

pub(super) fn prepare_ownership(
    reader: &LayoutReader<'_>,
    geometry: &BoxGeometry,
    fragments: &mut FragmentWriter<'_>,
    carriers: &GroupedArena<usize>,
    scratch: &mut OwnershipScratch,
) {
    let state = fragments.state_mut();
    state.line_output.pagination_ownership.clear();
    prepare_box_facts(
        reader,
        geometry,
        &mut state.line_output.pagination_ownership,
    );
    prepare_table_sources(
        reader,
        geometry,
        &mut state.line_output.pagination_ownership,
        scratch,
    );
    collect_line_ownership(
        reader,
        &mut state.line_output,
        &state.fragment_output,
        carriers,
        scratch,
    );
    publish_membership_indexes(&mut state.line_output.pagination_ownership, scratch);
}

fn prepare_box_facts(
    reader: &LayoutReader<'_>,
    geometry: &BoxGeometry,
    ownership: &mut PaginationOwnership,
) {
    let count = reader.box_count();
    ownership.box_intervals = SourceIntervals::new(
        (0..count)
            .map(|owner| {
                let top = geometry.point(owner).y;
                (owner as u32, top, top + geometry.size(owner).height)
            })
            .collect(),
    );
    ownership.boxes.resize(count, BoxPaginationFacts::default());
    for owner in 0..count {
        let style = reader.style(owner);
        ownership.boxes[owner] = BoxPaginationFacts {
            in_table: matches!(reader.box_layout_mode(owner), Some(LayoutMode::Table(_)))
                || reader
                    .get_parent(owner)
                    .is_some_and(|parent| ownership.boxes[parent].in_table),
            independent: reader.box_facts(owner).absolute
                || matches!(reader.box_facts(owner).float, Float::Left | Float::Right),
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
}

fn prepare_table_sources(
    reader: &LayoutReader<'_>,
    geometry: &BoxGeometry,
    ownership: &mut PaginationOwnership,
    scratch: &mut OwnershipScratch,
) {
    let count = reader.box_count();
    // Rows containing a spanning cell are handled by the whole-table source.
    // Split/partial tables retain the general projection path.
    scratch.sources.clear();
    scratch.sources.resize(count, None);
    for owner in 0..count {
        let eligible = match reader.box_layout_mode(owner) {
            Some(LayoutMode::Table(_)) => true,
            Some(LayoutMode::TableRow(row)) => row.cells.iter().all(|&cell| {
                matches!(reader.box_layout_mode(cell as usize), Some(LayoutMode::TableCell(cell)) if cell.rowspan == 1)
            }),
            _ => false,
        };
        if eligible {
            scratch.sources[owner] = Some(ownership.table_fragments.len() as u32);
            ownership.table_fragments.push(TableFragmentSource {
                bounds: geometry.decoration_rect(owner).unwrap_or_else(|| {
                    Rect::from_origin_size(geometry.point(owner), geometry.size(owner))
                }),
                lines: 0..0,
                line_count: 0,
            });
        }
    }
    for owner in 0..count {
        for current in once(owner).chain(reader.layout_tree().ancestors(owner)) {
            if let Some(source) = scratch.sources[current] {
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
        }
    }
}

fn collect_line_ownership(
    reader: &LayoutReader<'_>,
    output: &mut LineOutput,
    fragments: &FragmentOutput,
    carriers: &GroupedArena<usize>,
    scratch: &mut OwnershipScratch,
) {
    let count = reader.box_count();
    let ownership = &mut output.pagination_ownership;
    scratch
        .table_spans
        .reset_groups(ownership.table_fragments.len());
    scratch.content_marks.clear();
    scratch.content_marks.resize(count, usize::MAX);
    scratch.flow_spans.reset_groups(count);
    scratch.flow_marks.clear();
    scratch.flow_marks.resize(count, usize::MAX);
    for (index, line) in output.lines.iter().enumerate() {
        let images = fragments.image_fragments_by_line.get(index).unwrap_or(&[]);
        let mut line_ownership = LineFragmentOwnership::default();
        for &carrier in carriers.get(index).expect("valid carrier group") {
            for owner in once(carrier).chain(reader.layout_tree().ancestors(carrier)) {
                if scratch.content_marks[owner] == index {
                    break;
                }
                scratch.content_marks[owner] = index;
                if output.box_line_ranges[owner].contains(&(index as u32)) {
                    line_ownership.content.push(owner as u32);
                    if let Some(source) = scratch.sources[owner as usize] {
                        ownership.table_fragments[source as usize].line_count += 1;
                        extend_span(&mut scratch.table_spans, source as usize, index as u32);
                    }
                }
            }
        }
        let image_only = line.glyphs.is_empty() && !images.is_empty();
        let flow_carriers = (!image_only)
            .then_some(line.owner_box_idx as usize)
            .into_iter()
            .chain(
                images
                    .iter()
                    .filter(|_| image_only)
                    .map(|&image| fragments.image_fragments[image].box_idx as usize),
            );
        for carrier in flow_carriers {
            for owner in once(carrier).chain(reader.layout_tree().ancestors(carrier)) {
                if scratch.flow_marks[owner] == index {
                    break;
                }
                scratch.flow_marks[owner] = index;
                extend_span(&mut scratch.flow_spans, owner, index as u32);
                if ownership.boxes[owner].independent {
                    break;
                }
            }
        }
        ownership.lines.push(line_ownership);
    }
}

fn publish_membership_indexes(ownership: &mut PaginationOwnership, scratch: &mut OwnershipScratch) {
    scratch.table_spans.freeze();
    scratch.flow_spans.freeze();
    for (source, fragment) in ownership.table_fragments.iter_mut().enumerate() {
        let start = ownership.table_line_spans.len() as u32;
        ownership
            .table_line_spans
            .extend_from_slice(scratch.table_spans.get(source).expect("valid table group"));
        fragment.lines = start..ownership.table_line_spans.len() as u32;
    }
    ownership.table_line_index = LineMembershipIndex::new(
        ownership
            .table_fragments
            .iter()
            .enumerate()
            .map(|(source, fragment)| {
                (
                    source as u32,
                    &ownership.table_line_spans
                        [fragment.lines.start as usize..fragment.lines.end as usize],
                )
            }),
        ownership.table_line_spans.len(),
    );
    for owner in 0..scratch.flow_spans.len() {
        let start = ownership.flow_line_spans.len() as u32;
        ownership
            .flow_line_spans
            .extend_from_slice(scratch.flow_spans.get(owner).expect("valid flow group"));
        ownership
            .flow_line_ranges
            .push(start..ownership.flow_line_spans.len() as u32);
    }
    ownership.flow_line_index = LineMembershipIndex::new(
        ownership
            .flow_line_ranges
            .iter()
            .enumerate()
            .map(|(source, range)| {
                (
                    source as u32,
                    &ownership.flow_line_spans[range.start as usize..range.end as usize],
                )
            }),
        ownership.flow_line_spans.len(),
    );
}

/// Coalesce encounter-ordered lines; the arena groups spans by owner on freeze.
fn extend_span(spans: &mut GroupedArena<Range<u32>>, owner: usize, line: u32) {
    if let Some(previous) = spans.last_mut(owner)
        && previous.end == line
    {
        previous.end += 1;
    } else {
        spans.push_to(owner, line..line + 1);
    }
}
