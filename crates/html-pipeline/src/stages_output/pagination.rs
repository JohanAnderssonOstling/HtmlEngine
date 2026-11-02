//! Ordered pagination inputs, independent of spatial storage and paint order.

use super::{LineId, RenderLine, RenderView};
use crate::layout_model::{LineOutput, LinePaginationGroup};
use kurbo::Point;
use std::ops::Range;

#[derive(Clone, Debug)]
pub struct PaginationLine {
    id: LineId,
    point: Point,
    bounds: Range<f64>,
}

impl PaginationLine {
    pub fn id(&self) -> LineId {
        self.id
    }
    pub fn point(&self) -> Point {
        self.point
    }
    pub fn height(&self) -> f64 {
        self.bounds.end - self.point.y
    }
    pub fn vertical_bounds(&self) -> Range<f64> {
        self.bounds.clone()
    }
}

/// One indivisible placement with explicit line membership. Lines keep their
/// own paint identities and positions relative to the group's common origin.
pub struct PaginationGroup<'a> {
    view: RenderView<'a>,
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
    pub fn vertical_bounds(&self) -> Range<f64> {
        self.bounds.clone()
    }
    pub fn lines(&self) -> impl DoubleEndedIterator<Item = RenderLine> + '_ {
        self.members.iter().map(|id| {
            self.view
                .text()
                .line(id.index())
                .expect("published group member")
        })
    }
}

/// Pagination traversal packs related lines together without incorporating
/// unrelated lines that happen to lie between them in spatial order.
#[derive(Clone, Copy)]
pub struct PaginationLines<'a> {
    pub(super) view: RenderView<'a>,
}

impl<'a> PaginationLines<'a> {
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

    /// A member's geometry in pagination order; its cursor is not a spatial rank.
    pub fn line(self, cursor: usize) -> Option<RenderLine> {
        self.view.text().line(self.order().get(cursor)?.index())
    }
    pub fn at(self, cursor: usize) -> Option<PaginationLine> {
        self.by_id(*self.order().get(cursor)?)
    }
    pub fn by_id(self, id: LineId) -> Option<PaginationLine> {
        let line = self.view.text().line(id.index())?;
        let cursor = self.cursor(id)?;
        let bounds = self
            .data(cursor)
            .map(|group| group.bounds.clone())
            .or_else(|| self.view.geometry().line_vertical_bounds(id.index()))?;
        Some(PaginationLine {
            id,
            point: line.point(),
            bounds,
        })
    }
    pub fn cursor(self, id: LineId) -> Option<usize> {
        let output = self.output();
        let ranks = if output.pagination_ranks.len() == output.lines.len() {
            &output.pagination_ranks
        } else {
            &output.spatial_ranks
        };
        ranks.get(id.index()).map(|&rank| rank as usize)
    }
    pub fn group(self, cursor: usize) -> Option<PaginationGroup<'a>> {
        let line = self.line(cursor)?;
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
                self.view.geometry().line_vertical_bounds(line.index())?,
            )
        };
        Some(PaginationGroup {
            view: self.view,
            members: &self.order()[cursors.clone()],
            cursors,
            point,
            bounds,
        })
    }
    pub fn group_start(self, cursor: usize) -> Option<usize> {
        Some(self.group(cursor)?.cursors.start)
    }
    pub fn group_end(self, cursor: usize) -> Option<usize> {
        self.group(cursor)?.cursors.end.checked_sub(1)
    }

    /// Source-bearing line for reading positions; generated markers share the
    /// first content line's address without changing either paint identity.
    pub fn source_line(self, cursor: usize) -> Option<RenderLine> {
        let mut id = self.line(cursor)?.id();
        let anchors = &self.output().pagination_anchors;
        while let Ok(index) = anchors.binary_search_by_key(&id, |anchor| anchor.marker) {
            id = anchors[index].source;
        }
        self.view.text().line(id.index())
    }

    pub fn start_at(self, offset_y: f64, epsilon: f64) -> usize {
        // A continuation starts at a group's exact source origin. Prefer that
        // group over an earlier oversized/overlapping group already consumed.
        let origins = &self.output().pagination_origins;
        let index = origins.partition_point(|&cursor| {
            self.group(cursor as usize).unwrap().point.y < offset_y - epsilon
        });
        if let Some(&cursor) = origins.get(index)
            && self.group(cursor as usize).unwrap().point.y <= offset_y + epsilon
        {
            return cursor as usize;
        }
        let spatial = self.view.geometry().start_line_at(offset_y, epsilon);
        self.view
            .text()
            .spatial_order()
            .get(spatial)
            .and_then(|&id| self.cursor(id))
            .and_then(|cursor| self.group_start(cursor))
            .unwrap_or(self.len())
    }
    pub fn end_at(self, offset_y: f64, epsilon: f64) -> Option<usize> {
        let spatial = self.view.geometry().end_line_at(offset_y, epsilon)?;
        let mut cursor = self.cursor(*self.view.text().spatial_order().get(spatial)?)?;
        loop {
            let group = self.group(cursor)?;
            if group.bounds.end <= offset_y + epsilon {
                return group.cursors.end.checked_sub(1);
            }
            cursor = group.cursors.start.checked_sub(1)?;
        }
    }
}
