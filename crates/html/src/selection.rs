//! Document selection and copy payloads, independent of viewports and UI state.
//!
//! A selection borrows an engine render view. Applications own gestures, screen
//! projection, highlight colors, and clipboard access.

use crate::layout::{ContentRole, RenderTable, RenderView, merge_text_ranges};
use std::collections::HashSet;
use std::ops::Range;

mod state;
pub use state::{
    SelectionAnchor, SelectionContent, SelectionInteraction, SelectionMode, SelectionState,
    TableCellRange, TableHit, TableSelectionState, table_selection_markdown,
};

/// Plain-text and Markdown representations of the same selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionText {
    pub plain: Option<String>,
    pub markdown: Option<String>,
}

/// A logical, half-open character range in a laid-out document.
///
/// Indices are the engine's logical glyph indices, not bytes or UTF-16 offsets.
/// Empty, reversed, and out-of-bounds ranges produce no text or promoted content.
#[derive(Clone)]
pub struct Selection<'a> {
    view: RenderView<'a>,
    range: Range<u32>,
    semantic: bool,
}

/// Complete geometry interpretation shared by highlighting and hit testing.
/// Text intervals exclude cells represented by `cell_boxes`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionGeometry {
    pub text_ranges: Vec<Range<u32>>,
    pub cell_boxes: Vec<usize>,
}

struct SemanticTableSelection {
    table: RenderTable,
    range: TableCellRange,
    glyph_start: u32,
    glyph_end: u32,
    cell_glyph_ranges: Vec<Range<u32>>,
}

impl SemanticTableSelection {
    fn cell_boxes(&self) -> impl Iterator<Item = usize> + '_ {
        self.range
            .cells(&self.table)
            .map(|cell| cell.box_idx())
    }
}

struct SemanticTextUnit {
    glyph_start: u32,
    glyph_end: u32,
    glyph_ranges: Vec<Range<u32>>,
    box_idx: usize,
}

impl<'a> Selection<'a> {
    pub fn new(view: RenderView<'a>, range: Range<u32>) -> Self {
        Self {
            view,
            range,
            semantic: false,
        }
    }

    /// Include complete touched list items and rectangular table selections.
    /// Code blocks are promoted only when the selection crosses their boundary.
    /// This policy is shared by copy payloads and highlight queries.
    pub fn with_semantics(mut self) -> Self {
        self.semantic = true;
        self
    }

    fn is_valid(&self) -> bool {
        self.range.start < self.range.end
            && self.range.end as usize <= self.view.text().glyph_count()
    }

    /// Build copy payloads on demand; geometry queries do not serialize Markdown.
    /// Plain text retains the requested range; semantic promotion affects Markdown.
    pub fn text(&self) -> SelectionText {
        if !self.is_valid() {
            return SelectionText::default();
        }
        let (plain, markdown) = if self.semantic {
            self.semantic_text(self.range.start, self.range.end)
        } else {
            self.view.export().selection_text(self.range.clone())
        };
        SelectionText { plain, markdown }
    }

    /// Logical ranges belonging to the selected table cells. Hosts can exclude
    /// these from ordinary text highlighting and highlight the cell boxes instead.
    pub fn cell_glyph_ranges(&self) -> Vec<Range<u32>> {
        self.semantic_table_selections(self.range.start, self.range.end)
            .into_iter()
            .flat_map(|selection| selection.cell_glyph_ranges)
            .collect()
    }

    /// Complete list-item and code-block ranges added by semantic selection.
    pub fn promoted_glyph_ranges(&self) -> Vec<Range<u32>> {
        self.semantic_text_units(self.range.start, self.range.end)
            .into_iter()
            .flat_map(|unit| unit.glyph_ranges)
            .collect()
    }

    /// Selected table-cell box IDs, including empty cells inside the rectangle.
    /// The host projects these document boxes into its own coordinate system.
    pub fn table_cell_boxes(&self) -> Vec<usize> {
        let tables = self.semantic_table_selections(self.range.start, self.range.end);
        tables
            .iter()
            .flat_map(SemanticTableSelection::cell_boxes)
            .collect()
    }

    /// Normalize promoted text and selected cells into disjoint paint units.
    pub fn geometry_plan(&self) -> SelectionGeometry {
        if !self.is_valid() {
            return SelectionGeometry::default();
        }
        let mut ranges = vec![self.range.clone()];
        ranges.extend(self.promoted_glyph_ranges());
        merge_text_ranges(&mut ranges);
        let tables = self.semantic_table_selections(self.range.start, self.range.end);
        let cell_boxes = tables
            .iter()
            .flat_map(SemanticTableSelection::cell_boxes)
            .collect();
        let mut cells = tables
            .into_iter()
            .flat_map(|table| table.cell_glyph_ranges)
            .collect();
        merge_text_ranges(&mut cells);
        if cells.is_empty() {
            return SelectionGeometry {
                text_ranges: ranges,
                cell_boxes,
            };
        }
        let mut cells = cells.into_iter().peekable();
        let mut text_ranges = Vec::new();
        for range in ranges {
            let mut start = range.start;
            while let Some(cell) = cells.peek() {
                if cell.end <= start {
                    cells.next();
                    continue;
                }
                if cell.start >= range.end {
                    break;
                }
                if start < cell.start {
                    text_ranges.push(start..cell.start);
                }
                start = start.max(cell.end);
                if start >= range.end {
                    break;
                }
            }
            if start < range.end {
                text_ranges.push(start..range.end);
            }
        }
        SelectionGeometry {
            text_ranges,
            cell_boxes,
        }
    }

    fn semantic_text_units(
        &self,
        selection_start: u32,
        selection_end: u32,
    ) -> Vec<SemanticTextUnit> {
        if !self.semantic || !self.is_valid() {
            return Vec::new();
        }
        let view = self.view;
        let semantics = view.semantics();
        let boxes = view.boxes();
        let mut list_items = HashSet::new();
        let mut code_blocks = HashSet::new();
        for run in view.text().visible_source_runs() {
            let glyphs = run.glyphs();
            if glyphs.start >= selection_end
                || glyphs.end <= selection_start
                || semantics
                    .nearest(run.box_idx(), ContentRole::Table)
                    .is_some()
            {
                continue;
            }
            if let Some(block) = semantics.nearest(run.box_idx(), ContentRole::CodeBlock) {
                code_blocks.insert(block);
            } else if let Some(item) = semantics.nearest(run.box_idx(), ContentRole::ListItem) {
                list_items.insert(item);
            }
        }
        let selected_list_items = list_items.clone();
        list_items.retain(|item| {
            !boxes
                .ancestors(*item)
                .any(|ancestor| selected_list_items.contains(&ancestor))
        });
        code_blocks.retain(|block| {
            !boxes
                .ancestors(*block)
                .any(|ancestor| list_items.contains(&ancestor))
        });

        let mut units = Vec::new();
        for (box_idx, is_list_item) in list_items
            .into_iter()
            .map(|item| (item, true))
            .chain(code_blocks.into_iter().map(|block| (block, false)))
        {
            let ranges = semantics.text_ranges(box_idx, is_list_item);
            if let (Some(start), Some(end)) = (
                ranges.iter().map(|range| range.start).min(),
                ranges.iter().map(|range| range.end).max(),
            ) {
                // Code remains character-selectable until a drag crosses its
                // boundary. List items also highlight their generated marker.
                if !is_list_item && selection_start >= start && selection_end <= end {
                    continue;
                }
                units.push(SemanticTextUnit {
                    glyph_start: start,
                    glyph_end: end,
                    glyph_ranges: ranges,
                    box_idx,
                });
            }
        }
        units.sort_by_key(|unit| unit.glyph_start);
        units
    }

    fn semantic_table_selections(
        &self,
        selection_start: u32,
        selection_end: u32,
    ) -> Vec<SemanticTableSelection> {
        if !self.semantic || !self.is_valid() {
            return Vec::new();
        }
        let view = self.view;
        let boxes = view.boxes();
        let mut selections = Vec::new();
        for table_box in (0..boxes.len()).filter(|&index| boxes.is_table(index)) {
            let Some(table) = boxes.table(table_box) else {
                continue;
            };
            let cells = table.cells();
            let Some(range) = cells
                .clone()
                .filter(|cell| {
                    cell.glyphs().is_some_and(|glyphs| {
                        glyphs.start < selection_end && glyphs.end > selection_start
                    })
                })
                .map(|cell| TableCellRange::from_cell(table_box, cell))
                .reduce(TableCellRange::union)
            else {
                continue;
            };
            let cell_glyph_ranges = cells
                .filter(|cell| range.intersects(cell))
                .filter_map(|cell| cell.glyphs())
                .collect::<Vec<_>>();
            let glyph_start = cell_glyph_ranges
                .iter()
                .map(|range| range.start)
                .min()
                .unwrap_or(selection_start);
            let glyph_end = cell_glyph_ranges
                .iter()
                .map(|range| range.end)
                .max()
                .unwrap_or(selection_end);
            selections.push(SemanticTableSelection {
                table,
                range,
                glyph_start,
                glyph_end,
                cell_glyph_ranges,
            });
        }
        selections.sort_by_key(|selection| selection.glyph_start);
        selections
    }

    fn semantic_text(
        &self,
        selection_start: u32,
        selection_end: u32,
    ) -> (Option<String>, Option<String>) {
        let export = self.view.export();
        // Discovery is shared with highlighting; serialization happens only
        // when copying. Failed promotion leaves the ordinary selection intact.
        let mut units = self
            .semantic_text_units(selection_start, selection_end)
            .into_iter()
            .filter_map(|unit| {
                export
                    .markdown_for_box(unit.box_idx)
                    .map(|markdown| (unit.glyph_start, unit.glyph_end, markdown))
            })
            .collect::<Vec<_>>();
        units.extend(
            self.semantic_table_selections(selection_start, selection_end)
                .into_iter()
                .filter_map(|selection| {
                    table_selection_markdown(&selection.table, selection.range)
                        .map(|markdown| (selection.glyph_start, selection.glyph_end, markdown))
                }),
        );
        units.sort_by_key(|unit| unit.0);
        if units.is_empty() {
            return export.selection_text(selection_start..selection_end);
        }
        let plain = export.plain_text(selection_start..selection_end);
        let mut markdown = String::new();
        let mut cursor = selection_start;
        let append = |target: &mut String, value: &str| {
            if value.is_empty() {
                return;
            }
            if !target.is_empty() && !target.ends_with('\n') {
                target.push('\n');
            }
            target.push_str(value);
        };
        for (start, end, value) in units {
            let ordinary_end = start.min(selection_end);
            if cursor < ordinary_end
                && let Some(value) = export.markdown(cursor..ordinary_end)
            {
                append(&mut markdown, &value);
            }
            append(&mut markdown, &value);
            cursor = cursor.max(end);
        }
        if cursor < selection_end
            && let Some(value) = export.markdown(cursor..selection_end)
        {
            append(&mut markdown, &value);
        }
        (plain, (!markdown.is_empty()).then_some(markdown))
    }
}
