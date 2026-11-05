use html_pipeline::engine::box_queries;
use html_engine_model::layout_state;
use html_engine_model::shaping_contracts;
use html_pipeline::engine::text_paint;
use crate::layout_model::BoxId;
use crate::layout_model::LineId;
use crate::parser::DocumentFactory;
use crate::test_support::TestGlyphShaper;
use crate::{GlyphRequest, ShapedGlyph, ShapedRun};
use crate::{
    GlyphResourceGeneration, GlyphResourceStore, GlyphShaper, LaidOutDocument, LayoutConstraints,
    ShapeError, ShapedTextRun, TextDirection, TextRunId, TextRunShapeRequest,
};
use std::sync::Arc;

fn layout(html: &str, width: f64, resources: &mut GlyphResourceStore) -> LaidOutDocument<'static> {
    DocumentFactory::new()
        .parse_with_new_pipeline(html, None)
        .shape(&mut TestGlyphShaper::new(), resources)
        .unwrap()
        .layout(LayoutConstraints::new(width, 16.0).unwrap(), resources)
        .expect("layout requires the shaper-aware API for first-line styles")
}

fn visual_text(document: &LaidOutDocument, line: LineId, resources: &GlyphResourceStore) -> String {
    let text = document.render_view(resources).text();
    let mut fragments: Vec<_> = text_paint::render_text_view_line_text_fragments(text, line)
        .unwrap()
        .collect();
    fragments.sort_by(|a, b| {
        text_paint::fragment_offset_x(&a).total_cmp(&text_paint::fragment_offset_x(&b))
    });
    fragments
        .into_iter()
        .flat_map(|fragment| fragment.glyphs())
        .filter_map(|index| {
            let glyph = text_paint::paint_glyph_at(text, index)?;
            Some(text_paint::glyph_metric(text, glyph)?.ch())
        })
        .collect()
}

#[test]
fn rtl_paragraph_reorders_text_without_changing_source_order() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<body style='margin:0'><p dir='rtl' style='margin:0'>אבג</p></body>",
        100.0,
        &mut document_resources,
    );
    let text = document.render_view(&document_resources).text();
    let logical: String = text_paint::render_line_glyphs(&text
        .line(LineId::new(0))
        .unwrap())
        .map(|index| {
            text_paint::glyph_metric(text, text_paint::glyph_at(text, index as usize).unwrap())
                .unwrap()
                .ch()
        })
        .collect();
    assert_eq!(logical, "אבג");
    assert_eq!(
        visual_text(&document, LineId::new(0), &document_resources),
        "גבא"
    );
    assert!(text.line(LineId::new(0)).unwrap().point().x > 50.0);
}

#[test]
fn rtl_keeps_numbers_and_latin_runs_in_ltr_order() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p dir='rtl'>אבג 123 abc</p>",
        300.0,
        &mut document_resources,
    );
    assert_eq!(
        visual_text(&document, LineId::new(0), &document_resources),
        "abc 123 גבא"
    );
}

#[test]
fn rtl_also_reorders_embedded_text_in_an_ltr_paragraph() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p>abc אבג def</p>", 300.0, &mut document_resources);
    assert_eq!(
        visual_text(&document, LineId::new(0), &document_resources),
        "abc גבא def"
    );
}

#[test]
fn rtl_mirrors_brackets_without_changing_logical_characters() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout("<p dir='rtl'>(אבג)</p>", 300.0, &mut document_resources);
    assert_eq!(
        visual_text(&document, LineId::new(0), &document_resources),
        "(גבא)"
    );
}

#[test]
fn rtl_html_override_reverses_latin_text() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p>A<bdo dir='rtl'>abc</bdo>Z</p>",
        300.0,
        &mut document_resources,
    );
    assert_eq!(
        visual_text(&document, LineId::new(0), &document_resources),
        "AcbaZ"
    );
}

#[test]
fn rtl_block_override_applies_to_anonymous_text_but_does_not_inherit() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<div style='direction:rtl;unicode-bidi:bidi-override'>abc<p>DEF</p>ghi</div>",
        300.0,
        &mut document_resources,
    );
    assert_eq!(
        (0..document.line_count())
            .map(LineId::new)
            .map(|line| visual_text(&document, line, &document_resources))
            .collect::<Vec<_>>(),
        ["cba", "DEF", "ihg"]
    );
}

#[test]
fn rtl_auto_direction_ignores_nested_directional_elements() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p dir='auto'><bdi>abc</bdi> אבג</p>",
        300.0,
        &mut document_resources,
    );
    assert_eq!(
        visual_text(&document, LineId::new(0), &document_resources),
        "גבא abc"
    );
}

#[test]
fn rtl_wraps_in_logical_order_and_reorders_each_line() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<body style='margin:0'><p dir='rtl' style='margin:0'>אבג דהו זחט</p></body>",
        40.0,
        &mut document_resources,
    );
    assert_eq!(document.line_count(), 3);
    assert_eq!(
        (0..3)
            .map(LineId::new)
            .map(|line| visual_text(&document, line, &document_resources))
            .collect::<Vec<_>>(),
        ["גבא", "והד", "טחז"]
    );
}

#[test]
fn rtl_preserves_combining_marks_with_their_base() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p dir='rtl'>ב א\u{05b0}</p>",
        300.0,
        &mut document_resources,
    );
    assert_eq!(
        visual_text(&document, LineId::new(0), &document_resources),
        "א\u{05b0} ב"
    );
}

#[test]
fn rtl_directional_controls_have_no_advance_with_letter_spacing() {
    let mut plain_resources = GlyphResourceStore::default();
    let plain = layout(
        "<body style='margin:0'><p dir='rtl' style='margin:0;letter-spacing:2px'>אב</p></body>",
        100.0,
        &mut plain_resources,
    );
    let mut control_resources = GlyphResourceStore::default();
    let control = layout(
        "<body style='margin:0'><p dir='rtl' style='margin:0;letter-spacing:2px'>א\u{200f}ב</p></body>",
        100.0,
        &mut control_resources,
    );
    assert_eq!(
        visual_text(&plain, LineId::new(0), &plain_resources),
        visual_text(&control, LineId::new(0), &control_resources)
    );
    assert_eq!(
        plain
            .render_view(&plain_resources)
            .text()
            .line(LineId::new(0))
            .unwrap()
            .point()
            .x,
        control
            .render_view(&control_resources)
            .text()
            .line(LineId::new(0))
            .unwrap()
            .point()
            .x
    );
}

#[test]
fn rtl_out_of_flow_anchors_do_not_split_visual_runs() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<p dir='rtl'>אב<span style='position:absolute'>X</span>גד</p>",
        300.0,
        &mut document_resources,
    );
    let text = document.render_view(&document_resources).text();
    let line = text_paint::render_text_view_spatial_order(text)
        .iter()
        .copied()
        .find(|&line| {
            text_paint::render_line_glyphs(&text.line(line).unwrap()).any(|index| {
                text_paint::glyph_metric(text, text_paint::glyph_at(text, index as usize).unwrap())
                    .unwrap()
                    .ch()
                    == 'א'
            })
        })
        .unwrap();
    assert_eq!(visual_text(&document, line, &document_resources), "דגבא");
}

#[test]
fn rtl_plaintext_uses_each_paragraphs_first_strong_direction() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<body style='margin:0'><pre style='margin:0;unicode-bidi:plaintext'>אבג\nabc</pre></body>",
        100.0,
        &mut document_resources,
    );
    assert_eq!(
        visual_text(&document, LineId::new(0), &document_resources),
        "גבא"
    );
    assert_eq!(
        visual_text(&document, LineId::new(1), &document_resources),
        "abc"
    );
    let text = document.render_view(&document_resources).text();
    assert!(text.line(LineId::new(0)).unwrap().point().x > 50.0);
    assert_eq!(text.line(LineId::new(1)).unwrap().point().x, 0.0);
}

#[test]
fn rtl_indent_and_unexpanded_justification_align_from_the_right() {
    let mut plain_resources = GlyphResourceStore::default();
    let plain = layout(
        "<body style='margin:0'><p dir='rtl' style='margin:0'>אבג</p></body>",
        100.0,
        &mut plain_resources,
    );
    let mut indented_resources = GlyphResourceStore::default();
    let indented = layout(
        "<body style='margin:0'><p dir='rtl' style='margin:0;text-indent:10px'>אבג</p></body>",
        100.0,
        &mut indented_resources,
    );
    let mut justified_resources = GlyphResourceStore::default();
    let justified = layout(
        "<body style='margin:0'><p dir='rtl' style='margin:0;text-align:justify'>אבג</p></body>",
        100.0,
        &mut justified_resources,
    );
    let x = plain
        .render_view(&plain_resources)
        .text()
        .line(LineId::new(0))
        .unwrap()
        .point()
        .x;
    assert!(
        (indented
            .render_view(&indented_resources)
            .text()
            .line(LineId::new(0))
            .unwrap()
            .point()
            .x
            - (x - 10.0))
            .abs()
            < 0.001
    );
    assert!(
        (justified
            .render_view(&justified_resources)
            .text()
            .line(LineId::new(0))
            .unwrap()
            .point()
            .x
            - x)
            .abs()
            < 0.001
    );
}

#[test]
fn rtl_overflow_ellipsis_is_placed_at_the_left_end() {
    for direction in ["direction:rtl", "unicode-bidi:plaintext"] {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(
            &format!(
                "<body style='margin:0'><p style='margin:0;{direction};white-space:nowrap;overflow:hidden;text-overflow:ellipsis'>אבגדהוזחט</p></body>"
            ),
            40.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let marker = text_paint::ellipsis_for_line(text, LineId::new(0)).unwrap();
        let leftmost = text_paint::render_text_view_line_text_fragments(text, LineId::new(0))
            .unwrap()
            .map(|fragment| text_paint::fragment_offset_x(&fragment))
            .fold(f64::INFINITY, f64::min);
        assert!(marker.offset().x < leftmost, "{direction}");
    }
}

fn geometry(
    document: &LaidOutDocument,
    id: &str,
    resources: &GlyphResourceStore,
) -> (kurbo::Point, kurbo::Size) {
    let view = document.render_view(resources);
    let boxes = view.boxes();
    let index = (0..box_queries::len(boxes))
        .map(layout_state::box_id_new)
        .find(|&index| box_queries::attribute(boxes, index, "id") == Some(id))
        .unwrap();
    (
        box_queries::render_box_view_point(boxes, index).unwrap(),
        box_queries::render_box_view_size(boxes, index).unwrap(),
    )
}

#[test]
fn rtl_flex_and_grid_place_the_first_item_on_the_right() {
    for display in ["flex", "grid"] {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(
            &format!(
                "<div style='display:{display};direction:rtl;width:100px;grid-template-columns:20px 20px'><div id='a' style='width:20px;height:10px'></div><div id='b' style='width:20px;height:10px'></div></div>"
            ),
            300.0,
            &mut document_resources,
        );
        assert!(
            geometry(&document, "a", &document_resources).0.x
                > geometry(&document, "b", &document_resources).0.x,
            "{display}"
        );
    }
}

#[test]
fn rtl_physical_alignment_and_self_direction_remain_distinct() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<div id='left' style='display:flex;direction:rtl;width:100px;justify-content:left'><div id='a' style='width:20px;height:10px'></div></div><div id='self' style='display:grid;direction:rtl;width:100px;grid-template-columns:100px'><div id='b' style='direction:ltr;justify-self:self-start;width:20px;height:10px'></div></div>",
        300.0,
        &mut document_resources,
    );
    assert!(
        (geometry(&document, "a", &document_resources).0.x
            - geometry(&document, "left", &document_resources).0.x)
            .abs()
            < 0.001
    );
    assert!(
        (geometry(&document, "b", &document_resources).0.x
            - geometry(&document, "self", &document_resources).0.x)
            .abs()
            < 0.001
    );
}

#[test]
fn rtl_inline_padding_consumes_the_corresponding_physical_edges() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<body style='margin:0'><p dir='rtl' style='margin:0'><span style='padding-left:3px;padding-right:11px;border-left:2px solid;border-right:4px solid'>אבג</span></p></body>",
        100.0,
        &mut document_resources,
    );
    let text = document.render_view(&document_resources).text();
    let left = text_paint::render_text_view_line_text_fragments(text, LineId::new(0))
        .unwrap()
        .map(|fragment| text_paint::fragment_offset_x(&fragment))
        .fold(f64::INFINITY, f64::min);
    assert!((left - 5.0).abs() < 0.001, "physical left padding: {left}");
    let right = text_paint::render_text_view_line_text_fragments(text, LineId::new(0))
        .unwrap()
        .map(|fragment| text_paint::fragment_offset_x(&fragment) + 8.0)
        .fold(0.0, f64::max);
    assert!((100.0 - (text.line(LineId::new(0)).unwrap().point().x + right) - 15.0).abs() < 0.001);
}

#[test]
fn rtl_tables_reverse_columns_and_preserve_colspan_widths() {
    for collapse in ["separate", "collapse"] {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout(
            &format!(
                "<table style='direction:rtl;border-collapse:{collapse};border-spacing:3px;width:120px;table-layout:fixed'><tr><td id='a'>A</td><td id='b'>B</td><td id='c'>C</td></tr><tr><td id='span' colspan='2'>D</td><td>E</td></tr></table>"
            ),
            300.0,
            &mut document_resources,
        );
        let (a, _) = geometry(&document, "a", &document_resources);
        let (b, b_size) = geometry(&document, "b", &document_resources);
        let (c, _) = geometry(&document, "c", &document_resources);
        let (span, span_size) = geometry(&document, "span", &document_resources);
        assert!(a.x > b.x && b.x > c.x, "{collapse}: {a:?} {b:?} {c:?}");
        assert!((span.x - b.x).abs() < 0.001);
        assert!(span_size.width > b_size.width * 1.9);
    }
}

#[test]
fn rtl_outside_list_marker_hangs_to_the_right() {
    let mut document_resources = GlyphResourceStore::default();
    let document = layout(
        "<ul dir='rtl'><li id='item' style='border:2px solid;padding:4px'>אבג</li></ul>",
        300.0,
        &mut document_resources,
    );
    let view = document.render_view(&document_resources);
    let boxes = view.boxes();
    let item = (0..box_queries::len(boxes))
        .map(layout_state::box_id_new)
        .find(|&index| box_queries::attribute(boxes, index, "id") == Some("item"))
        .unwrap();
    let marker = box_queries::list_marker(boxes, item).unwrap().marker_box();
    let (point, size) = geometry(&document, "item", &document_resources);
    assert!(box_queries::render_box_view_point(boxes, marker).unwrap().x > point.x + size.width);
}

#[derive(Default)]
struct DirectionalShaper {
    fallback: TestGlyphShaper,
    requests: Vec<(String, TextDirection)>,
    checkpoint: Option<usize>,
}

impl GlyphShaper for DirectionalShaper {
    fn reset(&mut self) {
        self.fallback.reset();
        self.requests.clear();
        self.checkpoint = None;
    }
    fn begin_append_shaping(&mut self) -> Result<(), ShapeError> {
        self.fallback.begin_append_shaping()?;
        self.checkpoint = Some(self.requests.len());
        Ok(())
    }
    fn commit_append_shaping(&mut self) {
        self.fallback.commit_append_shaping();
        self.checkpoint = None;
    }
    fn rollback_append_shaping(&mut self) {
        self.fallback.rollback_append_shaping();
        if let Some(checkpoint) = self.checkpoint.take() {
            self.requests.truncate(checkpoint);
        }
    }
    fn resource_generation(&self) -> GlyphResourceGeneration {
        self.fallback.resource_generation()
    }
    fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
        self.fallback.shape_glyph(request)
    }
    fn shape_run(
        &mut self,
        request: TextRunShapeRequest<'_>,
    ) -> Result<Option<ShapedRun>, ShapeError> {
        let direction = shaping_contracts::text_run_shape_request_style(&request).direction;
        let advance = shaping_contracts::text_run_shape_request_style(&request).font_size * 0.25;
        let run = TextRunId(self.requests.len() as u32);
        self.requests.push((shaping_contracts::text_run_shape_request_text(&request).to_owned(), direction));
        let count = shaping_contracts::text_run_shape_request_text(&request).chars().count();
        let stops: Vec<_> = (0..=count)
            .map(|index| {
                (if direction == TextDirection::Rtl {
                    count - index
                } else {
                    index
                }) as f32
                    * advance
            })
            .collect();
        Ok(Some(shaping_contracts::shaped_run_geometry(shaping_contracts::shaped_text_run_with_backend_run(Arc::from(vec![advance; count]),
            Arc::from(vec![true; count + 1]),
            run,
            stops.into(),
            12.0,))))
    }
}

#[test]
fn rtl_native_runs_receive_resolved_direction_and_share_the_correct_origin() {
    let mut shaper = DirectionalShaper::default();
    let mut document_resources = GlyphResourceStore::default();
    let document = DocumentFactory::new()
        .parse_with_new_pipeline("<p dir='rtl'>אבג abc 123</p>", None)
        .shape(&mut shaper, &mut document_resources)
        .unwrap()
        .layout(
            LayoutConstraints::new(300.0, 16.0).unwrap(),
            &document_resources,
        )
        .expect("layout requires the shaper-aware API for first-line styles");
    assert!(
        shaper
            .requests
            .iter()
            .any(|(text, direction)| text.contains('א') && *direction == TextDirection::Rtl)
    );
    assert!(
        shaper
            .requests
            .iter()
            .any(|(text, direction)| text.contains("abc") && *direction == TextDirection::Ltr)
    );
    let text = document.render_view(&document_resources).text();
    let fragments =
        text_paint::prepared_text_runs(text, LineId::new(0)).expect("complete native coverage");
    let rtl_run = shaper
        .requests
        .iter()
        .position(|(text, _)| text.contains('א'))
        .unwrap() as u32;
    let origins: Vec<_> = fragments
        .iter()
        .filter(|fragment| fragment.run.0 == rtl_run)
        .map(|fragment| fragment.offset.x)
        .collect();
    assert_eq!(
        origins.len(),
        shaper.requests[rtl_run as usize].0.chars().count()
    );
    assert!(
        origins
            .windows(2)
            .all(|pair| (pair[0] - pair[1]).abs() < 0.001),
        "{origins:?}"
    );
}

#[test]
fn rtl_first_line_refinement_preserves_native_coverage_of_later_lines() {
    let mut shaper = DirectionalShaper::default();
    let mut shaped_resources = GlyphResourceStore::default();
    let shaped = DocumentFactory::new()
        .parse_with_new_pipeline(
            "<p dir='rtl'>אבג דהו זחט יכל</p>",
            Some("html,body,p {margin:0} p::first-line {font-size:32px}"),
        )
        .shape(&mut shaper, &mut shaped_resources)
        .unwrap();
    let document = shaped
        .layout_with_metrics_and_shaper(
            LayoutConstraints::new(35.0, 16.0).unwrap(),
            &crate::ImageMetrics::default(),
            &mut shaper,
            &mut shaped_resources,
        )
        .unwrap();
    let text = document.render_view(&shaped_resources).text();
    assert!(document.line_count() > 1);
    for line in text_paint::render_text_view_spatial_order(text).iter().copied() {
        assert!(
            text_paint::prepared_text_runs(text, line).is_some(),
            "native coverage for line {line:?}"
        );
    }
    let last =
        text_paint::prepared_text_runs(text, text_paint::render_text_view_spatial_order(text).iter().copied().last().unwrap())
            .unwrap();
    assert!(
        last.iter()
            .any(|fragment| fragment.run == TextRunId(0) && fragment.range.start > 0)
    );
}

#[test]
fn rtl_native_runs_keep_offsets_across_out_of_flow_source_gaps() {
    let mut shaper = DirectionalShaper::default();
    let mut document_resources = GlyphResourceStore::default();
    let document = DocumentFactory::new()
        .parse_with_new_pipeline(
            "<p dir='rtl'>אב<span style='position:absolute'>X</span>גד</p>",
            None,
        )
        .shape(&mut shaper, &mut document_resources)
        .unwrap()
        .layout(
            LayoutConstraints::new(300.0, 16.0).unwrap(),
            &document_resources,
        )
        .expect("layout requires the shaper-aware API for first-line styles");
    let text = document.render_view(&document_resources).text();
    let line = text_paint::render_text_view_spatial_order(text)
        .iter()
        .copied()
        .find(|&line| {
            text_paint::render_line_glyphs(&text.line(line).unwrap()).any(|index| {
                text_paint::glyph_metric(text, text_paint::glyph_at(text, index as usize).unwrap())
                    .unwrap()
                    .ch()
                    == 'א'
            })
        })
        .unwrap();
    let native =
        text_paint::prepared_text_runs(text, line).expect("native coverage across a source gap");
    let run = shaper
        .requests
        .iter()
        .position(|(source, _)| source == "אבגד")
        .unwrap() as u32;
    assert!(
        native
            .iter()
            .any(|fragment| fragment.run.0 == run && fragment.range.start >= 2)
    );
}

#[test]
fn rtl_arabic_shaping_keeps_joining_context_across_paint_styles() {
    let mut shaper = DirectionalShaper::default();
    let mut document_resources = GlyphResourceStore::default();
    let document = DocumentFactory::new()
        .parse_with_new_pipeline(
            "<p dir='rtl'>الس<span style='color:red'>لام</span> عليكم ١٢٣</p>",
            None,
        )
        .shape(&mut shaper, &mut document_resources)
        .unwrap()
        .layout(
            LayoutConstraints::new(300.0, 16.0).unwrap(),
            &document_resources,
        )
        .expect("layout requires the shaper-aware API for first-line styles");
    assert!(shaper.requests.iter().any(
        |(text, direction)| text.starts_with("السلام عليكم") && *direction == TextDirection::Rtl
    ));
    assert!(
        shaper
            .requests
            .iter()
            .any(|(text, direction)| text == "١٢٣" && *direction == TextDirection::Ltr)
    );
    assert_eq!(
        visual_text(&document, LineId::new(0), &document_resources),
        "١٢٣ مكيلع مالسلا"
    );
}
