use crate::engine::{GlyphResourceStore, ImageMetrics, LayoutConstraints};
use crate::test_support::{DocumentFactory, TestGlyphShaper};

fn first_line_document() -> crate::PreparedDocument<'static> {
    DocumentFactory::new().parse_with_new_pipeline(
        "<!doctype html><p>aa aa aa aa aa</p>",
        Some("html,body,p {margin:0} p {font-size:10px} p::first-line {font-size:20px}"),
    )
}

#[test]
fn first_line_spacing_keeps_suppressed_characters_zero_width() {
    for (base_spacing, first_line_spacing) in [(0, 2), (2, 0)] {
        let css = format!(
            "html,body,p {{margin:0}} p {{letter-spacing:{base_spacing}px}} \
             p::first-line {{letter-spacing:{first_line_spacing}px}}"
        );
        let prepared = DocumentFactory::new()
            .parse_with_new_pipeline("<p>A\u{00ad}B\u{200e}C</p>", Some(&css));
        let mut shaper = TestGlyphShaper::new();
        let mut resources = GlyphResourceStore::default();
        let document = prepared
            .shape_and_layout(
                LayoutConstraints::new(1000.0, 16.0).unwrap(),
                &mut shaper,
                &mut resources,
            )
            .unwrap();
        let text = document.render_view(&resources).text();
        let metric = text.glyph_metric(text.glyph_at(0).unwrap()).unwrap();
        assert_eq!(
            text.character_advance(0),
            Some(metric.advance() + (first_line_spacing - base_spacing) as f32),
            "the first-line spacing adjustment must be active"
        );
        for index in [1, 3] {
            assert_eq!(
                text.character_advance(index),
                Some(0.0),
                "{css}, index {index}"
            );
            assert_eq!(text.paint_glyph_at(index), None);
        }
    }
}

#[test]
fn static_layout_cannot_skip_required_first_line_shaping() {
    let mut shaper = TestGlyphShaper::new();
    let mut shaped_resources = GlyphResourceStore::default();
    let shaped = first_line_document()
        .shape(&mut shaper, &mut shaped_resources)
        .unwrap();
    let result = shaped.layout(
        LayoutConstraints::new(28.0, 16.0).unwrap(),
        &shaped_resources,
    );
    let Err(error) = result else {
        panic!("a shaper is required for first-line styles")
    };
    assert!(error.requires_first_line_shaper());
}

#[test]
fn rejected_static_resize_preserves_the_published_document() {
    let mut shaper = TestGlyphShaper::new();
    let initial = LayoutConstraints::new(28.0, 16.0).unwrap();
    let changed = LayoutConstraints::new(55.0, 16.0).unwrap();
    let mut document_resources = GlyphResourceStore::default();
    let mut document = first_line_document()
        .shape_and_layout(initial, &mut shaper, &mut document_resources)
        .unwrap();
    let snapshot = |doc: &crate::LaidOutDocument<'_>, resources: &GlyphResourceStore| {
        doc.render_view(resources)
            .text()
            .lines()
            .iter()
            .map(|line| (line.glyphs(), line.point(), line.height()))
            .collect::<Vec<_>>()
    };
    let before = snapshot(&document, &document_resources);
    assert!(
        document
            .relayout(changed, &document_resources)
            .unwrap_err()
            .requires_first_line_shaper()
    );
    assert_eq!(snapshot(&document, &document_resources), before);
    assert!(
        document
            .relayout_with_metrics(changed, &ImageMetrics::default(), &document_resources)
            .unwrap_err()
            .requires_first_line_shaper()
    );
    assert!(
        document
            .relayout_with_timings(changed, &document_resources)
            .unwrap_err()
            .requires_first_line_shaper()
    );
    assert!(
        document
            .relayout_with_metrics_and_timings(
                changed,
                &ImageMetrics::default(),
                &document_resources
            )
            .unwrap_err()
            .requires_first_line_shaper()
    );
    assert_eq!(snapshot(&document, &document_resources), before);
    document
        .relayout_with_shaper(changed, &mut shaper, &mut document_resources)
        .unwrap();
    assert_ne!(snapshot(&document, &document_resources), before);
}

#[test]
fn ordinary_layout_still_needs_no_backend_after_shaping() {
    let prepared = DocumentFactory::new().parse_with_new_pipeline("<p>ordinary text</p>", None);
    let mut shaper = TestGlyphShaper::new();
    let mut document_resources = GlyphResourceStore::default();
    let mut document = prepared
        .shape(&mut shaper, &mut document_resources)
        .unwrap()
        .layout(
            LayoutConstraints::new(100.0, 16.0).unwrap(),
            &document_resources,
        )
        .unwrap();
    document
        .relayout(
            LayoutConstraints::new(200.0, 16.0).unwrap(),
            &document_resources,
        )
        .unwrap();
    assert!(
        document
            .render_view(&document_resources)
            .text()
            .line_count()
            > 0
    );
}

#[test]
fn timed_layout_uses_the_same_first_line_coordinator() {
    let constraints = LayoutConstraints::new(28.0, 16.0).unwrap();
    let mut shaper = TestGlyphShaper::new();
    let mut expected_resources = GlyphResourceStore::default();
    let expected = first_line_document()
        .shape_and_layout(constraints, &mut shaper, &mut expected_resources)
        .unwrap();
    let mut timed_shaper = TestGlyphShaper::new();
    let mut timings_resources = GlyphResourceStore::default();
    let (actual, timings) = first_line_document()
        .shape(&mut timed_shaper, &mut timings_resources)
        .unwrap()
        .layout_with_timings_and_metrics_and_shaper(
            constraints,
            &ImageMetrics::default(),
            &mut timed_shaper,
            &mut timings_resources,
        )
        .unwrap();
    let lines = |doc: &crate::LaidOutDocument<'_>, resources: &GlyphResourceStore| {
        doc.render_view(resources)
            .text()
            .lines()
            .iter()
            .map(|line| (line.glyphs(), line.point(), line.height()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        lines(&actual, &timings_resources),
        lines(&expected, &expected_resources)
    );
    assert!(timings.measured_lines > 0);
}
