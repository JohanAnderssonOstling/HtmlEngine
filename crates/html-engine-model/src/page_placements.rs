//! Positioned page content, shared by composition and host painting.

use crate::LineId;
use kurbo::{Point, Size, Vec2};
use std::collections::HashMap;

/// Column geometry in unscaled layout coordinates. The host chooses viewport
/// size, column width/count/gap and applies its outer display scale.
#[derive(Clone, Copy, Debug, Default)]
pub struct PageGeometry {
    pub col_width: f64,
    pub col_count: f64,
    pub col_gap: f64,
    pub size: Size,
}


#[derive(Clone, Copy)]
pub struct LineScreen {
    pub point: Point,
    pub col_index: i32,
    /// Visible vertical band in page coordinates; absent for whole columns.
    pub visible_y: Option<(f64, f64)>,
}

impl LineScreen {
    pub fn visible_top(self) -> f64 {
        self.visible_y.map_or(0.0, |band| band.0)
    }

    pub fn visible_bottom(self, height: f64) -> f64 {
        self.visible_y.map_or(height, |band| band.1)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RepeatedTableHeader {
    pub source_left: f64,
    pub source_right: f64,
    pub source_top: f64,
    pub source_bottom: f64,
    pub col_index: i32,
}

/// The anchor determines how to compose the page; no direction flag is needed.
#[derive(Clone, Copy, Debug)]
pub enum PageRequest {
    /// Start with the indivisible group containing this pagination cursor.
    StartingAt(usize),
    /// Resolve an initial or restored document-space offset to a page start.
    /// Use cursor anchors for subsequent turns when source groups overlap.
    StartingAtOffset(f64),
    /// Finish with the indivisible group containing this pagination cursor.
    EndingAt(usize),
    /// Resolve a restored document-space end offset before composing backward.
    EndingAtOffset(f64),
}

impl Default for PageRequest {
    fn default() -> Self {
        Self::StartingAtOffset(0.0)
    }
}

/// A prepared source band belonging to a box's content or enclosing flow.
/// Horizontal translation excludes the column origin, so viewport repositioning
/// does not invalidate this mapping. Source order is pagination order.
#[derive(Clone, Copy, Debug)]
pub struct BoxProjectionBand {
    pub column: i32,
    pub translation: Vec2,
    pub source_y: f64,
    pub top: f64,
    pub bottom: f64,
    pub distance: usize,
}

/// One placement shared by consecutive lines and all boxes they carry.
#[derive(Clone, Copy, Debug)]
pub struct FlowPlacement {
    pub direct: BoxProjectionBand,
    pub enclosing: Option<BoxProjectionBand>,
}

/// First source position at which an owner participates in a shared placement.
#[derive(Clone, Copy, Debug)]
pub struct PlacementMembership {
    pub placement: usize,
    pub order: usize,
    pub source_y: f64,
}

/// Most owners participate in one span. Keep that membership inline rather
/// than allocating a separate vector for every box and enclosing flow.
pub struct PlacementMemberships {
    pub first: PlacementMembership,
    pub additional: Vec<PlacementMembership>,
}

impl PlacementMemberships {
    pub fn iter(&self) -> impl Iterator<Item = &PlacementMembership> {
        std::iter::once(&self.first).chain(self.additional.iter())
    }

    pub fn retain(&mut self, entry: PlacementMembership) {
        let last = self.additional.last().unwrap_or(&self.first);
        if last.placement != entry.placement {
            self.additional.push(entry);
        }
    }
}

#[derive(Default)]
pub struct PageFlowPlacements {
    /// Authoritative box fragments resolved during composition, shared by
    /// decoration painting and geometry queries.
    pub boxes: HashMap<u32, Vec<BoxProjectionBand>>,
    pub prepared: bool,
    pub spans: Vec<FlowPlacement>,
    pub content: HashMap<u32, PlacementMemberships>,
    pub flows: HashMap<u32, PlacementMemberships>,
}

impl PageFlowPlacements {
    pub fn clear(&mut self) {
        self.prepared = false;
        self.boxes.clear();
        self.spans.clear();
        self.content.clear();
        self.flows.clear();
    }
}

/// All visible fragments of a table or row, including partial continuations.
#[derive(Default)]
pub struct TableFragmentPlacement {
    pub bands: Vec<BoxProjectionBand>,
}

#[derive(Default)]
pub struct PagePositions {
    /// Resolved request that reproduces this page, including normalization at
    /// the document beginning. Hosts can install it without interpreting cursors.
    pub request: PageRequest,
    pub geometry: PageGeometry,
    pub paint_start_offset_y: f64,
    /// Pagination traversal cursor, independent of placed line identity.
    pub start_line: usize,
    /// Final pagination traversal cursor included in this page.
    pub last_line: Option<usize>,
    pub positions: VisibleLinePositions,
    pub offset_breaks: Vec<(f64, f64)>,
    pub repeated_table_headers: Vec<RepeatedTableHeader>,
    pub block_decoration_indices: Option<Vec<usize>>,
    pub flow_placements: PageFlowPlacements,
    pub table_fragments: HashMap<u32, TableFragmentPlacement>,
    /// There is no following content, regardless of composition direction.
    pub reached_end: bool,
    /// Geometric continuation. Use `continuation` before advancing to the next
    /// cursor: the final placed group can still have undisplayed content.
    pub next_start_offset_y: f64,
    /// The current group still has content below this page. Resume at this
    /// source coordinate instead of advancing past its last line.
    pub continuation: Option<(usize, f64)>,
}

impl PagePositions {
    /// Reuse a composed page after a width-only viewport change. The caller
    /// determines that its line membership and vertical composition still fit.
    pub fn reproject_columns(&mut self, geometry: PageGeometry) {
        for (_, screen) in line_positions_mut(&mut self.positions) {
            let column = f64::from(screen.col_index);
            let old_origin = col_x(&self.geometry, column, 0.0);
            let new_origin = col_x(&geometry, column, 0.0);
            screen.point.x += new_origin - old_origin;
        }
        self.geometry = geometry;
    }

    /// Discard the displayed page while retaining the reusable line buffers.
    pub fn clear(&mut self) {
        self.request = PageRequest::default();
        self.start_line = 0;
        self.last_line = None;
        clear_line_positions(&mut self.positions);
        self.paint_start_offset_y = 0.0;
        self.geometry = PageGeometry::default();
        self.offset_breaks.clear();
        self.repeated_table_headers.clear();
        self.block_decoration_indices = None;
        self.flow_placements.clear();
        self.table_fragments.clear();
        self.reached_end = false;
        self.next_start_offset_y = 0.0;
        self.continuation = None;
    }
}

/// Page-local placements keyed by stable engine line identity.
///
/// Positions retain pagination traversal order, while lookups
/// use line IDs. Only the visible page is indexed; document-sized sparse
/// position buffers are unnecessary.
#[derive(Default)]
pub struct VisibleLinePositions {
    first_cursor: usize,
    positions: Vec<LineScreen>,
    ids: Vec<LineId>,
    offsets: HashMap<LineId, usize>,
    /// Additional column fragments of a line that exceeds one column's height.
    continuations: Vec<(LineId, LineScreen)>,
}

impl VisibleLinePositions {
    /// Resolve a stable line identity supplied by the engine's paint or
    /// interaction queries. It is never interpreted as a traversal cursor.
    pub fn get(&self, line: LineId) -> Option<LineScreen> {
        let offset = *self.offsets.get(&line)?;
        self.positions.get(offset).copied()
    }

    pub fn fragments(&self, line: LineId) -> impl Iterator<Item = LineScreen> + '_ {
        self.get(line).into_iter().chain(
            self.continuations.iter()
                .filter_map(move |(id, screen)| (*id == line).then_some(*screen)),
        )
    }

    pub fn line_ids(&self) -> impl Iterator<Item = LineId> + Clone + '_ {
        self.ids.iter().copied()
    }

    pub fn contains(&self, line: LineId) -> bool {
        self.get(line).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Stable line identities and their placements, in pagination order.
    pub fn iter(&self) -> impl Iterator<Item = (LineId, LineScreen)> + Clone + '_ {
        self.ids
            .iter()
            .zip(&self.positions)
            .map(|(id, screen)| (*id, *screen))
            .chain(self.continuations.iter().copied())
    }



    pub fn into_buffer(mut self) -> Vec<LineScreen> {
        self.positions.clear();
        self.positions
    }
}

pub fn with_buffer(first_cursor: usize, mut positions: Vec<LineScreen>) -> VisibleLinePositions {
    positions.clear();
    VisibleLinePositions {
        first_cursor,
        positions,
        ids: Vec::new(),
        offsets: HashMap::new(),
        continuations: Vec::new(),
    }
}

pub fn push(value: &mut VisibleLinePositions, id: LineId, screen: LineScreen) {
    assert!(
        !value.offsets.contains_key(&id),
        "a line can be placed only once per page"
    );
    value.offsets.insert(id, value.positions.len());
    value.ids.push(id);
    value.positions.push(screen);
}

pub fn at_cursor(value: &VisibleLinePositions, cursor: usize) -> Option<LineScreen> {
    let offset = cursor.checked_sub(value.first_cursor)?;
    value.positions.get(offset).copied()
}

/// Retain another clipped column fragment without changing cursor ownership.
pub fn push_fragment(value: &mut VisibleLinePositions, id: LineId, screen: LineScreen) {
    if value.contains(id) {
        value.continuations.push((id, screen));
    } else {
        push(value, id, screen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_positions_keep_noncontiguous_ids_separate_from_traversal_cursors() {
        let mut positions = with_buffer(12, Vec::new());
        for (id, x) in [(42, 10.0), (7, 20.0), (99, 30.0)] {
            push(
                &mut positions,
                LineId::new(id),
                LineScreen {
                    point: Point::new(x, 5.0),
                    col_index: 0,
                    visible_y: None,
                },
            );
        }
        assert_eq!(positions.first_cursor, 12);
        assert_eq!(
            Some(positions.first_cursor + positions.iter().count() - 1),
            Some(14)
        );
        assert_eq!(at_cursor(&positions, 13).unwrap().point.x, 20.0);
        assert_eq!(positions.get(LineId::new(7)).unwrap().point.x, 20.0);
        assert!(positions.get(LineId::new(12)).is_none());
        assert!(positions.get(LineId::new(u32::MAX as usize - 1)).is_none());
        assert_eq!(
            positions.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            [42, 7, 99].map(LineId::new)
        );
        line_positions_mut(&mut positions)
            .find(|(id, _)| *id == LineId::new(99))
            .unwrap()
            .1
            .point
            .y = 8.0;
        assert_eq!(at_cursor(&positions, 14).unwrap().point.y, 8.0);
        clear_line_positions(&mut positions);
        assert!(positions.is_empty());
        assert!(positions.get(LineId::new(7)).is_none());
        assert!(positions.iter().next().is_none());
    }
}

/// Horizontal column placement used by engine page composition and projection.
pub fn col_x(geometry: &PageGeometry, col_index: f64, local_x: f64) -> f64 {
    let content_width = geometry.col_count * geometry.col_width
        + (geometry.col_count - 1.0).max(0.0) * geometry.col_gap;
    let leading = ((geometry.size.width - content_width) / 2.0).max(0.0);
    leading + col_index * (geometry.col_width + geometry.col_gap) + local_x
}

pub fn line_positions_mut(
    value: &mut VisibleLinePositions,
) -> impl Iterator<Item = (LineId, &mut LineScreen)> + '_ {
    value
        .ids
        .iter()
        .zip(&mut value.positions)
        .map(|(id, screen)| (*id, screen))
        .chain(value.continuations.iter_mut().map(|(id, screen)| (*id, screen)))
}

pub fn clear_line_positions(value: &mut VisibleLinePositions) {
    value.first_cursor = 0;
    value.positions.clear();
    value.ids.clear();
    value.offsets.clear();
    value.continuations.clear();
}
