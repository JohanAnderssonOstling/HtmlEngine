//! Structural relationships and authored constraints. These queries describe
//! the document; callers choose whether and how to fragment or display it.

use crate::layout::ContentSemantics;
use crate::layout::{BoxId, ContentRole, RenderForcedBreak, RenderView};
use html_engine_model::layout_state;
use html_pipeline::engine::Pagination;
use kurbo::Rect;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

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
    pagination: Pagination<'a>,
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
        self.for_pagination(
            view,
            range,
            view.pagination(),
        )
    }

    pub(crate) fn for_page<'a>(
        &'a mut self,
        view: RenderView<'a>,
        range: Range<usize>,
        height: f64,
    ) -> StructureView<'a> {
        let pagination = view.pagination();
        if pagination
            .fragment_height()
            .is_none_or(|source| (source - height).abs() <= 0.01)
        {
            return self.for_lines(view, range);
        }
        self.for_pagination(view, range, pagination.for_page_height(height))
    }

    fn for_pagination<'a>(
        &'a mut self,
        view: RenderView<'a>,
        range: Range<usize>,
        pagination: Pagination<'a>,
    ) -> StructureView<'a> {
        index_lines(view, range, &mut self.index, pagination);
        StructureView {
            view,
            pagination,
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
    pub owner: BoxId,
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
    pub next_owner: BoxId,
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
        allow_content_break: bool,
    },
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TableGroup {
    pub table_box: BoxId,
    pub trigger_line: usize,
    pub top: f64,
    pub bottom: f64,
    pub kind: TableGroupKind,
}

#[derive(Default)]
struct TableScratch {
    groups: Vec<TableGroup>,
    table_indices: Vec<BoxId>,
    rows_by_table: Vec<(BoxId, Vec<BoxId>)>,
    row_spans: HashMap<BoxId, usize>,
}

/// Inclusive cursor bounds for a box's indexed descendant lines.
struct LineBounds {
    first: usize,
    last: usize,
}

/// Bounded structural anchors expressed as pagination traversal cursors.
/// These positions are independent of the stable IDs used for painting.
#[derive(Default)]
struct BoundaryIndex {
    forced_descendant: HashSet<BoxId>,
    block_parents: HashSet<BoxId>,
    line_bounds: HashMap<BoxId, LineBounds>,
    next_line: HashMap<BoxId, usize>,
    candidates: HashSet<BoxId>,
    line_range: Range<usize>,
    line_owners: Vec<(usize, BoxId)>,
    next_owner_lines: Vec<(BoxId, usize)>,
}

impl StructureView<'_> {
    pub(crate) fn pagination(&self) -> Pagination<'_> {
        self.pagination
    }

    pub(crate) fn first_line(&self, box_idx: BoxId) -> Option<usize> {
        self.index.line_bounds.get(&box_idx).map(|bounds| bounds.first)
    }

    pub(crate) fn before(&self, box_idx: BoxId) -> Option<usize> {
        self.index
            .line_bounds
            .get(&box_idx)
            .map(|bounds| bounds.first)
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
                owner: id,
                bounds: self.view.boxes().bounds(id)?,
                before: self.before(id),
                constraints: self.constraints(id),
                roles: (self.view).semantics().content(id),
                is_block_container: (self.view.boxes()).is_block_container(id),
            })
        })
    }

    pub(crate) fn lines(&self) -> impl Iterator<Item = usize> + '_ {
        self.index.line_range.clone()
    }

    pub(crate) fn authored_boundaries(&self) -> impl Iterator<Item = AuthoredBoundary> + '_ {
        let boxes = self.view.boxes();
        self.box_ids().flat_map(move |box_idx| {
            let resolved = self.pagination.float_breaks_resolved(box_idx);
            let before = self
                .before(box_idx)
                .zip(boxes.point(box_idx))
                .map(|(trigger_line, point)| AuthoredBoundary {
                    trigger_line,
                    top: point.y,
                    forced: (!resolved)
                        .then(|| boxes.forced_break_before(box_idx))
                        .flatten(),
                    avoid: boxes.avoids_break_before(box_idx),
                });
            let after = self.after(box_idx).and_then(|trigger_line| {
                Some(AuthoredBoundary {
                    trigger_line,
                    top: (self.pagination).at(trigger_line)?
                        .point()
                        .y,
                    forced: (!resolved)
                        .then(|| boxes.forced_break_after(box_idx))
                        .flatten(),
                    avoid: boxes.avoids_break_after(box_idx),
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
        let facts = boxes.pagination_facts(box_idx).expect("published pagination structure");
        BlockConstraints {
            avoid_inside: facts.avoid_inside,
            has_forced_descendant: self.index.forced_descendant.contains(&box_idx),
            owns_line_flow: facts.is_block_container && !facts.in_table && !self.index.block_parents.contains(&box_idx),
            orphans: facts.orphans,
            widows: facts.widows,
        }
    }

    pub(crate) fn sibling_pairs(&self) -> Vec<SiblingPair> {
        let boxes = self.view.boxes();
        let semantics = (self.view).semantics();
        let mut children: Vec<_> = self
            .box_ids()
            .map(|box_idx| (boxes.parent(box_idx), box_idx))
            .collect();
        children.sort_unstable();
        // The right sibling becomes the left sibling in the following pair.
        let mut previous_roles: Option<(BoxId, ContentSemantics)> = None;
        children
            .windows(2)
            .filter_map(|pair| {
                let [(parent, previous), (next_parent, next)] = pair else {
                    return None;
                };
                if parent != next_parent {
                    return None;
                }
                let left_roles = previous_roles
                    .filter(|(owner, _)| owner == previous)
                    .map_or_else(|| semantics.content(*previous), |(_, roles)| roles);
                let next_roles = semantics.content(*next);
                previous_roles = Some((*next, next_roles));
                Some(SiblingPair {
                    previous_bounds: boxes.bounds(*previous)?,
                    next_owner: *next,
                    next_bounds: boxes.bounds(*next)?,
                    trigger_line: self.before(*previous),
                    forced_boundary: boxes.forces_break_after(*previous)
                        || boxes.forces_break_before(*next),
                    avoid_boundary: boxes.avoids_break_after(*previous)
                        || boxes.avoids_break_before(*next),
                    definition_pair: left_roles.has_role(ContentRole::DefinitionTerm)
                        && next_roles.has_role(ContentRole::DefinitionDescription),
                    statement_label_pair: left_roles.has_role(ContentRole::Label)
                        && parent.is_some_and(|parent| {
                            semantics.has_role(parent, ContentRole::Statement)
                        }),
                    caption_pair: left_roles.has_role(ContentRole::Caption)
                        || next_roles.has_role(ContentRole::Caption),
                    heading_following: left_roles.has_role(ContentRole::Heading),
                })
            })
            .collect()
    }

    pub(crate) fn table_dependencies(&mut self) -> &[TableGroup] {
        let view = self.view;
        let anchors = self.index;
        let scratch = &mut *self.tables;
        let TableScratch {
            groups: events,
            table_indices,
            rows_by_table,
            row_spans,
        } = scratch;
        events.clear();
        let root = view;
        let boxes = root.boxes();
        table_indices.clear();
        table_indices.extend(
            anchors
                .candidates
                .iter()
                .copied()
                .filter(|&box_idx| boxes.is_table(box_idx)),
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
            if boxes.is_table_row(box_idx)
                && let Some(table_idx) = boxes.ancestors(box_idx)
                    .find(|&ancestor| boxes.is_table(ancestor))
                && let Ok(table_slot) = table_indices.binary_search(&table_idx)
            {
                rows_by_table[table_slot].1.push(box_idx);
            } else if boxes.is_table_cell(box_idx)
                && let Some(row_idx) = boxes.parent(box_idx)
            {
                row_spans
                    .entry(row_idx)
                    .and_modify(|span| {
                        *span = (*span).max(boxes.table_cell_rowspan(box_idx))
                    })
                    .or_insert_with(|| boxes.table_cell_rowspan(box_idx));
            }
        }

        for (table_idx, rows) in rows_by_table.iter_mut().take(table_indices.len()) {
            let table_idx = *table_idx;
            let (Some(table_point), Some(table_size)) = (
                boxes.point(table_idx),
                boxes.size(table_idx),
            ) else {
                continue;
            };
            let table_bottom = table_point.y + table_size.height;
            if let Some(bounds) = anchors.line_bounds.get(&table_idx) {
                let trigger_line = bounds.first;
                events.push(TableGroup {
                    table_box: table_idx,
                    trigger_line,
                    top: table_point.y,
                    bottom: table_bottom,
                    kind: TableGroupKind::WholeTable,
                });
            }

            rows.sort_by(|&a, &b| {
                let ay = boxes.point(a).map_or(0.0, |point| point.y);
                let by = boxes.point(b).map_or(0.0, |point| point.y);
                ay.total_cmp(&by).then_with(|| a.cmp(&b))
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
                    boxes.point(first_row).map(|point| point.y)
                else {
                    group_start = group_end;
                    continue;
                };
                let mut group_bottom = boxes.point(last_row)
                    .zip(boxes.size(last_row))
                    .map_or(group_top, |(point, size)| point.y + size.height);
                if group_end == rows.len()
                    && anchors.line_range.clone().any(|line_idx| {
                        self.pagination.at(line_idx)
                            .and_then(|line| view.text().line(line.id()))
                            .is_some_and(|line| {
                                view.semantics().contains(table_idx, line.owner_box())
                                    && line.point().y >= group_bottom - 0.01
                                    && line.point().y < table_bottom - 0.01
                            })
                    })
                {
                    // A bottom caption is part of the table box but not of the
                    // last row. Include it in the final atomic group.
                    group_bottom = group_bottom.max(table_bottom);
                }
                let forced = !self.pagination.float_breaks_resolved(first_row)
                    && (boxes.forces_break_before(first_row)
                        || (group_start > 0
                            && boxes.forces_break_after(rows[group_start - 1])));
                if let Some(trigger_line) = rows[group_start..group_end].iter()
                    .filter_map(|row| anchors.line_bounds.get(row).map(|bounds| bounds.first)).min()
                {
                    events.push(TableGroup {
                        table_box: table_idx,
                        trigger_line,
                        top: group_top,
                        bottom: group_bottom,
                        kind: TableGroupKind::RowGroup {
                            forced,
                            allow_content_break: group_end == group_start + 1
                                && boxes.table_row_allows_content_break(first_row),
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

fn index_lines(
    view: RenderView<'_>,
    range: Range<usize>,
    anchors: &mut BoundaryIndex,
    pagination: Pagination<'_>,
) {
    let boxes = view.boxes();
    let line_count = pagination.len();
    let end = range.end.min(line_count);
    anchors.line_range = range.start.min(end)..end;
    anchors.line_bounds.clear();
    anchors.next_line.clear();
    anchors.candidates.clear();
    let candidates = &mut anchors.candidates;
    let line_bounds = &mut anchors.line_bounds;
    anchors.line_owners.clear();
    let line_owners = &mut anchors.line_owners;

    let mut retain_bounds = |owner, first: usize, last: usize| {
        for box_idx in std::iter::once(owner).chain(boxes.ancestors(owner)) {
            candidates.insert(box_idx);
            line_bounds
                .entry(box_idx)
                .and_modify(|bounds| {
                    bounds.first = bounds.first.min(first);
                    bounds.last = bounds.last.max(last);
                })
                .or_insert(LineBounds { first, last });
        }
    };
    // Adjacent lines commonly share an owner. Their ancestor bounds need only
    // one update per owner run; retain every line below for cursor lookups.
    let mut pending: Option<(BoxId, usize, usize)> = None;
    for line_idx in anchors.line_range.clone() {
        let Some(owner) = pagination.line_owner(line_idx) else {
            continue;
        };
        let line_idx = pagination.group_start(line_idx).unwrap_or(line_idx);
        line_owners.push((line_idx, owner));
        match &mut pending {
            Some((previous, first, last)) if *previous == owner => {
                *first = (*first).min(line_idx);
                *last = (*last).max(line_idx);
            }
            _ => {
                if let Some((owner, first, last)) = pending {
                    retain_bounds(owner, first, last);
                }
                pending = Some((owner, line_idx, line_idx));
            }
        }
    }
    if let Some((owner, first, last)) = pending {
        retain_bounds(owner, first, last);
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
            // Every retained candidate already includes its ancestor chain.
            // Stop at the first existing member instead of re-inserting it all.
            for owner in std::iter::once(box_idx).chain(boxes.ancestors(box_idx)) {
                if !candidates.insert(owner) {
                    break;
                }
            }
        }
    }

    anchors.forced_descendant.clear();
    anchors.block_parents.clear();
    for &owner in candidates.iter() {
        let Some(facts) = boxes.pagination_facts(owner) else { continue };
        if facts.forced_break {
            // Mark strict ancestors of relevant candidates, excluding the box's
            // own break. An existing mark means its ancestors are marked too.
            for ancestor in boxes.ancestors(owner) {
                if !anchors.forced_descendant.insert(ancestor) {
                    break;
                }
            }
        }
        if facts.is_block_container && let Some(parent) = boxes.parent(owner) {
            anchors.block_parents.insert(parent);
        }
    }

    let next_line = &mut anchors.next_line;
    let by_owner = &mut anchors.next_owner_lines;
    by_owner.clear();
    for box_idx in candidates.iter().copied() {
        let next = if let Some(bounds) = line_bounds.get(&box_idx) {
            // Every indexed descendant contributed to these bounds, so the
            // first later cursor necessarily belongs outside this box.
            let next = line_owners.partition_point(|&(cursor, _)| cursor <= bounds.last);
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
