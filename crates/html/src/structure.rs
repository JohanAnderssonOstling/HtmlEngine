//! Structural relationships and authored constraints. These queries describe
//! the document; callers choose whether and how to fragment or display it.

use html_pipeline::engine::pagination_queries;
use html_engine_model::layout_state;
use html_pipeline::engine::semantic_queries;
use html_pipeline::engine::text_paint;
use crate::layout::{BoxId, ContentRole, RenderForcedBreak, RenderView};
use html_pipeline::engine::box_queries;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use kurbo::Rect;
use crate::layout::ContentSemantics;

/// Reusable structural queries for bounded pagination. Retain this service
/// across requests to reuse its indexing and table buffers.
#[derive(Default)]
pub(crate) struct Structure {
    index: BoundaryIndex,
    tables: TableScratch,
}

/// Structural facts bound to one document and a range of pagination cursors.
/// Created by [`Structure::for_lines`]; its indexes cannot be supplied separately.
pub(crate) struct StructureView<'a> {
    view: RenderView<'a>,
    index: &'a BoundaryIndex,
    tables: &'a mut TableScratch,
}

impl Structure {
    /// Rebuild the bounded index, preserving traversal order and buffer capacity.
    pub(crate) fn for_lines<'a>(
        &'a mut self,
        view: RenderView<'a>,
        range: Range<usize>,
    ) -> StructureView<'a> {
        index_lines(view, range, &mut self.index);
        StructureView {
            view,
            index: &self.index,
            tables: &mut self.tables,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AuthoredBoundary {
    pub trigger_line: usize,
    pub top: f64,
    pub forced: Option<RenderForcedBreak>,
    pub avoid: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BlockConstraints {
    pub avoid_inside: bool,
    pub has_forced_descendant: bool,
    pub owns_line_flow: bool,
    pub orphans: usize,
    pub widows: usize,
}

/// Complete document facts for a block in the bounded structural traversal.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlockFacts {
    pub bounds: Rect,
    pub before: Option<usize>,
    pub constraints: BlockConstraints,
    pub roles: ContentSemantics,
    pub is_block_container: bool,
}

/// Adjacent source siblings with explicit structural relationships. No keep
/// length, heading break, or placement preference is inferred from these facts.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SiblingPair {
    pub previous_bounds: Rect,
    pub next_bounds: Rect,
    pub trigger_line: Option<usize>,
    pub forced_boundary: bool,
    pub avoid_boundary: bool,
    pub definition_pair: bool,
    pub statement_label_pair: bool,
    pub caption_pair: bool,
    pub heading_following: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum TableGroupKind {
    WholeTable,
    RowGroup {
        forced: bool,
        header: Option<TableHeader>,
    },
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TableHeader {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TableGroup {
    pub table_box: BoxId,
    pub trigger_line: usize,
    pub top: f64,
    pub bottom: f64,
    pub kind: TableGroupKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum HeaderPolicy {
    #[default]
    ExplicitOnly,
    InferLeadingHeaderCells,
}

#[derive(Default)]
struct TableScratch {
    groups: Vec<TableGroup>,
    table_indices: Vec<BoxId>,
    rows_by_table: Vec<(BoxId, Vec<BoxId>)>,
    row_spans: HashMap<BoxId, usize>,
    leading_headers: Vec<BoxId>,
}

/// Bounded structural anchors expressed as pagination traversal cursors.
/// These positions are independent of the stable IDs used for painting.
#[derive(Default)]
struct BoundaryIndex {
    first_line: HashMap<BoxId, usize>,
    next_line: HashMap<BoxId, usize>,
    candidates: HashSet<BoxId>,
    line_indices: Vec<usize>,
    last_line: HashMap<BoxId, usize>,
    line_owners: Vec<(usize, BoxId)>,
    next_owner_lines: Vec<(BoxId, usize)>,
}

impl StructureView<'_> {
    pub(crate) fn first_line(&self, box_idx: BoxId) -> Option<usize> {
        self.index.first_line.get(&box_idx).copied()
    }

    pub(crate) fn before(&self, box_idx: BoxId) -> Option<usize> {
        self.index
            .first_line
            .get(&box_idx)
            .copied()
            .or_else(|| self.index.next_line.get(&box_idx).copied())
    }

    pub(crate) fn after(&self, box_idx: BoxId) -> Option<usize> {
        self.index.next_line.get(&box_idx).copied()
    }

    pub(crate) fn box_ids(&self) -> impl Iterator<Item = BoxId> + '_ {
        self.index.candidates.iter().copied()
    }

    pub(crate) fn blocks(&self) -> impl Iterator<Item = BlockFacts> + '_ {
        self.box_ids().filter_map(|id| {
            Some(BlockFacts {
                bounds: self.view.boxes().bounds(id)?,
                before: self.before(id),
                constraints: self.constraints(id),
                roles: semantic_queries::render_view_semantics(self.view).content(id),
                is_block_container: box_queries::render_box_view_is_block_container(
                    self.view.boxes(),
                    id,
                ),
            })
        })
    }

    pub(crate) fn lines(&self) -> impl Iterator<Item = usize> + '_ {
        self.index.line_indices.iter().copied()
    }

    pub(crate) fn authored_boundaries(&self) -> impl Iterator<Item = AuthoredBoundary> + '_ {
        let boxes = self.view.boxes();
        self.box_ids().flat_map(move |box_idx| {
            let before = self
                .before(box_idx)
                .zip(box_queries::render_box_view_point(boxes, box_idx))
                .map(|(trigger_line, point)| AuthoredBoundary {
                    trigger_line,
                    top: point.y,
                    forced: box_queries::forced_break_before(boxes, box_idx),
                    avoid: box_queries::avoids_break_before(boxes, box_idx),
                });
            let after = self.after(box_idx).and_then(|trigger_line| {
                Some(AuthoredBoundary {
                    trigger_line,
                    top: pagination_queries::pagination_at(
                        pagination_queries::render_view_pagination(self.view),
                        trigger_line,
                    )?
                    .point()
                    .y,
                    forced: box_queries::forced_break_after(boxes, box_idx),
                    avoid: box_queries::avoids_break_after(boxes, box_idx),
                })
            });
            [before, after]
                .into_iter()
                .flatten()
                .filter(|boundary| boundary.forced.is_some() || boundary.avoid)
        })
    }

    pub(crate) fn constraints(&self, box_idx: BoxId) -> BlockConstraints {
        let boxes = self.view.boxes();
        BlockConstraints {
            avoid_inside: box_queries::avoids_break_inside(boxes, box_idx),
            has_forced_descendant: self.box_ids().any(|descendant| {
                descendant != box_idx
                    && semantic_queries::render_semantics_view_contains(
                        semantic_queries::render_view_semantics(self.view),
                        box_idx,
                        descendant,
                    )
                    && (box_queries::forces_break_before(boxes, descendant)
                        || box_queries::forces_break_after(boxes, descendant))
            }),
            owns_line_flow: box_queries::render_box_view_is_block_container(boxes, box_idx)
                && !self.box_ids().any(|child| {
                    box_queries::parent(boxes, child) == Some(box_idx)
                        && box_queries::render_box_view_is_block_container(boxes, child)
                })
                && !box_queries::is_table(boxes, box_idx)
                && !box_queries::ancestors(boxes, box_idx)
                    .any(|ancestor| box_queries::is_table(boxes, ancestor)),
            orphans: box_queries::orphans(boxes, box_idx),
            widows: box_queries::widows(boxes, box_idx),
        }
    }

    pub(crate) fn sibling_pairs(&self) -> Vec<SiblingPair> {
        let boxes = self.view.boxes();
        let semantics = semantic_queries::render_view_semantics(self.view);
        let mut children: Vec<_> = self
            .box_ids()
            .map(|box_idx| (box_queries::parent(boxes, box_idx), box_idx))
            .collect();
        children.sort_unstable();
        children
            .windows(2)
            .filter_map(|pair| {
                let [(parent, previous), (next_parent, next)] = pair else {
                    return None;
                };
                if parent != next_parent {
                    return None;
                }
                let previous_roles = semantics.content(*previous);
                let next_roles = semantics.content(*next);
                Some(SiblingPair {
                    previous_bounds: boxes.bounds(*previous)?,
                    next_bounds: boxes.bounds(*next)?,
                    trigger_line: self.before(*previous),
                    forced_boundary: box_queries::forces_break_after(boxes, *previous)
                        || box_queries::forces_break_before(boxes, *next),
                    avoid_boundary: box_queries::avoids_break_after(boxes, *previous)
                        || box_queries::avoids_break_before(boxes, *next),
                    definition_pair: previous_roles.has_role(ContentRole::DefinitionTerm)
                        && next_roles.has_role(ContentRole::DefinitionDescription),
                    statement_label_pair: parent.is_some_and(|parent| {
                        semantic_queries::render_semantics_view_has_role(
                            semantics,
                            parent,
                            ContentRole::Statement,
                        )
                    }) && previous_roles.has_role(ContentRole::Label),
                    caption_pair: previous_roles.has_role(ContentRole::Caption)
                        || next_roles.has_role(ContentRole::Caption),
                    heading_following: previous_roles.has_role(ContentRole::Heading),
                })
            })
            .collect()
    }

    pub(crate) fn table_dependencies(&mut self, header_policy: HeaderPolicy) -> &[TableGroup] {
        let view = self.view;
        let anchors = self.index;
        let scratch = &mut *self.tables;
        let TableScratch {
            groups: events,
            table_indices,
            rows_by_table,
            row_spans,
            leading_headers,
        } = scratch;
        events.clear();
        let root = view;
        let boxes = root.boxes();
        let first_local_line_in_range = |top: f64, bottom: f64| {
            anchors.line_indices.iter().copied().find(|&line_idx| {
                pagination_queries::pagination_at(
                    pagination_queries::render_view_pagination(view),
                    line_idx,
                )
                .is_some_and(|line| line.point().y >= top - 0.01 && line.point().y < bottom - 0.01)
            })
        };
        table_indices.clear();
        table_indices.extend(
            anchors
                .candidates
                .iter()
                .copied()
                .filter(|&box_idx| box_queries::is_table(boxes, box_idx)),
        );
        table_indices.sort_unstable();
        while rows_by_table.len() < table_indices.len() {
            rows_by_table.push((layout_state::box_id_new(0), Vec::new()));
        }
        for ((slot, rows), &table_idx) in rows_by_table.iter_mut().zip(table_indices.iter()) {
            *slot = table_idx;
            rows.clear();
        }
        row_spans.clear();
        for box_idx in anchors.candidates.iter().copied() {
            if box_queries::is_table_row(boxes, box_idx)
                && let Some(table_idx) = box_queries::ancestors(boxes, box_idx)
                    .find(|&ancestor| box_queries::is_table(boxes, ancestor))
                && let Ok(table_slot) = table_indices.binary_search(&table_idx)
            {
                rows_by_table[table_slot].1.push(box_idx);
            } else if box_queries::is_table_cell(boxes, box_idx)
                && let Some(row_idx) = box_queries::parent(boxes, box_idx)
            {
                row_spans
                    .entry(row_idx)
                    .and_modify(|span| {
                        *span = (*span).max(box_queries::table_cell_rowspan(boxes, box_idx))
                    })
                    .or_insert_with(|| box_queries::table_cell_rowspan(boxes, box_idx));
            }
        }

        for (table_idx, rows) in rows_by_table.iter_mut().take(table_indices.len()) {
            let table_idx = *table_idx;
            let (Some(table_point), Some(table_size)) = (
                box_queries::render_box_view_point(boxes, table_idx),
                box_queries::render_box_view_size(boxes, table_idx),
            ) else {
                continue;
            };
            let table_bottom = table_point.y + table_size.height;
            if let Some(trigger_line) = first_local_line_in_range(table_point.y, table_bottom) {
                events.push(TableGroup {
                    table_box: table_idx,
                    trigger_line,
                    top: table_point.y,
                    bottom: table_bottom,
                    kind: TableGroupKind::WholeTable,
                });
            }

            rows.sort_by(|&a, &b| {
                let ay = box_queries::render_box_view_point(boxes, a).map_or(0.0, |point| point.y);
                let by = box_queries::render_box_view_point(boxes, b).map_or(0.0, |point| point.y);
                ay.total_cmp(&by).then_with(|| a.cmp(&b))
            });

            // A repeated header is a visible dependency even when its source
            // row is above this viewport. Inspect only the leading rows of the
            // table and stop at its first body row; continuation depth never
            // affects this work.
            leading_headers.clear();
            let contains = |scan: usize| {
                scan < box_queries::len(boxes)
                    && semantic_queries::render_semantics_view_contains(
                        semantic_queries::render_view_semantics(view),
                        table_idx,
                        layout_state::box_id_new(scan),
                    )
            };
            let mut scan = layout_state::box_id_index(table_idx) + 1;
            while contains(scan) {
                if !box_queries::is_table_row(boxes, layout_state::box_id_new(scan)) {
                    scan += 1;
                    continue;
                }
                let row = layout_state::box_id_new(scan);
                let explicit_header = box_queries::ancestors(boxes, row)
                    .take_while(|&ancestor| ancestor != table_idx)
                    .any(|ancestor| box_queries::is_table_header_group(boxes, ancestor));
                let mut cell_count = 0usize;
                let mut header_cell_count = 0usize;
                scan += 1;
                while contains(scan)
                    && !box_queries::is_table_row(boxes, layout_state::box_id_new(scan))
                {
                    let cell = layout_state::box_id_new(scan);
                    if box_queries::is_table_cell(boxes, cell)
                        && box_queries::ancestors(boxes, cell).any(|ancestor| ancestor == row)
                    {
                        cell_count += 1;
                        if semantic_queries::render_view_semantics(view)
                            .content(cell)
                            .has_role(ContentRole::TableHeaderCell)
                        {
                            header_cell_count += 1;
                        }
                    }
                    scan += 1;
                }
                let inferred_header = header_policy == HeaderPolicy::InferLeadingHeaderCells
                    && leading_headers.is_empty()
                    && cell_count > 0
                    && cell_count == header_cell_count;
                if explicit_header || inferred_header {
                    leading_headers.push(row);
                } else {
                    break;
                }
            }
            let repeated_header = leading_headers
                .first()
                .zip(leading_headers.last())
                .and_then(|(&first, &last)| {
                    let top = box_queries::render_box_view_point(boxes, first)?.y;
                    let bottom = box_queries::render_box_view_point(boxes, last)?.y
                        + box_queries::render_box_view_size(boxes, last)?.height;
                    (bottom > top + 0.01).then_some(TableHeader {
                        left: table_point.x,
                        right: table_point.x + table_size.width,
                        top,
                        bottom,
                    })
                });

            let mut group_start = 0usize;
            while group_start < rows.len() {
                let mut group_end = group_start + 1;
                let mut scan = group_start;
                while scan < group_end {
                    group_end = group_end
                        .max(scan.saturating_add(row_spans.get(&rows[scan]).copied().unwrap_or(1)))
                        .min(rows.len());
                    scan += 1;
                }

                let first_row = rows[group_start];
                let last_row = rows[group_end - 1];
                let Some(group_top) =
                    box_queries::render_box_view_point(boxes, first_row).map(|point| point.y)
                else {
                    group_start = group_end;
                    continue;
                };
                let mut group_bottom = box_queries::render_box_view_point(boxes, last_row)
                    .zip(box_queries::render_box_view_size(boxes, last_row))
                    .map_or(group_top, |(point, size)| point.y + size.height);
                if group_end == rows.len()
                    && anchors.line_indices.iter().copied().any(|line_idx| {
                        pagination_queries::pagination_at(
                            pagination_queries::render_view_pagination(view),
                            line_idx,
                        )
                        .is_some_and(|line| {
                            line.point().y >= group_bottom - 0.01
                                && line.point().y < table_bottom - 0.01
                        })
                    })
                {
                    // A bottom caption is part of the table box but not of the
                    // last row. Include it in the final atomic group.
                    group_bottom = group_bottom.max(table_bottom);
                }
                let forced = box_queries::forces_break_before(boxes, first_row)
                    || (group_start > 0
                        && box_queries::forces_break_after(boxes, rows[group_start - 1]));
                if let Some(trigger_line) = first_local_line_in_range(group_top, group_bottom) {
                    events.push(TableGroup {
                        table_box: table_idx,
                        trigger_line,
                        top: group_top,
                        bottom: group_bottom,
                        kind: TableGroupKind::RowGroup {
                            forced,
                            header: repeated_header
                                .filter(|header| group_top >= header.bottom - 0.01),
                        },
                    });
                }
                group_start = group_end;
            }
        }
        events.sort_by_key(|event| {
            (
                event.trigger_line,
                matches!(event.kind, TableGroupKind::RowGroup { .. }),
            )
        });
        events
    }
}

fn index_lines(view: RenderView<'_>, range: Range<usize>, anchors: &mut BoundaryIndex) {
    let boxes = view.boxes();
    let line_count = pagination_queries::render_view_pagination(view).len();
    let local_start = range.start;
    let local_end = range
        .end
        .saturating_sub(1)
        .min(line_count.saturating_sub(1));
    anchors.first_line.clear();
    anchors.next_line.clear();
    anchors.candidates.clear();
    anchors.line_indices.clear();
    if line_count > 0 && range.start < range.end && range.start < line_count {
        anchors.line_indices.extend(local_start..=local_end);
    }
    let line_indices = &anchors.line_indices;
    let candidates = &mut anchors.candidates;
    let first_line = &mut anchors.first_line;
    anchors.last_line.clear();
    anchors.line_owners.clear();
    let last_line = &mut anchors.last_line;
    let line_owners = &mut anchors.line_owners;

    for line_idx in line_indices.iter().copied() {
        let Some(line) = pagination_queries::pagination_at(
            pagination_queries::render_view_pagination(view),
            line_idx,
        )
        .and_then(|line| view.text().line(line.id())) else {
            continue;
        };
        let owner = text_paint::render_line_owner_box(&line);
        let line_idx = pagination_queries::pagination_group_start(
            pagination_queries::render_view_pagination(view),
            line_idx,
        )
        .unwrap_or(line_idx);
        line_owners.push((line_idx, owner));
        for box_idx in std::iter::once(owner).chain(box_queries::ancestors(boxes, owner)) {
            candidates.insert(box_idx);
            first_line
                .entry(box_idx)
                .and_modify(|first: &mut usize| *first = (*first).min(line_idx))
                .or_insert(line_idx);
            last_line
                .entry(box_idx)
                .and_modify(|last: &mut usize| *last = (*last).max(line_idx))
                .or_insert(line_idx);
        }
    }

    // Empty boxes publish no line owner, but break-before/after on an empty
    // sibling applies to the following visible line. Layout boxes are in
    // source preorder, so only inspect the bounded gaps between local owners.
    if line_owners.len() > 1 {
        let start = line_owners.iter().map(|&(_, owner)| owner).min().unwrap();
        let end = line_owners.iter().map(|&(_, owner)| owner).max().unwrap();
        for box_idx in (layout_state::box_id_index(start)..=layout_state::box_id_index(end))
            .map(layout_state::box_id_new)
        {
            candidates.insert(box_idx);
            for ancestor in box_queries::ancestors(boxes, box_idx) {
                candidates.insert(ancestor);
            }
        }
    }

    let next_line = &mut anchors.next_line;
    let by_owner = &mut anchors.next_owner_lines;
    by_owner.clear();
    for box_idx in candidates.iter().copied() {
        let next = if let Some(last) = last_line.get(&box_idx).copied() {
            // Every indexed descendant contributed to last_line, so the
            // first later cursor necessarily belongs outside this box.
            let next = line_owners.partition_point(|&(cursor, _)| cursor <= last);
            line_owners.get(next).map(|&(cursor, _)| cursor)
        } else {
            if by_owner.is_empty() && !line_owners.is_empty() {
                by_owner.extend(line_owners.iter().map(|&(cursor, owner)| (owner, cursor)));
                by_owner.sort_unstable_by_key(|&(owner, _)| owner);
                let mut next = usize::MAX;
                for (_, cursor) in by_owner.iter_mut().rev() {
                    next = next.min(*cursor);
                    *cursor = next;
                }
            }
            let next = by_owner.partition_point(|&(owner, _)| owner <= box_idx);
            by_owner.get(next).map(|&(_, cursor)| cursor)
        };
        if let Some(next) = next {
            next_line.insert(box_idx, next);
        }
    }
}
