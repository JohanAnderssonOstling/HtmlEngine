use html_pipeline::engine::{
    GlyphMetric, GlyphResourceGeneration, GlyphResourceStore, GlyphShaper, LaidOutDocument,
    LayoutConstraints, PreparedDocument, ReadingFragment, ShapeError,
};
use html_pipeline::{GlyphRequest, ShapedGlyph};

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
        Ok(ShapedGlyph::from_metric(
            request.character as u32,
            GlyphMetric::try_new(
                request.character,
                request.font_size / 2.0,
                request.font_size,
                0.0,
            )
            .unwrap(),
        ))
    }
}

fn layout(body: &str, resources: &mut GlyphResourceStore) -> LaidOutDocument<'static> {
    let source = format!("<!doctype html><html><body>{body}</body></html>");
    let document = html_parse::parse_dom_document(&source).unwrap();
    let (document, styles) = html_style::style_document(document, &[]).into_parts();
    let prepared = PreparedDocument::try_new(document, styles).unwrap();
    prepared
        .shape(&mut TestShaper::default(), resources)
        .unwrap()
        .layout(LayoutConstraints::new(600.0, 16.0).unwrap(), resources)
        .expect("layout requires the shaper-aware API for first-line styles")
}

fn text_for_ranges(
    document: &LaidOutDocument,
    fragment: &ReadingFragment,
    resources: &GlyphResourceStore,
) -> String {
    let text = document.render_view(resources).text();
    fragment
        .target
        .glyph_ranges
        .iter()
        .flat_map(|range| range.clone())
        .map(|index| {
            text.glyph_metric(text.glyph_at(index as usize).unwrap())
                .unwrap()
                .ch()
        })
        .collect()
}

#[test]
fn cjk_wrapping_and_segment_break_joining_preserve_utf16_source_positions() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p lang='ja' style='width:16px'>中𠀀\nが文</p>",
        &mut document_resources,
    );
    assert!(
        document
            .render_view(&document_resources)
            .text()
            .line_count()
            > 1
    );
    let view = document.render_view(&document_resources);
    let addressing = view.addressing();
    let fragments = addressing.reading_fragments(0);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].text, "中𠀀が文");
    assert_eq!(
        text_for_ranges(&document, &fragments[0], &document_resources),
        "中𠀀が文"
    );
    let mut offsets = Vec::new();
    for range in &fragments[0].target.glyph_ranges {
        for index in range.clone() {
            let source = addressing.source_position_for_glyph(index, false).unwrap();
            let steps = source
                .elements()
                .map(|element| element.step())
                .collect::<Vec<_>>();
            assert_eq!(
                addressing.resolve_source_position(
                    &steps,
                    Some(source.text_step()),
                    Some(source.utf16_offset()),
                    false
                ),
                Some(index)
            );
            offsets.push(source.utf16_offset());
        }
    }
    assert_eq!(offsets, [0, 1, 4, 5, 6]);
}

#[test]
fn bidi_controls_and_collapsed_newlines_keep_source_positions_in_order() {
    for (source, expected, expected_offsets) in [
        ("中\n\u{200e}文", "中\u{200e}文", vec![0, 2, 3]),
        ("中\u{200e}\n文", "中\u{200e}文", vec![0, 1, 3]),
        ("A\n\u{200e}B", "A \u{200e}B", vec![0, 1, 2, 3]),
    ] {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(&format!("<p>{source}</p>"), &mut document_resources);
        let view = document.render_view(&document_resources);
        let addressing = view.addressing();
        let text = view.text();
        let actual: String = (0..text.glyph_count())
            .map(|index| {
                text.glyph_metric(text.glyph_at(index).unwrap())
                    .unwrap()
                    .ch()
            })
            .collect();
        assert_eq!(actual, expected);
        let offsets: Vec<_> = (0..text.glyph_count())
            .map(|index| {
                let source = addressing
                    .source_position_for_glyph(index as u32, false)
                    .unwrap();
                let steps: Vec<_> = source.elements().map(|element| element.step()).collect();
                assert_eq!(
                    addressing.resolve_source_position(
                        &steps,
                        Some(source.text_step()),
                        Some(source.utf16_offset()),
                        false
                    ),
                    Some(index as u32)
                );
                source.utf16_offset()
            })
            .collect();
        assert_eq!(offsets, expected_offsets);
    }
}

#[test]
fn rtl_reading_and_source_positions_keep_logical_order() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p dir='rtl'>(אבג) <span dir='ltr'>abc 123</span></p>",
        &mut document_resources,
    );
    let view = document.render_view(&document_resources);
    let addressing = view.addressing();
    let fragments = addressing.reading_fragments(0);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].text, "(אבג) abc 123");
    assert_eq!(
        text_for_ranges(&document, &fragments[0], &document_resources),
        "(אבג) abc 123"
    );
    assert_eq!(
        addressing.reading_text(&fragments[0].target),
        fragments[0].text
    );
    for range in &fragments[0].target.glyph_ranges {
        for index in range.clone() {
            let source = addressing.source_position_for_glyph(index, false).unwrap();
            let steps: Vec<_> = source.elements().map(|element| element.step()).collect();
            assert_eq!(
                addressing.resolve_source_position(
                    &steps,
                    Some(source.text_step()),
                    Some(source.utf16_offset()),
                    false
                ),
                Some(index)
            );
        }
    }
}

#[test]
fn hidden_text_is_excluded_from_fragments_and_chunks() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>Visible <span style='visibility:hidden'>SECRET</span> text.</p><p style='visibility:hidden'>Entire hidden paragraph</p>",
        &mut document_resources,
    );
    let addressing = document.render_view(&document_resources).addressing();
    let fragments = addressing.reading_fragments(0);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].text, "Visible text.");
    let (chunk, next) = addressing
        .reading_chunk(&fragments[0].target, 0, 400, |text| Some(0..text.len()))
        .unwrap();
    assert_eq!(chunk.text, "Visible text.");
    assert_eq!(next, None);
    assert!(!text_for_ranges(&document, &chunk, &document_resources).contains("SECRET"));
    assert_eq!(addressing.reading_text(&chunk.target), chunk.text);
}

#[test]
fn visible_descendant_of_hidden_parent_remains_readable() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p style='visibility:hidden'>Hidden <span style='visibility:visible'>Readable</span> hidden</p>",
        &mut document_resources,
    );
    let fragments = document
        .render_view(&document_resources)
        .addressing()
        .reading_fragments(0);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].text, "Readable");
}

#[test]
fn explicit_breaks_separate_words_without_creating_glyphs() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p><br>Hello<br><br><em>world</em><br></p>",
        &mut document_resources,
    );
    let addressing = document.render_view(&document_resources).addressing();
    let fragments = addressing.reading_fragments(0);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].text, "Hello world");
    let (chunk, next) = addressing
        .reading_chunk(&fragments[0].target, 0, 400, |text| Some(0..text.len()))
        .unwrap();
    assert_eq!(chunk.text, "Hello world");
    assert_eq!(next, None);
    assert_eq!(
        text_for_ranges(&document, &chunk, &document_resources),
        "Helloworld"
    );
    assert_eq!(addressing.reading_text(&chunk.target), chunk.text);
}

#[test]
fn chunks_resume_correctly_before_and_after_a_break_separator() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>Hello<br>world</p>", &mut document_resources);
    let addressing = document.render_view(&document_resources).addressing();
    let target = &addressing.reading_targets_in(0..u32::MAX)[0];
    for limit in [5, 6] {
        let (first, next) = addressing
            .reading_chunk(target, 0, limit, |text| {
                Some(
                    0..text
                        .char_indices()
                        .nth(limit)
                        .map_or(text.len(), |(offset, _)| offset),
                )
            })
            .unwrap();
        assert_eq!(first.text.trim(), "Hello");
        assert_eq!(
            text_for_ranges(&document, &first, &document_resources),
            "Hello"
        );
        let next = next.unwrap();
        assert_eq!(next, 5);
        let (second, next) = addressing
            .reading_chunk(target, next, 400, |text| Some(0..text.len()))
            .unwrap();
        assert_eq!(second.text, "world");
        assert_eq!(next, None);
    }
    assert_eq!(addressing.reading_fragments(5)[0].text, "world");
}

#[test]
fn inline_markup_does_not_insert_word_separators() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>Hel<em>lo</em><span>world</span></p>",
        &mut document_resources,
    );
    let addressing = document.render_view(&document_resources).addressing();
    assert_eq!(addressing.reading_fragments(0)[0].text, "Helloworld");
    assert_eq!(addressing.reading_boundary_after_word(3), 10);
}

#[test]
fn paragraph_boundaries_preserve_the_next_word() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>Hello</p><p>World again</p>", &mut document_resources);
    let addressing = document.render_view(&document_resources).addressing();
    let fragments = addressing.reading_fragments(0);
    let second = fragments[1].target.glyph_ranges[0].start;
    assert_eq!(addressing.reading_boundary_after_word(second), second);
    assert_eq!(addressing.reading_fragments(second)[0].text, "World again");
    assert_eq!(addressing.reading_boundary_after_word(2), second);
    assert_eq!(
        addressing.reading_boundary_after_word(second + 2),
        second + 6
    );
}

#[test]
fn explicit_break_boundaries_preserve_the_next_word() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>Hello<br>world again</p>", &mut document_resources);
    let addressing = document.render_view(&document_resources).addressing();
    assert_eq!(addressing.reading_boundary_after_word(5), 5);
    assert_eq!(addressing.reading_boundary_after_word(2), 5);
    assert_eq!(addressing.reading_boundary_after_word(7), 11);
}

#[test]
fn css_block_boundaries_also_separate_words() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>Hello<span style='display:block'>world</span>again</p>",
        &mut document_resources,
    );
    let addressing = document.render_view(&document_resources).addressing();
    assert_eq!(addressing.reading_fragments(0)[0].text, "Hello world again");
    assert_eq!(addressing.reading_boundary_after_word(5), 5);
    assert_eq!(addressing.reading_boundary_after_word(2), 5);
}

#[test]
fn unicode_chunks_and_note_gaps_preserve_source_ranges() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>界界界 <em>ångström</em><a role='doc-noteref'>99</a> cafés<br>界界界.</p>",
        &mut document_resources,
    );
    let addressing = document.render_view(&document_resources).addressing();
    let target = &addressing.reading_targets_in(0..u32::MAX)[0];
    let expected = "界界界 ångström cafés 界界界.";
    assert_eq!(addressing.reading_text(target), expected);
    let mut cursor = 0;
    let mut reconstructed = String::new();
    loop {
        let (chunk, next) = addressing
            .reading_chunk(target, cursor, 7, |text| {
                assert!(text.chars().count() <= 8);
                Some(
                    0..text
                        .char_indices()
                        .nth(7)
                        .map_or(text.len(), |(offset, _)| offset),
                )
            })
            .unwrap();
        assert_eq!(addressing.reading_text(&chunk.target), chunk.text.trim());
        reconstructed.push_str(&chunk.text);
        let Some(next) = next else { break };
        assert!(next > cursor);
        cursor = next;
    }
    assert_eq!(
        reconstructed
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>(),
        expected
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>()
    );
}

#[test]
fn legacy_footnotes_and_reference_markers_are_excluded() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>First<a class='fnanchor' href='#fn1'>[1]</a> sentence. <span class='footnote'>Inline note.</span> Second sentence.</p><div class='extra FOOTNOTES'><h2>Footnotes</h2><p id='fn1'>Nested note body.</p></div><p class='endnote'>Endnote body.</p><p class='note'>Ordinary note in the prose.</p><p>Read <a href='#chapter'>this link</a>.</p>",
        &mut document_resources,
    );
    let addressing = document.render_view(&document_resources).addressing();
    let items = addressing.reading_targets_in(0..u32::MAX);
    let text: Vec<_> = items
        .iter()
        .map(|item| addressing.reading_text(item))
        .collect();
    assert_eq!(
        text,
        [
            "First sentence. Second sentence.",
            "Ordinary note in the prose.",
            "Read this link."
        ]
    );
    let (chunk, next) = addressing
        .reading_chunk(&items[0], 0, 400, |text| Some(0..text.len()))
        .unwrap();
    assert_eq!(chunk.text, text[0]);
    assert_eq!(next, None);
    assert!(chunk.target.glyph_ranges.len() > 1);
    assert_eq!(addressing.reading_text(&chunk.target), chunk.text);
}

#[test]
fn explicit_references_exclude_unmarked_local_note_bodies() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>Reading<a role='doc-noteref' href='#n1'>1</a> continues.</p><div id='n1'><p>Unmarked note body.</p></div><p id='chapter'>Ordinary linked prose.</p><p><a href='#chapter'>Chapter link</a></p>",
        &mut document_resources,
    );
    let addressing = document.render_view(&document_resources).addressing();
    let fragments = addressing.reading_fragments(0);
    assert_eq!(
        fragments
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect::<Vec<_>>(),
        [
            "Reading continues.",
            "Ordinary linked prose.",
            "Chapter link"
        ]
    );
}

#[test]
fn footnote_only_documents_have_no_readable_items() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<section class='footnotes'><h1>Footnotes</h1><p>A note.</p></section><aside role='doc-footnote'><p>Semantic note.</p></aside>",
        &mut document_resources,
    );
    assert!(
        document
            .render_view(&document_resources)
            .addressing()
            .reading_targets_in(0..u32::MAX)
            .is_empty()
    );
}

#[test]
fn navigation_titles_match_whole_labels() {
    for title in [
        "Contents",
        " TABLE OF CONTENTS: ",
        "Brief Contents",
        "Contents at a Glance",
        "List of Figures",
        "Innehållsförteckning",
    ] {
        assert!(
            html_pipeline::engine::is_navigation_section_title(title),
            "{title}"
        );
    }
    for title in [
        "The Contents of the Letter",
        "Contents and Consequences",
        "Chapter One",
        "Introduction",
        "Bibliography",
        "Notes",
        "",
    ] {
        assert!(
            !html_pipeline::engine::is_navigation_section_title(title),
            "{title}"
        );
    }
}

#[test]
fn heading_titles_do_not_infer_narration_exclusions() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<h1>Contents</h1><p>Readable prose.</p><h2>List of Figures</h2><p>More prose.</p>",
        &mut document_resources,
    );
    assert_eq!(
        document
            .render_view(&document_resources)
            .addressing()
            .reading_fragments(0)
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect::<Vec<_>>(),
        [
            "Contents",
            "Readable prose.",
            "List of Figures",
            "More prose."
        ]
    );
}

#[test]
fn hidden_contents_heading_does_not_suppress_visible_prose() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<h1 style='display:none'>Contents</h1><p>Readable prose.</p>",
        &mut document_resources,
    );
    assert_eq!(
        document
            .render_view(&document_resources)
            .addressing()
            .reading_fragments(0)
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect::<Vec<_>>(),
        ["Readable prose."]
    );
}
