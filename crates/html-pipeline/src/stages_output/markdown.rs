//! Markdown clipboard payloads for logical text and box selections.

use super::{DocumentRead, RenderTable};
use crate::layout_model::InlineItemKind;
use html_dom::{Document, DomNodeId, ElementRef};
use html_style_model::{ComputedStyles, Display, FontStyle, Visibility};
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use std::ops::Range;

/// Read-only selection conversion. Applications own selection gestures and
/// write the returned UTF-8 string to their platform clipboard.
#[derive(Clone, Copy)]
pub struct RenderExportView<'a> {
    pub(super) doc: DocumentRead<'a>,
    pub(super) glyph_metrics: &'a crate::layout_model::GlyphMetrics,
}

impl<'a> RenderExportView<'a> {
    /// Whether an image resource represents a retained mathematical formula.
    pub fn is_equation(self, image: u32) -> bool {
        self.doc
            .render_view(self.glyph_metrics)
            .image_mathml(image)
            .is_some()
    }

    /// Build plain-text and Markdown versions of the same logical selection.
    /// The tuple contains `(plain_text, markdown)`; invalid or empty selections
    /// return `(None, None)`. Selection and clipboard state remain with callers.
    pub fn selection_text(self, glyphs: Range<u32>) -> (Option<String>, Option<String>) {
        let Some((root, selected)) = self.selection(glyphs) else {
            return (None, None);
        };
        let plain = selected.plain(root).trim_matches('\n').to_owned();
        ((!plain.is_empty()).then_some(plain), selected.finish(root))
    }

    /// Visible source text for a logical selection, without generating Markdown.
    pub fn plain_text(self, glyphs: Range<u32>) -> Option<String> {
        let (root, selected) = self.selection(glyphs)?;
        let plain = selected.plain(root).trim_matches('\n').to_owned();
        (!plain.is_empty()).then_some(plain)
    }

    /// Convert a half-open logical glyph range to Markdown. These are the same
    /// indices used by `RenderTextRun::glyphs`, not UTF-8 byte offsets.
    /// Empty, reversed, and out-of-bounds ranges return `None`.
    ///
    /// Preserves headings, emphasis, links, lists, quotes, code, and simple GFM
    /// tables. Complex tables use structural HTML. Only selected visible text
    /// is copied, in source order, without generated markers or layout hyphens.
    /// Inline images and authored breaks between selected characters survive;
    /// use `markdown_for_box` for objects without selectable text.
    pub fn markdown(self, glyphs: Range<u32>) -> Option<String> {
        let (root, selected) = self.selection(glyphs)?;
        selected.finish(root)
    }

    fn selection(self, glyphs: Range<u32>) -> Option<(DomNodeId, Markdown<'a>)> {
        let count = self.doc.inputs.text.content().characters().len();
        if glyphs.start >= glyphs.end || glyphs.end as usize > count {
            return None;
        }
        let root = self.doc.document().dom_root()?;
        Some((root, Markdown::new(self.doc, Some(glyphs), root)))
    }

    /// Convert a selected source-backed box, such as a table, code block, or
    /// image. Invalid and synthetic boxes return `None`.
    pub fn markdown_for_box(self, box_idx: usize) -> Option<String> {
        let document = self.doc.document();
        let root = document.node_id_from_raw(self.doc.box_dom_element_idx(box_idx)?)?;
        Markdown::new(self.doc, None, root).finish(root)
    }
}

struct Markdown<'a> {
    document: &'a Document,
    styles: &'a ComputedStyles,
    text: FxHashMap<DomNodeId, String>,
    active: FxHashSet<DomNodeId>,
    visible: FxHashSet<DomNodeId>,
    partial: bool,
    inline_pre: Option<DomNodeId>,
}

impl<'a> Markdown<'a> {
    fn new(doc: DocumentRead<'a>, selection: Option<Range<u32>>, root: DomNodeId) -> Self {
        let document = doc.document();
        let mut result = Self {
            document,
            styles: &doc.inputs.styles,
            text: FxHashMap::default(),
            active: FxHashSet::default(),
            visible: FxHashSet::default(),
            partial: selection.is_some(),
            inline_pre: None,
        };
        // Text ranges activate only selected ancestors. Box selections also
        // retain empty visible cells and objects within their source subtree.
        if selection.is_none() {
            for index in 0..doc.box_count() {
                if doc
                    .box_used_style(index)
                    .is_some_and(|style| style.visibility() == Visibility::Visible)
                    && let Some(node) = doc
                        .box_dom_element_idx(index)
                        .and_then(|raw| document.node_id_from_raw(raw))
                    && within(document, node, root)
                {
                    result.visible.insert(node);
                }
            }
        }
        let content = doc.inputs.text.content();
        let mut bounds = FxHashMap::default();
        let mut common_pre = None;
        for item in content.inline_items() {
            match &item.kind {
                InlineItemKind::Text { glyphs } => {
                    let Some(node) = item
                        .dom_text_node
                        .and_then(|raw| document.node_id_from_raw(raw))
                    else {
                        continue;
                    };
                    if !within(document, node, root) {
                        continue;
                    }
                    bounds
                        .entry(node)
                        .and_modify(|range: &mut Range<u32>| {
                            range.start = range.start.min(glyphs.start);
                            range.end = range.end.max(glyphs.end);
                        })
                        .or_insert_with(|| glyphs.clone());
                    if !doc
                        .box_used_style(item.box_idx as usize)
                        .is_some_and(|style| style.visibility() == Visibility::Visible)
                    {
                        continue;
                    }
                    let start = selection
                        .as_ref()
                        .map_or(glyphs.start, |range| glyphs.start.max(range.start));
                    let end = selection
                        .as_ref()
                        .map_or(glyphs.end, |range| glyphs.end.min(range.end));
                    if start >= end {
                        continue;
                    }
                    let pre = document
                        .dom_ancestors(node)
                        .find(|&ancestor| document.get_dom_tag(ancestor) == Some("pre"));
                    common_pre = Some(match common_pre {
                        None => pre,
                        Some(previous) if previous == pre => pre,
                        _ => None,
                    });
                    let text = result.text.entry(node).or_default();
                    text.extend(
                        (start..end)
                            .filter_map(|index| {
                                content
                                    .character_at(index as usize)
                                    .and_then(char::from_u32)
                            })
                            .filter(|&ch| ch != '\u{00ad}'),
                    );
                    result.activate(node);
                }
                _ => {}
            }
        }
        if selection.is_some() {
            result.inline_pre = common_pre.flatten();
        }
        result.include_objects(doc, root, &bounds, &mut 0, selection.as_ref());
        result
    }

    fn include_objects(
        &mut self,
        doc: DocumentRead<'_>,
        node: DomNodeId,
        bounds: &FxHashMap<DomNodeId, Range<u32>>,
        cursor: &mut u32,
        selection: Option<&Range<u32>>,
    ) {
        if let Some(range) = bounds.get(&node) {
            *cursor = range.end;
            return;
        }
        let Some(element) = self.document.element_ref(node) else {
            return;
        };
        let style = doc
            .inputs
            .styles
            .style_for_node(node)
            .and_then(|style| doc.inputs.styles.view(style));
        if excluded(element.tag()) || style.is_some_and(|style| style.display() == Display::None) {
            return;
        }
        if matches!(element.tag(), "br" | "hr" | "img")
            && style.is_some_and(|style| style.visibility() == Visibility::Visible)
        {
            self.visible.insert(node);
            if selection.is_none_or(|range| range.start < *cursor && *cursor < range.end) {
                self.activate(node);
            }
        }
        for child in element.children() {
            self.include_objects(doc, child, bounds, cursor, selection);
        }
    }

    fn activate(&mut self, mut node: DomNodeId) {
        while self.active.insert(node) {
            let Some(parent) = self.document.get_dom_parent(node) else {
                break;
            };
            node = parent;
        }
    }

    fn includes(&self, node: DomNodeId) -> bool {
        self.active.contains(&node) || (!self.partial && self.visible.contains(&node))
    }

    fn finish(&self, root: DomNodeId) -> Option<String> {
        let output = self.node(root);
        let output = output.trim().to_owned();
        (!output.is_empty()).then_some(output)
    }

    fn children(&self, element: ElementRef<'_>) -> String {
        let mut output = String::new();
        for node in element.children() {
            let content = self.node(node);
            if output.ends_with('\n') && content.starts_with('\n') {
                output.truncate(output.trim_end_matches('\n').len());
                output.push_str("\n\n");
                output.push_str(content.trim_start_matches('\n'));
            } else {
                output.push_str(&content);
            }
        }
        output
    }

    fn node(&self, node: DomNodeId) -> String {
        if let Some(text) = self.text.get(&node) {
            return escape_text(text);
        }
        let Some(element) = self.document.element_ref(node) else {
            return String::new();
        };
        let tag = element.tag();
        if !self.includes(node) || excluded(tag) {
            return String::new();
        }
        match tag {
            "img" => element.attr("src").map_or_else(String::new, |src| {
                format!(
                    "![{}]({}{})",
                    escape_text(element.attr("alt").unwrap_or("")),
                    destination(src),
                    title(element.attr("title"))
                )
            }),
            "br" => "  \n".into(),
            "hr" => block("---"),
            "pre" => {
                let text = self.raw_text(node);
                if text.is_empty() {
                    return String::new();
                }
                if self.inline_pre == Some(node) {
                    return code_span(&text);
                }
                let fence = "`".repeat(longest_backticks(&text).max(2) + 1);
                let language = code_language(element)
                    .or_else(|| {
                        element
                            .children()
                            .filter_map(|child| self.document.element_ref(child))
                            .find(|child| child.tag() == "code")
                            .and_then(code_language)
                    })
                    .filter(|language| {
                        language
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '+'))
                    })
                    .unwrap_or("");
                block(&format!(
                    "{fence}{language}\n{text}{}{fence}",
                    if text.ends_with('\n') { "" } else { "\n" }
                ))
            }
            "code" | "kbd" | "samp" => code_span(&self.raw_text(node)),
            "ul" | "ol" => self.list(element),
            "li" => {
                let parent = element
                    .parent()
                    .and_then(|node| self.document.element_ref(node));
                let marker = if let Some(parent) = parent.filter(|parent| parent.tag() == "ol") {
                    let items = parent
                        .children()
                        .filter_map(|node| self.document.element_ref(node))
                        .filter(|child| child.tag() == "li")
                        .collect::<Vec<_>>();
                    let step = if parent.has_attr("reversed") { -1 } else { 1 };
                    let mut number = parent
                        .attr("start")
                        .and_then(|value| value.parse::<i64>().ok())
                        .unwrap_or(if step < 0 { items.len() as i64 } else { 1 });
                    for item in items {
                        number = item
                            .attr("value")
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(number);
                        if item.node_id() == node {
                            break;
                        }
                        number = number.saturating_add(step);
                    }
                    if !(0..=999_999_999).contains(&number) {
                        return block(&format!(
                            "<ol start=\"{number}\">{}</ol>",
                            self.html(node, false)
                        ));
                    }
                    format!("{number}. ")
                } else {
                    "- ".into()
                };
                block(&self.list_item(element, &marker))
            }
            "table" => self.table(element),
            _ => {
                let content = self.styled(element, self.children(element));
                if content.trim().is_empty() {
                    return content;
                }
                match tag {
                    "h1" => block(&format!("# {}", content.trim())),
                    "h2" => block(&format!("## {}", content.trim())),
                    "h3" => block(&format!("### {}", content.trim())),
                    "h4" => block(&format!("#### {}", content.trim())),
                    "h5" => block(&format!("##### {}", content.trim())),
                    "h6" => block(&format!("###### {}", content.trim())),
                    "strong" | "b" => inline(&content, "**"),
                    "em" | "i" => inline(&content, "*"),
                    "del" | "s" | "strike" => inline(&content, "~~"),
                    "a" => element.attr("href").map_or(content.clone(), |href| {
                        let trimmed = content.trim();
                        content.replacen(
                            trimmed,
                            &format!(
                                "[{trimmed}]({}{})",
                                destination(href),
                                title(element.attr("title"))
                            ),
                            1,
                        )
                    }),
                    "blockquote" => block(
                        &content
                            .trim()
                            .lines()
                            .map(|line| format!("> {line}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                    tag if block_tag(tag) => block(&content),
                    _ => content,
                }
            }
        }
    }

    fn styled(&self, element: ElementRef<'_>, mut content: String) -> String {
        let style = |node| {
            self.styles
                .style_for_node(node)
                .and_then(|indices| self.styles.view(indices))
        };
        let Some(current) = style(element.node_id()) else {
            return content;
        };
        let parent = element.parent().and_then(style);
        if current.font_weight() >= 600
            && parent.is_none_or(|parent| parent.font_weight() < 600)
            && !matches!(
                element.tag(),
                "strong" | "b" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            )
        {
            content = inline(&content, "**");
        }
        if matches!(current.font_style(), FontStyle::Italic | FontStyle::Oblique)
            && parent.is_none_or(|parent| {
                !matches!(parent.font_style(), FontStyle::Italic | FontStyle::Oblique)
            })
            && !matches!(element.tag(), "em" | "i")
        {
            content = inline(&content, "*");
        }
        if current.text_decoration_lines().line_through()
            && parent.is_none_or(|parent| !parent.text_decoration_lines().line_through())
            && !matches!(element.tag(), "s" | "del" | "strike")
        {
            content = inline(&content, "~~");
        }
        content
    }

    fn raw_text(&self, node: DomNodeId) -> String {
        if let Some(text) = self.text.get(&node) {
            return text.clone();
        }
        let Some(element) = self.document.element_ref(node) else {
            return String::new();
        };
        if !self.includes(node) || excluded(element.tag()) {
            return String::new();
        }
        if element.tag() == "br" {
            return "\n".into();
        }
        element
            .children()
            .map(|child| self.raw_text(child))
            .collect()
    }

    fn plain(&self, node: DomNodeId) -> String {
        if let Some(text) = self.text.get(&node) {
            return text.clone();
        }
        let Some(element) = self.document.element_ref(node) else {
            return String::new();
        };
        if !self.includes(node) || excluded(element.tag()) {
            return String::new();
        }
        if element.tag() == "br" {
            return "\n".into();
        }
        let mut output = String::new();
        for child in element.children() {
            let content = self.plain(child);
            if output.ends_with('\n') && content.starts_with('\n') {
                output.push_str(content.trim_start_matches('\n'));
            } else {
                output.push_str(&content);
            }
        }
        if (block_tag(element.tag())
            || matches!(
                element.tag(),
                "h1" | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "li"
                    | "tr"
                    | "td"
                    | "th"
                    | "pre"
                    | "blockquote"
            ))
            && !output.is_empty()
        {
            format!("\n{}\n", output.trim_matches('\n'))
        } else {
            output
        }
    }

    fn list(&self, element: ElementRef<'_>) -> String {
        let ordered = element.tag() == "ol";
        let items = element
            .children()
            .filter_map(|node| self.document.element_ref(node))
            .filter(|child| child.tag() == "li")
            .collect::<Vec<_>>();
        let mut number = element
            .attr("start")
            .and_then(|start| start.parse::<i64>().ok())
            .unwrap_or(1);
        // Markdown cannot express reversed or independently renumbered items.
        if ordered
            && (element.has_attr("reversed")
                || !(0..=999_999_999).contains(&number)
                || items.iter().any(|item| item.has_attr("value")))
        {
            return block(&self.html(element.node_id(), false));
        }
        let mut output = Vec::new();
        for item in items {
            let marker = if ordered {
                format!("{number}. ")
            } else {
                "- ".into()
            };
            let text = self.list_item(item, &marker);
            if !text.is_empty() {
                output.push(text);
            }
            number = number.saturating_add(1);
        }
        block(&output.join("\n\n"))
    }

    fn list_item(&self, element: ElementRef<'_>, marker: &str) -> String {
        let mut content = String::new();
        for child in element.children() {
            let value = self.node(child);
            let nested = self
                .document
                .element_ref(child)
                .is_some_and(|child| matches!(child.tag(), "ul" | "ol"));
            if nested && !value.trim().is_empty() {
                content.truncate(content.trim_end_matches('\n').len());
                content.push('\n');
                content.push_str(value.trim_matches('\n'));
                content.push('\n');
            } else if content.ends_with('\n') && value.starts_with('\n') {
                content.truncate(content.trim_end_matches('\n').len());
                content.push_str("\n\n");
                content.push_str(value.trim_start_matches('\n'));
            } else {
                content.push_str(&value);
            }
        }
        if content.trim().is_empty() {
            return String::new();
        }
        let indent = " ".repeat(marker.len().max(4));
        let mut lines = content.trim().lines();
        let mut text = format!("{marker}{}", lines.next().unwrap_or(""));
        for line in lines {
            text.push('\n');
            if !line.is_empty() {
                text.push_str(&indent);
                text.push_str(line);
            }
        }
        text
    }

    fn table(&self, table: ElementRef<'_>) -> String {
        fn collect<'a>(
            document: &'a Document,
            element: ElementRef<'a>,
            rows: &mut Vec<ElementRef<'a>>,
        ) {
            for child in element
                .children()
                .filter_map(|node| document.element_ref(node))
            {
                match child.tag() {
                    "tr" => rows.push(child),
                    "thead" | "tbody" | "tfoot" => collect(document, child, rows),
                    _ => {}
                }
            }
        }
        let mut rows = Vec::new();
        collect(self.document, table, &mut rows);
        rows.retain(|row| self.includes(row.node_id()));
        let cells = rows
            .iter()
            .map(|row| {
                row.children()
                    .filter_map(|node| self.document.element_ref(node))
                    .filter(|cell| matches!(cell.tag(), "td" | "th"))
                    .filter(|cell| self.includes(cell.node_id()) || self.partial)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let caption = table
            .children()
            .filter_map(|node| self.document.element_ref(node))
            .filter(|child| child.tag() == "caption")
            .map(|caption| self.children(caption))
            .collect::<String>();
        let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return block(&caption);
        }
        let header = cells
            .first()
            .is_some_and(|row| !row.is_empty() && row.iter().all(|cell| cell.tag() == "th"));
        let complex = cells.iter().enumerate().any(|(index, row)| {
            row.iter().any(|cell| {
                cell.attr("rowspan")
                    .is_some_and(|span| span.parse::<usize>().is_ok_and(|span| span != 1))
                    || cell
                        .attr("colspan")
                        .is_some_and(|span| span.parse::<usize>().is_ok_and(|span| span != 1))
                    || (cell.tag() == "th" && !(index == 0 && header))
                    || self.has_block_children(*cell)
            })
        });
        if complex {
            return block(&self.html(table.node_id(), false));
        }
        let mut output = if header {
            self.table_row(&cells[0], columns)
        } else {
            format!("|{}", " |".repeat(columns))
        };
        output.push('\n');
        output.push_str(&format!("|{}", " --- |".repeat(columns)));
        for row in cells.iter().skip(usize::from(header)) {
            output.push('\n');
            output.push_str(&self.table_row(row, columns));
        }
        format!("{}{}", block(&caption), block(&output))
    }

    fn table_row(&self, cells: &[ElementRef<'_>], columns: usize) -> String {
        let mut result = "|".to_owned();
        for column in 0..columns {
            let value = cells
                .get(column)
                .map_or_else(String::new, |cell| self.children(*cell));
            // GFM parses pipes before inline Markdown, including code spans.
            let mut escaped = String::new();
            let mut slashes = 0;
            for ch in value.trim().chars() {
                if ch == '|' && slashes % 2 == 0 {
                    escaped.push('\\');
                }
                escaped.push(ch);
                slashes = if ch == '\\' { slashes + 1 } else { 0 };
            }
            result.push_str(&format!(
                " {} |",
                escaped.replace("  \n", "<br>").replace('\n', "<br>")
            ));
        }
        result
    }

    fn has_block_children(&self, element: ElementRef<'_>) -> bool {
        element
            .children()
            .filter_map(|node| self.document.element_ref(node))
            .any(|child| {
                matches!(child.tag(), "table" | "pre" | "ul" | "ol" | "blockquote")
                    || block_tag(child.tag())
                    || self.has_block_children(child)
            })
    }

    /// Structural HTML fallback uses selected text and a small attribute set;
    /// it never copies styles, event handlers, or unselected authored text.
    fn html(&self, node: DomNodeId, empty_cell: bool) -> String {
        if let Some(text) = self.text.get(&node) {
            return escape_html(text);
        }
        let Some(element) = self.document.element_ref(node) else {
            return String::new();
        };
        let tag = element.tag();
        if excluded(tag) || (!self.includes(node) && !empty_cell) {
            return String::new();
        }
        let content = element
            .children()
            .map(|child| {
                let cell = tag == "tr"
                    && self
                        .document
                        .element_ref(child)
                        .is_some_and(|child| matches!(child.tag(), "td" | "th"));
                self.html(child, cell)
            })
            .collect::<String>();
        if !matches!(
            tag,
            "table"
                | "caption"
                | "thead"
                | "tbody"
                | "tfoot"
                | "tr"
                | "td"
                | "th"
                | "ul"
                | "ol"
                | "li"
                | "p"
                | "div"
                | "pre"
                | "code"
                | "blockquote"
                | "strong"
                | "b"
                | "em"
                | "i"
                | "s"
                | "del"
                | "span"
                | "a"
                | "img"
                | "br"
        ) {
            return content;
        }
        let mut output = format!("<{tag}");
        for name in [
            "rowspan", "colspan", "start", "reversed", "value", "href", "src", "alt", "title",
        ] {
            if let Some(value) = element.attr(name) {
                output.push_str(&format!(" {name}=\"{}\"", escape_html(value)));
            }
        }
        output.push('>');
        output.push_str(&content);
        if !matches!(tag, "br" | "img") {
            output.push_str(&format!("</{tag}>"));
        }
        output
    }
}

impl RenderTable {
    /// Portable structural table HTML, retaining cell spans without author styling.
    pub fn unstyled_html(&self) -> Option<String> {
        if self.rows().is_empty() {
            return None;
        }
        let mut html = String::from("<table>");
        for row in self.rows() {
            html.push_str("<tr>");
            for cell in row.cells() {
                let tag = if cell.is_header() { "th" } else { "td" };
                html.push('<');
                html.push_str(tag);
                if cell.rowspan() > 1 {
                    html.push_str(&format!(" rowspan=\"{}\"", cell.rowspan()));
                }
                if cell.colspan() > 1 {
                    html.push_str(&format!(" colspan=\"{}\"", cell.colspan()));
                }
                html.push('>');
                html.push_str(&escape_html(cell.text()));
                html.push_str("</");
                html.push_str(tag);
                html.push('>');
            }
            html.push_str("</tr>");
        }
        html.push_str("</table>");
        Some(html)
    }

    /// Copy the logical table grid as a GFM pipe table. Merged cells put their
    /// text in the first covered slot and leave the other slots empty. Tables
    /// without headers receive an empty header, preserving every data row.
    pub fn markdown(&self) -> Option<String> {
        self.markdown_in(0..self.rows().len(), 0..self.column_count())
    }

    /// Copy a rectangular cell selection. Row and column ranges are half-open
    /// logical grid coordinates. Ends are clipped to the table; an empty or
    /// reversed selection returns `None`. Cell contents are exported as text.
    /// Use `RenderExportView::markdown_for_box` to retain inline formatting or
    /// structural HTML for merged and nested tables instead.
    pub fn markdown_in(&self, rows: Range<usize>, columns: Range<usize>) -> Option<String> {
        let row_end = rows.end.min(self.rows().len());
        let column_end = columns.end.min(self.column_count());
        if rows.start >= row_end || columns.start >= column_end {
            return None;
        }
        let width = column_end - columns.start;
        let mut grid = vec![vec![String::new(); width]; row_end - rows.start];
        let mut header = false;
        for row in self.rows() {
            for cell in row.cells() {
                if cell.row().saturating_add(cell.rowspan()) <= rows.start
                    || cell.row() >= row_end
                    || cell.column().saturating_add(cell.colspan()) <= columns.start
                    || cell.column() >= column_end
                {
                    continue;
                }
                header |= cell.row() == rows.start && cell.is_header();
                let row = cell.row().max(rows.start) - rows.start;
                let column = cell.column().max(columns.start) - columns.start;
                grid[row][column] = escape_text(&cell.text().replace(['\r', '\n'], " "));
            }
        }
        let format_row = |row: &[String]| format!("| {} |", row.join(" | "));
        let mut lines = vec![if header {
            format_row(&grid[0])
        } else {
            format_row(&vec![String::new(); width])
        }];
        lines.push(format!("| {} |", vec!["---"; width].join(" | ")));
        lines.extend(
            grid.iter()
                .skip(usize::from(header))
                .map(|row| format_row(row)),
        );
        Some(lines.join("\n"))
    }
}

fn excluded(tag: &str) -> bool {
    matches!(tag, "head" | "script" | "style" | "template" | "title")
}

fn block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div"
            | "section"
            | "article"
            | "main"
            | "header"
            | "footer"
            | "nav"
            | "aside"
            | "address"
            | "figure"
            | "figcaption"
            | "details"
            | "summary"
            | "dl"
            | "dt"
            | "dd"
    )
}

fn block(content: &str) -> String {
    if content.trim().is_empty() {
        String::new()
    } else {
        format!("\n\n{}\n\n", content.trim())
    }
}

fn inline(content: &str, marker: &str) -> String {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return content.to_owned();
    }
    let start = content.len() - content.trim_start().len();
    format!(
        "{}{marker}{trimmed}{marker}{}",
        &content[..start],
        &content[start + trimmed.len()..]
    )
}

fn escape_text(text: &str) -> String {
    let mut output = String::new();
    let mut previous = None;
    let mut characters = text.chars().peekable();
    while let Some(ch) = characters.next() {
        if ch == '&'
            && characters
                .peek()
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == '#')
        {
            output.push_str("&amp;");
        } else {
            if matches!(
                ch,
                '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '!' | '|' | '~'
            ) || (matches!(ch, '-' | '+' | '=') && previous.is_none_or(|ch| ch == '\n'))
                || (ch == '.'
                    && previous.is_some_and(|ch: char| ch.is_ascii_digit())
                    && characters.peek().is_some_and(|ch| ch.is_whitespace()))
            {
                output.push('\\');
            }
            output.push(ch);
        }
        previous = Some(ch);
    }
    output
}

fn longest_backticks(text: &str) -> usize {
    text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0)
}

fn code_span(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let text = text.replace('\n', " ");
    let fence = "`".repeat(longest_backticks(&text) + 1);
    let pad = text.starts_with('`')
        || text.ends_with('`')
        || (text.starts_with(' ') && text.ends_with(' ') && !text.chars().all(|ch| ch == ' '));
    let pad = if pad { " " } else { "" };
    format!("{fence}{pad}{text}{pad}{fence}")
}

fn destination(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_whitespace()
            || ch.is_control()
            || matches!(ch, '(' | ')' | '<' | '>' | '\\' | '\"')
        {
            let mut buffer = [0; 4];
            for byte in ch.encode_utf8(&mut buffer).bytes() {
                output.push_str(&format!("%{byte:02X}"));
            }
        } else if ch == '&' {
            output.push_str("&amp;");
        } else {
            output.push(ch);
        }
    }
    output
}

fn title(value: Option<&str>) -> String {
    value.map_or_else(String::new, |value| {
        format!(" \"{}\"", escape_html(value).replace('\n', " "))
    })
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn within(document: &Document, mut node: DomNodeId, root: DomNodeId) -> bool {
    loop {
        if node == root {
            return true;
        }
        let Some(parent) = document.get_dom_parent(node) else {
            return false;
        };
        node = parent;
    }
}

fn code_language(element: ElementRef<'_>) -> Option<&str> {
    element
        .attr("class")
        .and_then(|classes| {
            classes.split_whitespace().find_map(|class| {
                class
                    .strip_prefix("language-")
                    .or_else(|| class.strip_prefix("lang-"))
            })
        })
        .or_else(|| element.attr("data-language"))
        .filter(|language| !language.is_empty())
}
