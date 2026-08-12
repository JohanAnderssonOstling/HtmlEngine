#[cfg(test)]
mod stage_tests {
    use crate::ShapeError;
    use crate::layout_model::{GlyphId, GlyphMetric, GlyphMetricError};
    use crate::test_support::{DocumentFactory, TestGlyphShaper};
    use crate::{GlyphShaper, LayoutConstraintError, LayoutConstraints, PreparedDocument, TextCompositionPolicy};
    use html_style_model::{ComputedStylesBuildError, ComputedStylesBuilder, ComputedStylesValidationError};

    struct InvalidGlyphShaper;

    impl GlyphShaper for InvalidGlyphShaper {
        fn reset(&mut self) {}

        fn shape_glyph<'a>(&mut self, _glyph_metrics: &mut crate::GlyphRegistry<'a>, _ch: char, _font_size: f32, _font_weight: u16, _font_slant: crate::FontSlant, _color: u32, _family: Option<&str>) -> Result<GlyphId, ShapeError> {
            Ok(17)
        }
    }

    #[test]
    fn constraints_reject_non_finite_and_out_of_domain_values() {
        assert_eq!(LayoutConstraints::new(-1.0, 20.0), Err(LayoutConstraintError::InvalidViewportWidth));
        assert_eq!(LayoutConstraints::new(f64::NAN, 20.0), Err(LayoutConstraintError::InvalidViewportWidth));
        assert_eq!(LayoutConstraints::new(f64::INFINITY, 20.0), Err(LayoutConstraintError::InvalidViewportWidth));
        assert_eq!(LayoutConstraints::new(100.0, 0.0), Err(LayoutConstraintError::InvalidLineHeight));
        assert_eq!(LayoutConstraints::new(100.0, f64::NEG_INFINITY), Err(LayoutConstraintError::InvalidLineHeight));
        assert_eq!(LayoutConstraints::new(100.0, 20.0).unwrap().with_viewport_height(Some(-1.0)), Err(LayoutConstraintError::InvalidViewportHeight));
        assert!(LayoutConstraints::new(0.0, 20.0).is_ok(), "a zero-width viewport is valid during window minimization");
    }

    #[test]
    fn unrestricted_hyphenation_remains_a_book_composition_policy() {
        assert!(TextCompositionPolicy::BookOptimized.is_book_optimized());
        assert!(TextCompositionPolicy::BookOptimized.uses_hyphenation_quality());
        assert!(TextCompositionPolicy::BookOptimizedUnrestrictedHyphenation.is_book_optimized());
        assert!(!TextCompositionPolicy::BookOptimizedUnrestrictedHyphenation.uses_hyphenation_quality());
        assert!(TextCompositionPolicy::SentencePerLine.is_book_optimized());
        assert!(TextCompositionPolicy::SentencePerLine.uses_hyphenation_quality());
        assert!(TextCompositionPolicy::SentencePerLine.uses_sentence_per_line());
        assert!(!TextCompositionPolicy::WebCompatible.is_book_optimized());
        assert!(!TextCompositionPolicy::WebCompatible.uses_hyphenation_quality());
        assert!(!TextCompositionPolicy::WebCompatible.uses_sentence_per_line());
    }

    #[test]
    fn shaping_skips_font_metric_queries_when_no_used_value_needs_font_metrics() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline_css_chunks("<html><body><p>text</p></body></html>", &["p { width: 10em; margin: 2px; }"]);
        let mut shaper = TestGlyphShaper::new();

        prepared.shape(&mut shaper).expect("ordinary document shapes");

        assert_eq!(shaper.font_metric_calls(), 0);
    }

    #[test]
    fn shaping_queries_font_metrics_when_closed_used_values_require_font_metrics() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline_css_chunks("<html><body><p>text</p></body></html>", &["p { width: 10ex; outline: 1ex solid; grid-template-columns: 2ex; }"]);
        let mut shaper = TestGlyphShaper::with_x_height_ratio(0.6);

        prepared.shape(&mut shaper).expect("font-relative document shapes");

        assert!(shaper.font_metric_calls() > 0);
    }

    #[test]
    fn shaping_queries_font_metrics_for_ch_box_lengths() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline_css_chunks("<html><body><p>00000</p></body></html>", &["p { width: 5ch; height: 2ch; }"]);
        let mut shaper = TestGlyphShaper::with_font_relative_ratios(0.5, 0.625);

        prepared.shape(&mut shaper).expect("ch-sized document shapes");

        assert!(shaper.font_metric_calls() > 0);
    }

    #[test]
    fn shaping_queries_font_metrics_for_text_decorations() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline_css_chunks("<html><body><p>text</p></body></html>", &["p { text-decoration: line-through; }"]);
        let mut shaper = TestGlyphShaper::new();

        prepared.shape(&mut shaper).expect("decorated document shapes");

        assert!(shaper.font_metric_calls() > 0);
    }

    #[test]
    fn shaping_queries_font_metrics_for_painted_inline_content_boxes() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline_css_chunks("<html><body><span>text</span></body></html>", &["span { background: black; }"]);
        let mut shaper = TestGlyphShaper::new();

        prepared.shape(&mut shaper).expect("inline background document shapes");

        assert!(shaper.font_metric_calls() > 0);
    }

    #[test]
    fn shaping_rejects_unregistered_glyph_ids() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline("<html><body>text</body></html>", None);
        let error = prepared.shape(&mut InvalidGlyphShaper).err().expect("invalid shaper must be rejected");

        assert_eq!(error.glyph_id(), 17);
        assert_eq!(error.metrics_len(), 0);
    }

    #[derive(Default)]
    struct InvalidMetricShaper;

    impl GlyphShaper for InvalidMetricShaper {
        fn reset(&mut self) {}

        fn shape_glyph<'a>(&mut self, glyph_metrics: &mut crate::GlyphRegistry<'a>, ch: char, font_size: f32, _font_weight: u16, _font_slant: crate::FontSlant, _color: u32, _family: Option<&str>) -> Result<GlyphId, ShapeError> {
            let metric = GlyphMetric::try_new(ch, font_size, -font_size, font_size * 0.25, font_size * 0.75).map_err(ShapeError::rejected_metric)?;
            glyph_metrics.register(metric)
        }
    }

    #[test]
    fn shaping_rejects_invalid_glyph_metrics() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline("<html><body>text</body></html>", None);
        let error = prepared.shape(&mut InvalidMetricShaper).err().expect("invalid metric must be rejected");
        assert!(matches!(error.reason(), Some(GlyphMetricError::NegativeValue { field: "ascent", .. })));
    }

    #[test]
    fn preparation_rejects_styles_from_another_document() {
        let first = html_parse::parse_dom_document("<html><body><p>first</p></body></html>").expect("valid HTML");
        let second = html_parse::parse_dom_document("<html><body><p>second</p></body></html>").expect("valid HTML");
        let first_styles = html_style::style_document(first, &[]).into_parts().1;

        let error = PreparedDocument::try_new(second, first_styles).err().expect("cross-document styles must be rejected");

        assert_eq!(error.reason(), ComputedStylesValidationError::WrongDocument);
    }

    fn prepare_with_notes(source: &str, note_flow: crate::NoteFlow) -> PreparedDocument {
        let document = html_parse::parse_dom_document(source).expect("valid HTML");
        let (document, styles) = html_style::style_document(document, &[]).into_parts();
        PreparedDocument::try_new_with_note_flow(document, styles, note_flow).expect("valid styles must prepare")
    }

    #[test]
    fn excluded_notes_generate_no_boxes_while_ordinary_markup_is_untouched() {
        const WITH_NOTE: &str = "<html><body><p>Reading</p><aside id='n' epub:type='footnote'><p>note body</p></aside><p>Continues</p></body></html>";
        // Same shape, but the aside carries no note semantics.
        const WITHOUT_NOTE: &str = "<html><body><p>Reading</p><aside id='n'><p>note body</p></aside><p>Continues</p></body></html>";

        let in_flow = prepare_with_notes(WITH_NOTE, crate::NoteFlow::InFlow).box_count();
        let excluded = prepare_with_notes(WITH_NOTE, crate::NoteFlow::Excluded).box_count();
        assert!(excluded < in_flow, "an excluded note must stop generating boxes ({excluded} boxes vs {in_flow} in flow)");

        // Exclusion is driven by note semantics alone, so markup the predicate
        // does not recognise lays out the same either way.
        assert_eq!(
            prepare_with_notes(WITHOUT_NOTE, crate::NoteFlow::Excluded).box_count(),
            prepare_with_notes(WITHOUT_NOTE, crate::NoteFlow::InFlow).box_count(),
            "markup without note semantics is unaffected by the note flow setting"
        );
    }

    #[test]
    fn note_exclusion_resolves_a_non_epub_prefix_only_under_xml_parsing() {
        // The predicate tries the literal `epub:type` attribute first and the
        // EPUB-namespaced `type` second. Namespace resolution is a property of
        // the parser, not the predicate: XHTML binds `e:` to the EPUB namespace
        // and the note is recognised, while the same bytes parsed as HTML leave
        // `e:type` an ordinary attribute name that matches nothing.
        const PREFIXED: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:e="http://www.idpf.org/2007/ops"><body><p>Reading</p><aside id="n" e:type="footnote"><p>note body</p></aside></body></html>"#;

        let prepare_xml = |note_flow| {
            let document = html_parse::parse_xml_document(PREFIXED).expect("valid XHTML").build_dom();
            let (document, styles) = html_style::style_document(document, &[]).into_parts();
            PreparedDocument::try_new_with_note_flow(document, styles, note_flow).expect("valid styles must prepare").box_count()
        };
        assert!(prepare_xml(crate::NoteFlow::Excluded) < prepare_xml(crate::NoteFlow::InFlow), "XHTML binds the prefix to the EPUB namespace, so the note is recognised");

        assert_eq!(
            prepare_with_notes(PREFIXED, crate::NoteFlow::Excluded).box_count(),
            prepare_with_notes(PREFIXED, crate::NoteFlow::InFlow).box_count(),
            "parsed as HTML the same markup carries no EPUB semantics, so nothing is held back"
        );
    }

    #[test]
    fn a_note_excluded_from_the_flow_still_lays_out_when_scoped_to() {
        const WITH_NOTE: &str = "<html><body><p>Reading</p><aside id='n' epub:type='footnote'><p>note body</p></aside><p>Continues</p></body></html>";
        let prepared = prepare_with_notes(WITH_NOTE, crate::NoteFlow::Excluded);

        let scoped = prepared.scoped_to_element_id("n").expect("the note element must be scopable");

        assert!(scoped.box_count() > 0, "scoping to a note lays it out even though the containing document holds it back");
        assert!(scoped.box_count() < prepared_in_flow_box_count(WITH_NOTE), "a scoped note is only its own subtree, not the whole document");
        assert!(prepared.scoped_to_element_id("absent").is_none(), "an id that names nothing cannot be scoped to");
    }

    fn prepared_in_flow_box_count(source: &str) -> usize {
        prepare_with_notes(source, crate::NoteFlow::InFlow).box_count()
    }

    #[test]
    fn note_ids_lists_only_referenceable_note_bodies() {
        const MIXED: &str = "<html><body><aside id='a' epub:type='footnote'>one</aside><aside epub:type='footnote'>unreferenceable</aside><aside id='c' role='doc-endnote'>three</aside><aside id='d'>not a note</aside></body></html>";

        let prepared = prepare_with_notes(MIXED, crate::NoteFlow::Excluded);
        let ids = prepared.note_ids();

        assert_eq!(ids, vec!["a", "c"], "notes are listed in document order; one without an id and one without note semantics are not notes to a reference");
    }

    #[test]
    fn note_flow_defaults_to_laying_notes_out_as_authored() {
        const WITH_NOTE: &str = "<html><body><p>Reading</p><aside id='n' epub:type='footnote'><p>note body</p></aside></body></html>";
        let document = html_parse::parse_dom_document(WITH_NOTE).expect("valid HTML");
        let (document, styles) = html_style::style_document(document, &[]).into_parts();

        let default_boxes = PreparedDocument::try_new(document, styles).expect("valid styles must prepare").box_count();

        assert_eq!(default_boxes, prepare_with_notes(WITH_NOTE, crate::NoteFlow::InFlow).box_count(), "embedders that ask for nothing keep authored layout");
    }

    #[test]
    fn style_to_layout_boundary_moves_storage_without_cloning() {
        let document = html_parse::parse_dom_document("<html><body><p>text</p></body></html>").expect("valid HTML");
        let styled = html_style::style_document(document, &[]);
        let default_style = styled.styles().default_indices();
        let font_storage = styled.styles().font_style(default_style).expect("default style handle") as *const _;
        let (document, styles) = styled.into_parts();

        let prepared = PreparedDocument::try_new(document, styles).expect("valid styles must prepare");

        assert_eq!(prepared.styles().font_style(default_style).expect("preserved style handle") as *const _, font_storage);
    }

    #[test]
    fn incomplete_style_tables_cannot_be_finished() {
        let document = html_parse::parse_dom_document("<html><body><p>text</p></body></html>").expect("valid HTML");
        let error = ComputedStylesBuilder::new(&document).finish().err().expect("incomplete styles must not become a ComputedStyles value");

        assert!(matches!(error, ComputedStylesBuildError::MissingElementStyle { .. }));
    }

    #[test]
    fn relayout_reuses_phase_inputs_and_top_level_output_buffers() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline("<html><body><p id='first'>one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen</p></body></html>", None);
        let mut shaper = TestGlyphShaper::new();
        let shaped = prepared.shape(&mut shaper).expect("test shaper must register every glyph");
        let glyph_count = shaped.glyphs().len();
        let metric_count = shaped.glyph_metrics().len();
        let constraints = LayoutConstraints::new(180.0, 20.0).unwrap();
        let mut document = shaped.layout(constraints);

        let lines_ptr = document.layout_state.line_output.lines.as_ptr();
        let offsets_ptr = document.layout_state.line_output.line_glyph_offsets.as_ptr();
        let advances_ptr = document.layout_state.line_output.line_glyph_advances.as_ptr();
        let image_lines_ptr = document.layout_state.fragment_output.image_fragments_by_line.as_ptr();
        let glyphs_ptr = document.glyphs().as_ptr();
        let cached_text_plans = document.shaped.inline_plans.len();
        let scratch_capacities = document.layout_scratch.allocation_capacities();
        assert!(cached_text_plans > 0, "initial layout should retain width-independent text plans");
        assert!(scratch_capacities.0.0 > 0, "initial layout should retain line-owner storage");
        assert!(scratch_capacities.1.0 > 0, "initial layout should retain line-sort storage");
        assert!(scratch_capacities.1.1 > 0, "initial layout should retain line-remapping storage");

        let relayout_timings = document.relayout_with_timings(constraints);

        assert_eq!(document.glyphs().len(), glyph_count);
        assert_eq!(document.glyph_metrics().len(), metric_count);
        assert_eq!(document.glyphs().as_ptr(), glyphs_ptr);
        assert_eq!(document.layout_state.line_output.lines.as_ptr(), lines_ptr);
        assert_eq!(document.layout_state.line_output.line_glyph_offsets.as_ptr(), offsets_ptr);
        assert_eq!(document.layout_state.line_output.line_glyph_advances.as_ptr(), advances_ptr);
        assert_eq!(document.layout_state.fragment_output.image_fragments_by_line.as_ptr(), image_lines_ptr);
        assert_eq!(document.shaped.inline_plans.len(), cached_text_plans);
        let retained_scratch_capacities = document.layout_scratch.allocation_capacities();
        assert_eq!(retained_scratch_capacities.0.0 + retained_scratch_capacities.1.2, scratch_capacities.0.0 + scratch_capacities.1.2, "the two rotating line-owner buffers must retain their combined allocation");
        assert_eq!(retained_scratch_capacities.1.0, scratch_capacities.1.0, "line-sort storage must be recycled");
        assert_eq!(retained_scratch_capacities.1.1, scratch_capacities.1.1, "line-remapping storage must be recycled");
        assert!(!relayout_timings.root_box_layout.is_zero(), "explicit timing must still read and report the clock");
        assert!(relayout_timings.layout_tree_traversal >= relayout_timings.root_box_layout, "tree timing must include root layout");
        assert_eq!(relayout_timings.build_inline_tokens_from_runs, std::time::Duration::ZERO, "pure-text relayout should not rebuild token plans");

        let cloned = document.clone();
        assert_eq!(cloned.layout_scratch.allocation_capacities(), ((0, 0), (0, 0, 0, 0, 0, 0)), "cloning semantic layout state must not duplicate transient scratch allocations");
        assert!(std::sync::Arc::ptr_eq(&cloned.placement, &document.placement), "cloning a laid-out document must share its retained local placement snapshot");
    }

    #[test]
    fn shaped_inline_plans_are_shared_for_short_text_contexts() {
        let mut factory = DocumentFactory::new();
        let mut shaper = TestGlyphShaper::new();
        let shaped = factory
            .parse_with_new_pipeline("<html><body><p>tiny</p></body></html>", None)
            .shape(&mut shaper)
            .expect("short text shapes");

        let first = shaped.clone().layout(LayoutConstraints::new(100.0, 16.0).unwrap());
        let prepared_plan_count = first.shaped.inline_plans.len();
        assert!(prepared_plan_count > 0, "short contexts must enter the shaped plan arena");

        let (second, timings) = shaped.layout_with_timings(LayoutConstraints::new(80.0, 16.0).unwrap());
        assert_eq!(second.shaped.inline_plans.len(), prepared_plan_count);
        assert_eq!(timings.build_inline_tokens_from_runs, std::time::Duration::ZERO, "a second layout from the same shaped document must not rebuild short text tokens");
    }

    #[test]
    fn phase_wrappers_share_immutable_prepared_and_shaped_inputs() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline("<html><body><p>shared phase inputs</p></body></html>", None);
        let mut shaper = TestGlyphShaper::new();
        let shaped = prepared.shape(&mut shaper).expect("test shaper must register every glyph");
        let document = shaped.clone().layout(LayoutConstraints::new(180.0, 20.0).unwrap());

        assert!(std::sync::Arc::ptr_eq(&prepared.inputs, &shaped.inputs));
        assert!(std::sync::Arc::ptr_eq(&prepared.inputs, &document.inputs));
        assert!(std::sync::Arc::ptr_eq(&shaped.shaped, &document.shaped));
    }

    #[test]
    fn exact_line_refinement_keeps_immutable_shaped_storage_shared() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline("<html><body><p>shared exact geometry</p></body></html>", None);
        let mut shaper = TestGlyphShaper::new();
        let shaped = prepared.shape(&mut shaper).expect("test shaper must register every glyph");
        let document = shaped.clone().layout_with_metrics_and_shaper(LayoutConstraints::new(180.0, 20.0).unwrap(), &crate::ImageMetrics::default(), &mut shaper).expect("line refinement must succeed");

        assert!(std::sync::Arc::ptr_eq(&shaped.shaped, &document.shaped), "exact refinement must not copy inline content, glyph metrics, or shaping maps");
        assert!(document.text_geometry().advance(0).is_some(), "the shared shaped storage retains its text geometry through layout");
    }

    #[test]
    fn first_line_refinement_shapes_through_paint_only_span_boundaries() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(
            "<html><body><div><span class='one'>One</span><span class='two'>Two</span></div></body></html>",
            Some("div::first-line { color: black; } span.one { background-color: green; } span.two { background-color: lime; }"),
        );
        let mut shaper = TestGlyphShaper::new();
        let shaped = prepared.shape(&mut shaper).expect("test text shapes");
        let document = shaped.layout(LayoutConstraints::new(180.0, 20.0).unwrap());

        let ranges = super::first_line_style_ranges(&document);

        assert!(ranges.iter().any(|range| range.glyphs.end - range.glyphs.start == 6), "the fictional first-line box must retain one shaping range across compatible spans");
    }

    #[test]
    fn auxiliary_registry_can_only_append_valid_metrics() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline("<html><body>text</body></html>", None);
        let mut shaper = TestGlyphShaper::new();
        let shaped = prepared.shape(&mut shaper).expect("valid shaping");
        let constraints = LayoutConstraints::new(180.0, 20.0).unwrap();
        let mut document = shaped.layout(constraints);

        let original_len = document.glyph_metrics().len();
        let original_first = document.glyph_metrics().get_checked(0);
        let metric = GlyphMetric::try_new('#', 4.0, 3.0, 1.0, 0.0).expect("valid auxiliary metric");
        let appended = document.auxiliary_glyph_registry().register(metric).expect("append auxiliary glyph");

        assert_eq!(appended as usize, original_len);
        assert_eq!(document.glyph_metrics().len(), original_len + 1);
        assert_eq!(document.glyph_metrics().get_checked(0), original_first);
    }

    #[test]
    fn render_view_and_semantic_iterators_allocate_nothing() {
        use crate::allocation_test_support::count_allocations;
        use std::hint::black_box;

        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline("<html><body><p id='anchor'>one two three</p></body></html>", None);
        let mut shaper = TestGlyphShaper::new();
        let document = prepared.shape(&mut shaper).expect("valid shaping").layout(LayoutConstraints::new(180.0, 20.0).unwrap());

        let (_, allocations) = count_allocations(|| {
            let view = black_box(document.render_view());
            let text = black_box(view.text());
            let fragments = black_box(view.fragments());
            let addressing = black_box(view.addressing());
            let boxes = black_box(view.boxes());
            black_box(text.lines().iter().count());
            black_box(text.text_runs().count());
            black_box(text.marker_runs().count());
            black_box(fragments.decorations().iter().count());
            black_box(fragments.images().iter().count());
            black_box(addressing.anchor_positions().iter().count());
            black_box(addressing.anchor_glyphs().count());
            black_box(boxes.len());
            black_box(text.glyph_slice(0..text.glyph_count() as u32));
            if let Some(glyph) = text.glyph_at(0) {
                black_box(text.glyph_metric(glyph));
            }
        });

        assert_eq!(allocations, 0, "render views must stay allocation-free");
    }

    #[test]
    fn viewport_height_dependency_is_reported_from_used_layout_values() {
        let mut factory = DocumentFactory::new();
        let mut shaper = TestGlyphShaper::new();
        let ordinary = factory
            .parse_with_new_pipeline("<html><body><p>ordinary book text</p></body></html>", None)
            .shape(&mut shaper)
            .expect("ordinary document shapes")
            .layout(LayoutConstraints::new(300.0, 20.0).unwrap().with_viewport_height(Some(600.0)).unwrap());
        assert!(!ordinary.layout_depends_on_viewport_height());

        let percentage = factory
            .parse_with_new_pipeline(
                "<html><body><main>height-sensitive</main></body></html>",
                Some("html, body, main { height: 100%; }"),
            )
            .shape(&mut shaper)
            .expect("percentage-height document shapes")
            .layout(LayoutConstraints::new(300.0, 20.0).unwrap().with_viewport_height(Some(600.0)).unwrap());
        assert!(percentage.layout_depends_on_viewport_height());
    }
}
