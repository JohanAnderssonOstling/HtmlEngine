use html_layout::{
    FontSlant, GlyphId, GlyphMetric, GlyphResourceStore, GlyphShaper, LaidOutDocument,
    LayoutConstraints, PreparedDocument, ReadingFragment, ShapeError,
};

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
        Ok(self
            .0
            .register(GlyphMetric::try_new(ch, size / 2.0, size, 0.0, 0.0).unwrap())?)
    }
}

fn layout(body: &str) -> LaidOutDocument {
    let source = format!("<!doctype html><html><body>{body}</body></html>");
    let document = html_parse::parse_dom_document(&source).unwrap();
    let (document, styles) = html_style::style_document(document, &[]).into_parts();
    let prepared = PreparedDocument::try_new(document, styles).unwrap();
    prepared
        .shape(&mut TestShaper::default())
        .unwrap()
        .layout(LayoutConstraints::new(600.0, 16.0).unwrap())
}

fn text_for_ranges(document: &LaidOutDocument, fragment: &ReadingFragment) -> String {
    let text = document.render_view().text();
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
    let document = layout("<p lang='ja' style='width:16px'>中𠀀\nが文</p>");
    assert!(document.render_view().text().line_count() > 1);
    let view = document.render_view();
    let addressing = view.addressing();
    let fragments = addressing.reading_fragments(0);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].text, "中𠀀が文");
    assert_eq!(text_for_ranges(&document, &fragments[0]), "中𠀀が文");
    let mut offsets = Vec::new();
    for range in &fragments[0].target.glyph_ranges {
        for index in range.clone() {
            let source = addressing.source_position_for_glyph(index, false).unwrap();
            let steps = source.elements().map(|element| element.step()).collect::<Vec<_>>();
            assert_eq!(addressing.resolve_source_position(&steps, Some(source.text_step()), Some(source.utf16_offset()), false), Some(index));
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
        let document = layout(&format!("<p>{source}</p>"));
        let view = document.render_view();
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
    let document = layout("<p dir='rtl'>(אבג) <span dir='ltr'>abc 123</span></p>");
    let view = document.render_view();
    let addressing = view.addressing();
    let fragments = addressing.reading_fragments(0);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].text, "(אבג) abc 123");
    assert_eq!(text_for_ranges(&document, &fragments[0]), "(אבג) abc 123");
    assert_eq!(addressing.reading_text(&fragments[0].target), fragments[0].text);
    for range in &fragments[0].target.glyph_ranges {
        for index in range.clone() {
            let source = addressing.source_position_for_glyph(index, false).unwrap();
            let steps: Vec<_> = source.elements().map(|element| element.step()).collect();
            assert_eq!(addressing.resolve_source_position(&steps, Some(source.text_step()), Some(source.utf16_offset()), false), Some(index));
        }
    }
}

#[test]
fn hidden_text_is_excluded_from_fragments_and_chunks() {
    let document = layout(
        "<p>Visible <span style='visibility:hidden'>SECRET</span> text.</p><p style='visibility:hidden'>Entire hidden paragraph</p>",
    );
    let addressing = document.render_view().addressing();
    let fragments = addressing.reading_fragments(0);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].text, "Visible text.");
    let (chunk, next) = addressing
        .reading_chunk(&fragments[0].target, 0, 400, |text| Some(0..text.len()))
        .unwrap();
    assert_eq!(chunk.text, "Visible text.");
    assert_eq!(next, None);
    assert!(!text_for_ranges(&document, &chunk).contains("SECRET"));
    assert_eq!(addressing.reading_text(&chunk.target), chunk.text);
}

#[test]
fn visible_descendant_of_hidden_parent_remains_readable() {
    let document = layout(
        "<p style='visibility:hidden'>Hidden <span style='visibility:visible'>Readable</span> hidden</p>",
    );
    let fragments = document.render_view().addressing().reading_fragments(0);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].text, "Readable");
}

#[test]
fn explicit_breaks_separate_words_without_creating_glyphs() {
    let document = layout("<p><br>Hello<br><br><em>world</em><br></p>");
    let addressing = document.render_view().addressing();
    let fragments = addressing.reading_fragments(0);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].text, "Hello world");
    let (chunk, next) = addressing
        .reading_chunk(&fragments[0].target, 0, 400, |text| Some(0..text.len()))
        .unwrap();
    assert_eq!(chunk.text, "Hello world");
    assert_eq!(next, None);
    assert_eq!(text_for_ranges(&document, &chunk), "Helloworld");
    assert_eq!(addressing.reading_text(&chunk.target), chunk.text);
}

#[test]
fn chunks_resume_correctly_before_and_after_a_break_separator() {
    let document = layout("<p>Hello<br>world</p>");
    let addressing = document.render_view().addressing();
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
        assert_eq!(text_for_ranges(&document, &first), "Hello");
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
    let document = layout("<p>Hel<em>lo</em><span>world</span></p>");
    let addressing = document.render_view().addressing();
    assert_eq!(addressing.reading_fragments(0)[0].text, "Helloworld");
    assert_eq!(addressing.reading_boundary_after_word(3), 10);
}

#[test]
fn paragraph_boundaries_preserve_the_next_word() {
    let document = layout("<p>Hello</p><p>World again</p>");
    let addressing = document.render_view().addressing();
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
    let document = layout("<p>Hello<br>world again</p>");
    let addressing = document.render_view().addressing();
    assert_eq!(addressing.reading_boundary_after_word(5), 5);
    assert_eq!(addressing.reading_boundary_after_word(2), 5);
    assert_eq!(addressing.reading_boundary_after_word(7), 11);
}

#[test]
fn css_block_boundaries_also_separate_words() {
    let document = layout("<p>Hello<span style='display:block'>world</span>again</p>");
    let addressing = document.render_view().addressing();
    assert_eq!(addressing.reading_fragments(0)[0].text, "Hello world again");
    assert_eq!(addressing.reading_boundary_after_word(5), 5);
    assert_eq!(addressing.reading_boundary_after_word(2), 5);
}

#[test]
fn unicode_chunks_and_note_gaps_preserve_source_ranges() {
    let document =
        layout("<p>界界界 <em>ångström</em><a role='doc-noteref'>99</a> cafés<br>界界界.</p>");
    let addressing = document.render_view().addressing();
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
    let document = layout(
        "<p>First<a class='fnanchor' href='#fn1'>[1]</a> sentence. <span class='footnote'>Inline note.</span> Second sentence.</p><div class='extra FOOTNOTES'><h2>Footnotes</h2><p id='fn1'>Nested note body.</p></div><p class='endnote'>Endnote body.</p><p class='note'>Ordinary note in the prose.</p><p>Read <a href='#chapter'>this link</a>.</p>",
    );
    let addressing = document.render_view().addressing();
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
    let document = layout(
        "<p>Reading<a role='doc-noteref' href='#n1'>1</a> continues.</p><div id='n1'><p>Unmarked note body.</p></div><p id='chapter'>Ordinary linked prose.</p><p><a href='#chapter'>Chapter link</a></p>",
    );
    let addressing = document.render_view().addressing();
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
    let document = layout(
        "<section class='footnotes'><h1>Footnotes</h1><p>A note.</p></section><aside role='doc-footnote'><p>Semantic note.</p></aside>",
    );
    assert!(
        document
            .render_view()
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
        assert!(html_layout::is_navigation_section_title(title), "{title}");
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
        assert!(!html_layout::is_navigation_section_title(title), "{title}");
    }
}

#[test]
fn contents_headings_exclude_the_section_and_resume_at_the_next_peer_heading() {
    let document = layout(
        "<p>Preface prose.</p><h1>Table of <em>Contents</em></h1><p>Chapter One ... 1</p><h2>Part Two</h2><p>Chapter Two ... 12</p><h1>The Contents of the Letter</h1><p>Actual story.</p>",
    );
    let addressing = document.render_view().addressing();
    let fragments = addressing.reading_fragments(0);
    assert_eq!(
        fragments
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect::<Vec<_>>(),
        [
            "Preface prose.",
            "The Contents of the Letter",
            "Actual story."
        ]
    );
    let after_contents_heading = document
        .render_view()
        .text()
        .text_runs()
        .find(|run| {
            addressing
                .reading_text(&html_layout::ReadingTarget {
                    element: 0,
                    glyph_ranges: vec![run.glyphs()],
                })
                .contains("Chapter One")
        })
        .unwrap()
        .glyphs()
        .start;
    assert_eq!(
        addressing
            .reading_fragments(after_contents_heading)
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect::<Vec<_>>(),
        ["The Contents of the Letter", "Actual story."]
    );
}

#[test]
fn contents_exclusion_ends_with_its_structural_section() {
    let document = layout(
        "<section><h2>Contents</h2><ul><li>Chapter One</li></ul></section><section><p>Readable chapter without a heading.</p></section>",
    );
    assert_eq!(
        document
            .render_view()
            .addressing()
            .reading_fragments(0)
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect::<Vec<_>>(),
        ["Readable chapter without a heading."]
    );
}

#[test]
fn hidden_contents_heading_does_not_suppress_visible_prose() {
    let document = layout("<h1 style='display:none'>Contents</h1><p>Readable prose.</p>");
    assert_eq!(
        document
            .render_view()
            .addressing()
            .reading_fragments(0)
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect::<Vec<_>>(),
        ["Readable prose."]
    );
}
