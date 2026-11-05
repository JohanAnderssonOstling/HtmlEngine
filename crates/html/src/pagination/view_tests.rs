use html_pipeline::engine::LaidOutDocument;
use html_pipeline::engine::source_queries;
use html_pipeline::engine::ContentRole;
use html_pipeline::engine::{box_queries, text_paint};

use crate::text::{FontMetrics, GlyphRequest, ShapedGlyph};
use html_pipeline::engine::TextExport;
use html_pipeline::engine::TextExportFormat;

use super::*;
use crate::text::TextSearchOptions as Opts;
use crate::text::{FontSlant, GlyphHandle, GlyphResourceGeneration, GlyphShaper};
use html_engine_model::{
    LineScreen, RepeatedTableHeader, VisibleLinePositions, layout_state, page_placements,
};
use html_pipeline::DocumentFactory;
use html_pipeline::engine::GlyphResourceStore;
use html_pipeline::engine::LayoutConstraints;
use html_pipeline::engine::find_text_matches;
use html_pipeline::engine::{export_queries, geometry_queries, search_queries, semantic_queries};
use std::collections::HashMap;

#[derive(Default)]
struct TestShaper {
    glyphs: HashMap<(char, u32), GlyphHandle>,
    generation: GlyphResourceGeneration,
    append_checkpoint: Option<(HashMap<(char, u32), GlyphHandle>, GlyphResourceGeneration)>,
}

impl GlyphShaper for TestShaper {
    fn reset(&mut self) {
        self.glyphs.clear();
        self.generation = GlyphResourceGeneration::default();
    }

    fn resource_generation(&self) -> GlyphResourceGeneration {
        self.generation
    }

    fn begin_append_shaping(&mut self) -> Result<(), crate::text::ShapeError> {
        assert!(self.append_checkpoint.is_none());
        self.append_checkpoint = Some((self.glyphs.clone(), self.generation.clone()));
        Ok(())
    }

    fn commit_append_shaping(&mut self) {
        self.append_checkpoint = None;
    }

    fn rollback_append_shaping(&mut self) {
        if let Some((glyphs, glyph_store)) = self.append_checkpoint.take() {
            self.glyphs = glyphs;
            self.generation = glyph_store;
        }
    }

    fn shape_glyph(
        &mut self,
        request: GlyphRequest<'_>,
    ) -> Result<ShapedGlyph, crate::text::ShapeError> {
        let key = (request.character, request.style.font_size.to_bits());
        let advance = request.style.font_size * 0.5;
        let metrics = FontMetrics {
            ascent: request.style.font_size * 0.75,
            descent: request.style.font_size * 0.25,
        };
        if let Some(&glyph) = self.glyphs.get(&key) {
            return Ok(ShapedGlyph::new(glyph, advance, metrics));
        }
        let glyph = GlyphHandle(self.glyphs.len() as u32);
        self.glyphs.insert(key, glyph);
        Ok(ShapedGlyph::new(glyph, advance, metrics))
    }
}

fn layout(html: &str, resources: &mut GlyphResourceStore) -> LaidOutDocument<'static> {
    let mut factory = DocumentFactory::new();
    let mut shaper = TestShaper::default();
    factory
        .parse_with_new_pipeline(html, None)
        .shape(&mut shaper, resources)
        .expect("the test shaper registers every glyph")
        .layout(
            LayoutConstraints::new(600.0, 16.0).expect("constraints must be valid"),
            resources,
        )
        .expect("test layout succeeds")
}

fn options() -> Opts {
    Opts {
        match_case: false,
        whole_word: false,
        match_diacritics: false,
    }
}

/// The glyph index of the first occurrence of `wanted`.
fn glyph_of(doc: RenderView<'_>, wanted: char) -> u32 {
    let text = doc.text();
    (0..text.glyph_count() as u32)
        .find(|index| {
            text_paint::glyph_at(text, *index as usize)
                .and_then(|glyph| text_paint::glyph_metric(text, glyph))
                .is_some_and(|metric| metric.ch() == wanted)
        })
        .expect("character must be present")
}

#[test]
fn rectangles_cross_offset_breaks_and_optionally_repeat_in_headers() {
    let mut resources = GlyphResourceStore::default();
    let document = layout("<p>text</p>", &mut resources);
    let page = PagePositions {
        geometry: super::super::PageGeometry {
            col_width: 100.0,
            col_gap: 10.0,
            col_count: 2.0,
            size: kurbo::Size::new(210.0, 100.0),
        },
        positions: VisibleLinePositions::default(),
        offset_breaks: vec![(90.0, 10.0)],
        repeated_table_headers: vec![RepeatedTableHeader {
            source_left: 0.0,
            source_right: 50.0,
            source_top: 80.0,
            source_bottom: 90.0,
            col_index: 1,
        }],
        ..PagePositions::default()
    };
    let geometry = PageView::new(document.render_view(&resources), &page);
    let source = Rect::new(5.0, 80.0, 25.0, 120.0);
    let mut projected = Vec::new();
    geometry.project_rect(source, None, false, &mut projected);
    let fragments: Vec<_> = projected
        .iter()
        .map(|fragment| (fragment.source, fragment.destination))
        .collect();
    assert_eq!(
        fragments,
        vec![
            (Rect::new(5.0, 80.0, 25.0, 90.0), Point::new(5.0, 80.0)),
            (Rect::new(5.0, 90.0, 25.0, 120.0), Point::new(115.0, 0.0)),
        ]
    );
    projected.clear();
    geometry.project_rect(source, None, true, &mut projected);
    let repeated: Vec<_> = projected
        .iter()
        .map(|fragment| (fragment.source, fragment.destination))
        .collect();
    assert_eq!(&repeated[..2], fragments.as_slice());
    assert_eq!(
        repeated[2],
        (Rect::new(5.0, 80.0, 25.0, 90.0), Point::new(115.0, 0.0))
    );
}

#[test]
fn line_rectangles_follow_recorded_inline_placement() {
    let mut resources = GlyphResourceStore::default();
    let document = layout("<p>text</p>", &mut resources);
    let doc = (&document).render_view(&resources);
    let source = doc.text().line(LineId::new(0)).unwrap().point();
    let destination = Point::new(125.0, 30.0);
    let mut positions = page_placements::with_buffer(0, Vec::new());
    page_placements::push(
        &mut positions,
        LineId::new(0),
        LineScreen {
            point: destination,
            col_index: 1,
        },
    );
    let page = PagePositions {
        positions,
        ..PagePositions::default()
    };
    let geometry = PageView::new(doc, &page);
    let rect = Rect::from_origin_size(source + (2.0, 3.0), kurbo::Size::new(10.0, 20.0));
    let mut projected = Vec::new();
    geometry.project_rect(rect, Some(LineId::new(0)), false, &mut projected);
    let fragments: Vec<_> = projected
        .iter()
        .map(|fragment| (fragment.source, fragment.destination))
        .collect();
    assert_eq!(fragments, vec![(rect, destination + (2.0, 3.0))]);
}

#[test]
fn unavailable_line_addresses_remain_unavailable() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>Text</p>", &mut document_resources);
    let doc = (&document).render_view(&document_resources);
    assert_eq!(
        source_queries::source_location_on_line(
            doc.addressing(),
            LineId::new(u32::MAX as usize - 1),
            Some(0)
        ),
        None
    );
    assert_eq!(
        source_queries::source_location_on_line(doc.addressing(), LineId::new(0), Some(u32::MAX)),
        None
    );
}

// -- text extraction ----------------------------------------------------

#[test]
fn selection_text_spans_block_boundaries_with_newlines() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>alpha</p><p>beta</p>", &mut document_resources);
    let doc = (&document).render_view(&document_resources);
    let TextExport { plain, .. } = export_queries::render_export_view_text(
        doc.export(),
        0..doc.text().glyph_count() as u32,
        TextExportFormat::PlainAndMarkdown,
    );

    let plain = plain.expect("a non-empty selection yields text");
    assert!(plain.contains("alpha") && plain.contains("beta"));
    assert!(
        plain.contains('\n'),
        "separate blocks must not run together: {plain:?}"
    );
}

#[test]
fn selection_markdown_marks_emphasis_and_headings() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<h2>Title</h2><p><em>soft</em> and <strong>hard</strong></p>",
        &mut document_resources,
    );
    let doc = (&document).render_view(&document_resources);
    let TextExport { markdown, .. } = export_queries::render_export_view_text(
        doc.export(),
        0..doc.text().glyph_count() as u32,
        TextExportFormat::PlainAndMarkdown,
    );

    let markdown = markdown.expect("a non-empty selection yields markdown");
    assert!(
        markdown.starts_with("## "),
        "h2 becomes a level-two heading: {markdown:?}"
    );
    assert!(
        markdown.contains("Title"),
        "the heading text survives: {markdown:?}"
    );
    assert!(
        markdown.contains("*soft*"),
        "em becomes emphasis: {markdown:?}"
    );
    assert!(
        markdown.contains("**hard**"),
        "strong becomes bold: {markdown:?}"
    );
}

#[test]
fn touch_words_span_inline_runs_preserve_unicode_and_stop_at_blocks() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>ca<em>fe\u{301}</em> can't 中文</p><p>next</p>",
        &mut document_resources,
    );
    let doc = (&document).render_view(&document_resources);
    for (character, expected) in [
        ('f', "cafe\u{301}"),
        ('\u{301}', "cafe\u{301}"),
        ('\'', "can't"),
        ('n', "can't"),
    ] {
        let range =
            search_queries::render_text_view_word_range_at(doc.text(), glyph_of(doc, character))
                .unwrap();
        assert_eq!(
            export_queries::render_export_view_text(
                doc.export(),
                range,
                TextExportFormat::PlainAndMarkdown
            )
            .plain
            .unwrap_or_default(),
            expected
        );
    }
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>alpha</p><p>beta</p>", &mut document_resources);
    let doc = (&document).render_view(&document_resources);
    assert_eq!(
        export_queries::render_export_view_text(
            doc.export(),
            search_queries::render_text_view_word_range_at(doc.text(), glyph_of(doc, 'a')).unwrap(),
            TextExportFormat::PlainAndMarkdown
        )
        .plain
        .unwrap_or_default(),
        "alpha"
    );
    assert_eq!(
        export_queries::render_export_view_text(
            doc.export(),
            search_queries::render_text_view_word_range_at(doc.text(), glyph_of(doc, 'b')).unwrap(),
            TextExportFormat::PlainAndMarkdown
        )
        .plain
        .unwrap_or_default(),
        "beta"
    );
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>one two</p>", &mut document_resources);
    let doc = (&document).render_view(&document_resources);
    assert!(
        search_queries::render_text_view_word_range_at(doc.text(), glyph_of(doc, ' ')).is_none()
    );
}

#[test]
fn an_empty_range_yields_no_text() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>alpha</p>", &mut document_resources);
    let doc = (&document).render_view(&document_resources);
    assert_eq!(
        export_queries::render_export_view_text(
            doc.export(),
            3..3,
            TextExportFormat::PlainAndMarkdown
        ),
        TextExport::default()
    );
}

#[test]
fn code_spans_use_delimiters_longer_than_their_contents() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p><code>a`b</code></p>", &mut document_resources);
    let doc = (&document).render_view(&document_resources);
    let TextExport { markdown, .. } = export_queries::render_export_view_text(
        doc.export(),
        0..doc.text().glyph_count() as u32,
        TextExportFormat::PlainAndMarkdown,
    );
    assert_eq!(markdown.as_deref(), Some("``a`b``"));
}

// -- box tree -----------------------------------------------------------

#[test]
fn ancestor_lookup_finds_the_nearest_match_and_stops_at_the_root() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<table><tr><td><p id='cell'>x</p></td></tr></table>",
        &mut document_resources,
    );
    let doc = (&document).render_view(&document_resources);
    let boxes = doc.boxes();
    let paragraph = (0..box_queries::len(boxes))
        .map(layout_state::box_id_new)
        .find(|&i| box_queries::attribute(boxes, i, "id") == Some("cell"))
        .expect("the paragraph is in the box tree");

    let table = semantic_queries::render_semantics_view_nearest(
        semantic_queries::render_view_semantics(doc),
        paragraph,
        ContentRole::Table,
    )
    .expect("the paragraph sits in a table");
    assert!(semantic_queries::render_semantics_view_contains(
        semantic_queries::render_view_semantics(doc),
        table,
        paragraph
    ));
    assert!(
        semantic_queries::render_semantics_view_nearest(
            semantic_queries::render_view_semantics(doc),
            paragraph,
            ContentRole::TableCell
        )
        .is_some()
    );
    assert!(
        semantic_queries::render_semantics_view_nearest(
            semantic_queries::render_view_semantics(doc),
            paragraph,
            ContentRole::Quote
        )
        .is_none(),
        "a tag that is not an ancestor must not match"
    );
}

#[test]
fn a_box_descends_from_itself() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>alpha</p>", &mut document_resources);
    let doc = (&document).render_view(&document_resources);
    assert!(semantic_queries::render_semantics_view_contains(
        semantic_queries::render_view_semantics(doc),
        layout_state::box_id_new(0),
        layout_state::box_id_new(0)
    ));
}

#[test]
fn heading_prefixes_cover_every_level() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<h1>a</h1><h3>b</h3><p>c</p>", &mut document_resources);
    let doc = (&document).render_view(&document_resources);
    let boxes = doc.boxes();
    let tagged = |want: &str| {
        (0..box_queries::len(boxes))
            .map(layout_state::box_id_new)
            .find(|&i| box_queries::tag(boxes, i).is_some_and(|t| t.eq_ignore_ascii_case(want)))
            .expect("tag present")
    };

    assert_eq!(
        export_queries::render_export_view_markdown_for_box(doc.export(), tagged("h1")).as_deref(),
        Some("# a")
    );
    assert_eq!(
        export_queries::render_export_view_markdown_for_box(doc.export(), tagged("h3")).as_deref(),
        Some("### b")
    );
    assert_eq!(
        export_queries::render_export_view_markdown_for_box(doc.export(), tagged("p")).as_deref(),
        Some("c")
    );
}

// -- line lookup --------------------------------------------------------

#[test]
fn start_and_end_line_lookup_agree_on_a_single_line_document() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>alpha</p>", &mut document_resources);
    let doc = (&document).render_view(&document_resources);
    assert_eq!(
        geometry_queries::render_geometry_view_start_spatial_rank_at(
            geometry_queries::render_view_geometry(doc),
            0.0,
            0.1
        ),
        0
    );
    assert_eq!(
        text_paint::render_text_view_line_for_glyph(doc.text(), 0),
        Some(LineId::new(0))
    );
}

#[test]
fn line_lookup_on_an_empty_document_does_not_panic() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("", &mut document_resources);
    let doc = (&document).render_view(&document_resources);
    assert_eq!(
        geometry_queries::render_geometry_view_start_spatial_rank_at(
            geometry_queries::render_view_geometry(doc),
            0.0,
            0.1
        ),
        0
    );
    assert_eq!(
        geometry_queries::render_geometry_view_end_spatial_rank_at(
            geometry_queries::render_view_geometry(doc),
            0.0,
            0.1
        ),
        None
    );
    assert_eq!(
        text_paint::render_text_view_line_for_glyph(doc.text(), 0),
        None
    );
}

#[test]
fn start_line_advances_past_earlier_lines() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>alpha</p><p>beta</p><p>gamma</p>",
        &mut document_resources,
    );
    let doc = (&document).render_view(&document_resources);
    let last = doc.text().lines().len() - 1;
    let last_y = text_paint::render_lines_get(&doc.text().lines(), last)
        .expect("last line")
        .point()
        .y;
    assert_eq!(
        geometry_queries::render_geometry_view_start_spatial_rank_at(
            geometry_queries::render_view_geometry(doc),
            last_y,
            0.1
        ),
        last,
        "an offset at the last line's top selects it"
    );
    assert_eq!(
        geometry_queries::render_geometry_view_start_spatial_rank_at(
            geometry_queries::render_view_geometry(doc),
            0.0,
            0.1
        ),
        0
    );
}

// -- search -------------------------------------------------------------

#[test]
fn matching_is_case_and_diacritic_insensitive_by_default() {
    assert_eq!(
        find_text_matches("Café CAFE cafeteria", "cafe", options().into()).len(),
        3
    );
    assert_eq!(
        find_text_matches(
            "Café CAFE cafeteria",
            "cafe",
            (Opts {
                whole_word: true,
                ..options()
            })
            .into()
        )
        .len(),
        2
    );
    assert_eq!(
        find_text_matches(
            "Café CAFE",
            "cafe",
            (Opts {
                match_case: true,
                ..options()
            })
            .into()
        )
        .len(),
        0
    );
    assert_eq!(
        find_text_matches(
            "Café CAFE",
            "cafe",
            (Opts {
                match_diacritics: true,
                ..options()
            })
            .into()
        )
        .len(),
        1
    );
}

#[test]
fn an_empty_query_matches_nothing() {
    assert!(find_text_matches("alpha", "", options().into()).is_empty());
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>alpha</p>", &mut document_resources);
    assert!(
        search_queries::render_text_view_find_matches(
            (&document).render_view(&document_resources).text(),
            "",
            options().into()
        )
        .is_empty()
    );
}

#[test]
fn document_matching_returns_glyph_ranges_covering_the_query() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>alpha beta alpha</p>", &mut document_resources);
    let doc = (&document).render_view(&document_resources);
    let matches =
        search_queries::render_text_view_find_matches(doc.text(), "alpha", options().into());

    assert_eq!(matches.len(), 2, "both occurrences are found");
    for range in matches {
        let (start, end) = (range.start, range.end);
        assert_eq!(end - start, 5, "each match spans the query length");
        let range = start..end;
        assert_eq!(
            export_queries::render_export_view_text(
                doc.export(),
                range,
                TextExportFormat::PlainAndMarkdown
            )
            .plain
            .unwrap_or_default()
            .to_lowercase(),
            "alpha"
        );
    }
}

#[test]
fn list_markers_are_excluded_from_search() {
    // Browsers exclude generated ::marker content from find-in-page.
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<ol><li>item</li></ol>", &mut document_resources);
    let doc = (&document).render_view(&document_resources);
    assert!(
        search_queries::render_text_view_find_matches(doc.text(), "1", options().into()).is_empty(),
        "the generated list marker must not be searchable"
    );
}

// -- anchors ------------------------------------------------------------

#[test]
fn a_missing_anchor_has_no_target() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p id='here'>alpha</p>", &mut document_resources);
    let doc = (&document).render_view(&document_resources);
    assert!(doc.addressing().element("absent").is_none());
}
