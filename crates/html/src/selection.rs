//! Document selection and copy payloads, independent of viewports and UI state.
//!
//! A selection borrows an engine render view. Applications own gestures, screen
//! projection, highlight colors, and clipboard access.
use html_pipeline::engine::text_paint;
use html_engine_model::layout_state;
use html_pipeline::engine::export_queries;
use html_pipeline::engine::semantic_queries;
use crate::export::{TextExport, TextExportFormat};
use html_pipeline::engine::box_queries;

use crate::layout::BoxId;
use crate::layout::ContentRole;
use html_pipeline::engine::RenderTable;
use crate::layout::RenderView;
use crate::layout::merge_text_ranges;
use std::collections::HashSet;
use std::iter::once;
use std::ops::Range;

mod state;
mod controller;
pub use controller::{
    SelectionController, SelectionExport, SelectionGeometry, SelectionHit, SelectionInput,
    SelectionKind, SelectionLayout, SelectionOutcome, SelectionPaintScratch, SelectionUnit,
};
pub use state::{SelectionMode, TableHit};
pub(crate) use state::{SelectionAnchor, SelectionState, TableCellRange, TableSelectionState};

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
/// The engine partitions selected text and cells into disjoint paint units.
#[derive(Default)]
pub(crate) struct SelectionPlan {
    pub(crate) text_ranges: Vec<Range<u32>>,
    pub(crate) cell_boxes: Vec<BoxId>,
}

struct SemanticTableSelection {
    table: RenderTable,
    range: TableCellRange,
    glyph_start: u32,
    glyph_end: u32,
    cell_glyph_ranges: Vec<Range<u32>>,
}

impl SemanticTableSelection {
    fn cell_boxes(&self) -> impl Iterator<Item = BoxId> + '_ {
        self.range.cells(&self.table).map(|cell| cell.box_id())
    }
}

struct SemanticTextUnit {
    glyph_start: u32,
    glyph_end: u32,
    glyph_ranges: Vec<Range<u32>>,
    box_idx: BoxId,
}

impl<'a> Selection<'a> {
    /// Select the nearest authored block, flex or grid container at this character.
    /// Inline descendants stay part of their paragraph, heading, item or cell.
    /// The range covers the whole element, independent of viewport clipping.
    pub(crate) fn containing_element(view: RenderView<'a>, glyph: u32) -> Option<Self> {
        let run = text_paint::render_text_view_visible_source_runs(view.text())
            .find(|run| run.glyphs().contains(&glyph))?;
        let boxes = view.boxes();
        let element = once(text_paint::render_text_run_box_id(&run))
            .chain(box_queries::ancestors(
                boxes,
                text_paint::render_text_run_box_id(&run),
            ))
            .find(|&index| {
                box_queries::tag(boxes, index).is_some()
                    && (box_queries::render_box_view_is_block_container(boxes, index)
                        || box_queries::is_flex_or_grid_container(boxes, index))
            })?;
        let ranges = semantic_queries::render_semantics_view_text_ranges(
            semantic_queries::render_view_semantics(view),
            element,
            false,
        );
        let start = ranges.iter().map(|range| range.start).min()?;
        let end = ranges.iter().map(|range| range.end).max()?;
        (start < end).then(|| Self::new(view, start..end))
    }

    pub fn range(&self) -> Range<u32> {
        self.range.clone()
    }

    pub(crate) fn new(view: RenderView<'a>, range: Range<u32>) -> Self {
        Self {
            view,
            range,
            semantic: false,
        }
    }

    /// Include complete touched list items and rectangular table selections.
    /// Code blocks are promoted only when the selection crosses their boundary.
    /// This policy is shared by copy payloads and highlight queries.
    pub(crate) fn with_semantics(mut self) -> Self {
        self.semantic = true;
        self
    }

    fn is_valid(&self) -> bool {
        self.range.start < self.range.end
            && self.range.end as usize <= self.view.text().glyph_count()
    }

    /// Build copy payloads on demand; geometry queries do not serialize Markdown.
    /// Plain text retains the requested range; semantic promotion affects Markdown.
    pub(crate) fn text(&self) -> TextExport {
        if !self.is_valid() {
            return TextExport::default();
        }
        if self.semantic {
            TextExport {
                plain: self.plain_text(),
                markdown: self.semantic_markdown(self.range.start, self.range.end),
            }
        } else {
            export_queries::render_export_view_text(self.view.export(), self.range.clone(), TextExportFormat::PlainAndMarkdown)
        }
    }

    /// Read the selected source text only when an action needs it.
    /// Semantic promotion affects Markdown, not the selected passage.
    pub fn plain_text(&self) -> Option<String> {
        if !self.is_valid() {
            return None;
        }
        export_queries::render_export_view_text(self.view.export(), self.range.clone(), TextExportFormat::Plain).plain
    }

    /// Prepare Markdown only when a copy or export action requests it.
    pub fn markdown(&self) -> Option<String> {
        if !self.is_valid() {
            return None;
        }
        if self.semantic {
            self.semantic_markdown(self.range.start, self.range.end)
        } else {
            export_queries::render_export_view_text(self.view.export(), self.range.clone(), TextExportFormat::Markdown).markdown
        }
    }

    /// Complete list-item and code-block ranges added by semantic selection.
    fn promoted_glyph_ranges(&self) -> Vec<Range<u32>> {
        self.semantic_text_units(self.range.start, self.range.end)
            .into_iter()
            .flat_map(|unit| unit.glyph_ranges)
            .collect()
    }

    /// Normalize promoted text and selected cells into disjoint paint units.
    pub(crate) fn geometry_plan(&self) -> SelectionPlan {
        if !self.is_valid() {
            return SelectionPlan::default();
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
            return SelectionPlan {
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
        SelectionPlan {
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
        let semantics = semantic_queries::render_view_semantics(view);
        let boxes = view.boxes();
        let mut list_items = HashSet::new();
        let mut code_blocks = HashSet::new();
        for run in text_paint::render_text_view_visible_source_runs(view.text()) {
            let glyphs = run.glyphs();
            if glyphs.start >= selection_end
                || glyphs.end <= selection_start
                || semantic_queries::render_semantics_view_nearest(
                    semantics,
                    text_paint::render_text_run_box_id(&run),
                    ContentRole::Table,
                )
                .is_some()
            {
                continue;
            }
            if let Some(block) = semantic_queries::render_semantics_view_nearest(
                semantics,
                text_paint::render_text_run_box_id(&run),
                ContentRole::CodeBlock,
            ) {
                code_blocks.insert(block);
            } else if let Some(item) = semantic_queries::render_semantics_view_nearest(
                semantics,
                text_paint::render_text_run_box_id(&run),
                ContentRole::ListItem,
            ) {
                list_items.insert(item);
            }
        }
        let selected_list_items = list_items.clone();
        list_items.retain(|item| {
            !box_queries::ancestors(boxes, *item)
                .any(|ancestor| selected_list_items.contains(&ancestor))
        });
        code_blocks.retain(|block| {
            !box_queries::ancestors(boxes, *block).any(|ancestor| list_items.contains(&ancestor))
        });

        let mut units = Vec::new();
        for (box_idx, is_list_item) in list_items
            .into_iter()
            .map(|item| (item, true))
            .chain(code_blocks.into_iter().map(|block| (block, false)))
        {
            let ranges = semantic_queries::render_semantics_view_text_ranges(
                semantics,
                box_idx,
                is_list_item,
            );
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
        for table_box in (0..box_queries::len(boxes))
            .map(layout_state::box_id_new)
            .filter(|&index| box_queries::is_table(boxes, index))
        {
            let Some(table) = box_queries::render_box_view_table(boxes, table_box) else {
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

    fn semantic_markdown(
        &self,
        selection_start: u32,
        selection_end: u32,
    ) -> Option<String> {
        let export = self.view.export();
        // Discovery is shared with highlighting; serialization happens only
        // when copying. Failed promotion leaves the ordinary selection intact.
        let mut units = self
            .semantic_text_units(selection_start, selection_end)
            .into_iter()
            .filter_map(|unit| {
                export_queries::render_export_view_markdown_for_box(export, unit.box_idx)
                    .map(|markdown| (unit.glyph_start, unit.glyph_end, markdown))
            })
            .collect::<Vec<_>>();
        units.extend(
            self.semantic_table_selections(selection_start, selection_end)
                .into_iter()
                .filter_map(|selection| {
                    selection
                        .range
                        .markdown(&selection.table)
                        .map(|markdown| (selection.glyph_start, selection.glyph_end, markdown))
                }),
        );
        units.sort_by_key(|unit| unit.0);
        if units.is_empty() {
            return export_queries::render_export_view_text(export, selection_start..selection_end, TextExportFormat::Markdown).markdown;
        }
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
                && let Some(value) = export_queries::render_export_view_text(export, cursor..ordinary_end, TextExportFormat::Markdown).markdown
            {
                append(&mut markdown, &value);
            }
            append(&mut markdown, &value);
            cursor = cursor.max(end);
        }
        if cursor < selection_end
            && let Some(value) = export_queries::render_export_view_text(export, cursor..selection_end, TextExportFormat::Markdown).markdown
        {
            append(&mut markdown, &value);
        }
        (!markdown.is_empty()).then_some(markdown)
    }
}
