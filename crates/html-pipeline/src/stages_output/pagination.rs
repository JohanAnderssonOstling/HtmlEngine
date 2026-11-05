//! Ordered pagination inputs, independent of spatial storage and paint order.
//! Lines are addressed by cursors in pagination order; results are values.

use html_engine_model::layout_state;
use crate::stages::geometry_queries;
use crate::stages::text_paint;
use super::{LineId, RenderView};
use crate::layout_model::{LineOutput, LinePaginationGroup};
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
}

impl<'a> Pagination<'a> {
    pub fn locate(self, target: PaginationTarget) -> Option<PaginationPosition<'a>> {
        let cursor = match target {
            PaginationTarget::Cursor(cursor) => cursor,
            PaginationTarget::Line(id) => pagination_cursor(self, id)?,
        };
        let group = pagination_group(self, cursor)?;
        let line = self.view.text().line(*self.order().get(cursor)?)?;
        let line = PaginationLine {
            id: line.id(),
            point: line.point(),
            bounds: group.bounds.clone(),
            glyphs: text_paint::render_line_glyphs(&line),
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
        let groups = &self.output().pagination_groups;
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

    fn line_at(self, id: LineId, cursor: usize) -> Option<PaginationLine> {
        let line = self.view.text().line(id)?;
        let bounds = self
            .data(cursor)
            .map(|group| group.bounds.clone())
            .or_else(|| {
                geometry_queries::render_geometry_view_line_vertical_bounds(
                    geometry_queries::render_view_geometry(self.view),
                    id,
                )
            })?;
        Some(PaginationLine {
            id,
            point: line.point(),
            bounds,
            glyphs: text_paint::render_line_glyphs(&line),
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
        pagination_by_id(self, id)
    }

    pub fn start_at(self, offset_y: f64, epsilon: f64) -> usize {
        // A continuation starts at a group's exact source origin. Prefer that
        // group over an earlier oversized/overlapping group already consumed.
        let origins = &self.output().pagination_origins;
        let index = origins.partition_point(|&cursor| {
            pagination_group(self, cursor as usize).unwrap().point.y < offset_y - epsilon
        });
        if let Some(&cursor) = origins.get(index)
            && pagination_group(self, cursor as usize).unwrap().point.y <= offset_y + epsilon
        {
            return cursor as usize;
        }
        let spatial = geometry_queries::render_geometry_view_start_spatial_rank_at(
            geometry_queries::render_view_geometry(self.view),
            offset_y,
            epsilon,
        );
        text_paint::render_text_view_spatial_order(self.view.text())
            .get(spatial)
            .and_then(|&id| pagination_cursor(self, id))
            .and_then(|cursor| pagination_group_start(self, cursor))
            .unwrap_or(self.len())
    }
    pub fn end_at(self, offset_y: f64, epsilon: f64) -> Option<usize> {
        let spatial = geometry_queries::render_geometry_view_end_spatial_rank_at(
            geometry_queries::render_view_geometry(self.view),
            offset_y,
            epsilon,
        )?;
        let mut cursor = pagination_cursor(
            self,
            *text_paint::render_text_view_spatial_order(self.view.text()).get(spatial)?,
        )?;
        loop {
            let group = pagination_group(self, cursor)?;
            if group.bounds.end <= offset_y + epsilon {
                return group.cursors.end.checked_sub(1);
            }
            cursor = group.cursors.start.checked_sub(1)?;
        }
    }
}

pub fn pagination_at<'a>(value: Pagination<'a>, cursor: usize) -> Option<PaginationLine> {
    value.line_at(*value.order().get(cursor)?, cursor)
}

pub(crate) fn pagination_by_id<'a>(value: Pagination<'a>, id: LineId) -> Option<PaginationLine> {
    value.line_at(id, pagination_cursor(value, id)?)
}

pub(crate) fn pagination_cursor<'a>(value: Pagination<'a>, id: LineId) -> Option<usize> {
    let output = value.output();
    let ranks = if output.pagination_order.ranks().len() == output.lines.len() {
        &output.pagination_order.ranks()
    } else {
        &output.spatial_order.ranks()
    };
    ranks
        .get(layout_state::line_id_index(id))
        .map(|&rank| rank as usize)
}

pub(crate) fn pagination_group<'a>(
    value: Pagination<'a>,
    cursor: usize,
) -> Option<PaginationGroup<'a>> {
    let line = value.view.text().line(*value.order().get(cursor)?)?;
    let (cursors, point, bounds) = if let Some(group) = value.data(cursor) {
        (
            group.members.start as usize..group.members.end as usize,
            group.point,
            group.bounds.clone(),
        )
    } else {
        (
            cursor..cursor + 1,
            line.point(),
            geometry_queries::render_geometry_view_line_vertical_bounds(
                geometry_queries::render_view_geometry(value.view),
                line.id(),
            )?,
        )
    };
    Some(PaginationGroup {
        pagination: value,
        members: &value.order()[cursors.clone()],
        cursors,
        point,
        bounds,
    })
}

pub fn pagination_group_start<'a>(value: Pagination<'a>, cursor: usize) -> Option<usize> {
    Some(pagination_group(value, cursor)?.cursors.start)
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
        let group = pagination_group(self.pagination, self.remaining.start)?;
        self.remaining.start = group.cursors.end.min(self.remaining.end);
        Some(group)
    }
}

impl DoubleEndedIterator for Groups<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.remaining.start >= self.remaining.end {
            return None;
        }
        let group = pagination_group(self.pagination, self.remaining.end - 1)?;
        self.remaining.end = group.cursors.start.max(self.remaining.start);
        Some(group)
    }
}

pub fn render_view_pagination(value: RenderView<'_>) -> Pagination<'_> {
    Pagination { view: value }
}
