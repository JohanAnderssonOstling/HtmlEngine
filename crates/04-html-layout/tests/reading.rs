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
