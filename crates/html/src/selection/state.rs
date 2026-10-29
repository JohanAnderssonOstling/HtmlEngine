//! Logical selection transitions. Hosts translate their input into character
//! positions and decide how selections are displayed or placed on a clipboard.
use super::Selection;
use crate::layout::{RenderTable, RenderTableCell, RenderView};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionMode {
    #[default]
    Plain,
    Semantic,
}

impl SelectionMode {
    pub fn is_semantic(self) -> bool {
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
pub struct SelectionAnchor {
    range: Range<u32>,
}

impl SelectionAnchor {
    pub fn new(range: Range<u32>) -> Self {
        Self { range }
    }

    pub fn extend_to(&self, word: Range<u32>) -> Range<u32> {
        word.start.min(self.range.start)..word.end.max(self.range.end)
    }
}

#[derive(Clone, Default)]
pub struct SelectionState {
    interaction: SelectionInteraction,
    content: SelectionContent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionInteraction {
    selecting: bool,
    anchor: Option<u32>,
    active: Option<u32>,
    mode: SelectionMode,
}

impl SelectionInteraction {
    pub fn is_selecting(self) -> bool {
        self.selecting
    }

    pub fn anchor(self) -> Option<u32> {
        self.anchor
    }

    pub fn active(self) -> Option<u32> {
        self.active
    }

    pub fn is_semantic(self) -> bool {
        self.mode.is_semantic()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionContent {
    plain: Option<String>,
    markdown: Option<String>,
}

impl SelectionContent {
    pub fn plain(&self) -> Option<&str> {
        self.plain.as_deref()
    }

    pub fn markdown(&self) -> Option<&str> {
        self.markdown.as_deref()
    }
}

impl SelectionState {
    pub fn range(&self) -> Option<Range<u32>> {
        let (Some(anchor), Some(active)) = (self.interaction.anchor, self.interaction.active)
        else {
            return None;
        };
        (anchor != active).then_some(anchor.min(active)..anchor.max(active))
    }

    pub fn selection<'a>(&self, view: RenderView<'a>) -> Option<Selection<'a>> {
        let selection = Selection::new(view, self.range()?);
        Some(if self.interaction.is_semantic() {
            selection.with_semantics()
        } else {
            selection
        })
    }

    pub fn interaction(&self) -> SelectionInteraction {
        self.interaction
    }

    pub fn content(&self) -> &SelectionContent {
        &self.content
    }

    /// Start updating a logical selection from a document character position.
    pub fn begin_at_glyph(&mut self, glyph: u32, mode: SelectionMode) {
        self.interaction.anchor = Some(glyph);
        self.interaction.active = Some(glyph);
        self.interaction.selecting = true;
        self.interaction.mode = mode;
    }

    /// Update an active range. A missing position preserves its endpoint while
    /// still allowing structural expansion to be enabled.
    pub fn update_interaction(&mut self, glyph: Option<u32>, mode: SelectionMode) -> bool {
        if !self.interaction.selecting {
            return false;
        }
        self.interaction.mode = self.interaction.mode.promote(mode);
        let Some(glyph) = glyph else {
            return false;
        };
        self.interaction.active = Some(glyph);
        true
    }

    /// Finish updating the logical range. Input gestures remain a host concern.
    pub fn finish_interaction(&mut self) {
        self.interaction.selecting = false;
    }

    /// Cancels only the in-progress gesture, preserving the selected range and
    /// its derived copy payload.
    pub fn cancel_interaction(&mut self) {
        self.interaction.selecting = false;
    }

    /// Replaces the logical range outside a pointer gesture, for example when
    /// search or another semantic command creates a selection.
    pub fn select_range(&mut self, anchor: u32, active: u32, mode: SelectionMode) {
        self.interaction.anchor = Some(anchor);
        self.interaction.active = Some(active);
        self.interaction.mode = mode;
        self.interaction.selecting = false;
        self.clear_text();
    }

    pub fn install_text(&mut self, plain: Option<String>, markdown: Option<String>) {
        self.content.plain = plain;
        self.content.markdown = markdown;
    }

    /// Refresh copy payloads through the same structural selection used by geometry.
    pub fn refresh_text(&mut self, view: RenderView<'_>) {
        let text = self
            .selection(view)
            .map(|selection| selection.text())
            .unwrap_or_default();
        self.install_text(text.plain, text.markdown);
    }

    pub fn clear_text(&mut self) {
        self.install_text(None, None);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableCellRange {
    pub table_box: usize,
    pub row_start: usize,
    pub row_end: usize,
    pub column_start: usize,
    pub column_end: usize,
}

impl TableCellRange {
    pub(crate) fn from_cell(table_box: usize, cell: &RenderTableCell) -> Self {
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

    pub fn intersects(self, cell: &RenderTableCell) -> bool {
        let rows = cell.row_range();
        let columns = cell.column_range();
        self.row_start <= self.row_end
            && self.column_start <= self.column_end
            && rows.start <= self.row_end
            && rows.end > self.row_start
            && columns.start <= self.column_end
            && columns.end > self.column_start
    }

    pub fn cells(self, table: &RenderTable) -> impl Iterator<Item = &RenderTableCell> {
        table
            .cells()
            .filter(move |cell| table.box_idx() == self.table_box && self.intersects(cell))
    }
}

#[derive(Clone, Copy)]
enum TableAnchorMode {
    Anchored,
    Dragging,
}
#[derive(Default)]
pub struct TableSelectionState {
    range: Option<TableCellRange>,
    anchor: Option<TableCellRange>,
    dragging: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct TableHit {
    pub table_box: usize,
    pub row: usize,
    pub column: usize,
    pub row_end: usize,
    pub column_end: usize,
}

impl TableHit {
    fn range(self) -> TableCellRange {
        TableCellRange {
            table_box: self.table_box,
            row_start: self.row,
            row_end: self.row_end,
            column_start: self.column,
            column_end: self.column_end,
        }
    }

    pub(crate) fn from_cell(table_box: usize, cell: &RenderTableCell) -> Self {
        let range = TableCellRange::from_cell(table_box, cell);
        Self {
            table_box,
            row: range.row_start,
            column: range.column_start,
            row_end: range.row_end,
            column_end: range.column_end,
        }
    }
}

pub fn table_selection_markdown(table: &RenderTable, range: TableCellRange) -> Option<String> {
    table.markdown_in(
        range.row_start..range.row_end.saturating_add(1),
        range.column_start..range.column_end.saturating_add(1),
    )
}

impl TableSelectionState {
    pub fn is_active(&self) -> bool {
        self.range.is_some()
    }

    pub fn range(&self) -> Option<TableCellRange> {
        self.range
    }

    pub fn covers(&self, hit: TableHit) -> bool {
        self.range
            .is_some_and(|range| range.table_box == hit.table_box)
    }

    fn anchor_to(&mut self, hit: TableHit, mode: TableAnchorMode) {
        let range = Some(hit.range());
        self.anchor = range;
        self.range = range;
        self.dragging = matches!(mode, TableAnchorMode::Dragging);
    }

    pub fn begin(&mut self, hit: TableHit) {
        self.anchor_to(hit, TableAnchorMode::Anchored);
    }

    /// Starts a drag from an existing selection. Returns whether `hit` was in
    /// the selected table; a drag may not start in a different one.
    pub fn begin_drag(&mut self, hit: TableHit) -> bool {
        if !self.covers(hit) {
            return false;
        }
        self.anchor_to(hit, TableAnchorMode::Dragging);
        true
    }

    /// Extends an in-progress drag to `hit`. Returns `None` when no drag is in
    /// progress, otherwise whether the selected range actually moved. A hit
    /// outside the table, or no hit at all, holds the range where it is rather
    /// than cancelling the drag.
    pub fn update_drag(&mut self, hit: Option<TableHit>) -> Option<bool> {
        if !self.dragging {
            return None;
        }
        let (Some(hit), Some(anchor)) = (hit, self.anchor) else {
            return Some(false);
        };
        if hit.table_box != anchor.table_box {
            return Some(false);
        }
        let next = anchor.union(hit.range());
        if self.range == Some(next) {
            return Some(false);
        }
        self.range = Some(next);
        Some(true)
    }

    pub fn finish_drag(&mut self) -> bool {
        std::mem::replace(&mut self.dragging, false)
    }

    /// Drops the selection. Returns whether there was one to drop.
    pub fn clear(&mut self) -> bool {
        if self.range.take().is_none() {
            return false;
        }
        self.anchor = None;
        self.dragging = false;
        true
    }
}
