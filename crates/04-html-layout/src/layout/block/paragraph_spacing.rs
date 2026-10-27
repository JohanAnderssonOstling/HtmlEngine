use crate::layout_model::{Children, InlineContent, InlineItemKind, LayoutMode, LayoutTree};
use html_dom::Document;
use html_style_model::{
    ComputedStyles, Display, Float, PositionMode, TextAlign, UsedPreferredSize, UsedStyleView,
};

const BEFORE_MASK: u8 = 3;
const BEFORE_ZERO: u8 = 1;
const BEFORE_GAP: u8 = 2;
const AFTER_ZERO: u8 = 4;
const PROSE: u8 = 8;

#[derive(Clone, Copy, Default)]
struct ProseEdges {
    first: Option<usize>,
    last: Option<usize>,
}

/// One byte per box when paragraph controls are active; Book needs no storage.
/// Overrides belong to prose blocks and adjoining edges, never to computed CSS.
#[derive(Default)]
pub(in crate::layout) struct ParagraphSpacing {
    gap: Option<f64>,
    indent: Option<f64>,
    edges: Vec<u8>,
}

impl ParagraphSpacing {
    pub(in crate::layout) fn new(
        document: &Document,
        styles: &ComputedStyles,
        topology: &LayoutTree,
        content: &InlineContent,
        boxes: &[UsedStyleView<'_>],
    ) -> Self {
        if styles.reader_paragraph_spacing_em().is_none()
            && styles.reader_first_line_indent_em().is_none()
        {
            return Self::default();
        }
        let pixels = |em: f32| f64::from(em) * f64::from(document.root_font_size());
        let mut spacing = Self {
            gap: styles.reader_paragraph_spacing_em().map(pixels),
            indent: styles.reader_first_line_indent_em().map(pixels),
            edges: vec![0; boxes.len()],
        };
        let mut excluded = vec![false; boxes.len()];
        for index in 0..boxes.len() {
            let parent_excluded = topology
                .get_box_parent(index)
                .is_some_and(|parent| excluded[parent]);
            let element = topology
                .box_at(index)
                .and_then(|layout_box| layout_box.get_element(document));
            excluded[index] = parent_excluded
                || element.is_some_and(|element| {
                    matches!(
                        element.tag(),
                        "h1" | "h2"
                            | "h3"
                            | "h4"
                            | "h5"
                            | "h6"
                            | "hgroup"
                            | "ul"
                            | "ol"
                            | "li"
                            | "menu"
                            | "dl"
                            | "dt"
                            | "dd"
                            | "table"
                            | "thead"
                            | "tbody"
                            | "tfoot"
                            | "tr"
                            | "td"
                            | "th"
                            | "caption"
                            | "pre"
                            | "figure"
                            | "figcaption"
                            | "nav"
                            | "header"
                            | "footer"
                    ) || ["class", "epub:type"].iter().any(|attribute| {
                        element.attr(attribute).is_some_and(|value| {
                            value.split_ascii_whitespace().any(|token| {
                                matches!(
                                    token,
                                    "verse"
                                        | "poem"
                                        | "poetry"
                                        | "stanza"
                                        | "z3998:poem"
                                        | "z3998:verse"
                                        | "z3998:stanza"
                                )
                            })
                        })
                    }) || element.attr("role").is_some_and(|role| {
                        role.split_ascii_whitespace()
                            .any(|role| matches!(role, "heading" | "list" | "listitem" | "table"))
                    })
                })
                || boxes[index].white_space().preserves_spaces()
                || boxes[index].display() != Display::Block
                || boxes[index].float() != Float::None
                || boxes[index].position() == PositionMode::Absolute;
        }

        let mut prose = vec![ProseEdges::default(); boxes.len()];
        // Topology stores parents before children. Process descendants first
        // so plain wrappers can expose their first/last prose block.
        for index in (0..boxes.len()).rev() {
            if excluded[index] {
                continue;
            }
            let runs = match topology.get_box_layout_mode(index) {
                Some(LayoutMode::Anonymous(runs)) => Some(runs),
                Some(LayoutMode::Block(block)) => match &block.children {
                    Children::InlineItems(runs) => Some(runs),
                    _ => None,
                },
                _ => None,
            };
            if let Some(runs) = runs {
                // Centered title paragraphs and punctuation-only scene breaks
                // are not ordinary prose, even when encoded as <p>.
                if boxes[index].text_align() != TextAlign::Center
                    && content.inline_items()[runs.start as usize..runs.end as usize]
                        .iter()
                        .any(|run| {
                            let InlineItemKind::Text { glyphs } = &run.kind else {
                                return false;
                            };
                            if !owns_run(topology, index, run.box_idx as usize) {
                                return false;
                            }
                            glyphs.clone().any(|glyph| {
                                content
                                    .character_at(glyph as usize)
                                    .and_then(char::from_u32)
                                    .is_some_and(char::is_alphanumeric)
                            })
                        })
                {
                    spacing.edges[index] |= PROSE;
                    prose[index] = ProseEdges {
                        first: Some(index),
                        last: Some(index),
                    };
                }
                continue;
            }
            let Some(LayoutMode::Block(block)) = topology.get_box_layout_mode(index) else {
                continue;
            };
            let Children::Blocks(children) = &block.children else {
                continue;
            };
            let mut first = None;
            let mut last = None;
            let mut previous: Option<usize> = None;
            let mut has_child = false;
            for &child in children {
                let child = child as usize;
                if boxes[child].position() == PositionMode::Absolute {
                    continue;
                }
                if !has_child {
                    first = prose[child].first;
                    has_child = true;
                }
                if spacing.gap.is_some()
                    && let Some(previous) = previous
                    && let Some(before) = prose[previous].last
                    && let Some(after) = prose[child].first
                {
                    spacing.clear_after(topology, before, previous);
                    spacing.set_before(topology, after, child);
                }
                last = prose[child].last;
                previous = Some(child);
            }
            let style = boxes[index];
            // Only open, ordinary wrappers expose edges to their siblings.
            // Borders, padding and fixed heights keep their authored separation.
            if style.padding_top().is_zero()
                && style.padding_bottom().is_zero()
                && style.border_top_width() == 0.0
                && style.border_bottom_width() == 0.0
                && style.height() == UsedPreferredSize::Auto
                && style.min_height() == UsedPreferredSize::Auto
            {
                prose[index] = ProseEdges { first, last };
            }
        }
        spacing
    }

    fn clear_after(&mut self, topology: &LayoutTree, mut leaf: usize, outer: usize) {
        loop {
            self.edges[leaf] |= AFTER_ZERO;
            if leaf == outer {
                break;
            }
            let Some(parent) = topology.get_box_parent(leaf) else {
                break;
            };
            leaf = parent;
        }
    }

    fn set_before(&mut self, topology: &LayoutTree, mut leaf: usize, outer: usize) {
        loop {
            self.edges[leaf] = (self.edges[leaf] & !BEFORE_MASK)
                | if leaf == outer {
                    BEFORE_GAP
                } else {
                    BEFORE_ZERO
                };
            if leaf == outer {
                break;
            }
            let Some(parent) = topology.get_box_parent(leaf) else {
                break;
            };
            leaf = parent;
        }
    }

    pub(in crate::layout) fn indent(&self, box_idx: usize) -> Option<f64> {
        self.is_prose(box_idx).then_some(self.indent).flatten()
    }

    pub(in crate::layout) fn gap(&self, box_idx: usize) -> Option<f64> {
        self.is_prose(box_idx).then_some(self.gap).flatten()
    }

    pub(in crate::layout) fn is_prose(&self, box_idx: usize) -> bool {
        self.edges
            .get(box_idx)
            .is_some_and(|flags| flags & PROSE != 0)
    }

    pub(in crate::layout) fn resolve(&self, box_idx: usize, before: f64, after: f64) -> (f64, f64) {
        let flags = self.edges.get(box_idx).copied().unwrap_or(0);
        let before = match flags & BEFORE_MASK {
            BEFORE_ZERO => 0.0,
            BEFORE_GAP => self.gap.unwrap_or(before),
            _ => before,
        };
        (before, if flags & AFTER_ZERO != 0 { 0.0 } else { after })
    }
}

fn owns_run(topology: &LayoutTree, container: usize, mut owner: usize) -> bool {
    loop {
        if owner == container {
            return true;
        }
        if !matches!(
            topology.get_box_layout_mode(owner),
            Some(LayoutMode::Inline(_))
        ) {
            return false;
        }
        let Some(parent) = topology.get_box_parent(owner) else {
            return false;
        };
        owner = parent;
    }
}

#[cfg(test)]
mod tests {
    use crate::GlyphResourceStore;

    use crate::test_support::TestGlyphShaper;
    use crate::{LaidOutDocument, LayoutConstraints, PreparedDocument, ReaderStyleOverrides};
    use html_dom::RootFontSize;

    fn layout(
        source: &str,
        base_size: f32,
        spacing: Option<f32>,
        resources: &mut GlyphResourceStore,
    ) -> LaidOutDocument<'static> {
        layout_controls(source, base_size, spacing, None, 1200.0, resources)
    }

    fn layout_controls(
        source: &str,
        base_size: f32,
        spacing: Option<f32>,
        indent: Option<f32>,
        width: f64,
        resources: &mut GlyphResourceStore,
    ) -> LaidOutDocument<'static> {
        layout_controls_with_css(source, base_size, spacing, indent, width, &[], resources)
    }

    fn layout_controls_with_css(
        source: &str,
        base_size: f32,
        spacing: Option<f32>,
        indent: Option<f32>,
        width: f64,
        css: &[&str],
        resources: &mut GlyphResourceStore,
    ) -> LaidOutDocument<'static> {
        let mut document = html_parse::parse_dom_document(source).unwrap();
        document.set_root_font_size(RootFontSize::new(base_size).unwrap());
        let (document, book) = html_style::style_document(document, css).into_parts();
        let styles = book
            .with_reader_overrides(
                &document,
                &ReaderStyleOverrides {
                    paragraph_spacing_em: spacing,
                    first_line_indent_em: indent,
                    ..Default::default()
                },
            )
            .unwrap();
        PreparedDocument::try_new(document, styles)
            .unwrap()
            .shape_and_layout(
                LayoutConstraints::new(width, f64::from(base_size)).unwrap(),
                &mut TestGlyphShaper::new(),
                resources,
            )
            .expect("test glyphs shape and lay out")
    }

    fn line_geometry(
        document: &LaidOutDocument,
        resources: &GlyphResourceStore,
    ) -> Vec<(f64, f64, f64)> {
        document
            .render_view(resources)
            .text()
            .lines()
            .iter()
            .map(|line| (line.point().x, line.point().y, line.height()))
            .collect()
    }

    fn assert_gap(
        document: &LaidOutDocument,
        before: usize,
        after: usize,
        expected: f64,
        resources: &GlyphResourceStore,
    ) {
        let geometry = line_geometry(document, resources);
        let actual = geometry[after].1 - geometry[before].1 - geometry[before].2;
        assert!(
            (actual - expected).abs() < 1e-4,
            "gap {before} -> {after}: expected {expected}, actual {actual}; {geometry:?}"
        );
    }

    #[test]
    fn reader_paragraph_spacing_covers_the_prologue_span_and_oversized_wrapper() {
        // Mirrors the EPUB structure around the selected Casden paragraph;
        // narrative text is replaced with a short, synthetic fixture.
        let source = "<html><body style='margin:0;font-size:1.29167em;line-height:1.2'><div>\
            <span style='font-size:0.77419em'>Opening passage.</span>\
            <p style='margin:1em 0'><span style='font-size:0.77419em'>Casden took a deep breath.</span></p>\
            <p style='margin:1em 0'><span style='font-size:0.77419em'>Following passage.</span></p>\
            </div></body></html>";
        for base in [16.0, 24.0, 32.0] {
            for em in [0.0, 0.5, 1.0] {
                let mut document_resources = GlyphResourceStore::default();
                let document = layout(source, base, Some(em), &mut document_resources);
                assert_eq!(document.line_count(), 3);
                assert_gap(&document, 0, 1, f64::from(base * em), &document_resources);
                assert_gap(&document, 1, 2, f64::from(base * em), &document_resources);
            }
            let mut book_resources = GlyphResourceStore::default();
            let book = layout(source, base, None, &mut book_resources);
            assert_gap(&book, 0, 1, f64::from(base) * 1.29167, &book_resources);
            assert_gap(&book, 1, 2, f64::from(base) * 1.29167, &book_resources);
        }
    }

    #[test]
    fn reader_paragraph_spacing_handles_anonymous_text_between_paragraphs() {
        let source = "<html><body style='margin:0;line-height:1'><p style='margin:30px 0'>First.</p>\
            <span>Middle <em>styled</em> text.</span><p style='margin:40px 0'>Last.</p></body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(source, 24.0, Some(0.5), &mut document_resources);
        assert_eq!(document.line_count(), 3);
        assert_gap(&document, 0, 1, 12.0, &document_resources);
        assert_gap(&document, 1, 2, 12.0, &document_resources);
    }

    #[test]
    fn reader_paragraph_spacing_crosses_plain_wrappers_without_doubling_margins() {
        let source = "<html><body style='margin:0;line-height:1'>\
            <div style='margin:40px 0'><p style='margin:50px 0'>First.</p></div>\
            <section style='margin:60px 0'><div><span>Second.</span><p style='margin:70px 0'>Third.</p></div></section>\
            <p style='margin:80px 0'>Fourth.</p></body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(source, 24.0, Some(0.5), &mut document_resources);
        assert_eq!(document.line_count(), 4);
        for index in 0..3 {
            assert_gap(&document, index, index + 1, 12.0, &document_resources);
        }
    }

    #[test]
    fn reader_paragraph_spacing_preserves_headings_lists_tables_and_scene_breaks() {
        let mut comparison_resources_15865 = GlyphResourceStore::default();

        let mut comparison_resources_15804 = GlyphResourceStore::default();

        let source = "<html><body style='margin:0;line-height:1'>\
            <h1>Heading</h1><p>Body paragraph.</p>\
            <ul><li><p>List one.</p><p>List two.</p></li></ul>\
            <table><tr><td><p>Cell one.</p><p>Cell two.</p></td></tr></table>\
            <p style='text-align:center'>Chapter title</p><p>After title.</p>\
            <p style='margin:3em 0'>* * *</p><p>After scene break.</p>\
            <pre>Code block.</pre><p>After code.</p>\
            </body></html>";
        assert_eq!(
            line_geometry(
                &layout(source, 24.0, Some(0.5), &mut comparison_resources_15804),
                &comparison_resources_15804
            ),
            line_geometry(
                &layout(source, 24.0, None, &mut comparison_resources_15865),
                &comparison_resources_15865
            )
        );
    }

    #[test]
    fn reader_paragraph_spacing_preserves_internal_line_breaks_and_padded_containers() {
        let mut comparison_resources_16339 = GlyphResourceStore::default();

        let mut comparison_resources_16278 = GlyphResourceStore::default();

        let source = "<html><body style='margin:0;line-height:1'>\
            <p>First<br/>Second</p><div style='padding:12px;border:1px solid'><p>Inside.</p></div>\
            <p>After container.</p></body></html>";
        assert_eq!(
            line_geometry(
                &layout(source, 24.0, Some(0.5), &mut comparison_resources_16278),
                &comparison_resources_16278
            ),
            line_geometry(
                &layout(source, 24.0, None, &mut comparison_resources_16339),
                &comparison_resources_16339
            )
        );
    }
    #[test]
    fn reader_indentation_covers_divs_and_anonymous_prose_in_reader_units() {
        let source = "<html><body style='margin:0;font-size:1.29167em;line-height:1;text-indent:20px'>            <span>Anonymous opening.</span><p style='margin:0'><span style='font-size:.77419em'>Paragraph.</span></p>            <div>Div paragraph.</div><span>Anonymous ending.</span></body></html>";
        for base in [16.0, 24.0, 32.0] {
            for indent in [0.0, 0.5, 1.5] {
                let mut document_resources = GlyphResourceStore::default();
                let document = layout_controls(
                    source,
                    base,
                    None,
                    Some(indent),
                    1200.0,
                    &mut document_resources,
                );
                let lines = line_geometry(&document, &document_resources);
                assert_eq!(lines.len(), 4);
                for line in &lines {
                    assert!(
                        (line.0 - f64::from(base * indent)).abs() < 1e-4,
                        "{lines:?}"
                    );
                }
            }
            let mut book_resources = GlyphResourceStore::default();
            let book = layout_controls(source, base, None, None, 1200.0, &mut book_resources);
            let mut spacing_only_resources = GlyphResourceStore::default();
            let spacing_only = layout_controls(
                source,
                base,
                Some(0.5),
                None,
                1200.0,
                &mut spacing_only_resources,
            );
            assert_eq!(
                line_geometry(&book, &book_resources)
                    .iter()
                    .map(|line| line.0)
                    .collect::<Vec<_>>(),
                line_geometry(&spacing_only, &spacing_only_resources)
                    .iter()
                    .map(|line| line.0)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn reader_double_breaks_use_paragraph_gap_and_reapply_custom_indentation() {
        for source in [
            "<html><body style='margin:0;line-height:1'>First paragraph.<br><br>Second paragraph.<br><br>Third paragraph.</body></html>",
            "<html><body style='margin:0;line-height:1'><div><span>First paragraph.<br> <br>Second paragraph.</span><br><br>Third paragraph.</div></body></html>",
        ] {
            for base in [16.0, 24.0] {
                for gap in [0.0, 0.5, 1.0] {
                    let mut document_resources = GlyphResourceStore::default();
                    let document = layout_controls(
                        source,
                        base,
                        Some(gap),
                        Some(1.0),
                        1200.0,
                        &mut document_resources,
                    );
                    let lines = line_geometry(&document, &document_resources);
                    assert_eq!(lines.len(), 3, "{lines:?}");
                    assert_gap(&document, 0, 1, f64::from(base * gap), &document_resources);
                    assert_gap(&document, 1, 2, f64::from(base * gap), &document_resources);
                    assert!(
                        lines
                            .iter()
                            .all(|line| (line.0 - f64::from(base)).abs() < 1e-4),
                        "{lines:?}"
                    );
                }
                let mut indent_only_resources = GlyphResourceStore::default();
                let indent_only = layout_controls(
                    source,
                    base,
                    None,
                    Some(1.0),
                    1200.0,
                    &mut indent_only_resources,
                );
                assert_gap(&indent_only, 0, 1, f64::from(base), &indent_only_resources);
                assert_gap(&indent_only, 1, 2, f64::from(base), &indent_only_resources);
            }
        }
    }

    #[test]
    fn reader_double_breaks_preserve_book_indentation_and_single_breaks() {
        let source = "<html><body style='margin:0;line-height:1;text-indent:20px'>First.<br>Continued.<br><br>Second.</body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_controls(
            source,
            24.0,
            Some(0.5),
            None,
            1200.0,
            &mut document_resources,
        );
        let lines = line_geometry(&document, &document_resources);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].0, 20.0);
        assert_eq!(lines[1].0, 0.0);
        assert_eq!(lines[2].0, 0.0);
        assert_gap(&document, 0, 1, 0.0, &document_resources);
        assert_gap(&document, 1, 2, 12.0, &document_resources);
        let each_line = source.replace("text-indent:20px", "text-indent:20px each-line");
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_controls(
            &each_line,
            24.0,
            Some(0.5),
            None,
            1200.0,
            &mut document_resources,
        );
        assert!(
            line_geometry(&document, &document_resources)
                .iter()
                .all(|line| line.0 == 20.0)
        );
    }

    #[test]
    fn reader_double_breaks_preserve_long_runs_leading_trailing_breaks_and_verse() {
        let mut comparison_resources_20990 = GlyphResourceStore::default();

        let mut comparison_resources_20924 = GlyphResourceStore::default();

        for content in [
            "First.<br><br><br>Second.",
            "First.<br><br>* * *<br><br>Second.",
            "<br><br>First.<br><br>",
            "<div class='poem'>First.<br><br>Second.</div>",
            "<div epub:type='z3998:verse'>First.<br><br>Second.</div>",
            "<div style='white-space:pre-wrap'>First.<br><br>Second.</div>",
            "<p style='text-align:center'>Chapter<br><br>One</p>",
            "<div><span style='float:left;width:12px;height:12px'>X</span>First.<br><br>Second.</div>",
            "<div>First.<span style='position:absolute'>Note</span><br><br>Second.</div>",
        ] {
            let source =
                format!("<html><body style='margin:0;line-height:1'>{content}</body></html>");
            assert_eq!(
                line_geometry(
                    &layout(&source, 24.0, Some(0.5), &mut comparison_resources_20924),
                    &comparison_resources_20924
                ),
                line_geometry(
                    &layout(&source, 24.0, None, &mut comparison_resources_20990),
                    &comparison_resources_20990
                ),
                "{content}"
            );
        }
    }

    #[test]
    fn reader_double_breaks_only_apply_first_line_pseudo_to_the_original_first_line() {
        let source = "<html><body style='margin:0;line-height:1'><p style='margin:0'>First.<br><br>Second.</p></body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_controls_with_css(
            source,
            16.0,
            Some(0.5),
            Some(1.0),
            1200.0,
            &["p::first-line {font-size:32px;color:red}"],
            &mut document_resources,
        );
        let lines = line_geometry(&document, &document_resources);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].2, 32.0);
        assert_eq!(lines[1].2, 16.0);
        assert_gap(&document, 0, 1, 8.0, &document_resources);
    }
    #[test]
    fn reader_indentation_preserves_headings_lists_tables_and_explicit_verse() {
        let mut comparison_resources_22317 = GlyphResourceStore::default();

        let source = "<html><body style='margin:0;text-indent:12px'>            <h1>Heading</h1><ul><li>List</li></ul><table><tr><td>Table</td></tr></table>            <div class='poem'>First.<br><br>Second.</div>            <div style='white-space:pre-wrap'>Preformatted text.</div>            </body></html>";
        let mut book_resources = GlyphResourceStore::default();
        let book = line_geometry(
            &layout_controls(source, 24.0, None, None, 1200.0, &mut book_resources),
            &book_resources,
        );
        for indent in [0.0, 1.0] {
            assert_eq!(
                line_geometry(
                    &layout_controls(
                        source,
                        24.0,
                        Some(0.5),
                        Some(indent),
                        1200.0,
                        &mut comparison_resources_22317
                    ),
                    &comparison_resources_22317
                ),
                book
            );
        }
    }
}
