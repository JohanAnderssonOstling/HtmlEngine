//! Logical selection transitions. Hosts translate their input into character
//! positions and decide how selections are displayed or placed on a clipboard.
use super::Selection;
use crate::export::TableExportRequest;
use crate::geometry::WordHit;
use crate::layout::BoxId;
use html_pipeline::engine::RenderTable;
use html_pipeline::engine::RenderTableCell;
use crate::layout::RenderView;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionMode {
    #[default]
    Plain,
    Semantic,
}

impl SelectionMode {
    fn is_semantic(self) -> bool {
        matches!(self, Self::Semantic)
    }

    fn promote(self, requested: Self) -> Self {
        if self.is_semantic() || requested.is_semantic() {
            Self::Semantic
        } else {
            Self::Plain
        }
    }
}

/// A fixed logical interval used when extending a selection by whole words.
/// A zero-length interval fixes one boundary, such as a selection handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectionAnchor {
    range: Range<u32>,
}

impl SelectionAnchor {
    pub(crate) fn new(range: Range<u32>) -> Self {
        Self { range }
    }

    pub(crate) fn extend_to(&self, word: Range<u32>) -> Range<u32> {
        word.start.min(self.range.start)..word.end.max(self.range.end)
    }
}

/// Logical range and gesture state. Text is exported only by explicit actions.
#[derive(Clone, Default)]
pub(crate) struct SelectionState {
    phase: TextSelectionPhase,
}

#[derive(Clone, Default)]
enum TextSelectionPhase {
    #[default]
    Empty,
    Positioned(SelectionEndpoints),
    Dragging {
        endpoints: SelectionEndpoints,
        anchor: WordHit,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SelectionEndpoints {
    anchor: u32,
    active: u32,
    mode: SelectionMode,
}

impl SelectionEndpoints {
    fn range(self) -> Option<Range<u32>> {
        (self.anchor != self.active)
            .then_some(self.anchor.min(self.active)..self.anchor.max(self.active))
    }
}

impl TextSelectionPhase {
    fn endpoints(&self) -> Option<SelectionEndpoints> {
        match self {
            Self::Empty => None,
            Self::Positioned(endpoints) | Self::Dragging { endpoints, .. } => Some(*endpoints),
        }
    }
}

impl SelectionState {
    pub(crate) fn range(&self) -> Option<Range<u32>> {
        self.phase.endpoints()?.range()
    }

    pub(crate) fn selection<'a>(&self, view: RenderView<'a>) -> Option<Selection<'a>> {
        let endpoints = self.phase.endpoints()?;
        let selection = Selection::new(view, endpoints.range()?);
        Some(if endpoints.mode.is_semantic() {
            selection.with_semantics()
        } else {
            selection
        })
    }

    /// Whether a selection gesture is active, including a collapsed click.
    pub(crate) fn is_selecting(&self) -> bool {
        matches!(self.phase, TextSelectionPhase::Dragging { .. })
    }

    /// Keep a click collapsed until the pointer moves, then select complete
    /// words around the initial word in either drag direction.
    pub(crate) fn begin_at_word(&mut self, hit: WordHit, mode: SelectionMode) {
        self.phase = TextSelectionPhase::Dragging {
            endpoints: SelectionEndpoints {
                anchor: hit.caret,
                active: hit.caret,
                mode,
            },
            anchor: hit,
        };
    }

    /// Select the initial word immediately and keep it anchored while dragging,
    /// as when a double-click begins a word selection.
    pub(crate) fn begin_selected_word(&mut self, hit: WordHit, mode: SelectionMode) {
        self.phase = TextSelectionPhase::Dragging {
            endpoints: SelectionEndpoints {
                anchor: hit.word.start,
                active: hit.word.end,
                mode,
            },
            anchor: hit,
        };
    }

    /// `drag_distance` is host policy, measured in the coordinates of `WordHit`.
    pub(crate) fn update_word_interaction(
        &mut self,
        hit: Option<WordHit>,
        mode: SelectionMode,
        drag_distance: f64,
    ) -> bool {
        let TextSelectionPhase::Dragging { endpoints, anchor } = &mut self.phase else {
            return false;
        };
        let previous_mode = endpoints.mode;
        endpoints.mode = endpoints.mode.promote(mode);
        let mode_changed = endpoints.mode != previous_mode;
        let previous_range = endpoints.range();
        let Some(hit) = hit else {
            return mode_changed && previous_range.is_some();
        };
        // A caret can stay unchanged while the pointer crosses word boundaries.
        // Keep small pointer jitter a click, then snap the complete range.
        if previous_range.is_none()
            && anchor.position.distance_squared(hit.position) < drag_distance * drag_distance
        {
            return false;
        }
        let range = anchor.word.start.min(hit.word.start)..anchor.word.end.max(hit.word.end);
        if previous_range.as_ref() == Some(&range) {
            return mode_changed;
        }
        endpoints.anchor = range.start;
        endpoints.active = range.end;
        true
    }

    /// Finish an active gesture, returning its caret if it stayed collapsed.
    /// Completed selections and inactive gestures return `None`. The host
    /// decides whether the returned caret activates a link or another action.
    pub(crate) fn finish_interaction(&mut self) -> Option<u32> {
        let TextSelectionPhase::Dragging { endpoints, .. } = &self.phase else {
            return None;
        };
        let endpoints = *endpoints;
        self.phase = TextSelectionPhase::Positioned(endpoints);
        (endpoints.anchor == endpoints.active).then_some(endpoints.anchor)
    }

    /// Replaces the logical range outside a pointer gesture, for example when
    /// search or another semantic command creates a selection.
    pub(crate) fn select_range(&mut self, anchor: u32, active: u32, mode: SelectionMode) {
        self.phase = TextSelectionPhase::Positioned(SelectionEndpoints {
            anchor,
            active,
            mode,
        });
    }
}

/// Selected table cells, retaining merged-cell spans for export and painting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TableCellRange {
    table_box: BoxId,
    row_start: usize,
    row_end: usize,
    column_start: usize,
    column_end: usize,
}

impl TableCellRange {
    /// The owning table's document-local box index.
    pub(crate) fn table_box(self) -> BoxId {
        self.table_box
    }

    pub(crate) fn from_cell(table_box: BoxId, cell: &RenderTableCell) -> Self {
        let rows = cell.row_range();
        let columns = cell.column_range();
        Self {
            table_box,
            row_start: rows.start,
            row_end: rows.end.saturating_sub(1),
            column_start: columns.start,
            column_end: columns.end.saturating_sub(1),
        }
    }

    pub(crate) fn union(self, other: Self) -> Self {
        debug_assert_eq!(self.table_box, other.table_box);
        Self {
            table_box: self.table_box,
            row_start: self.row_start.min(other.row_start),
            row_end: self.row_end.max(other.row_end),
            column_start: self.column_start.min(other.column_start),
            column_end: self.column_end.max(other.column_end),
        }
    }

    pub(super) fn intersects(self, cell: &RenderTableCell) -> bool {
        let rows = cell.row_range();
        let columns = cell.column_range();
        rows.start <= self.row_end
            && rows.end > self.row_start
            && columns.start <= self.column_end
            && columns.end > self.column_start
    }

    /// Selected cells in table order, including cells spanning the selection.
    /// A different table yields no cells.
    pub(crate) fn cells(self, table: &RenderTable) -> impl Iterator<Item = &RenderTableCell> {
        table
            .cells()
            .filter(move |cell| table.box_id() == self.table_box && self.intersects(cell))
    }

    pub(crate) fn markdown(self, table: &RenderTable) -> Option<String> {
        table.export(TableExportRequest::CellsMarkdown {
            rows: self.row_start..self.row_end.saturating_add(1),
            columns: self.column_start..self.column_end.saturating_add(1),
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct TableSelectionState {
    phase: TableSelectionPhase,
}

#[derive(Clone, Default)]
enum TableSelectionPhase {
    #[default]
    Empty,
    Positioned(TableCellRange),
    Dragging {
        range: TableCellRange,
        anchor: TableCellRange,
    },
}

/// A cell hit produced by document geometry, including its merged-cell spans.
#[derive(Clone, Copy, Debug)]
pub struct TableHit {
    range: TableCellRange,
}

impl TableHit {
    pub(crate) fn table_box(self) -> BoxId {
        self.range.table_box
    }

    pub(crate) fn from_cell(table_box: BoxId, cell: &RenderTableCell) -> Self {
        Self {
            range: TableCellRange::from_cell(table_box, cell),
        }
    }
}

impl TableSelectionState {
    pub(crate) fn is_dragging(&self) -> bool {
        matches!(self.phase, TableSelectionPhase::Dragging { .. })
    }

    pub(crate) fn is_active(&self) -> bool {
        !matches!(self.phase, TableSelectionPhase::Empty)
    }

    pub(crate) fn range(&self) -> Option<TableCellRange> {
        match &self.phase {
            TableSelectionPhase::Empty => None,
            TableSelectionPhase::Positioned(range)
            | TableSelectionPhase::Dragging { range, .. } => Some(*range),
        }
    }

    /// Whether the hit belongs to the selected table, including cells outside
    /// the current selection rectangle.
    pub(crate) fn is_in_selected_table(&self, hit: TableHit) -> bool {
        self.range()
            .is_some_and(|range| range.table_box == hit.range.table_box)
    }

    pub(crate) fn begin(&mut self, hit: TableHit) {
        self.phase = TableSelectionPhase::Positioned(hit.range);
    }

    /// Starts a drag from an existing selection. Returns whether `hit` was in
    /// the selected table; a drag may not start in a different one.
    pub(crate) fn begin_drag(&mut self, hit: TableHit) -> bool {
        if !self.is_in_selected_table(hit) {
            return false;
        }
        let range = hit.range;
        self.phase = TableSelectionPhase::Dragging {
            range,
            anchor: range,
        };
        true
    }

    /// Extends an in-progress drag to `hit`. Returns `None` when no drag is in
    /// progress, otherwise whether the selected range actually moved. A hit
    /// outside the table, or no hit at all, holds the range where it is rather
    /// than cancelling the drag.
    pub(crate) fn update_drag(&mut self, hit: Option<TableHit>) -> Option<bool> {
        let TableSelectionPhase::Dragging { range, anchor } = &mut self.phase else {
            return None;
        };
        let Some(hit) = hit else {
            return Some(false);
        };
        if hit.range.table_box != anchor.table_box {
            return Some(false);
        }
        let next = anchor.union(hit.range);
        if *range == next {
            return Some(false);
        }
        *range = next;
        Some(true)
    }

    pub(crate) fn finish_drag(&mut self) -> bool {
        let TableSelectionPhase::Dragging { range, .. } = &self.phase else {
            return false;
        };
        self.phase = TableSelectionPhase::Positioned(*range);
        true
    }

    /// Drops the selection. Returns whether there was one to drop.
    pub(crate) fn clear(&mut self) -> bool {
        if !self.is_active() {
            return false;
        }
        self.phase = TableSelectionPhase::Empty;
        true
    }
}
