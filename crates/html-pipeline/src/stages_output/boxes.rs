//! Renderer-facing box geometry, semantics, and table queries.

use super::text::enclosing_text_range;
use super::*;
use crate::layout_model::{parse_positive_span, place_table_cells};
use html_dom::{DomNodeId, ElementRef, write_escaped_html_attribute, write_escaped_html_text};
use html_style_model::{BreakBetween, Float, PositionMode};

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
    /// Whether a line carries content belonging to this box or a descendant.
    /// Replaced and inline content may use an ancestor's carrier line.
    pub fn contains_line(self, box_idx: usize, line_idx: usize) -> bool {
        if !self.content_line_range(box_idx).contains(&line_idx) {
            return false;
        }
        let view = self.doc.render_view(self.glyph_metrics);
        let output = &self.doc.layout_state.line_output;
        let Some(line) = output.lines.get(line_idx) else {
            return false;
        };
        let contains = |owner| view.semantics().contains(box_idx, owner);
        if contains(line.owner_box_idx as usize) {
            return true;
        }
        let inline = line.inline_box_fragments(&output.line_details);
        output.inline_box_fragments[inline.start as usize..inline.end as usize]
            .iter()
            .any(|fragment| contains(fragment.box_idx as usize))
            || view
                .fragments()
                .images_for_line(line_idx)
                .iter()
                .any(|image| contains(image.box_idx()))
    }

    /// Distance to the enclosing box whose flow can carry this box's geometry
    /// alongside a visible line. Absolutely positioned and floating subtrees do
    /// not establish continuations for one another.
    pub fn fragmentation_distance(self, box_idx: usize, line_idx: usize) -> Option<usize> {
        if self.contains_line(box_idx, line_idx) {
            return Some(0);
        }
        let view = self.doc.render_view(self.glyph_metrics);
        let line = view.text().line(line_idx)?;
        let images = view.fragments().images_for_line(line_idx);
        if line.glyphs().is_empty() && !images.is_empty() {
            images
                .iter()
                .filter_map(|image| self.shared_flow_distance(box_idx, image.box_idx()))
                .min()
        } else {
            self.shared_flow_distance(box_idx, line.owner_box_idx())
        }
    }

    fn shared_flow_distance(self, box_idx: usize, line_owner: usize) -> Option<usize> {
        let contains = |ancestor| {
            self.doc
                .render_view(self.glyph_metrics)
                .semantics()
                .contains(ancestor, line_owner)
        };
        let independent = |owner| {
            self.used_style(owner).is_some_and(|style| {
                style.position() == PositionMode::Absolute
                    || matches!(style.float(), Float::Left | Float::Right)
            })
        };
        let mut ancestor = box_idx;
        let mut distance = 0;
        loop {
            if contains(ancestor) {
                let mut carrier = line_owner;
                while carrier != ancestor {
                    if independent(carrier) {
                        return None;
                    }
                    carrier = self.parent(carrier)?;
                }
                return Some(distance);
            }
            // A contentless decoration has no independently placed group;
            // retain its enclosing flow's mapping even if it is positioned.
            if independent(ancestor)
                && (ancestor != box_idx || !self.content_line_range(box_idx).is_empty())
            {
                return None;
            }
            ancestor = self.parent(ancestor)?;
            distance += 1;
        }
    }

    /// Bounds in stable line storage for content carried by this box and its
    /// descendants, including image carriers and split inline spans.
    pub fn content_line_range(self, box_idx: usize) -> Range<usize> {
        self.doc
            .layout_state
            .line_output
            .box_line_ranges
            .get(box_idx)
            .map_or(0..0, |range| range.start as usize..range.end as usize)
    }

    pub fn len(self) -> usize {
        self.doc.box_count()
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn tag(self, box_idx: usize) -> Option<&'a str> {
        self.doc
            .element_for_box(box_idx)
            .map(|element| element.tag())
    }

    pub fn attribute(self, box_idx: usize, name: &str) -> Option<&'a str> {
        self.doc
            .element_for_box(box_idx)
            .and_then(|element| element.attr(name))
    }

    pub fn attribute_expanded(
        self,
        box_idx: usize,
        namespace: Option<&str>,
        local_name: &str,
    ) -> Option<&'a str> {
        self.doc
            .element_for_box(box_idx)
            .and_then(|element| element.attr_expanded(namespace, local_name))
    }

    pub fn id(self, box_idx: usize) -> Option<u16> {
        self.doc
            .element_for_box(box_idx)
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
            .element_for_box(box_idx)
            .and_then(|element| element.attr_value_idx_no_namespace(href_name))
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
                    for word in value.text().split_whitespace() {
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(word);
                    }
                }
                Some(NodeRef::Element(element)) => {
                    if element.tag().eq_ignore_ascii_case("table") {
                        return;
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
                        .get(&cell.node_idx())
                        .and_then(|&box_idx| match self.doc.box_layout_mode(box_idx) {
                            Some(LayoutMode::TableCell(cell)) => Some(cell.rowspan),
                            _ => None,
                        })
                        .unwrap_or_else(|| parse_positive_span(cell.attr("rowspan")));
                    (
                        cell.node_idx(),
                        parse_positive_span(cell.attr("colspan")),
                        rowspan,
                    )
                })
        });
        let column_count = place_table_cells(source_rows, |placement| {
            let Some(&cell_box) = box_for_node.get(&placement.cell_idx) else {
                return;
            };
            let cell_element = document
                .node_id_from_raw(placement.cell_idx as u32)
                .and_then(|node| document.element_ref(node))
                .expect("grid cells originate from source table elements");
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
        self.break_before(box_idx).is_forced()
    }

    pub fn forces_break_after(self, box_idx: usize) -> bool {
        self.break_after(box_idx).is_forced()
    }

    pub fn avoids_break_before(self, box_idx: usize) -> bool {
        self.break_before(box_idx) == BreakBetween::Avoid
    }

    pub fn avoids_break_after(self, box_idx: usize) -> bool {
        self.break_after(box_idx) == BreakBetween::Avoid
    }

    pub fn break_before(self, box_idx: usize) -> BreakBetween {
        self.used_style(box_idx)
            .map_or(BreakBetween::Auto, |style| style.break_before())
    }

    fn break_after(self, box_idx: usize) -> BreakBetween {
        self.used_style(box_idx)
            .map_or(BreakBetween::Auto, |style| style.break_after())
    }

    pub fn forced_break_before(self, box_idx: usize) -> Option<RenderForcedBreak> {
        render_forced_break(self.break_before(box_idx))
    }

    pub fn forced_break_after(self, box_idx: usize) -> Option<RenderForcedBreak> {
        render_forced_break(self.break_after(box_idx))
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

    /// Flex and grid containers, including their inline display variants.
    pub fn is_flex_or_grid_container(self, box_idx: usize) -> bool {
        matches!(
            self.doc.inputs.layout_tree.get_box_layout_mode(box_idx),
            Some(LayoutMode::Flex(_) | LayoutMode::Grid(_))
        )
    }

    pub fn ancestors(self, box_idx: usize) -> impl Iterator<Item = usize> + 'a {
        self.doc.inputs.layout_tree.ancestors(box_idx)
    }

    fn used_style(self, box_idx: usize) -> Option<html_style_model::UsedStyleView<'a>> {
        self.doc.box_used_style(box_idx)
    }
}

fn render_forced_break(value: BreakBetween) -> Option<RenderForcedBreak> {
    match value {
        BreakBetween::Column => Some(RenderForcedBreak::Column),
        BreakBetween::Page => Some(RenderForcedBreak::Page),
        BreakBetween::Auto | BreakBetween::Avoid => None,
    }
}
