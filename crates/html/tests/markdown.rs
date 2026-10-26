use html::layout::{LaidOutDocument, LayoutConstraints, RenderExportView, TextCompositionPolicy};
use html::pipeline::DocumentFactory;
use html::text::{FontSlant, GlyphId, GlyphMetric, GlyphResourceStore, GlyphShaper, ShapeError};
use std::ops::Range;

#[derive(Default)]
struct TestShaper(GlyphResourceStore);

impl GlyphShaper for TestShaper {
    fn reset(&mut self) {
        self.0.clear();
    }
    fn glyph_resources(&mut self) -> &mut GlyphResourceStore {
        &mut self.0
    }
    fn shape_glyph(
        &mut self,
        ch: char,
        size: f32,
        _: u16,
        _: FontSlant,
        _: u32,
        _: Option<&str>,
    ) -> Result<GlyphId, ShapeError> {
        self.0
            .register(GlyphMetric::try_new(ch, size / 2.0, size, 0.0, 0.0).unwrap())
    }
}

fn layout(body: &str) -> LaidOutDocument {
    DocumentFactory::new()
        .parse_with_new_pipeline(
            &format!("<!doctype html><html><body>{body}</body></html>"),
            None,
        )
        .shape(&mut TestShaper::default())
        .unwrap()
        .layout(LayoutConstraints::new(600.0, 16.0).unwrap())
}

fn all(document: &LaidOutDocument) -> String {
    let view = document.render_view();
    let export: RenderExportView<'_> = view.export();
    export
        .markdown(0..view.text().glyph_count() as u32)
        .unwrap()
}

fn range(document: &LaidOutDocument, wanted: &str) -> Range<u32> {
    let text = document.render_view().text();
    let characters: String = (0..text.glyph_count())
        .map(|index| {
            text.glyph_metric(text.glyph_at(index).unwrap())
                .unwrap()
                .ch()
        })
        .collect();
    let byte = characters.find(wanted).expect("selected text exists");
    let start = characters[..byte].chars().count() as u32;
    start..start + wanted.chars().count() as u32
}

fn box_for(document: &LaidOutDocument, id: &str) -> usize {
    let boxes = document.render_view().boxes();
    (0..boxes.len())
        .find(|&index| boxes.attribute(index, "id") == Some(id))
        .unwrap()
}

#[test]
fn public_api_copies_partial_formatted_text_without_neighbors() {
    let document =
        layout("<p>Before <strong>bold <em>café 😀</em> end</strong> after</p><p>Unselected</p>");
    assert_eq!(
        document
            .render_view()
            .export()
            .markdown(range(&document, "café 😀")),
        Some("***café 😀***".into())
    );
    assert_eq!(
        document
            .render_view()
            .export()
            .markdown(range(&document, "bold café 😀")),
        Some("**bold *café 😀***".into())
    );
}

#[test]
fn headings_links_quotes_and_paragraphs_preserve_structure() {
    let document = layout(
        "<h2>Title</h2><p>A <a href='https://example.org/a(b)?x=1&amp;y=2' title='A &quot;title&quot;'>link</a> and <del>old</del>.</p><blockquote><p>Quoted</p><p>Again</p></blockquote>",
    );
    assert_eq!(
        all(&document),
        "## Title\n\nA [link](https://example.org/a%28b%29?x=1&amp;y=2 \"A &quot;title&quot;\") and ~~old~~.\n\n> Quoted\n> \n> Again"
    );
}

#[test]
fn invisible_and_generated_content_do_not_leak_into_selection() {
    let document = layout(
        "<style>p::before { content: 'Generated'; }</style><p>Visible <span style='display:none'>HIDDEN</span><span style='visibility:hidden'>SECRET</span> tail</p><p style='visibility:hidden'>Hidden <span style='visibility:visible'>Readable</span></p>",
    );
    let markdown = all(&document);
    assert!(markdown.contains("Visible") && markdown.contains("Readable"));
    for hidden in ["HIDDEN", "SECRET", "Generated", "Hidden"] {
        assert!(!markdown.contains(hidden), "{markdown:?}");
    }
}

#[test]
fn literal_markdown_and_entities_are_escaped() {
    let document =
        layout("<p>*literal* [label] &amp;copy; &lt;tag&gt; \\</p><p>1. literal list</p>");
    assert_eq!(
        all(&document),
        "\\*literal\\* \\[label\\] &amp;copy; \\<tag\\> \\\\\n\n1\\. literal list"
    );
}

#[test]
fn inline_and_block_code_use_safe_backtick_delimiters() {
    let document = layout(
        "<p>Use <code>a`b</code> and <code>`x`</code>.</p><pre id='code'><code class='language-rust'>let x = \"```\";\n  x\n</code></pre>",
    );
    assert!(all(&document).starts_with("Use ``a`b`` and `` `x` ``."));
    assert_eq!(
        document
            .render_view()
            .export()
            .markdown_for_box(box_for(&document, "code")),
        Some("````rust\nlet x = \"```\";\n  x\n````".into())
    );
}

#[test]
fn lists_keep_nesting_and_selected_item_numbering() {
    let document =
        layout("<ol start='3'><li>First<ul><li><em>Nested</em></li></ul></li><li>Second</li></ol>");
    assert_eq!(all(&document), "3. First\n    - *Nested*\n\n4. Second");
    assert_eq!(
        document
            .render_view()
            .export()
            .markdown(range(&document, "Second")),
        Some("4. Second".into())
    );
}

#[test]
fn simple_tables_preserve_headers_formatting_pipes_and_data_rows() {
    let document = layout(
        "<table id='table'><tr><th>Name</th><th>Value</th></tr><tr><td><strong>A|B</strong></td><td><code>x|y</code><br>next</td></tr></table>",
    );
    assert_eq!(
        document
            .render_view()
            .export()
            .markdown_for_box(box_for(&document, "table")),
        Some("| Name | Value |\n| --- | --- |\n| **A\\|B** | `x\\|y`<br>next |".into())
    );
    let document = layout("<table id='table'><tr><td>A</td><td>B</td></tr></table>");
    assert_eq!(
        document
            .render_view()
            .export()
            .markdown_for_box(box_for(&document, "table")),
        Some("| | |\n| --- | --- |\n| A | B |".into())
    );
}

#[test]
fn partial_tables_never_include_unselected_text() {
    let document = layout(
        "<table><tr><th>Header</th><th>Other</th></tr><tr><td>Selected</td><td>SECRET</td></tr><tr><td>After</td><td>Hidden</td></tr></table>",
    );
    assert_eq!(
        document
            .render_view()
            .export()
            .markdown(range(&document, "Selected")),
        Some("| | |\n| --- | --- |\n| Selected |  |".into())
    );
}

#[test]
fn merged_and_nested_tables_fall_back_to_selected_structural_html() {
    let document = layout(
        "<table id='table' onclick='bad()'><tr><td colspan='2' style='color:red'>Joined</td></tr><tr><td>One</td><td>SECRET</td></tr></table>",
    );
    let markdown = document
        .render_view()
        .export()
        .markdown_for_box(box_for(&document, "table"))
        .unwrap();
    assert_eq!(
        markdown,
        "<table><tbody><tr><td colspan=\"2\">Joined</td></tr><tr><td>One</td><td>SECRET</td></tr></tbody></table>"
    );
    assert_eq!(
        document
            .render_view()
            .export()
            .markdown(range(&document, "Joined")),
        Some("<table><tbody><tr><td colspan=\"2\">Joined</td></tr></tbody></table>".into())
    );
    let document = layout(
        "<table id='table'><tr><td>Outer<table><tr><td>Inner</td></tr></table></td></tr></table>",
    );
    let markdown = document
        .render_view()
        .export()
        .markdown_for_box(box_for(&document, "table"))
        .unwrap();
    assert_eq!(markdown.matches("<table>").count(), 2);
    assert!(markdown.contains("Outer") && markdown.contains("Inner"));
}

#[test]
fn images_and_explicit_breaks_survive_copying() {
    let document =
        layout("<p>Before<img id='image' src='a(b).png' alt='A [picture]'>after<br>Next</p>");
    assert_eq!(
        all(&document),
        "Before![A \\[picture\\]](a%28b%29.png)after  \nNext"
    );
    assert_eq!(
        document
            .render_view()
            .export()
            .markdown_for_box(box_for(&document, "image")),
        Some("![A \\[picture\\]](a%28b%29.png)".into())
    );
}

#[test]
fn empty_reversed_and_invalid_selections_return_none() {
    let document = layout("<p>Text</p>");
    let export = document.render_view().export();
    for selected in [0..0, 3..1, 0..100, u32::MAX..u32::MAX] {
        assert_eq!(export.markdown(selected), None);
    }
    assert_eq!(export.markdown_for_box(usize::MAX), None);
}

#[test]
fn copy_ignores_reflow_bidi_order_and_discretionary_hyphens() {
    let mut document =
        layout("<p>English <em>אבג</em> hy\u{00ad}phenation words wrap across lines.</p>");
    let initial = all(&document);
    assert_eq!(
        initial,
        "English *אבג* hyphenation words wrap across lines."
    );
    for width in [40.0, 90.0, 600.0] {
        document.relayout(
            LayoutConstraints::new(width, 16.0)
                .unwrap()
                .with_text_composition_policy(TextCompositionPolicy::BookOptimized),
        );
        assert_eq!(all(&document), initial);
    }
}

#[test]
fn plain_and_markdown_payloads_share_the_same_selection() {
    let document = layout("<p>Before <em>first</em><br>second</p><p>Third</p><p>After</p>");
    let start = range(&document, "first").start;
    let end = range(&document, "Third").end;
    let export = document.render_view().export();
    assert_eq!(
        export.selection_text(start..end),
        (
            Some("first\nsecond\nThird".into()),
            Some("*first*  \nsecond\n\nThird".into())
        )
    );
    assert_eq!(export.selection_text(2..2), (None, None));
}

#[test]
fn selected_list_items_and_language_hints_are_exported_independently() {
    let document = layout(
        "<ol start='4'><li>Before</li><li id='item'>Selected<ul><li>Nested</li></ul></li><li>After</li></ol><pre id='code' data-language='rust'>let x = 1;</pre>",
    );
    let export = document.render_view().export();
    assert_eq!(
        export.markdown_for_box(box_for(&document, "item")),
        Some("5. Selected\n    - Nested".into())
    );
    assert_eq!(
        export.markdown_for_box(box_for(&document, "code")),
        Some("```rust\nlet x = 1;\n```".into())
    );
}

#[test]
fn rectangular_table_api_preserves_grid_slots_and_clips_selections() {
    let document = layout(
        "<table id='table'><tr><th colspan='2'>A | B</th></tr><tr><td rowspan='2'>One</td><td>Two &amp; more</td></tr><tr><td>Three</td></tr></table>",
    );
    let table = document
        .render_view()
        .boxes()
        .table(box_for(&document, "table"))
        .unwrap();
    assert_eq!(
        table.markdown(),
        Some("| A \\| B |  |\n| --- | --- |\n| One | Two & more |\n|  | Three |".into())
    );
    assert_eq!(
        table.markdown_in(1..usize::MAX, 1..usize::MAX),
        Some("|  |\n| --- |\n| Two & more |\n| Three |".into())
    );
    assert_eq!(table.markdown_in(0..0, 0..1), None);
    assert_eq!(table.markdown_in(0..1, 3..1), None);
    assert_eq!(table.markdown_in(9..10, 0..1), None);
}

#[test]
fn authored_css_emphasis_survives_copying() {
    let document = layout(
        "<p><span style='font-weight:700'>Bold</span> <span style='font-style:italic'>Italic</span> <span style='text-decoration:line-through'>Deleted</span></p>",
    );
    assert_eq!(all(&document), "**Bold** *Italic* ~~Deleted~~");
}

#[test]
fn granular_code_selections_remain_inline_until_a_block_boundary_is_crossed() {
    let document = layout(
        "<p>Before</p><pre id='code'><code class='language-rust'>let x = 1;</code></pre><p>After</p>",
    );
    let export = document.render_view().export();
    assert_eq!(export.markdown(range(&document, "x")), Some("`x`".into()));
    let start = range(&document, "Before").start;
    let end = range(&document, "x").end;
    assert_eq!(
        export.markdown(start..end),
        Some("Before\n\n```rust\nlet x\n```".into())
    );
    assert_eq!(
        export.markdown_for_box(box_for(&document, "code")),
        Some("```rust\nlet x = 1;\n```".into())
    );
}

#[test]
fn unsupported_list_numbering_keeps_html_list_semantics() {
    let document = layout("<ol start='-3'><li>First</li><li id='item'>Second</li></ol>");
    assert_eq!(
        document
            .render_view()
            .export()
            .markdown_for_box(box_for(&document, "item")),
        Some("<ol start=\"-2\"><li>Second</li></ol>".into())
    );
    let document = layout("<ol id='list' reversed><li>First</li><li>Second</li></ol>");
    assert_eq!(
        document
            .render_view()
            .export()
            .markdown_for_box(box_for(&document, "list")),
        Some("<ol reversed=\"\"><li>First</li><li>Second</li></ol>".into())
    );
}

#[test]
fn captions_and_visible_breaks_remain_bounded_by_the_selection() {
    let document = layout(
        "<table><caption>Caption</caption><tr><td>SECRET</td></tr></table><p>Before<span style='visibility:hidden'>Hidden</span><br>After</p>",
    );
    assert_eq!(
        document
            .render_view()
            .export()
            .markdown(range(&document, "Caption")),
        Some("Caption".into())
    );
    let start = range(&document, "Before").start;
    let end = range(&document, "After").end;
    assert_eq!(
        document.render_view().export().markdown(start..end),
        Some("Before  \nAfter".into())
    );
}

#[test]
fn plain_export_matches_selection_payload_without_markdown() {
    let document = layout("<h2>Title</h2><p><em>Visible</em><span style='visibility:hidden'>Hidden</span> text</p>");
    let export = document.render_view().export();
    let all = 0..document.render_view().text().glyph_count() as u32;
    assert_eq!(export.plain_text(all.clone()), export.selection_text(all).0);
    for invalid in [0..0, 8..2, 0..u32::MAX] {
        assert_eq!(export.plain_text(invalid), None);
    }
}
