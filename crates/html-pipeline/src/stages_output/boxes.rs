//! Renderer-facing box geometry, semantics, and table queries.

use super::*;
use crate::layout_model::{parse_positive_span, place_table_cells};
use html_dom::{DomNodeId, collapse_whitespace, write_escaped_html_attribute, write_escaped_html_text};

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

    pub fn marker_box(self) -> usize {
        self.marker_box as usize
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
    pub fn box_idx(&self) -> usize {
        self.box_idx
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
    pub fn box_idx(&self) -> usize {
        self.box_idx
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

/// Backend-neutral resolved decoration, clip, and image fragments.
#[derive(Clone, Copy)]

pub struct BoxTextFormat {
    pub font_size: f32,
    pub font_weight: u16,
    pub font_style: html_style_model::FontStyle,
    pub color: u32,
    pub font_family: Option<html_style_model::StyleStringId>,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub text_decoration: TextDecorationLines,
}

impl<'a> RenderBoxView<'a> {
    pub fn len(self) -> usize {
        self.doc.box_count()
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn tag(self, box_idx: usize) -> Option<&'a str> {
        self.doc
            .inputs
            .layout_tree
            .box_at(box_idx)
            .and_then(|layout_box| {
                layout_box
                    .get_element(self.doc.document())
                    .map(|element| element.tag())
            })
    }

    pub fn attribute(self, box_idx: usize, name: &str) -> Option<&'a str> {
        self.doc
            .inputs
            .layout_tree
            .box_at(box_idx)
            .and_then(|layout_box| {
                layout_box
                    .get_element(self.doc.document())
                    .and_then(|element| element.attr(name))
            })
    }

    pub fn attribute_expanded(
        self,
        box_idx: usize,
        namespace: Option<&str>,
        local_name: &str,
    ) -> Option<&'a str> {
        self.doc
            .inputs
            .layout_tree
            .box_at(box_idx)
            .and_then(|layout_box| {
                layout_box
                    .get_element(self.doc.document())
                    .and_then(|element| element.attr_expanded(namespace, local_name))
            })
    }

    pub fn id(self, box_idx: usize) -> Option<u16> {
        self.doc
            .inputs
            .layout_tree
            .box_at(box_idx)?
            .get_element(self.doc.document())
            .and_then(|element| element.id_idx())
    }

    pub fn dom_node_index(self, box_idx: usize) -> Option<usize> {
        self.doc
            .box_dom_element_idx(box_idx)
            .map(|node_idx| node_idx as usize)
    }

    pub fn href(self, box_idx: usize) -> Option<u16> {
        let href_name = self.doc.document().lookup_string("href")?;
        self.doc
            .inputs
            .layout_tree
            .box_at(box_idx)
            .and_then(|layout_box| {
                layout_box
                    .get_element(self.doc.document())
                    .and_then(|element| element.attr_value_idx_no_namespace(href_name))
            })
    }

    pub fn point(self, box_idx: usize) -> Option<Point> {
        (box_idx < self.doc.inputs.layout_tree.box_count())
            .then(|| self.doc.geometry.point(box_idx))
    }

    pub fn size(self, box_idx: usize) -> Option<Size> {
        (box_idx < self.doc.inputs.layout_tree.box_count()).then(|| self.doc.geometry.size(box_idx))
    }

    pub fn is_table(self, box_idx: usize) -> bool {
        matches!(
            self.doc.box_layout_mode(box_idx),
            Some(LayoutMode::Table(_))
        )
    }

    /// Returns the source table's logical grid, retaining the laid-out box for
    /// every origin cell so renderers can hit-test and paint selections.
    pub fn table(self, box_idx: usize) -> Option<RenderTable> {
        if !self.is_table(box_idx) {
            return None;
        }
        let document = self.doc.document();
        let table_element = self
            .doc
            .inputs
            .layout_tree
            .box_at(box_idx)?
            .get_element(document)?;
        if !table_element.tag().eq_ignore_ascii_case("table") {
            return None;
        }

        fn collect_rows(
            document: &Document,
            node: DomNodeId,
            root_table: DomNodeId,
            rows: &mut Vec<DomNodeId>,
        ) {
            let Some(element) = document.element_ref(node) else {
                return;
            };
            for child in element.children() {
                let Some(child_element) = document.element_ref(child) else {
                    continue;
                };
                if child_element.tag().eq_ignore_ascii_case("table") && child != root_table {
                    continue;
                }
                if child_element.tag().eq_ignore_ascii_case("tr") {
                    rows.push(child);
                } else {
                    collect_rows(document, child, root_table, rows);
                }
            }
        }

        fn serialize_authored_html(
            document: &Document,
            node: DomNodeId,
            output: &mut String,
        ) {
            match document.node_ref(node) {
                Some(NodeRef::Text(text)) => write_escaped_html_text(output, text.text()),
                Some(NodeRef::Element(element)) => {
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
                    for child in element.children() {
                        serialize_authored_html(document, child, output);
                    }
                    output.push_str("</");
                    output.push_str(tag);
                    output.push('>');
                }
                None => {}
            }
        }

        fn collect_text(document: &Document, node: DomNodeId, text: &mut String) {
            match document.node_ref(node) {
                Some(NodeRef::Text(value)) => {
                    text.push_str(value.text());
                    text.push(' ');
                }
                Some(NodeRef::Element(element)) => {
                    if element.tag().eq_ignore_ascii_case("table") {
                        return;
                    }
                    if element.tag().eq_ignore_ascii_case("br") {
                        text.push(' ');
                    }
                    for child in element.children() {
                        collect_text(document, child, text);
                    }
                }
                None => {}
            }
        }

        let mut box_for_node = FxHashMap::default();
        for candidate in 0..self.len() {
            if let Some(node_idx) = self.dom_node_index(candidate) {
                box_for_node.entry(node_idx).or_insert(candidate);
            }
        }

        let table_node = table_element.node_id();
        let mut row_nodes = Vec::new();
        collect_rows(document, table_node, table_node, &mut row_nodes);
        let source_rows = row_nodes.iter().map(|&node| {
            document
                .element_ref(node)
                .into_iter()
                .flat_map(|row| row.children())
                .filter_map(|node| {
                    let cell = document.element_ref(node)?;
                    if !cell.tag().eq_ignore_ascii_case("th") && !cell.tag().eq_ignore_ascii_case("td")
                    {
                        return None;
                    }
                    Some((
                        cell.node_idx(),
                        parse_positive_span(cell.attr("colspan")),
                        parse_positive_span(cell.attr("rowspan")),
                    ))
                })
        });
        let mut rows = row_nodes
            .iter()
            .map(|_| RenderTableRow { cells: Vec::new() })
            .collect::<Vec<_>>();
        let column_count = place_table_cells(source_rows, |placement| {
            let Some(&cell_box) = box_for_node.get(&placement.cell_idx) else {
                return;
            };
            let cell_element = document
                .node_id_from_raw(placement.cell_idx as u32)
                .and_then(|node| document.element_ref(node))
                .expect("grid cells originate from source table elements");
            let mut raw_text = String::new();
            for child in cell_element.children() {
                collect_text(document, child, &mut raw_text);
            }
            let text = collapse_whitespace(&raw_text);
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
        for run in self.doc.render_view(self.glyph_metrics).text().text_runs() {
            let mut owner = None;
            for ancestor in std::iter::once(run.box_idx()).chain(self.ancestors(run.box_idx())) {
                if let Some(&cell) = cells.get(&ancestor) {
                    owner.get_or_insert(cell);
                }
                if self.is_table(ancestor) {
                    if ancestor == box_idx
                        && let Some((row, column)) = owner
                    {
                        let glyphs = &mut rows[row].cells[column].glyphs;
                        let range = run.glyphs();
                        if let Some(existing) = glyphs {
                            existing.start = existing.start.min(range.start);
                            existing.end = existing.end.max(range.end);
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

    pub fn is_table_row(self, box_idx: usize) -> bool {
        matches!(
            self.doc.box_layout_mode(box_idx),
            Some(LayoutMode::TableRow(_))
        )
    }

    pub fn is_table_cell(self, box_idx: usize) -> bool {
        matches!(
            self.doc.box_layout_mode(box_idx),
            Some(LayoutMode::TableCell(_))
        )
    }

    pub fn is_table_header_group(self, box_idx: usize) -> bool {
        self.used_style(box_idx)
            .is_some_and(|style| style.display() == html_style_model::Display::TableHeaderGroup)
    }

    pub fn table_cell_rowspan(self, box_idx: usize) -> usize {
        match self.doc.box_layout_mode(box_idx) {
            Some(LayoutMode::TableCell(cell)) => cell.rowspan.max(1),
            _ => 1,
        }
    }

    pub fn forces_break_before(self, box_idx: usize) -> bool {
        self.used_style(box_idx)
            .is_some_and(|style| style.break_before().is_forced())
    }

    pub fn forces_break_after(self, box_idx: usize) -> bool {
        self.used_style(box_idx)
            .is_some_and(|style| style.break_after().is_forced())
    }

    pub fn avoids_break_before(self, box_idx: usize) -> bool {
        self.used_style(box_idx)
            .is_some_and(|style| style.break_before() == html_style_model::BreakBetween::Avoid)
    }

    pub fn avoids_break_after(self, box_idx: usize) -> bool {
        self.used_style(box_idx)
            .is_some_and(|style| style.break_after() == html_style_model::BreakBetween::Avoid)
    }

    pub fn break_before(self, box_idx: usize) -> html_style_model::BreakBetween {
        self.used_style(box_idx)
            .map_or(html_style_model::BreakBetween::Auto, |style| {
                style.break_before()
            })
    }

    pub fn forced_break_before(self, box_idx: usize) -> Option<RenderForcedBreak> {
        render_forced_break(self.break_before(box_idx))
    }

    pub fn forced_break_after(self, box_idx: usize) -> Option<RenderForcedBreak> {
        render_forced_break(
            self.used_style(box_idx)
                .map_or(html_style_model::BreakBetween::Auto, |style| {
                    style.break_after()
                }),
        )
    }

    pub fn avoids_break_inside(self, box_idx: usize) -> bool {
        self.used_style(box_idx)
            .is_some_and(|style| style.break_inside() == html_style_model::BreakInside::Avoid)
    }

    pub fn widows(self, box_idx: usize) -> usize {
        self.used_style(box_idx)
            .map_or(2, |style| style.widows() as usize)
    }

    pub fn orphans(self, box_idx: usize) -> usize {
        self.used_style(box_idx)
            .map_or(2, |style| style.orphans() as usize)
    }

    pub fn parent(self, box_idx: usize) -> Option<usize> {
        self.doc.inputs.layout_tree.get_box_parent(box_idx)
    }

    pub fn text_format(self, box_idx: usize) -> BoxTextFormat {
        self.doc.box_text_format(box_idx)
    }

    pub fn list_marker(self, box_idx: usize) -> Option<RenderListItemMarker> {
        self.doc.list_marker(box_idx)
    }

    pub fn is_block_container(self, box_idx: usize) -> bool {
        self.doc.is_block_container_box(box_idx)
    }

    pub fn ancestors(self, box_idx: usize) -> impl Iterator<Item = usize> + 'a {
        self.doc.inputs.layout_tree.ancestors(box_idx)
    }

    fn used_style(self, box_idx: usize) -> Option<html_style_model::UsedStyleView<'a>> {
        self.doc.box_used_style(box_idx)
    }
}

fn render_forced_break(value: html_style_model::BreakBetween) -> Option<RenderForcedBreak> {
    match value {
        html_style_model::BreakBetween::Column => Some(RenderForcedBreak::Column),
        html_style_model::BreakBetween::Page => Some(RenderForcedBreak::Page),
        html_style_model::BreakBetween::Auto | html_style_model::BreakBetween::Avoid => None,
    }
}
