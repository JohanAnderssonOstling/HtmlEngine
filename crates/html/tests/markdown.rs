use html_pipeline::engine::text_paint;
use html_engine_model::layout_state;
use html_pipeline::engine::box_queries;
use html_pipeline::engine::export_queries;
use crate::export::TableExportFormat;
use html_pipeline::engine::TableExportRequest;
use html_pipeline::engine::TextExport;
use html_pipeline::engine::TextExportFormat;
use crate::layout::BoxId;
use html_pipeline::engine::{LaidOutDocument, LayoutConstraints};
use crate::layout::{RenderExportView, TextCompositionPolicy};
use html_pipeline::DocumentFactory;
use crate::text::{GlyphResourceGeneration, GlyphShaper, ShapeError};
use html_pipeline::engine::GlyphResourceStore;
use crate::text::{GlyphRequest, FontMetrics, GlyphHandle, ShapedGlyph};
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
    fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
        Ok(ShapedGlyph::new(
            GlyphHandle(request.character as u32),
            request.style.font_size / 2.0,
            FontMetrics {
                ascent: request.style.font_size,
                descent: 0.0,
            },
        ))
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
    export_queries::render_export_view_text(export, 0..view.text().glyph_count() as u32, TextExportFormat::Markdown).markdown
        .unwrap()
}

fn range(document: &LaidOutDocument, wanted: &str, resources: &GlyphResourceStore) -> Range<u32> {
    let text = document.render_view(resources).text();
    let characters: String = (0..text.glyph_count())
        .map(|index| {
            text_paint::glyph_metric(text, text_paint::glyph_at(text, index).unwrap())
                .unwrap()
                .ch()
        })
        .collect();
    let byte = characters.find(wanted).expect("selected text exists");
    let start = characters[..byte].chars().count() as u32;
    start..start + wanted.chars().count() as u32
}

fn box_for(document: &LaidOutDocument, id: &str, resources: &GlyphResourceStore) -> BoxId {
    let boxes = document.render_view(resources).boxes();
    (0..box_queries::len(boxes))
        .map(layout_state::box_id_new)
        .find(|&index| box_queries::attribute(boxes, index, "id") == Some(id))
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
        export_queries::render_export_view_text(document
            .render_view(&document_resources)
            .export(), range(&document, "café 😀", &document_resources), TextExportFormat::Markdown).markdown,
        Some("***café 😀***".into())
    );
    assert_eq!(
        export_queries::render_export_view_text(document
            .render_view(&document_resources)
            .export(), range(&document, "bold café 😀", &document_resources), TextExportFormat::Markdown).markdown,
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
        export_queries::render_export_view_markdown_for_box(document
            .render_view(&document_resources)
            .export(), box_for(&document, "code", &document_resources)),
        Some("````rust\nlet x = \"```\";\n  x\n````".into())
    );
}

#[test]
fn code_text_keeps_authored_newlines_while_plain_exports_separate_blocks() {
    let mut resources = GlyphResourceStore::default();
    let document = layout(
        "<pre id='code'><code><div>\nA\n\n</div><div>B<br>C\n</div></code></pre><p>After</p>",
        &mut resources,
    );
    let view = document.render_view(&resources);
    let export = view.export();
    assert_eq!(
        export_queries::render_export_view_markdown_for_box(export, box_for(&document, "code", &resources)),
        Some("```\n\nA\n\nB\nC\n```".into())
    );
    let all = 0..view.text().glyph_count() as u32;
    assert_eq!(
        export_queries::render_export_view_text(export, all.clone(), TextExportFormat::Plain).plain.as_deref(),
        Some("A\nB\nC\nAfter")
    );
    assert_eq!(
        export_queries::render_export_view_text(export, all, TextExportFormat::PlainWithParagraphBreaks).plain.as_deref(),
        Some("A\n\nB\nC\n\nAfter")
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
        export_queries::render_export_view_text(document
            .render_view(&document_resources)
            .export(), range(&document, "Second", &document_resources), TextExportFormat::Markdown).markdown,
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
        export_queries::render_export_view_markdown_for_box(document
            .render_view(&document_resources)
            .export(), box_for(&document, "table", &document_resources)),
        Some("| Name | Value |\n| --- | --- |\n| **A\\|B** | `x\\|y`<br>next |".into())
    );
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<table id='table'><tr><td>A</td><td>B</td></tr></table>",
        &mut document_resources,
    );
    assert_eq!(
        export_queries::render_export_view_markdown_for_box(document
            .render_view(&document_resources)
            .export(), box_for(&document, "table", &document_resources)),
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
        export_queries::render_export_view_text(document
            .render_view(&document_resources)
            .export(), range(&document, "Selected", &document_resources), TextExportFormat::Markdown).markdown,
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
    let markdown = export_queries::render_export_view_markdown_for_box(document
        .render_view(&document_resources)
        .export(), box_for(&document, "table", &document_resources))
        .unwrap();
    assert_eq!(
        markdown,
        "<table><tbody><tr><td colspan=\"2\">Joined</td></tr><tr><td>One</td><td>SECRET</td></tr></tbody></table>"
    );
    assert_eq!(
        export_queries::render_export_view_text(document
            .render_view(&document_resources)
            .export(), range(&document, "Joined", &document_resources), TextExportFormat::Markdown).markdown,
        Some("<table><tbody><tr><td colspan=\"2\">Joined</td></tr></tbody></table>".into())
    );
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<table id='table'><tr><td>Outer<table><tr><td>Inner</td></tr></table></td></tr></table>",
        &mut document_resources,
    );
    let markdown = export_queries::render_export_view_markdown_for_box(document
        .render_view(&document_resources)
        .export(), box_for(&document, "table", &document_resources))
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
        export_queries::render_export_view_markdown_for_box(document
            .render_view(&document_resources)
            .export(), box_for(&document, "image", &document_resources)),
        Some("![A \\[picture\\]](a%28b%29.png)".into())
    );
}

#[test]
fn empty_reversed_and_invalid_selections_return_none() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>Text</p>", &mut document_resources);
    let export = document.render_view(&document_resources).export();
    for selected in [0..0, 3..1, 0..100, u32::MAX..u32::MAX] {
        assert_eq!(export_queries::render_export_view_text(export, selected, TextExportFormat::Markdown).markdown, None);
    }
    assert_eq!(export_queries::render_export_view_markdown_for_box(export, layout_state::box_id_new(u32::MAX as usize)), None);
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
        export_queries::render_export_view_text(export, start..end, TextExportFormat::PlainAndMarkdown),
        TextExport {
            plain: Some("first\nsecond\nThird".into()),
            markdown: Some("*first*  \nsecond\n\nThird".into()),
        }
    );
    assert_eq!(export_queries::render_export_view_text(export, 2..2, TextExportFormat::PlainAndMarkdown), TextExport::default());
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
        export_queries::render_export_view_markdown_for_box(export, box_for(&document, "item", &document_resources)),
        Some("5. Selected\n    - Nested".into())
    );
    assert_eq!(
        export_queries::render_export_view_markdown_for_box(export, box_for(&document, "code", &document_resources)),
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
    let table = box_queries::render_box_view_table(document
        .render_view(&document_resources)
        .boxes(), box_for(&document, "table", &document_resources))
        .unwrap();
    assert_eq!(
        table.export(TableExportRequest::Whole(TableExportFormat::Markdown)),
        Some("| A \\| B |  |\n| --- | --- |\n| One | Two & more |\n|  | Three |".into())
    );
    assert_eq!(
        table.export(TableExportRequest::CellsMarkdown { rows: 1..usize::MAX, columns: 1..usize::MAX }),
        Some("|  |\n| --- |\n| Two & more |\n| Three |".into())
    );
    assert_eq!(table.export(TableExportRequest::CellsMarkdown { rows: 0..0, columns: 0..1 }), None);
    assert_eq!(table.export(TableExportRequest::CellsMarkdown { rows: 0..1, columns: 3..1 }), None);
    assert_eq!(table.export(TableExportRequest::CellsMarkdown { rows: 9..10, columns: 0..1 }), None);
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
        export_queries::render_export_view_text(export, range(&document, "x", &document_resources), TextExportFormat::Markdown).markdown,
        Some("`x`".into())
    );
    let start = range(&document, "Before", &document_resources).start;
    let end = range(&document, "x", &document_resources).end;
    assert_eq!(
        export_queries::render_export_view_text(export, start..end, TextExportFormat::Markdown).markdown,
        Some("Before\n\n```rust\nlet x\n```".into())
    );
    assert_eq!(
        export_queries::render_export_view_markdown_for_box(export, box_for(&document, "code", &document_resources)),
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
        export_queries::render_export_view_markdown_for_box(document
            .render_view(&document_resources)
            .export(), box_for(&document, "item", &document_resources)),
        Some("<ol start=\"-2\"><li>Second</li></ol>".into())
    );
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<ol id='list' reversed><li>First</li><li>Second</li></ol>",
        &mut document_resources,
    );
    assert_eq!(
        export_queries::render_export_view_markdown_for_box(document
            .render_view(&document_resources)
            .export(), box_for(&document, "list", &document_resources)),
        Some("<ol reversed=\"\"><li>First</li><li>Second</li></ol>".into())
    );
    for (attributes, item_value, expected) in [
        ("reversed", "", 1),
        ("reversed start='8'", "", 7),
        ("start='3'", "value='9'", 10),
        ("reversed", "value='9'", 8),
    ] {
        let mut resources = GlyphResourceStore::default();
        let document = layout(
            &format!("<ol {attributes}><li {item_value}>First</li><li id='item'>Second</li></ol>"),
            &mut resources,
        );
        assert_eq!(
            export_queries::render_export_view_markdown_for_box(document
                .render_view(&resources)
                .export(), box_for(&document, "item", &resources)),
            Some(format!("{expected}. Second"))
        );
    }
}

#[test]
fn captions_and_visible_breaks_remain_bounded_by_the_selection() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<table><caption>Caption</caption><tr><td>SECRET</td></tr></table><p>Before<span style='visibility:hidden'>Hidden</span><br>After</p>",
        &mut document_resources,
    );
    assert_eq!(
        export_queries::render_export_view_text(document
            .render_view(&document_resources)
            .export(), range(&document, "Caption", &document_resources), TextExportFormat::Markdown).markdown,
        Some("Caption".into())
    );
    let start = range(&document, "Before", &document_resources).start;
    let end = range(&document, "After", &document_resources).end;
    assert_eq!(
        export_queries::render_export_view_text(document
            .render_view(&document_resources)
            .export(), start..end, TextExportFormat::Markdown).markdown,
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
    assert_eq!(export_queries::render_export_view_text(export, all.clone(), TextExportFormat::Plain).plain, export_queries::render_export_view_text(export, all, TextExportFormat::PlainAndMarkdown).plain);
    for invalid in [0..0, 8..2, 0..u32::MAX] {
        assert_eq!(export_queries::render_export_view_text(export, invalid, TextExportFormat::Plain).plain, None);
    }
}
