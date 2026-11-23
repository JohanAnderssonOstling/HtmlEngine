//! Ordered pagination inputs, independent of spatial storage and paint order.
//! Lines are addressed by cursors in pagination order; results are values.

use super::{BoxId, LineId, RenderView};
use crate::layout_model::{LineOutput, LinePaginationGroup};
use html_engine_model::layout_state;
use html_style_model::{Display, Float};
use kurbo::Point;
use std::ops::Range;

/// One line's pagination geometry. `vertical_bounds` covers its whole
/// indivisible group when the line belongs to one.
#[derive(Clone, Debug)]
pub struct PaginationLine {
    id: LineId,
    point: Point,
    bounds: Range<f64>,
    glyphs: Range<u32>,
}

impl PaginationLine {
    pub fn id(&self) -> LineId {
        self.id
    }
    pub fn point(&self) -> Point {
        self.point
    }
    /// Logical glyphs on this line; empty for image-only or marker lines.
    pub fn glyphs(&self) -> Range<u32> {
        self.glyphs.clone()
    }
    pub fn height(&self) -> f64 {
        self.bounds.end - self.point.y
    }
    pub(crate) fn vertical_bounds(&self) -> Range<f64> {
        self.bounds.clone()
    }
}

/// One indivisible placement with explicit line membership. Lines keep their
/// own paint identities and positions relative to the group's common origin.
pub struct PaginationGroup<'a> {
    pagination: Pagination<'a>,
    members: &'a [LineId],
    cursors: Range<usize>,
    point: Point,
    bounds: Range<f64>,
}

impl PaginationGroup<'_> {
    pub fn cursors(&self) -> Range<usize> {
        self.cursors.clone()
    }
    pub fn point(&self) -> Point {
        self.point
    }
    pub fn height(&self) -> f64 {
        self.bounds.end - self.point.y
    }
    pub(crate) fn vertical_bounds(&self) -> Range<f64> {
        self.bounds.clone()
    }
    pub fn lines(&self) -> impl DoubleEndedIterator<Item = PaginationLine> + '_ {
        self.members
            .iter()
            .zip(self.cursors.clone())
            .map(|(&id, cursor)| {
                self.pagination
                    .line_at(id, cursor)
                    .expect("published group member")
            })
    }
}

/// A location in pagination order or a retained source line identity.
#[derive(Clone, Copy, Debug)]
pub enum PaginationTarget {
    Cursor(usize),
    Line(LineId),
}

/// Cursor, member geometry and its complete indivisible group resolved together.
pub struct PaginationPosition<'a> {
    pub cursor: usize,
    pub line: PaginationLine,
    pub group: PaginationGroup<'a>,
}

/// Pagination traversal packs related lines together without incorporating
/// unrelated lines that happen to lie between them in spatial order.
#[derive(Clone, Copy)]
pub struct Pagination<'a> {
    pub(super) view: RenderView<'a>,
    uses_float_groups: bool,
}

impl<'a> Pagination<'a> {
    /// Use float groups only with the column height that produced their layout.
    /// Other geometries retain inline-row and marker attachments without treating a
    /// larger source-column band as an indivisible oversized line.
    pub fn for_page_height(mut self, height: f64) -> Self {
        self.uses_float_groups = self
            .fragment_height()
            .is_none_or(|source| (source - height).abs() <= 0.01);
        self
    }

    pub fn locate(self, target: PaginationTarget) -> Option<PaginationPosition<'a>> {
        let cursor = match target {
            PaginationTarget::Cursor(cursor) => cursor,
            PaginationTarget::Line(id) => self.cursor(id)?,
        };
        let group = self.group(cursor)?;
        let line = self.view.text().line(*self.order().get(cursor)?)?;
        let line = PaginationLine {
            id: line.id(),
            point: line.point(),
            bounds: group.bounds.clone(),
            glyphs: line.glyphs(),
        };
        Some(PaginationPosition {
            cursor,
            line,
            group,
        })
    }

    /// Complete groups intersecting the requested cursor interval. Supports
    /// forward and backward traversal without exposing group-step arithmetic.
    pub fn groups(
        self,
        interval: Range<usize>,
    ) -> impl DoubleEndedIterator<Item = PaginationGroup<'a>> + 'a {
        Groups {
            pagination: self,
            remaining: interval.start.min(self.len())..interval.end.min(self.len()),
        }
    }

    fn output(self) -> &'a LineOutput {
        &self.view.doc.layout_state.line_output
    }
    fn order(self) -> &'a [LineId] {
        let output = self.output();
        if output.pagination_order.len() == output.lines.len() {
            &output.pagination_order
        } else {
            &output.spatial_order
        }
    }
    fn data(self, cursor: usize) -> Option<&'a LinePaginationGroup> {
        let groups = if self.uses_float_groups {
            &self.output().pagination_groups
        } else {
            &self.output().pagination_base_groups
        };
        let index = groups.partition_point(|group| group.members.end as usize <= cursor);
        groups
            .get(index)
            .filter(|group| group.members.contains(&(cursor as u32)))
    }
    pub fn len(self) -> usize {
        self.order().len()
    }
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Column height used to resolve float placement and its line exclusions.
    pub fn fragment_height(self) -> Option<f64> {
        self.output().pagination_fragment_height
    }

    /// Float breaks were applied independently during layout and must
    /// not be replayed as breaks in the surrounding normal flow.
    pub fn float_breaks_resolved(self, owner: BoxId) -> bool {
        if !self.uses_float_groups || self.fragment_height().is_none() {
            return false;
        }
        let owner = layout_state::box_id_index(owner);
        let mut current = Some(owner);
        let mut inside_float = false;
        while let Some(index) = current {
            let Some(layout_box) = self.view.doc.inputs.layout_tree.box_at(index) else {
                return false;
            };
            if layout_box.facts.absolute {
                return false;
            }
            // Cells and atomic inline contents suppress fragmentation, even
            // for nested floats. Check all ancestors before trusting a float.
            if layout_box.facts.display == Display::TableCell || layout_box.facts.atomic_inline {
                return false;
            }
            if layout_box.facts.float != Float::None {
                inside_float = true;
            }
            current = self.view.doc.inputs.layout_tree.get_box_parent(index);
        }
        inside_float
    }

    /// Float-content keeps triggered in this cursor interval. Surrounding
    /// wrapped text remains independent. Callers may split oversized ranges.
    pub fn float_keeps(
        self,
        interval: Range<usize>,
    ) -> impl Iterator<Item = (usize, Range<f64>)> + 'a {
        let keeps = &self.output().pagination_float_keeps;
        let start = keeps.partition_point(|keep| (keep.trigger as usize) < interval.start);
        let end = keeps.partition_point(|keep| (keep.trigger as usize) < interval.end);
        keeps[start..end.max(start)]
            .iter()
            .map(|keep| (keep.trigger as usize, keep.bounds.clone()))
    }

    fn line_at(self, id: LineId, cursor: usize) -> Option<PaginationLine> {
        let line = self.view.text().line(id)?;
        let bounds = self
            .data(cursor)
            .map(|group| group.bounds.clone())
            .or_else(|| ((self.view).geometry()).line_vertical_bounds(id))?;
        Some(PaginationLine {
            id,
            point: line.point(),
            bounds,
            glyphs: line.glyphs(),
        })
    }

    /// Source-bearing line for reading positions; generated markers share the
    /// first content line's address without changing either paint identity.
    pub fn source_line(self, cursor: usize) -> Option<PaginationLine> {
        let mut id = *self.order().get(cursor)?;
        let anchors = &self.output().pagination_anchors;
        while let Ok(index) = anchors.binary_search_by_key(&id, |anchor| anchor.marker) {
            id = anchors[index].source;
        }
        self.by_id(id)
    }

    pub fn start_at(self, offset_y: f64, epsilon: f64) -> usize {
        // A continuation starts at a group's exact source origin. Prefer that
        // group over an earlier oversized/overlapping group already consumed.
        let origins = if self.uses_float_groups {
            self.output().pagination_origins.as_slice()
        } else {
            self.output().pagination_base_origins.as_slice()
        };
        let index = origins.partition_point(|&cursor| {
            self.group(cursor as usize).unwrap().point.y < offset_y - epsilon
        });
        if let Some(&cursor) = origins.get(index)
            && self.group(cursor as usize).unwrap().point.y <= offset_y + epsilon
        {
            return cursor as usize;
        }
        let spatial = ((self.view).geometry()).start_spatial_rank_at(offset_y, epsilon);
        (self.view.text())
            .spatial_order()
            .get(spatial)
            .and_then(|&id| self.cursor(id))
            .and_then(|cursor| self.group_start(cursor))
            .unwrap_or(self.len())
    }
    pub fn end_at(self, offset_y: f64, epsilon: f64) -> Option<usize> {
        let spatial = ((self.view).geometry()).end_spatial_rank_at(offset_y, epsilon)?;
        let mut cursor = self.cursor(*(self.view.text()).spatial_order().get(spatial)?)?;
        loop {
            let group = self.group(cursor)?;
            if group.bounds.end <= offset_y + epsilon {
                return group.cursors.end.checked_sub(1);
            }
            cursor = group.cursors.start.checked_sub(1)?;
        }
    }

    /// Structural ownership without constructing line or fragment geometry.
    pub fn line_owner(self, cursor: usize) -> Option<BoxId> {
        let id = *self.order().get(cursor)?;
        let line = self.output().lines.get(layout_state::line_id_index(id))?;
        Some(layout_state::box_id_new(line.owner_box_idx as usize))
    }

    pub fn at(self, cursor: usize) -> Option<PaginationLine> {
        self.line_at(*self.order().get(cursor)?, cursor)
    }

    pub(crate) fn by_id(self, id: LineId) -> Option<PaginationLine> {
        self.line_at(id, self.cursor(id)?)
    }

    pub(crate) fn cursor(self, id: LineId) -> Option<usize> {
        let output = self.output();
        let ranks = if output.pagination_order.ranks().len() == output.lines.len() {
            &output.pagination_order.ranks()
        } else {
            &output.spatial_order.ranks()
        };
        ranks
            .get(layout_state::line_id_index(id))
            .map(|&rank| rank as usize)
    }

    pub(crate) fn group(self, cursor: usize) -> Option<PaginationGroup<'a>> {
        let line = self.view.text().line(*self.order().get(cursor)?)?;
        let (cursors, point, bounds) = if let Some(group) = self.data(cursor) {
            (
                group.members.start as usize..group.members.end as usize,
                group.point,
                group.bounds.clone(),
            )
        } else {
            (
                cursor..cursor + 1,
                line.point(),
                ((self.view).geometry()).line_vertical_bounds(line.id())?,
            )
        };
        Some(PaginationGroup {
            pagination: self,
            members: &self.order()[cursors.clone()],
            cursors,
            point,
            bounds,
        })
    }

    pub fn group_start(self, cursor: usize) -> Option<usize> {
        // Published traversal entries refer to valid laid-out lines. The
        // group start depends only on cursor membership, not its geometry.
        self.order().get(cursor)?;
        Some(self.data(cursor).map_or(cursor, |group| group.members.start as usize))
    }
}

struct Groups<'a> {
    pagination: Pagination<'a>,
    remaining: Range<usize>,
}

impl<'a> Iterator for Groups<'a> {
    type Item = PaginationGroup<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.start >= self.remaining.end {
            return None;
        }
        let group = (self.pagination).group(self.remaining.start)?;
        self.remaining.start = group.cursors.end.min(self.remaining.end);
        Some(group)
    }
}

impl DoubleEndedIterator for Groups<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.remaining.start >= self.remaining.end {
            return None;
        }
        let group = (self.pagination).group(self.remaining.end - 1)?;
        self.remaining.end = group.cursors.start.max(self.remaining.start);
        Some(group)
    }
}

impl<'a> RenderView<'a> {
    pub fn pagination(self) -> Pagination<'a> {
        Pagination {
            view: self,
            uses_float_groups: true,
        }
    }
}
