use crate::engine::LayoutRequest;
use crate::engine::{GlyphResourceStore, LayoutConstraints};
use crate::test_support::{DocumentFactory, TestGlyphShaper};

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
        let metric =
            text.glyph_metric(text.glyph_at(0).unwrap()).unwrap();
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
fn ordinary_layout_and_resize_reuse_shaped_text() {
    let prepared = DocumentFactory::new().parse_with_new_pipeline("<p>ordinary text</p>", None);
    let mut shaper = TestGlyphShaper::new();
    let mut document_resources = GlyphResourceStore::default();
    let mut document = prepared
        .shape(&mut shaper, &mut document_resources)
        .unwrap()
        .layout(
            LayoutRequest::new(LayoutConstraints::new(100.0, 16.0).unwrap()),
            &mut shaper,
            &mut document_resources,
        )
        .unwrap();
    document
        .relayout(
            LayoutRequest::new(LayoutConstraints::new(200.0, 16.0).unwrap()),
            &mut shaper,
            &mut document_resources,
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
