//! Structural relationships and authored constraints. These queries describe
//! the document; callers choose whether and how to fragment or display it.

use crate::layout::{ContentRole, RenderForcedBreak, RenderView};
use kurbo::Rect;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

#[derive(Clone, Copy)]
pub struct Structure<'a> {
    view: RenderView<'a>,
}

#[derive(Clone, Copy, Debug)]
pub struct AuthoredBoundary {
    pub trigger_line: usize,
    pub top: f64,
    pub forced: Option<RenderForcedBreak>,
    pub avoid: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct BlockConstraints {
    pub avoid_inside: bool,
    pub has_forced_descendant: bool,
    pub owns_line_flow: bool,
    pub orphans: usize,
    pub widows: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoxRegion {
    pub box_idx: usize,
    pub bounds: Rect,
    pub first_line: Option<usize>,
    pub last_line: Option<usize>,
}

/// Adjacent source siblings with explicit structural relationships. No keep
/// length, heading break, or placement preference is inferred from these facts.
#[derive(Clone, Copy, Debug)]
pub struct SiblingPair {
    pub previous: usize,
    pub next: usize,
    pub forced_boundary: bool,
    pub avoid_boundary: bool,
    pub definition_pair: bool,
    pub statement_label_pair: bool,
    pub caption_pair: bool,
    pub heading_following: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TableGroupKind {
    WholeTable,
    RowGroup,
}

#[derive(Clone, Copy, Debug)]
pub struct TableHeader {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct TableGroup {
    pub table_box: usize,
    pub trigger_line: usize,
    pub top: f64,
    pub bottom: f64,
    pub kind: TableGroupKind,
    pub forced: bool,
    pub header: Option<TableHeader>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeaderPolicy {
    #[default]
    ExplicitOnly,
    InferLeadingHeaderCells,
}

#[derive(Default)]
pub struct TableScratch {
    groups: Vec<TableGroup>,
    table_indices: Vec<usize>,
    rows_by_table: Vec<(usize, Vec<usize>)>,
    row_spans: HashMap<usize, usize>,
    leading_headers: Vec<usize>,
}

impl TableScratch {
    pub fn capacities(&self) -> [usize; 5] {
        [
            self.groups.capacity(),
            self.table_indices.capacity(),
            self.rows_by_table.capacity(),
            self.row_spans.capacity(),
            self.leading_headers.capacity(),
        ]
    }
}

#[derive(Default)]
pub struct BoundaryIndex {
    first_line: HashMap<usize, usize>,
    next_line: HashMap<usize, usize>,
    candidates: HashSet<usize>,
    line_indices: Vec<usize>,
    last_line: HashMap<usize, usize>,
    line_owners: Vec<(usize, usize)>,
}

impl BoundaryIndex {
    pub fn first_line(&self, box_idx: usize) -> Option<usize> {
        self.first_line.get(&box_idx).copied()
    }

    pub fn last_line(&self, box_idx: usize) -> Option<usize> {
        self.last_line.get(&box_idx).copied()
    }
    pub fn capacities(&self) -> [usize; 3] {
        [
            self.line_indices.capacity(),
            self.line_owners.capacity(),
            self.candidates.capacity(),
        ]
    }

    pub fn before(&self, box_idx: usize) -> Option<usize> {
        self.first_line
            .get(&box_idx)
            .copied()
            .or_else(|| self.next_line.get(&box_idx).copied())
    }

    pub fn after(&self, box_idx: usize) -> Option<usize> {
        self.next_line.get(&box_idx).copied()
    }

    pub fn boxes(&self) -> impl Iterator<Item = usize> + '_ {
        self.candidates.iter().copied()
    }

    pub fn lines(&self) -> impl Iterator<Item = usize> + '_ {
        self.line_indices.iter().copied()
    }
}

impl<'a> Structure<'a> {
    pub fn new(view: RenderView<'a>) -> Self {
        Self { view }
    }

    pub fn box_region(self, box_idx: usize, index: &BoundaryIndex) -> Option<BoxRegion> {
        let boxes = self.view.boxes();
        Some(BoxRegion {
            box_idx,
            bounds: Rect::from_origin_size(boxes.point(box_idx)?, boxes.size(box_idx)?),
            first_line: index.first_line(box_idx),
            last_line: index.last_line(box_idx),
        })
    }

    /// Authored table captions in the indexed region, excluding nested tables.
    pub fn table_captions(self, table: usize, index: &BoundaryIndex) -> Vec<BoxRegion> {
        let boxes = self.view.boxes();
        let mut captions: Vec<_> = index
            .boxes()
            .filter(|&box_idx| {
                boxes
                    .tag(box_idx)
                    .is_some_and(|tag| tag.eq_ignore_ascii_case("caption"))
                    && boxes
                        .ancestors(box_idx)
                        .find(|&ancestor| boxes.is_table(ancestor))
                        == Some(table)
            })
            .filter_map(|box_idx| self.box_region(box_idx, index))
            .collect();
        captions.sort_by_key(|caption| caption.box_idx);
        captions
    }

    pub fn authored_boundaries<'s>(
        self,
        index: &'s BoundaryIndex,
    ) -> impl Iterator<Item = AuthoredBoundary> + 's
    where
        'a: 's,
    {
        let boxes = self.view.boxes();
        index.boxes().flat_map(move |box_idx| {
            let before =
                index
                    .before(box_idx)
                    .zip(boxes.point(box_idx))
                    .map(|(trigger_line, point)| AuthoredBoundary {
                        trigger_line,
                        top: point.y,
                        forced: boxes.forced_break_before(box_idx),
                        avoid: boxes.avoids_break_before(box_idx),
                    });
            let after = index.after(box_idx).and_then(|trigger_line| {
                Some(AuthoredBoundary {
                    trigger_line,
                    top: self.view.text().line(trigger_line)?.point().y,
                    forced: boxes.forced_break_after(box_idx),
                    avoid: boxes.avoids_break_after(box_idx),
                })
            });
            [before, after]
                .into_iter()
                .flatten()
                .filter(|boundary| boundary.forced.is_some() || boundary.avoid)
        })
    }

    pub fn constraints(self, box_idx: usize, index: &BoundaryIndex) -> BlockConstraints {
        let boxes = self.view.boxes();
        BlockConstraints {
            avoid_inside: boxes.avoids_break_inside(box_idx),
            has_forced_descendant: index.boxes().any(|descendant| {
                descendant != box_idx
                    && self.view.semantics().contains(box_idx, descendant)
                    && (boxes.forces_break_before(descendant)
                        || boxes.forces_break_after(descendant))
            }),
            owns_line_flow: boxes.is_block_container(box_idx)
                && !index.boxes().any(|child| {
                    boxes.parent(child) == Some(box_idx) && boxes.is_block_container(child)
                })
                && !boxes.is_table(box_idx)
                && !boxes
                    .ancestors(box_idx)
                    .any(|ancestor| boxes.is_table(ancestor)),
            orphans: boxes.orphans(box_idx),
            widows: boxes.widows(box_idx),
        }
    }

    /// Return the containing figure for a caption when authored containment
    /// supplies that relationship. Adjacent captions are described separately.
    pub fn caption_owner(self, caption: usize) -> Option<usize> {
        let semantics = self.view.semantics();
        semantics
            .has_role(caption, ContentRole::Caption)
            .then_some(())?;
        self.view
            .boxes()
            .ancestors(caption)
            .find(|&ancestor| semantics.has_role(ancestor, ContentRole::Figure))
    }

    pub fn sibling_pairs(self, index: &BoundaryIndex) -> Vec<SiblingPair> {
        let boxes = self.view.boxes();
        let semantics = self.view.semantics();
        let mut children: Vec<_> = index
            .boxes()
            .map(|box_idx| (boxes.parent(box_idx), box_idx))
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
                    previous: *previous,
                    next: *next,
                    forced_boundary: boxes.forces_break_after(*previous)
                        || boxes.forces_break_before(*next),
                    avoid_boundary: boxes.avoids_break_after(*previous)
                        || boxes.avoids_break_before(*next),
                    definition_pair: previous_roles.has_role(ContentRole::DefinitionTerm)
                        && next_roles.has_role(ContentRole::DefinitionDescription),
                    statement_label_pair: parent
                        .is_some_and(|parent| semantics.has_role(parent, ContentRole::Statement))
                        && previous_roles.has_role(ContentRole::Label),
                    caption_pair: previous_roles.has_role(ContentRole::Caption)
                        || next_roles.has_role(ContentRole::Caption),
                    heading_following: previous_roles.has_role(ContentRole::Heading),
                })
            })
            .collect()
    }

    pub fn index_lines(self, range: Range<usize>, anchors: &mut BoundaryIndex) {
        let boxes = self.view.boxes();
        let text = self.view.text();
        let line_count = text.line_count();
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
            let Some(line) = text.line(line_idx) else {
                continue;
            };
            let owner = line.owner_box_idx();
            line_owners.push((line_idx, owner));
            for box_idx in std::iter::once(owner).chain(boxes.ancestors(owner)) {
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
        for pair in line_owners.windows(2) {
            let [(_, left), (_, right)] = pair else {
                continue;
            };
            let (start, end) = if left <= right {
                (*left, *right)
            } else {
                (*right, *left)
            };
            for box_idx in start..=end {
                candidates.insert(box_idx);
                for ancestor in boxes.ancestors(box_idx) {
                    candidates.insert(ancestor);
                }
            }
        }

        let next_line = &mut anchors.next_line;
        for box_idx in candidates.iter().copied() {
            let next = if let Some(last) = last_line.get(&box_idx).copied() {
                line_owners.iter().find_map(|&(line_idx, owner)| {
                    (line_idx > last && !self.view.semantics().contains(box_idx, owner))
                        .then_some(line_idx)
                })
            } else {
                line_owners.iter().find_map(|&(line_idx, owner)| {
                    (owner > box_idx && !self.view.semantics().contains(box_idx, owner))
                        .then_some(line_idx)
                })
            };
            if let Some(next) = next {
                next_line.insert(box_idx, next);
            }
        }
    }

    pub fn table_dependencies<'s>(
        self,
        anchors: &BoundaryIndex,
        header_policy: HeaderPolicy,
        scratch: &'s mut TableScratch,
    ) -> &'s [TableGroup] {
        let TableScratch {
            groups: events,
            table_indices,
            rows_by_table,
            row_spans,
            leading_headers,
        } = scratch;
        events.clear();
        let root = self.view;
        let boxes = root.boxes();
        let text = root.text();
        let first_local_line_in_range = |top: f64, bottom: f64| {
            anchors.lines().find(|&line_idx| {
                text.line(line_idx).is_some_and(|line| {
                    line.point().y >= top - 0.01 && line.point().y < bottom - 0.01
                })
            })
        };
        table_indices.clear();
        table_indices.extend(anchors.boxes().filter(|&box_idx| boxes.is_table(box_idx)));
        table_indices.sort_unstable();
        while rows_by_table.len() < table_indices.len() {
            rows_by_table.push((0, Vec::new()));
        }
        for ((slot, rows), &table_idx) in rows_by_table.iter_mut().zip(table_indices.iter()) {
            *slot = table_idx;
            rows.clear();
        }
        row_spans.clear();
        for box_idx in anchors.boxes() {
            if boxes.is_table_row(box_idx)
                && let Some(table_idx) = boxes
                    .ancestors(box_idx)
                    .find(|&ancestor| boxes.is_table(ancestor))
                && let Ok(table_slot) = table_indices.binary_search(&table_idx)
            {
                rows_by_table[table_slot].1.push(box_idx);
            } else if boxes.is_table_cell(box_idx)
                && let Some(row_idx) = boxes.parent(box_idx)
            {
                row_spans
                    .entry(row_idx)
                    .and_modify(|span| *span = (*span).max(boxes.table_cell_rowspan(box_idx)))
                    .or_insert_with(|| boxes.table_cell_rowspan(box_idx));
            }
        }

        for (table_idx, rows) in rows_by_table.iter_mut().take(table_indices.len()) {
            let table_idx = *table_idx;
            let (Some(table_point), Some(table_size)) =
                (boxes.point(table_idx), boxes.size(table_idx))
            else {
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
                    forced: false,
                    header: None,
                });
            }

            rows.sort_by(|&a, &b| {
                let ay = boxes.point(a).map_or(0.0, |point| point.y);
                let by = boxes.point(b).map_or(0.0, |point| point.y);
                ay.total_cmp(&by).then_with(|| a.cmp(&b))
            });

            // A repeated header is a visible dependency even when its source
            // row is above this viewport. Inspect only the leading rows of the
            // table and stop at its first body row; continuation depth never
            // affects this work.
            leading_headers.clear();
            let mut scan = table_idx.saturating_add(1);
            while scan < boxes.len() && self.view.semantics().contains(table_idx, scan) {
                if !boxes.is_table_row(scan) {
                    scan += 1;
                    continue;
                }
                let row = scan;
                let explicit_header = boxes
                    .ancestors(row)
                    .take_while(|&ancestor| ancestor != table_idx)
                    .any(|ancestor| boxes.is_table_header_group(ancestor));
                let mut cell_count = 0usize;
                let mut header_cell_count = 0usize;
                scan += 1;
                while scan < boxes.len()
                    && self.view.semantics().contains(table_idx, scan)
                    && !boxes.is_table_row(scan)
                {
                    if boxes.is_table_cell(scan)
                        && boxes.ancestors(scan).any(|ancestor| ancestor == row)
                    {
                        cell_count += 1;
                        if self
                            .view
                            .semantics()
                            .content(scan)
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
                    let top = boxes.point(first)?.y;
                    let bottom = boxes.point(last)?.y + boxes.size(last)?.height;
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
                let Some(group_top) = boxes.point(first_row).map(|point| point.y) else {
                    group_start = group_end;
                    continue;
                };
                let mut group_bottom = boxes
                    .point(last_row)
                    .zip(boxes.size(last_row))
                    .map_or(group_top, |(point, size)| point.y + size.height);
                if group_end == rows.len()
                    && anchors.lines().any(|line_idx| {
                        self.view.text().line(line_idx).is_some_and(|line| {
                            line.point().y >= group_bottom - 0.01
                                && line.point().y < table_bottom - 0.01
                        })
                    })
                {
                    // A bottom caption is part of the table box but not of the
                    // last row. Include it in the final atomic group.
                    group_bottom = group_bottom.max(table_bottom);
                }
                let forced = boxes.forces_break_before(first_row)
                    || (group_start > 0 && boxes.forces_break_after(rows[group_start - 1]));
                if let Some(trigger_line) = first_local_line_in_range(group_top, group_bottom) {
                    events.push(TableGroup {
                        table_box: table_idx,
                        trigger_line,
                        top: group_top,
                        bottom: group_bottom,
                        kind: TableGroupKind::RowGroup,
                        forced,
                        header: repeated_header.filter(|header| group_top >= header.bottom - 0.01),
                    });
                }
                group_start = group_end;
            }
        }
        events.sort_by_key(|event| (event.trigger_line, event.kind));
        events
    }
}
