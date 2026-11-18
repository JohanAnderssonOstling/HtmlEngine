//! Renderer-facing box geometry, semantics, and table queries.

use super::text::enclosing_text_range;
use super::*;
use crate::layout_model::parse_positive_span;
use html_dom::{DomNodeId, ElementRef, write_escaped_html_attribute, write_escaped_html_text};
use html_engine_model::layout_state::{self, BoxPaginationFacts, PaginationOwnership};
use html_layout::place_table_cells;
use html_style_model::{BreakBetween, BreakInside, Float, LayoutStyle};
use kurbo::Rect;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RenderForcedBreak {
    Column,
    Page,
}

#[derive(Clone, Copy, Debug)]
pub struct RenderListItemMarker {
    marker_box: u32,
    position: ListStylePosition,
}

impl RenderListItemMarker {
    pub(in crate::stages) fn from_model(marker: ListItemMarker) -> Self {
        Self {
            marker_box: marker.marker_box,
            position: marker.position,
        }
    }

    pub fn marker_box(self) -> BoxId {
        layout_state::box_id_new(self.marker_box as usize)
    }

    pub fn position(self) -> ListStylePosition {
        self.position
    }
}

/// Used box geometry and semantic box queries.
#[derive(Clone, Copy)]
pub struct RenderBoxView<'a> {
    pub(super) doc: DocumentRead<'a>,
    pub(super) glyph_metrics: &'a crate::layout_model::GlyphMetrics,
}

/// Semantic and geometric data for one laid-out HTML table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderTable {
    box_idx: usize,
    rows: Vec<RenderTableRow>,
    column_count: usize,
    authored_html: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderTableRow {
    cells: Vec<RenderTableCell>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderTableCell {
    box_idx: usize,
    row: usize,
    column: usize,
    rowspan: usize,
    colspan: usize,
    header: bool,
    text: String,
    glyphs: Option<Range<u32>>,
}

impl RenderTable {
    pub fn box_id(&self) -> BoxId {
        layout_state::box_id_new(self.box_idx)
    }

    pub fn rows(&self) -> &[RenderTableRow] {
        &self.rows
    }

    /// Cells in source row order, visiting each spanning cell once.
    pub fn cells(&self) -> impl Iterator<Item = &RenderTableCell> + Clone {
        self.rows.iter().flat_map(|row| row.cells())
    }

    /// Sanitized authored markup for rich table export. This preserves
    /// classes, inline styles, and structural attributes while dropping event
    /// handlers; consumers that want portable structure without styling can
    /// generate it from `rows()` instead.
    pub fn authored_html(&self) -> &str {
        &self.authored_html
    }

    pub fn column_count(&self) -> usize {
        self.column_count
    }
}

impl RenderTableRow {
    pub fn cells(&self) -> &[RenderTableCell] {
        &self.cells
    }
}

impl RenderTableCell {
    pub fn box_id(&self) -> BoxId {
        layout_state::box_id_new(self.box_idx)
    }

    pub fn row(&self) -> usize {
        self.row
    }

    pub fn column(&self) -> usize {
        self.column
    }

    pub fn rowspan(&self) -> usize {
        self.rowspan
    }

    pub fn colspan(&self) -> usize {
        self.colspan
    }

    /// Half-open logical row coordinates covered by this cell.
    pub fn row_range(&self) -> Range<usize> {
        self.row..self.row.saturating_add(self.rowspan)
    }

    /// Half-open logical column coordinates covered by this cell.
    pub fn column_range(&self) -> Range<usize> {
        self.column..self.column.saturating_add(self.colspan)
    }

    pub fn is_header(&self) -> bool {
        self.header
    }

    /// Logical source interval owned by this cell, excluding nested tables.
    pub fn glyphs(&self) -> Option<Range<u32>> {
        self.glyphs.clone()
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Resolved text formatting for one layout box, borrowed from its document.
#[derive(Clone, Copy)]
pub struct BoxTextFormat<'a> {
    pub font_size: f32,
    pub font_weight: u16,
    pub font_style: html_style_model::FontStyle,
    pub color: u32,
    pub font_family: Option<&'a str>,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub text_decoration: TextDecorationLines,
}

impl<'a> RenderBoxView<'a> {
    /// Published structure belongs to this layout revision, not to a viewport.
    pub fn pagination_facts(self, owner: BoxId) -> Option<&'a BoxPaginationFacts> {
        self.doc.layout_state.line_output.pagination_ownership.boxes.get(layout_state::box_id_index(owner))
    }

    pub fn pagination_ownership(self) -> &'a PaginationOwnership {
        &self.doc.layout_state.line_output.pagination_ownership
    }

    /// Border-box geometry resolved together from the published layout.
    pub fn bounds(self, box_id: BoxId) -> Option<Rect> {
        Some(Rect::from_origin_size(
            self.point(box_id)?,
            self.size(box_id)?,
        ))
    }

    /// Whether this box participates in float layout rather than normal flow.
    pub fn is_floating(self, box_id: BoxId) -> bool {
        self.doc
            .inputs
            .layout_tree
            .box_at(layout_state::box_id_index(box_id))
            .is_some_and(|layout_box| {
                !layout_box.facts.absolute && layout_box.facts.float != Float::None
            })
    }

    fn break_before(self, box_id: BoxId) -> BreakBetween {
        let box_idx = layout_state::box_id_index(box_id);
        self.layout_style(box_idx)
            .map_or(BreakBetween::Auto, |style| style.break_before)
    }

    fn break_after(self, box_idx: usize) -> BreakBetween {
        self.layout_style(box_idx)
            .map_or(BreakBetween::Auto, |style| style.break_after)
    }

    /// Break policies are stored values; they need no font-relative resolution.
    fn layout_style(self, box_idx: usize) -> Option<&'a LayoutStyle> {
        let indices = self.doc.box_style_indices(box_idx)
            .unwrap_or_else(|| self.doc.inputs.styles.default_indices());
        self.doc.inputs.styles.layout_style(indices)
    }

    fn used_style(self, box_idx: usize) -> Option<html_style_model::UsedStyleView<'a>> {
        self.doc.box_used_style(box_idx)
    }

    /// Bounds in stable line storage for content carried by this box and its
    /// descendants, including image carriers and split inline spans.
    pub fn content_line_range(self, box_id: BoxId) -> Range<LineId> {
        let box_idx = layout_state::box_id_index(box_id);
        let empty = LineId::new(0);
        self.doc
            .layout_state
            .line_output
            .box_line_ranges
            .get(box_idx)
            .map_or(empty..empty, |range| {
                LineId::new(range.start as usize)..LineId::new(range.end as usize)
            })
    }

    pub fn len(self) -> usize {
        self.doc.box_count()
    }

    pub fn tag(self, box_id: BoxId) -> Option<&'a str> {
        let box_idx = layout_state::box_id_index(box_id);
        self.doc
            .element_for_box(box_idx)
            .map(|element| element.tag())
    }

    pub fn attribute(self, box_id: BoxId, name: &str) -> Option<&'a str> {
        let box_idx = layout_state::box_id_index(box_id);
        self.doc
            .element_for_box(box_idx)
            .and_then(|element| element.attr(name))
    }

    /// Source identity in this document's canonical DOM.
    pub fn dom_node(self, box_id: BoxId) -> Option<DomNodeId> {
        self.doc
            .element_for_box(layout_state::box_id_index(box_id))
            .map(|element| element.node_id())
    }

    pub fn is_table(self, box_id: BoxId) -> bool {
        let box_idx = layout_state::box_id_index(box_id);
        matches!(
            self.doc.box_layout_mode(box_idx),
            Some(LayoutMode::Table(_))
        )
    }

    pub fn is_table_row(self, box_id: BoxId) -> bool {
        let box_idx = layout_state::box_id_index(box_id);
        matches!(
            self.doc.box_layout_mode(box_idx),
            Some(LayoutMode::TableRow(_))
        )
    }

    /// A single non-spanning cell can continue in normal flow without
    /// coordinating a common cut with neighbouring cells.
    pub fn table_row_allows_content_break(self, box_id: BoxId) -> bool {
        let row_index = layout_state::box_id_index(box_id);
        let Some(LayoutMode::TableRow(row)) = self.doc.box_layout_mode(row_index) else {
            return false;
        };
        let [cell] = row.cells.as_slice() else { return false };
        let cell_id = layout_state::box_id_new(*cell as usize);
        self.table_cell_rowspan(cell_id) == 1
            && !self.avoids_break_inside(box_id)
            && !self.avoids_break_inside(cell_id)
    }

    pub fn is_table_cell(self, box_id: BoxId) -> bool {
        let box_idx = layout_state::box_id_index(box_id);
        matches!(
            self.doc.box_layout_mode(box_idx),
            Some(LayoutMode::TableCell(_))
        )
    }

    pub fn table_cell_rowspan(self, box_id: BoxId) -> usize {
        let box_idx = layout_state::box_id_index(box_id);
        match self.doc.box_layout_mode(box_idx) {
            Some(LayoutMode::TableCell(cell)) => cell.rowspan.max(1),
            _ => 1,
        }
    }

    pub fn forces_break_before(self, box_id: BoxId) -> bool {
        self.break_before(box_id).is_forced()
    }

    pub fn forces_break_after(self, box_id: BoxId) -> bool {
        let box_idx = layout_state::box_id_index(box_id);
        self.break_after(box_idx).is_forced()
    }

    pub fn avoids_break_before(self, box_id: BoxId) -> bool {
        self.break_before(box_id) == BreakBetween::Avoid
    }

    pub fn avoids_break_after(self, box_id: BoxId) -> bool {
        let box_idx = layout_state::box_id_index(box_id);
        self.break_after(box_idx) == BreakBetween::Avoid
    }

    pub fn forced_break_before(self, box_id: BoxId) -> Option<RenderForcedBreak> {
        render_forced_break(self.break_before(box_id))
    }

    pub fn forced_break_after(self, box_id: BoxId) -> Option<RenderForcedBreak> {
        let box_idx = layout_state::box_id_index(box_id);
        render_forced_break(self.break_after(box_idx))
    }

    pub fn avoids_break_inside(self, box_id: BoxId) -> bool {
        let box_idx = layout_state::box_id_index(box_id);
        self.layout_style(box_idx)
            .is_some_and(|style| style.break_inside == BreakInside::Avoid)
    }

    pub fn widows(self, box_id: BoxId) -> usize {
        let box_idx = layout_state::box_id_index(box_id);
        self.used_style(box_idx)
            .map_or(2, |style| style.widows() as usize)
    }

    pub fn orphans(self, box_id: BoxId) -> usize {
        let box_idx = layout_state::box_id_index(box_id);
        self.used_style(box_idx)
            .map_or(2, |style| style.orphans() as usize)
    }

    pub fn parent(self, box_id: BoxId) -> Option<BoxId> {
        let box_idx = layout_state::box_id_index(box_id);
        self.doc
            .inputs
            .layout_tree
            .get_box_parent(box_idx)
            .map(layout_state::box_id_new)
    }

    pub fn text_format(self, box_id: BoxId) -> BoxTextFormat<'a> {
        let box_idx = layout_state::box_id_index(box_id);
        self.doc.box_text_format(box_idx)
    }

    pub fn list_marker(self, box_id: BoxId) -> Option<RenderListItemMarker> {
        let box_idx = layout_state::box_id_index(box_id);
        self.doc.list_marker(box_idx)
    }

    /// Flex and grid containers, including their inline display variants.
    pub fn is_flex_or_grid_container(self, box_id: BoxId) -> bool {
        let box_idx = layout_state::box_id_index(box_id);
        matches!(
            self.doc.inputs.layout_tree.get_box_layout_mode(box_idx),
            Some(LayoutMode::Flex(_) | LayoutMode::Grid(_))
        )
    }

    pub fn ancestors(self, box_id: BoxId) -> impl Iterator<Item = BoxId> + 'a {
        let box_idx = layout_state::box_id_index(box_id);
        self.doc
            .inputs
            .layout_tree
            .ancestors(box_idx)
            .map(layout_state::box_id_new)
    }

    /// Returns the source table's logical grid, retaining the laid-out box for
    /// every origin cell so renderers can hit-test and paint selections.
    pub fn table(self, box_id: BoxId) -> Option<RenderTable> {
        let box_idx = layout_state::box_id_index(box_id);
        if !self.is_table(box_id) {
            return None;
        }
        let document = self.doc.document();
        let table_element = self.doc.element_for_box(box_idx)?;
        if !table_element.tag().eq_ignore_ascii_case("table") {
            return None;
        }

        fn collect_rows<'dom>(element: ElementRef<'dom>, rows: &mut Vec<ElementRef<'dom>>) {
            for child in element.child_elements() {
                if child.tag().eq_ignore_ascii_case("table") {
                    continue;
                }
                if child.tag().eq_ignore_ascii_case("tr") {
                    rows.push(child);
                } else {
                    collect_rows(child, rows);
                }
            }
        }

        fn serialize_authored_html(document: &Document, node: DomNodeId, output: &mut String) {
            match document.node_ref(node) {
                NodeRef::Text(text) => write_escaped_html_text(output, text.text()),
                NodeRef::Element(element) => {
                    let tag = element.tag();
                    output.push('<');
                    output.push_str(tag);
                    for attribute in element.attributes() {
                        let name = attribute.name();
                        // Event handlers are dropped: exported markup is data,
                        // not behaviour.
                        if name
                            .get(..2)
                            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("on"))
                        {
                            continue;
                        }
                        output.push(' ');
                        output.push_str(name);
                        output.push_str("=\"");
                        write_escaped_html_attribute(output, attribute.value());
                        output.push('"');
                    }
                    output.push('>');
                    // HTML void elements have neither children nor closing tags.
                    // In particular, </br> can be parsed as another line break.
                    if matches!(tag, "area" | "base" | "br" | "col" | "embed" | "hr" | "img" | "input" | "link" | "meta" | "param" | "source" | "track" | "wbr") {
                        return;
                    }
                    for child in element.children() {
                        serialize_authored_html(document, child, output);
                    }
                    output.push_str("</");
                    output.push_str(tag);
                    output.push('>');
                }
            }
        }

        fn collect_text(document: &Document, node: DomNodeId, text: &mut String) {
            match document.node_ref(node) {
                NodeRef::Text(node_text) => {
                    for word in node_text.text().split_whitespace() {
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(word);
                    }
                }
                NodeRef::Element(element) => {
                    if element.tag().eq_ignore_ascii_case("table") {
                        return;
                    }
                    for child in element.children() {
                        collect_text(document, child, text);
                    }
                }
            }
        }

        let mut box_for_node = FxHashMap::default();
        for candidate in 0..self.len() {
            if let Some(node) = self.dom_node(layout_state::box_id_new(candidate)) {
                box_for_node.entry(node).or_insert(candidate);
            }
        }

        let mut row_elements = Vec::new();
        collect_rows(table_element, &mut row_elements);
        let mut rows = vec![RenderTableRow { cells: Vec::new() }; row_elements.len()];
        let source_rows = row_elements.into_iter().map(|row| {
            row.child_elements()
                .filter(|cell| {
                    cell.tag().eq_ignore_ascii_case("th") || cell.tag().eq_ignore_ascii_case("td")
                })
                .map(|cell| {
                    let rowspan = box_for_node
                        .get(&cell.node_id())
                        .and_then(|&box_idx| match self.doc.box_layout_mode(box_idx) {
                            Some(LayoutMode::TableCell(cell)) => Some(cell.rowspan),
                            _ => None,
                        })
                        .unwrap_or_else(|| parse_positive_span(cell.attr("rowspan")));
                    (
                        cell.node_id(),
                        parse_positive_span(cell.attr("colspan")),
                        rowspan,
                    )
                })
        });
        let column_count = place_table_cells(source_rows, |placement| {
            let Some(&cell_box) = box_for_node.get(&placement.cell_idx) else {
                return;
            };
            let cell_element = document.element_ref(placement.cell_idx);
            let mut text = String::new();
            for child in cell_element.children() {
                collect_text(document, child, &mut text);
            }
            rows[placement.row].cells.push(RenderTableCell {
                box_idx: cell_box,
                row: placement.row,
                column: placement.col,
                rowspan: placement.rowspan,
                colspan: placement.colspan,
                header: cell_element.tag().eq_ignore_ascii_case("th"),
                text,
                glyphs: None,
            });
        });
        // Resolve all cell intervals in one traversal rather than rescanning
        // the document for every cell during semantic selection.
        let cells = rows
            .iter()
            .enumerate()
            .flat_map(|(row, data)| {
                data.cells
                    .iter()
                    .enumerate()
                    .map(move |(column, cell)| (cell.box_idx, (row, column)))
            })
            .collect::<FxHashMap<_, _>>();
        for run in (self.doc.render_view(self.glyph_metrics).text()).text_runs() {
            let mut owner = None;
            for ancestor in std::iter::once(run.box_id()).chain(self.ancestors(run.box_id())) {
                if let Some(&cell) = cells.get(&layout_state::box_id_index(ancestor)) {
                    owner.get_or_insert(cell);
                }
                if self.is_table(ancestor) {
                    if ancestor == box_id
                        && let Some((row, column)) = owner
                    {
                        let glyphs = &mut rows[row].cells[column].glyphs;
                        let range = run.glyphs();
                        if let Some(existing) = glyphs {
                            *existing = enclosing_text_range(existing.clone(), range);
                        } else if !range.is_empty() {
                            *glyphs = Some(range);
                        }
                    }
                    break;
                }
            }
        }
        let mut authored_html = String::new();
        serialize_authored_html(document, table_element.node_id(), &mut authored_html);
        (!rows.is_empty() && column_count > 0).then_some(RenderTable {
            box_idx,
            rows,
            column_count,
            authored_html,
        })
    }

    pub fn point(self, box_id: BoxId) -> Option<Point> {
        let box_idx = layout_state::box_id_index(box_id);
        (box_idx < self.doc.inputs.layout_tree.box_count())
            .then(|| self.doc.geometry.point(box_idx))
    }

    pub fn size(self, box_id: BoxId) -> Option<Size> {
        let box_idx = layout_state::box_id_index(box_id);
        (box_idx < self.doc.inputs.layout_tree.box_count()).then(|| self.doc.geometry.size(box_idx))
    }

    pub fn is_block_container(self, box_id: BoxId) -> bool {
        let box_idx = layout_state::box_id_index(box_id);
        self.doc.is_block_container_box(box_idx)
    }

    /// Enclosing flows in nearest-first order, including the box at distance zero.
    /// Empty independent boxes may inherit their parent's placement; other
    /// independent flows terminate the chain after including themselves.
    pub fn flow_ancestors(self, owner: BoxId) -> impl Iterator<Item = (u32, usize)> + 'a {
        let mut current = Some(owner);
        let mut distance = 0;
        std::iter::from_fn(move || {
            let box_id = current?;
            let index = layout_state::box_id_index(box_id);
            let facts = self.pagination_ownership().boxes.get(index)?;
            let stop = facts.independent
                && (distance != 0 || !self.content_line_range(owner).is_empty());
            current = if stop { None } else { self.parent(box_id) };
            let entry = (index as u32, distance);
            distance += 1;
            Some(entry)
        })
    }

    pub fn fragmentation_distance(self, box_id: BoxId, line_idx: LineId) -> Option<usize> {
        let ownership = self.pagination_ownership();
        let owner = layout_state::box_id_index(box_id) as u32;
        let line = ownership.lines.get(layout_state::line_id_index(line_idx))?;
        if line.content.contains(&owner) {
            return Some(0);
        }
        self.flow_ancestors(box_id)
            .find_map(|(flow, distance)| {
                ownership.flow_contains_line(flow, layout_state::line_id_index(line_idx) as u32)
                    .then_some(distance)
            })
    }
}

fn render_forced_break(value: BreakBetween) -> Option<RenderForcedBreak> {
    match value {
        BreakBetween::Column => Some(RenderForcedBreak::Column),
        BreakBetween::Page => Some(RenderForcedBreak::Page),
        BreakBetween::Auto | BreakBetween::Avoid => None,
    }
}
