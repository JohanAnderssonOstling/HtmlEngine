//! Document selection and copy payloads, independent of viewports and UI state.
//!
//! A selection borrows an engine render view. Applications own gestures, screen
//! projection, highlight colors, and clipboard access.
use html_engine_model::layout_state;
use crate::export::{TextExport, TextExportFormat};

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
    SelectionKind, SelectionLayout, SelectionOutcome, SelectionPaintScratch, SelectionPaintStyle,
    SelectionUnit};
pub use state::{SelectionMode, TableHit};
pub(crate) use state::{SelectionAnchor, SelectionState, TableCellRange, TableSelectionState};

/// A pointer gesture stays in normal flow or in the float where it began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionFlow {
    Body,
    Float(BoxId),
}

impl SelectionFlow {
    pub(crate) fn at(view: RenderView<'_>, glyph: u32) -> Option<Self> {
        let owner = view.text().visible_source_runs()
            .find(|run| run.glyphs().contains(&glyph))?.box_id();
        let boxes = view.boxes();
        // An outer float owns its nested content throughout the gesture.
        Some(once(owner).chain(boxes.ancestors(owner))
            .filter(|&index| boxes.is_floating(index))
            .last().map_or(Self::Body, Self::Float))
    }

    pub(crate) fn contains(self, view: RenderView<'_>, owner: BoxId) -> bool {
        let boxes = view.boxes();
        let mut ancestors = once(owner).chain(boxes.ancestors(owner));
        match self {
            Self::Body => !ancestors.any(|index| boxes.is_floating(index)),
            Self::Float(float) => ancestors.any(|index| index == float),
        }
    }
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
    flow: Option<SelectionFlow>,
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
        let run = (view.text()).visible_source_runs()
            .find(|run| run.glyphs().contains(&glyph))?;
        let boxes = view.boxes();
        let element = once(run.box_id())
            .chain(boxes.ancestors(run.box_id()))
            .find(|&index| {
                boxes.tag(index).is_some()
                    && (boxes.is_block_container(index)
                        || boxes.is_flex_or_grid_container(index))
            })?;
        let ranges = (view.semantics()).text_ranges(element, false);
        let start = ranges.iter().map(|range| range.start).min()?;
        let end = ranges.iter().map(|range| range.end).max()?;
        (start < end).then(|| Self::new(view, start..end))
    }

    pub fn range(&self) -> Range<u32> {
        self.range.clone()
    }

    /// Source language at the beginning of the selected passage.
    pub fn language(&self) -> Option<&'a str> {
        self.view.addressing().language_at(self.range.start)
    }

    pub(crate) fn new(view: RenderView<'a>, range: Range<u32>) -> Self {
        Self {
            view,
            range,
            semantic: false,
            flow: None,
        }
    }

    pub(crate) fn with_flow(mut self, flow: Option<SelectionFlow>) -> Self {
        self.flow = flow;
        self
    }

    /// Keep the logical interval, excluding source text in other flows.
    pub(crate) fn selected_ranges(&self) -> Vec<Range<u32>> {
        self.restrict_to_flow(vec![self.range.clone()])
    }

    fn restrict_to_flow(&self, ranges: Vec<Range<u32>>) -> Vec<Range<u32>> {
        let Some(flow) = self.flow else { return ranges };
        let Some(first) = ranges.first() else { return ranges };
        let start = first.start;
        let end = ranges.last().map_or(start, |range| range.end);
        let mut excluded: Vec<_> = self.view.text().visible_source_runs()
            .filter(|run| run.glyphs().start < end && run.glyphs().end > start)
            .filter(|run| !flow.contains(self.view, run.box_id()))
            .map(|run| run.glyphs()).collect();
        merge_text_ranges(&mut excluded);
        let mut result = Vec::new();
        let mut index = 0;
        for range in ranges {
            let mut start = range.start;
            while index < excluded.len() && excluded[index].end <= start { index += 1; }
            let mut cursor = index;
            while let Some(skip) = excluded.get(cursor) {
                if skip.start >= range.end { break; }
                if skip.start > start { result.push(start..skip.start.min(range.end)); }
                start = start.max(skip.end);
                if start >= range.end { break; }
                cursor += 1;
            }
            if start < range.end { result.push(start..range.end); }
        }
        result
    }

    /// Semantic promotion must not reintroduce another flow through a parent.
    fn whole_box_in_flow(&self, owner: BoxId) -> bool {
        self.flow.is_none_or(|flow| {
            flow.contains(self.view, owner)
                && self.view.text().visible_source_runs().all(|run| {
                    !self.view.semantics().contains(owner, run.box_id())
                        || flow.contains(self.view, run.box_id())
                })
        })
    }

    fn export_range(&self, range: Range<u32>, format: TextExportFormat) -> TextExport {
        let Some(flow) = self.flow else { return self.view.export().text(range, format) };
        self.view.export().text_with_box_filter(range, format, |owner| {
            flow.contains(self.view, owner)
                || matches!(flow, SelectionFlow::Float(float)
                    if self.view.semantics().contains(owner, float))
        })
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
        if self.flow.is_some() {
            return TextExport { plain: self.plain_text(), markdown: self.markdown() };
        }
        if self.semantic {
            TextExport {
                plain: self.plain_text(),
                markdown: self.semantic_markdown(self.range.start, self.range.end),
            }
        } else {
            (self.view.export()).text(self.range.clone(), TextExportFormat::PlainAndMarkdown)
        }
    }

    /// Read the selected source text only when an action needs it.
    /// Semantic promotion affects Markdown, not the selected passage.
    pub fn plain_text(&self) -> Option<String> {
        self.plain_text_with_format(TextExportFormat::Plain)
    }

    pub(crate) fn plain_text_with_format(&self, format: TextExportFormat) -> Option<String> {
        if !self.is_valid() {
            return None;
        }
        let parts: Vec<_> = self.selected_ranges().into_iter()
            .filter_map(|range| self.export_range(range, format).plain).collect();
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    /// Prepare Markdown only when a copy or export action requests it.
    pub fn markdown(&self) -> Option<String> {
        if !self.is_valid() {
            return None;
        }
        if self.flow.is_some() {
            let parts: Vec<_> = self.selected_ranges().into_iter().filter_map(|range| {
                if self.semantic {
                    self.semantic_markdown(range.start, range.end)
                } else {
                    self.export_range(range, TextExportFormat::Markdown).markdown
                }
            }).collect();
            return (!parts.is_empty()).then(|| parts.join("\n\n"));
        }
        if self.semantic {
            self.semantic_markdown(self.range.start, self.range.end)
        } else {
            (self.view.export()).text(self.range.clone(), TextExportFormat::Markdown).markdown
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
        let ranges = self.restrict_to_flow(ranges);
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
        let semantics = view.semantics();
        let boxes = view.boxes();
        let mut list_items = HashSet::new();
        let mut code_blocks = HashSet::new();
        for run in (view.text()).visible_source_runs() {
            let glyphs = run.glyphs();
            if glyphs.start >= selection_end
                || glyphs.end <= selection_start
                || semantics.nearest(run.box_id(), ContentRole::Table)
                .is_some()
            {
                continue;
            }
            if let Some(block) = semantics.nearest(run.box_id(), ContentRole::CodeBlock) {
                code_blocks.insert(block);
            } else if let Some(item) = semantics.nearest(run.box_id(), ContentRole::ListItem) {
                list_items.insert(item);
            }
        }
        let selected_list_items = list_items.clone();
        list_items.retain(|item| {
            !boxes.ancestors(*item)
                .any(|ancestor| selected_list_items.contains(&ancestor))
        });
        code_blocks.retain(|block| {
            !boxes.ancestors(*block).any(|ancestor| list_items.contains(&ancestor))
        });

        let mut units = Vec::new();
        for (box_idx, is_list_item) in list_items
            .into_iter()
            .map(|item| (item, true))
            .chain(code_blocks.into_iter().map(|block| (block, false)))
        {
            if !self.whole_box_in_flow(box_idx) { continue; }
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
        for table_box in (0..boxes.len())
            .map(layout_state::box_id_new)
            .filter(|&index| boxes.is_table(index))
        {
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
            if !self.whole_box_in_flow(table_box) { continue; }
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
                export.markdown_for_box(unit.box_idx)
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
            return self.export_range(selection_start..selection_end, TextExportFormat::Markdown).markdown;
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
                && let Some(value) = self.export_range(cursor..ordinary_end, TextExportFormat::Markdown).markdown
            {
                append(&mut markdown, &value);
            }
            append(&mut markdown, &value);
            cursor = cursor.max(end);
        }
        if cursor < selection_end
            && let Some(value) = self.export_range(cursor..selection_end, TextExportFormat::Markdown).markdown
        {
            append(&mut markdown, &value);
        }
        (!markdown.is_empty()).then_some(markdown)
    }
}
