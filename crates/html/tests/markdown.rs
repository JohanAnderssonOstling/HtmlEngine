use html::layout::{LaidOutDocument, LayoutConstraints, RenderExportView, TextCompositionPolicy};
use html::pipeline::DocumentFactory;
use html::text::{
    FontSlant, GlyphId, GlyphMetric, GlyphResourceGeneration, GlyphResourceStore, GlyphShaper,
    ShapeError,
};
use std::ops::Range;

#[derive(Default)]
struct TestShaper(GlyphResourceGeneration);

impl GlyphShaper for TestShaper {
    fn reset(&mut self) {
        self.0 = GlyphResourceGeneration::default();
    }
    fn resource_generation(&self) -> GlyphResourceGeneration {
        self.0
    }
    fn shape_glyph(
        &mut self,
        ch: char,
        size: f32,
        _: u16,
        _: FontSlant,
        _: u32,
        _: Option<&str>,
        resources: &mut GlyphResourceStore,
    ) -> Result<GlyphId, ShapeError> {
        resources.register(GlyphMetric::try_new(ch, size / 2.0, size, 0.0, 0.0).unwrap())
    }
}

fn layout(body: &str, resources: &mut GlyphResourceStore) -> LaidOutDocument<'static> {
    DocumentFactory::new()
        .parse_with_new_pipeline(
            &format!("<!doctype html><html><body>{body}</body></html>"),
            None,
        )
        .shape(&mut TestShaper::default(), resources)
        .unwrap()
        .layout(LayoutConstraints::new(600.0, 16.0).unwrap(), resources)
        .expect("layout requires the shaper-aware API for first-line styles")
}

fn all(document: &LaidOutDocument, resources: &GlyphResourceStore) -> String {
    let view = document.render_view(resources);
    let export: RenderExportView<'_> = view.export();
    export
        .markdown(0..view.text().glyph_count() as u32)
        .unwrap()
}

fn range(document: &LaidOutDocument, wanted: &str, resources: &GlyphResourceStore) -> Range<u32> {
    let text = document.render_view(resources).text();
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

fn box_for(document: &LaidOutDocument, id: &str, resources: &GlyphResourceStore) -> usize {
    let boxes = document.render_view(resources).boxes();
    (0..boxes.len())
        .find(|&index| boxes.attribute(index, "id") == Some(id))
        .unwrap()
}

#[test]
fn public_api_copies_partial_formatted_text_without_neighbors() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>Before <strong>bold <em>café 😀</em> end</strong> after</p><p>Unselected</p>",
        &mut document_resources,
    );
    assert_eq!(
        document
            .render_view(&document_resources)
            .export()
            .markdown(range(&document, "café 😀", &document_resources)),
        Some("***café 😀***".into())
    );
    assert_eq!(
        document
            .render_view(&document_resources)
            .export()
            .markdown(range(&document, "bold café 😀", &document_resources)),
        Some("**bold *café 😀***".into())
    );
}

#[test]
fn headings_links_quotes_and_paragraphs_preserve_structure() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<h2>Title</h2><p>A <a href='https://example.org/a(b)?x=1&amp;y=2' title='A &quot;title&quot;'>link</a> and <del>old</del>.</p><blockquote><p>Quoted</p><p>Again</p></blockquote>",
        &mut document_resources,
    );
    assert_eq!(
        all(&document, &document_resources),
        "## Title\n\nA [link](https://example.org/a%28b%29?x=1&amp;y=2 \"A &quot;title&quot;\") and ~~old~~.\n\n> Quoted\n> \n> Again"
    );
}

#[test]
fn invisible_and_generated_content_do_not_leak_into_selection() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<style>p::before { content: 'Generated'; }</style><p>Visible <span style='display:none'>HIDDEN</span><span style='visibility:hidden'>SECRET</span> tail</p><p style='visibility:hidden'>Hidden <span style='visibility:visible'>Readable</span></p>",
        &mut document_resources,
    );
    let markdown = all(&document, &document_resources);
    assert!(markdown.contains("Visible") && markdown.contains("Readable"));
    for hidden in ["HIDDEN", "SECRET", "Generated", "Hidden"] {
        assert!(!markdown.contains(hidden), "{markdown:?}");
    }
}

#[test]
fn literal_markdown_and_entities_are_escaped() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>*literal* [label] &amp;copy; &lt;tag&gt; \\</p><p>1. literal list</p>",
        &mut document_resources,
    );
    assert_eq!(
        all(&document, &document_resources),
        "\\*literal\\* \\[label\\] &amp;copy; \\<tag\\> \\\\\n\n1\\. literal list"
    );
}

#[test]
fn inline_and_block_code_use_safe_backtick_delimiters() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>Use <code>a`b</code> and <code>`x`</code>.</p><pre id='code'><code class='language-rust'>let x = \"```\";\n  x\n</code></pre>",
        &mut document_resources,
    );
    assert!(all(&document, &document_resources).starts_with("Use ``a`b`` and `` `x` ``."));
    assert_eq!(
        document
            .render_view(&document_resources)
            .export()
            .markdown_for_box(box_for(&document, "code", &document_resources)),
        Some("````rust\nlet x = \"```\";\n  x\n````".into())
    );
}

#[test]
fn lists_keep_nesting_and_selected_item_numbering() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<ol start='3'><li>First<ul><li><em>Nested</em></li></ul></li><li>Second</li></ol>",
        &mut document_resources,
    );
    assert_eq!(
        all(&document, &document_resources),
        "3. First\n    - *Nested*\n\n4. Second"
    );
    assert_eq!(
        document
            .render_view(&document_resources)
            .export()
            .markdown(range(&document, "Second", &document_resources)),
        Some("4. Second".into())
    );
}

#[test]
fn simple_tables_preserve_headers_formatting_pipes_and_data_rows() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<table id='table'><tr><th>Name</th><th>Value</th></tr><tr><td><strong>A|B</strong></td><td><code>x|y</code><br>next</td></tr></table>",
        &mut document_resources,
    );
    assert_eq!(
        document
            .render_view(&document_resources)
            .export()
            .markdown_for_box(box_for(&document, "table", &document_resources)),
        Some("| Name | Value |\n| --- | --- |\n| **A\\|B** | `x\\|y`<br>next |".into())
    );
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<table id='table'><tr><td>A</td><td>B</td></tr></table>",
        &mut document_resources,
    );
    assert_eq!(
        document
            .render_view(&document_resources)
            .export()
            .markdown_for_box(box_for(&document, "table", &document_resources)),
        Some("| | |\n| --- | --- |\n| A | B |".into())
    );
}

#[test]
fn partial_tables_never_include_unselected_text() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<table><tr><th>Header</th><th>Other</th></tr><tr><td>Selected</td><td>SECRET</td></tr><tr><td>After</td><td>Hidden</td></tr></table>",
        &mut document_resources,
    );
    assert_eq!(
        document
            .render_view(&document_resources)
            .export()
            .markdown(range(&document, "Selected", &document_resources)),
        Some("| | |\n| --- | --- |\n| Selected |  |".into())
    );
}

#[test]
fn merged_and_nested_tables_fall_back_to_selected_structural_html() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<table id='table' onclick='bad()'><tr><td colspan='2' style='color:red'>Joined</td></tr><tr><td>One</td><td>SECRET</td></tr></table>",
        &mut document_resources,
    );
    let markdown = document
        .render_view(&document_resources)
        .export()
        .markdown_for_box(box_for(&document, "table", &document_resources))
        .unwrap();
    assert_eq!(
        markdown,
        "<table><tbody><tr><td colspan=\"2\">Joined</td></tr><tr><td>One</td><td>SECRET</td></tr></tbody></table>"
    );
    assert_eq!(
        document
            .render_view(&document_resources)
            .export()
            .markdown(range(&document, "Joined", &document_resources)),
        Some("<table><tbody><tr><td colspan=\"2\">Joined</td></tr></tbody></table>".into())
    );
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<table id='table'><tr><td>Outer<table><tr><td>Inner</td></tr></table></td></tr></table>",
        &mut document_resources,
    );
    let markdown = document
        .render_view(&document_resources)
        .export()
        .markdown_for_box(box_for(&document, "table", &document_resources))
        .unwrap();
    assert_eq!(markdown.matches("<table>").count(), 2);
    assert!(markdown.contains("Outer") && markdown.contains("Inner"));
}

#[test]
fn images_and_explicit_breaks_survive_copying() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>Before<img id='image' src='a(b).png' alt='A [picture]'>after<br>Next</p>",
        &mut document_resources,
    );
    assert_eq!(
        all(&document, &document_resources),
        "Before![A \\[picture\\]](a%28b%29.png)after  \nNext"
    );
    assert_eq!(
        document
            .render_view(&document_resources)
            .export()
            .markdown_for_box(box_for(&document, "image", &document_resources)),
        Some("![A \\[picture\\]](a%28b%29.png)".into())
    );
}

#[test]
fn empty_reversed_and_invalid_selections_return_none() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>Text</p>", &mut document_resources);
    let export = document.render_view(&document_resources).export();
    for selected in [0..0, 3..1, 0..100, u32::MAX..u32::MAX] {
        assert_eq!(export.markdown(selected), None);
    }
    assert_eq!(export.markdown_for_box(usize::MAX), None);
}

#[test]
fn copy_ignores_reflow_bidi_order_and_discretionary_hyphens() {
    let mut document_resources = GlyphResourceStore::default();
    let mut document = layout(
        "<p>English <em>אבג</em> hy\u{00ad}phenation words wrap across lines.</p>",
        &mut document_resources,
    );
    let initial = all(&document, &document_resources);
    assert_eq!(
        initial,
        "English *אבג* hyphenation words wrap across lines."
    );
    for width in [40.0, 90.0, 600.0] {
        document
            .relayout(
                LayoutConstraints::new(width, 16.0)
                    .unwrap()
                    .with_text_composition_policy(TextCompositionPolicy::BookOptimized),
                &document_resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles");
        assert_eq!(all(&document, &document_resources), initial);
    }
}

#[test]
fn plain_and_markdown_payloads_share_the_same_selection() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>Before <em>first</em><br>second</p><p>Third</p><p>After</p>",
        &mut document_resources,
    );
    let start = range(&document, "first", &document_resources).start;
    let end = range(&document, "Third", &document_resources).end;
    let export = document.render_view(&document_resources).export();
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
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<ol start='4'><li>Before</li><li id='item'>Selected<ul><li>Nested</li></ul></li><li>After</li></ol><pre id='code' data-language='rust'>let x = 1;</pre>",
        &mut document_resources,
    );
    let export = document.render_view(&document_resources).export();
    assert_eq!(
        export.markdown_for_box(box_for(&document, "item", &document_resources)),
        Some("5. Selected\n    - Nested".into())
    );
    assert_eq!(
        export.markdown_for_box(box_for(&document, "code", &document_resources)),
        Some("```rust\nlet x = 1;\n```".into())
    );
}

#[test]
fn rectangular_table_api_preserves_grid_slots_and_clips_selections() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<table id='table'><tr><th colspan='2'>A | B</th></tr><tr><td rowspan='2'>One</td><td>Two &amp; more</td></tr><tr><td>Three</td></tr></table>",
        &mut document_resources,
    );
    let table = document
        .render_view(&document_resources)
        .boxes()
        .table(box_for(&document, "table", &document_resources))
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
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p><span style='font-weight:700'>Bold</span> <span style='font-style:italic'>Italic</span> <span style='text-decoration:line-through'>Deleted</span></p>",
        &mut document_resources,
    );
    assert_eq!(
        all(&document, &document_resources),
        "**Bold** *Italic* ~~Deleted~~"
    );
}

#[test]
fn granular_code_selections_remain_inline_until_a_block_boundary_is_crossed() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>Before</p><pre id='code'><code class='language-rust'>let x = 1;</code></pre><p>After</p>",
        &mut document_resources,
    );
    let export = document.render_view(&document_resources).export();
    assert_eq!(
        export.markdown(range(&document, "x", &document_resources)),
        Some("`x`".into())
    );
    let start = range(&document, "Before", &document_resources).start;
    let end = range(&document, "x", &document_resources).end;
    assert_eq!(
        export.markdown(start..end),
        Some("Before\n\n```rust\nlet x\n```".into())
    );
    assert_eq!(
        export.markdown_for_box(box_for(&document, "code", &document_resources)),
        Some("```rust\nlet x = 1;\n```".into())
    );
}

#[test]
fn unsupported_list_numbering_keeps_html_list_semantics() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<ol start='-3'><li>First</li><li id='item'>Second</li></ol>",
        &mut document_resources,
    );
    assert_eq!(
        document
            .render_view(&document_resources)
            .export()
            .markdown_for_box(box_for(&document, "item", &document_resources)),
        Some("<ol start=\"-2\"><li>Second</li></ol>".into())
    );
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<ol id='list' reversed><li>First</li><li>Second</li></ol>",
        &mut document_resources,
    );
    assert_eq!(
        document
            .render_view(&document_resources)
            .export()
            .markdown_for_box(box_for(&document, "list", &document_resources)),
        Some("<ol reversed=\"\"><li>First</li><li>Second</li></ol>".into())
    );
}

#[test]
fn captions_and_visible_breaks_remain_bounded_by_the_selection() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<table><caption>Caption</caption><tr><td>SECRET</td></tr></table><p>Before<span style='visibility:hidden'>Hidden</span><br>After</p>",
        &mut document_resources,
    );
    assert_eq!(
        document
            .render_view(&document_resources)
            .export()
            .markdown(range(&document, "Caption", &document_resources)),
        Some("Caption".into())
    );
    let start = range(&document, "Before", &document_resources).start;
    let end = range(&document, "After", &document_resources).end;
    assert_eq!(
        document
            .render_view(&document_resources)
            .export()
            .markdown(start..end),
        Some("Before  \nAfter".into())
    );
}

#[test]
fn plain_export_matches_selection_payload_without_markdown() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<h2>Title</h2><p><em>Visible</em><span style='visibility:hidden'>Hidden</span> text</p>",
        &mut document_resources,
    );
    let export = document.render_view(&document_resources).export();
    let all = 0..document
        .render_view(&document_resources)
        .text()
        .glyph_count() as u32;
    assert_eq!(export.plain_text(all.clone()), export.selection_text(all).0);
    for invalid in [0..0, 8..2, 0..u32::MAX] {
        assert_eq!(export.plain_text(invalid), None);
    }
}
