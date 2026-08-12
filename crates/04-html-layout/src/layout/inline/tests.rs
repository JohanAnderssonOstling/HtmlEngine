#[cfg(test)]
mod tests {
    use super::{BreakKind, InlineToken, InlineTokenKind, InlineTokenMetrics, InlineTokens, TokenWrap, automatic_hyphen_fragments_are_long_enough, vertical_align_offset};
    use crate::parser::DocumentFactory;
    use crate::test_support::TestGlyphShaper as GlyphCache;
    use crate::{FontSlant, GlyphId, GlyphRegistry, GlyphShaper, LaidOutDocument, ShapeError, ShapedTextRun, TextRunShapeRequest, TextShapeRequest};
    use html_style_model::{VerticalAlignValue, WhiteSpace};
    use lightningcss::stylesheet::ParserOptions;
    use std::fs;
    use std::sync::Arc;

    struct NativeGeometryShaper {
        fallback: GlyphCache,
        advances: Vec<f32>,
        boundaries: Vec<bool>,
        exact_line_advance: Option<f32>,
        measured_texts: Vec<String>,
    }

    impl GlyphShaper for NativeGeometryShaper {
        fn reset(&mut self) {
            self.fallback.reset();
        }

        fn shape_glyph<'a>(&mut self, glyph_metrics: &mut GlyphRegistry<'a>, ch: char, font_size: f32, font_weight: u16, font_slant: FontSlant, color: u32, family: Option<&str>) -> Result<GlyphId, ShapeError> {
            self.fallback.shape_glyph(glyph_metrics, ch, font_size, font_weight, font_slant, color, family)
        }

        fn shape_text_run(&mut self, request: TextRunShapeRequest<'_>) -> Result<Option<ShapedTextRun>, ShapeError> {
            if request.text().chars().count() != self.advances.len() {
                return Ok(None);
            }
            Ok(ShapedTextRun::new(Arc::from(self.advances.clone()), Arc::from(self.boundaries.clone())))
        }

        fn measure_line(&mut self, request: TextShapeRequest<'_>) -> Result<Option<ShapedTextRun>, ShapeError> {
            let Some(advance) = self.exact_line_advance else { return Ok(None) };
            self.measured_texts.push(request.text().to_owned());
            let character_count = request.text().chars().count();
            Ok(ShapedTextRun::new(Arc::from(vec![advance; character_count]), Arc::from(vec![true; character_count + 1])))
        }
    }

    struct OscillatingBoundaryShaper {
        fallback: GlyphCache,
    }

    impl GlyphShaper for OscillatingBoundaryShaper {
        fn reset(&mut self) {
            self.fallback.reset();
        }

        fn shape_glyph<'a>(&mut self, glyph_metrics: &mut GlyphRegistry<'a>, ch: char, font_size: f32, font_weight: u16, font_slant: FontSlant, color: u32, family: Option<&str>) -> Result<GlyphId, ShapeError> {
            self.fallback.shape_glyph(glyph_metrics, ch, font_size, font_weight, font_slant, color, family)
        }

        fn shape_text_run(&mut self, request: TextRunShapeRequest<'_>) -> Result<Option<ShapedTextRun>, ShapeError> {
            let count = request.text().chars().count();
            Ok(ShapedTextRun::new(Arc::from(vec![4.0; count]), Arc::from(vec![true; count + 1])))
        }

        fn measure_line(&mut self, request: TextShapeRequest<'_>) -> Result<Option<ShapedTextRun>, ShapeError> {
            let count = request.text().chars().count();
            let advance = if count == 1 { 2.0 } else { 6.0 };
            Ok(ShapedTextRun::new(Arc::from(vec![advance; count]), Arc::from(vec![true; count + 1])))
        }
    }

    fn layout_with_native_geometry(html: &str, width: f64, advances: Vec<f32>, boundaries: Vec<bool>) -> LaidOutDocument {
        layout_with_native_geometry_policy(html, width, advances, boundaries, crate::TextCompositionPolicy::WebCompatible)
    }

    fn layout_with_native_geometry_policy(html: &str, width: f64, advances: Vec<f32>, boundaries: Vec<bool>, policy: crate::TextCompositionPolicy) -> LaidOutDocument {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = NativeGeometryShaper { fallback: GlyphCache::new(), advances, boundaries, exact_line_advance: None, measured_texts: Vec::new() };
        factory.parse_with_new_pipeline(html, None).shape(&mut glyph_cache).expect("native geometry must be valid").layout(crate::LayoutConstraints::new(width, 16.0).unwrap().with_text_composition_policy(policy))
    }

    #[test]
    fn width_only_layout_reuses_authoritative_document_shaping() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = NativeGeometryShaper { fallback: GlyphCache::new(), advances: vec![4.0; 4], boundaries: vec![true; 5], exact_line_advance: Some(6.0), measured_texts: Vec::new() };
        let shaped = factory.parse_with_new_pipeline("<html><body style='margin:0'><p style='margin:0;overflow-wrap:anywhere'>abcd</p></body></html>", None).shape(&mut glyph_cache).expect("native geometry must be valid");
        let constraints = crate::LayoutConstraints::new(9.0, 16.0).unwrap();
        let mut document = shaped.layout_with_metrics_and_shaper(constraints, &crate::ImageMetrics::default(), &mut glyph_cache).expect("exact boundary shaping must succeed");

        assert_eq!(document.line_count(), 2);
        for (line_index, expected) in ["ab", "cd"].iter().enumerate() {
            assert_eq!(line_text(&document, line_index), *expected);
        }

        assert!(glyph_cache.measured_texts.is_empty(), "layout must not invoke backend line reshaping");
        document.relayout_with_metrics_and_shaper(constraints, &crate::ImageMetrics::default(), &mut glyph_cache).expect("width-only relayout must remain valid");
        assert!(glyph_cache.measured_texts.is_empty(), "relayout must reuse authoritative shaped geometry");

        let wider = crate::LayoutConstraints::new(13.0, 16.0).unwrap();
        document.relayout_with_metrics_and_shaper(wider, &crate::ImageMetrics::default(), &mut glyph_cache).expect("changed exact boundaries must remain valid");
        assert_eq!(document.line_count(), 2);
    }

    #[test]
    fn authoritative_line_geometry_is_fragmented_around_inline_images() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = NativeGeometryShaper { fallback: GlyphCache::new(), advances: vec![8.0], boundaries: vec![true; 2], exact_line_advance: Some(8.0), measured_texts: Vec::new() };
        let shaped = factory
            .parse_with_new_pipeline("<html><body style='margin:0'><p style='margin:0;white-space:nowrap'>a<img src='x.png' style='width:20px;height:10px'>b</p></body></html>", None)
            .shape(&mut glyph_cache)
            .expect("fixture shaping must succeed");
        let constraints = crate::LayoutConstraints::new(200.0, 16.0).unwrap();
        let mut image_metrics = crate::ImageMetrics::default();
        image_metrics.set(0, 20, 10);
        let document = shaped.layout_with_metrics_and_shaper(constraints, &image_metrics, &mut glyph_cache).expect("fragmented exact shaping must succeed");

        assert!(glyph_cache.measured_texts.is_empty(), "inline-image fragmentation must not trigger backend line reshaping");

        assert_eq!(document.line_count(), 1);
        let fragments = document.render_view().text().line_text_fragments(0).unwrap().collect::<Vec<_>>();
        assert_eq!(fragments.len(), 2);
        assert_eq!(fragments[0].glyphs(), 0..1);
        assert_eq!(fragments[1].glyphs(), 1..2);
        assert_eq!(fragments[1].offset_x(), 28.0);
    }

    #[test]
    fn nested_atomic_text_has_unambiguous_line_ownership() {
        let document = layout_html("<html><body style='margin:0'><p style='margin:0;white-space:nowrap'>A<span style='display:inline-flex'>INNER</span>B</p></body></html>", 200.0);
        let view = document.render_view().text();
        let glyph_for = |target| (0..view.glyph_count() as u32).find(|index| view.glyph_at(*index as usize).and_then(|glyph| view.glyph_metric(glyph)).is_some_and(|metric| metric.ch() == target)).expect("fixture character must be shaped");
        let before = glyph_for('A');
        let nested = glyph_for('I');
        let after = glyph_for('B');
        let outer_line = view.line_index_for_glyph(before).expect("leading outer text must own a line");

        assert_eq!(view.line_index_for_glyph(after), Some(outer_line), "text after an atomic inline must remain owned by its outer line");
        assert_ne!(view.line_index_for_glyph(nested), Some(outer_line), "nested atomic text must retain its own line identity");
        assert_eq!(view.line(outer_line).map(|line| line.index()), Some(outer_line));
    }

    #[test]
    fn inline_border_fragments_retain_the_outer_line_across_an_atomic_table() {
        let document = layout_html("<html><body style='margin:0'><span style='font-size:15px;border:3px solid'>left <span style='display:table-cell'>right</span></span></body></html>", 400.0);
        let text = document.render_view().text();
        let leading = (0..text.glyph_count() as u32).find(|index| text.glyph_at(*index as usize).and_then(|glyph| text.glyph_metric(glyph)).is_some_and(|metric| metric.ch() == 'l')).expect("outer text must be shaped");
        let outer_line = text.line_index_for_glyph(leading).expect("outer text must own a line");
        let borders = document.render_view().fragments().decorations().iter().filter(|fragment| fragment.is_inline() && fragment.color() == 0x000000ff).collect::<Vec<_>>();

        let owned = borders.iter().filter(|fragment| fragment.line_idx().is_some()).collect::<Vec<_>>();

        assert_eq!(owned.len(), 4, "the inline segment must retain all four physical edges: {owned:?}");
        assert!(owned.iter().all(|fragment| fragment.line_idx() == Some(outer_line)), "every segment edge must project through the outer line rather than infer ownership from y geometry");
    }

    #[test]
    fn inline_border_uses_font_content_box_without_half_leading() {
        let document = layout_html("<html><body style='margin:0'><span style='font:10px/30px sans-serif;padding:2px 0;border:10px solid lime'>x</span></body></html>", 200.0);
        let borders = document.render_view().fragments().decorations().iter().filter(|decoration| decoration.is_inline() && decoration.color() == 0x00ff00ff).collect::<Vec<_>>();

        let top = borders.iter().map(|border| border.rect().y0).fold(f64::INFINITY, f64::min);
        let bottom = borders.iter().map(|border| border.rect().y1).fold(f64::NEG_INFINITY, f64::max);
        assert_eq!(borders.len(), 4);
        assert_eq!(bottom - top, 34.0, "10px font content + 4px padding + 20px border; line-height must not enlarge the border box");
    }

    #[test]
    fn inline_background_paint_order_precedes_its_text_after_an_atomic_inline() {
        let document = layout_html("<html><body style='margin:0'><span style='display:inline-block;width:15px;height:15px'></span> <span style='background:red;color:blue'>X</span></body></html>", 200.0);
        let text = document.render_view().text();
        let x = (0..text.glyph_count() as u32).find(|index| text.glyph_at(*index as usize).and_then(|glyph| text.glyph_metric(glyph)).is_some_and(|metric| metric.ch() == 'X')).expect("fixture X");
        let line_idx = text.line_index_for_glyph(x).expect("X line");
        let text_fragment = text.line_text_fragments(line_idx).expect("fragmented line").find(|fragment| fragment.glyphs().contains(&x)).expect("X fragment");
        let background = document.render_view().fragments().decorations_for_line(line_idx).iter().find(|decoration| decoration.color() == 0xff0000ff).expect("red inline background");

        assert!(background.paint_order() <= text_fragment.paint_order(), "inline background must not paint after its descendant text");
        assert!(text_fragment.offset_x() >= 15.0, "text after an atomic inline must retain its placed x position: {}", text_fragment.offset_x());
        assert_eq!(text_fragment.glyphs(), x..x + 1, "a fragment must not merge text from a differently styled owner; otherwise its earlier paint key can place the inline background above X");
    }

    #[test]
    fn layout_does_not_enter_backend_boundary_refinement() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = OscillatingBoundaryShaper { fallback: GlyphCache::new() };
        let shaped = factory.parse_with_new_pipeline("<html><body style='margin:0'><p style='margin:0;overflow-wrap:anywhere'>abcd</p></body></html>", None).shape(&mut glyph_cache).expect("fixture shaping must succeed");
        let document = shaped.layout_with_metrics_and_shaper(crate::LayoutConstraints::new(9.0, 16.0).unwrap(), &crate::ImageMetrics::default(), &mut glyph_cache).expect("layout uses authoritative document shaping");
        assert_eq!(document.line_count(), 2);
    }

    #[test]
    fn first_line_font_metrics_participate_in_line_breaking() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        let prepared = factory.parse_with_new_pipeline("<html><body><p>aa aa aa</p></body></html>", Some("html,body,p { margin:0 } p { font-size:10px } p::first-line { font-size:20px; font-weight:700; font-family:serif }"));
        let shaped = prepared.shape(&mut glyph_cache).expect("fixture shaping must succeed");
        let document = shaped.layout_with_metrics_and_shaper(crate::LayoutConstraints::new(28.0, 16.0).unwrap(), &crate::ImageMetrics::default(), &mut glyph_cache).expect("first-line refinement must shape successfully");

        assert_eq!(line_text(&document, 0).trim_end(), "aa", "20px pseudo text must not retain the 10px-font boundary");
        let first = document.render_view().text().line(0).expect("first line");
        let first_glyph = document.render_view().text().glyph_slice(first.glyphs()).expect("first-line glyphs")[0];
        assert_eq!(document.render_view().text().glyph_metric(first_glyph).expect("first glyph metric").advance(), 10.0);
    }

    #[test]
    fn ancestor_first_line_reaches_the_first_in_flow_block_descendant() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        let document = factory
            .parse_with_new_pipeline("<!doctype html><html><body style='margin:0'><div id='outer'><p style='margin:0'>First line</p><p>Second block</p></div></body></html>", Some("#outer::first-line { color:green }"))
            .shape(&mut glyph_cache)
            .expect("descendant first-line glyphs shape")
            .layout(crate::LayoutConstraints::new(500.0, 16.0).unwrap());

        assert_eq!(document.render_view().text().line(0).expect("first line").paint_color(), Some(0x008000ff));
        assert_eq!(document.render_view().text().line(1).expect("second block line").paint_color(), None);
    }

    #[test]
    fn first_line_spacing_participates_in_line_breaking() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        let prepared = factory.parse_with_new_pipeline("<html><body><p>aa aa aa</p></body></html>", Some("html,body,p { margin:0 } p { font-size:10px } p::first-line { letter-spacing:5px; word-spacing:5px }"));
        let shaped = prepared.shape(&mut glyph_cache).expect("fixture shaping must succeed");
        let document = shaped.layout_with_metrics_and_shaper(crate::LayoutConstraints::new(31.0, 16.0).unwrap(), &crate::ImageMetrics::default(), &mut glyph_cache).expect("first-line spacing refinement must shape successfully");

        assert_eq!(line_text(&document, 0).trim_end(), "aa", "pseudo spacing must be visible to the line breaker");
    }

    #[test]
    fn first_line_relayout_discards_the_previous_widths_pseudo_extent() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        let prepared = factory.parse_with_new_pipeline("<html><body><p>aa aa aa aa</p></body></html>", Some("html,body,p { margin:0 } p { font-size:10px } p::first-line { font-size:20px }"));
        let shaped = prepared.shape(&mut glyph_cache).expect("fixture shaping must succeed");
        let mut document = shaped.layout_with_metrics_and_shaper(crate::LayoutConstraints::new(28.0, 16.0).unwrap(), &crate::ImageMetrics::default(), &mut glyph_cache).expect("initial first-line refinement must succeed");
        assert_eq!(line_text(&document, 0).trim_end(), "aa");

        document.relayout_with_metrics_and_shaper(crate::LayoutConstraints::new(55.0, 16.0).unwrap(), &crate::ImageMetrics::default(), &mut glyph_cache).expect("width-dependent first-line refinement must succeed");
        assert_eq!(line_text(&document, 0).trim_end(), "aa aa");

        let second = document.render_view().text().line(1).expect("second line");
        let second_glyph = document.render_view().text().glyph_slice(second.glyphs()).expect("second-line glyphs")[0];
        assert_eq!(document.render_view().text().glyph_metric(second_glyph).expect("second-line metric").advance(), 5.0, "text outside the new first line must use the base 10px shape");
    }

    fn layout_html(html: &str, width: f64) -> LaidOutDocument {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        factory.parse_with_new_pipeline(html, None).shape(&mut glyph_cache).expect("test shaper must register every glyph").layout(crate::LayoutConstraints::new(width, 16.0).unwrap())
    }

    #[test]
    fn nested_inline_left_padding_separates_opening_borders() {
        let document =
            layout_html("<html><body style='margin:0'><div style='display:inline;border-left:5px solid blue;padding-left:50px'>\n  <div style='display:inline;border-left:5px solid orange'>Filler Text</div>\n</div></body></html>", 400.0);
        let fragments = document.render_view().text().line_text_fragments(0).expect("inline text line").collect::<Vec<_>>();

        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].offset_x(), 60.0, "5px outer border + 50px padding + 5px nested border must precede the text");
        let borders = document.render_view().fragments().decorations().iter().filter(|decoration| decoration.is_inline()).map(|decoration| (decoration.color(), decoration.rect())).collect::<Vec<_>>();
        assert!(borders.iter().any(|(color, rect)| *color == 0x0000ffff && rect.x0 == 0.0 && rect.x1 == 5.0), "outer opening border must paint before its padding: {borders:?}");
        assert!(borders.iter().any(|(color, rect)| *color == 0xffa500ff && rect.x0 == 55.0 && rect.x1 == 60.0), "nested opening border must paint after the outer padding: {borders:?}");
    }

    #[test]
    fn empty_inline_edges_are_published_as_line_fragments() {
        let document = layout_html("<html><body style='margin:0'><span style='padding:0 7px;border:3px solid blue'></span>x<span style='padding:0 5px;border:2px solid orange'></span></body></html>", 200.0);
        let decorations = document.render_view().fragments().decorations().iter().filter(|fragment| fragment.is_inline()).map(|fragment| (fragment.color(), fragment.rect(), fragment.line_idx())).collect::<Vec<_>>();
        let blue = decorations.iter().filter(|(color, _, _)| *color == 0x0000ffff).collect::<Vec<_>>();
        let orange = decorations.iter().filter(|(color, _, _)| *color == 0xffa500ff).collect::<Vec<_>>();

        assert_eq!(blue.len(), 4, "the first empty inline must retain four border edges: {decorations:?}");
        assert_eq!(orange.len(), 4, "the second empty inline must retain four border edges: {decorations:?}");
        assert!(blue.iter().all(|(_, _, line)| line.is_some()) && orange.iter().all(|(_, _, line)| line.is_some()), "empty inline borders must be owned by their line");
        let blue_right = blue.iter().map(|(_, rect, _)| rect.x1).fold(f64::NEG_INFINITY, f64::max);
        let orange_left = orange.iter().map(|(_, rect, _)| rect.x0).fold(f64::INFINITY, f64::min);
        assert!(orange_left >= blue_right, "successive empty fragments must retain source-order geometry: {decorations:?}");
    }

    #[test]
    fn empty_inline_line_height_uses_font_ascent_plus_half_leading() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::with_font_relative_metrics(0.5, 0.9);
        let candidate = factory
            .parse_with_new_pipeline("<html><body style='margin:0'><div style='font-size:16px;line-height:1'><span style='line-height:5'></span>X</div></body></html>", None)
            .shape(&mut glyph_cache)
            .expect("candidate shaping")
            .layout(crate::LayoutConstraints::new(200.0, 16.0).unwrap());
        let reference = factory
            .parse_with_new_pipeline("<html><body style='margin:0'><div style='font-size:16px;line-height:5'>X</div></body></html>", None)
            .shape(&mut glyph_cache)
            .expect("reference shaping")
            .layout(crate::LayoutConstraints::new(200.0, 16.0).unwrap());
        let candidate_line = candidate.render_view().text().line(0).expect("candidate line");
        let reference_line = reference.render_view().text().line(0).expect("reference line");

        assert!((candidate_line.height() - reference_line.height()).abs() < 1e-5, "candidate={candidate_line:?}, reference={reference_line:?}");
        assert!((candidate_line.baseline() - reference_line.baseline()).abs() < 1e-5, "candidate={candidate_line:?}, reference={reference_line:?}");
    }

    #[test]
    fn line_breaking_uses_native_run_advances_instead_of_character_fallbacks() {
        let document = layout_with_native_geometry("<html><body style='margin:0'><p style='margin:0;overflow-wrap:anywhere'>abcd</p></body></html>", 7.0, vec![3.0; 4], vec![true; 5]);

        assert_eq!(document.line_count(), 2);
        assert_eq!(line_text(&document, 0), "ab");
        assert_eq!(line_text(&document, 1), "cd");
    }

    #[test]
    fn emergency_wrapping_never_splits_a_native_cluster() {
        let document = layout_with_native_geometry("<html><body style='margin:0'><p style='margin:0;word-break:break-all'>abc</p></body></html>", 4.0, vec![3.0; 3], vec![true, false, true, true]);

        assert_eq!(document.line_count(), 2);
        assert_eq!(line_text(&document, 0), "ab");
        assert_eq!(line_text(&document, 1), "c");
    }

    #[test]
    fn justification_uses_the_native_shaped_line_width() {
        let document = layout_with_native_geometry("<html><body style='margin:0'><p style='margin:0;text-align:justify'>a a a</p></body></html>", 7.0, vec![2.0; 5], vec![true; 6]);

        let first = document.render_view().text().line(0).expect("wrapped first line");
        assert_eq!(line_text(&document, 0), "a a");
        assert!((first.word_spacing() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn book_composition_optimizes_left_aligned_paragraphs_without_changing_web_compatible_breaks() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:left'>aa aa b c</p></body></html>";
        let advances = vec![10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 5.0, 10.0, 5.0];
        let boundaries = vec![true; advances.len() + 1];
        let web = layout_with_native_geometry_policy(html, 70.0, advances.clone(), boundaries.clone(), crate::TextCompositionPolicy::WebCompatible);
        let book = layout_with_native_geometry_policy(html, 70.0, advances, boundaries, crate::TextCompositionPolicy::BookOptimized);

        assert_eq!((line_text(&web, 0), line_text(&web, 1)), ("aa aa b".to_owned(), "c".to_owned()), "web-compatible layout must retain greedy first-fit wrapping");
        assert_eq!((line_text(&book, 0), line_text(&book, 1)), ("aa aa".to_owned(), "b c".to_owned()), "book composition should improve the paragraph shape globally");
        for line_idx in 0..book.line_count() {
            let line = book.render_view().text().line(line_idx).expect("book line");
            assert_eq!(line.word_spacing(), 0.0, "ragged composition must preserve natural word spacing");
            assert_eq!(line.letter_spacing(), 0.0, "ragged composition must not justify through tracking");
        }
    }

    #[test]
    fn book_composition_hangs_opening_punctuation_without_changing_web_geometry() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:left'>“opening</p></body></html>";
        let advances = vec![10.0; 8];
        let boundaries = vec![true; advances.len() + 1];
        let web = layout_with_native_geometry_policy(html, 200.0, advances.clone(), boundaries.clone(), crate::TextCompositionPolicy::WebCompatible);
        let book = layout_with_native_geometry_policy(html, 200.0, advances, boundaries, crate::TextCompositionPolicy::BookOptimized);

        assert_eq!(web.render_view().text().line(0).expect("web line").optical_offset_x(), 0.0);
        assert!(book.render_view().text().line(0).expect("book line").optical_offset_x() < 0.0, "the opening quote should protrude into the left margin");
        assert_eq!(line_text(&web, 0), line_text(&book, 0), "optical alignment must not alter source ranges or line breaking");
    }

    #[test]
    fn book_composition_hangs_terminal_punctuation_on_a_right_aligned_line() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:right'>ending.”</p></body></html>";
        let advances = vec![10.0; 8];
        let boundaries = vec![true; advances.len() + 1];
        let book = layout_with_native_geometry_policy(html, 200.0, advances, boundaries, crate::TextCompositionPolicy::BookOptimized);

        assert!(book.render_view().text().line(0).expect("book line").optical_offset_x() > 0.0, "the closing quote should protrude into the right margin");
    }

    #[test]
    fn book_justification_reserves_optical_room_for_terminal_punctuation() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:justify'>a a.” bbbbb</p></body></html>";
        let advances = vec![10.0; 11];
        let boundaries = vec![true; advances.len() + 1];
        let book = layout_with_native_geometry_policy(html, 50.0, advances, boundaries, crate::TextCompositionPolicy::BookOptimized);

        let first = book.render_view().text().line(0).expect("first justified line");
        assert_eq!(line_text(&book, 0), "a a.”");
        assert!(first.word_spacing() > 0.0 || first.letter_spacing() > 0.0, "terminal punctuation should receive bounded optical room beyond the logical measure");
        assert_eq!(first.optical_offset_x(), 0.0, "right protrusion must not disturb the aligned left edge");
    }

    #[test]
    fn book_composition_hangs_terminal_dashes_on_right_aligned_lines() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:right'>ending—</p></body></html>";
        let advances = vec![10.0; 7];
        let boundaries = vec![true; advances.len() + 1];
        let book = layout_with_native_geometry_policy(html, 200.0, advances, boundaries, crate::TextCompositionPolicy::BookOptimized);

        assert!(book.render_view().text().line(0).expect("book line").optical_offset_x() > 0.0, "the terminal dash should protrude into the right margin");
    }

    #[test]
    fn centered_text_is_not_optically_shifted() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:center'>“centered”</p></body></html>";
        let advances = vec![10.0; 10];
        let boundaries = vec![true; advances.len() + 1];
        let book = layout_with_native_geometry_policy(html, 200.0, advances, boundaries, crate::TextCompositionPolicy::BookOptimized);

        assert_eq!(book.render_view().text().line(0).expect("book line").optical_offset_x(), 0.0);
    }

    #[test]
    fn justification_shrinks_spaces_when_knuth_plass_selects_a_tight_line() {
        let document = layout_with_native_geometry("<html><body style='margin:0'><p style='margin:0;text-align:justify'>a a a</p></body></html>", 5.5, vec![2.0; 5], vec![true; 6]);

        let first = document.render_view().text().line(0).expect("wrapped first line");
        assert_eq!(line_text(&document, 0), "a a");
        assert!((first.word_spacing() + 0.5).abs() < f64::EPSILON, "tight line must shrink its one space by 0.5px, got {}", first.word_spacing());
    }

    #[test]
    fn book_justification_distributes_space_by_punctuation_context() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:justify'>a, a a— a bbbbb</p></body></html>";
        let document = layout_with_native_geometry_policy(html, 20.85, vec![2.0; 15], vec![true; 16], crate::TextCompositionPolicy::BookOptimized);
        let text = document.render_view().text();
        let first = text.line(0).expect("first justified line");
        let advances = text.line_glyph_advances(0).expect("line advance overrides").iter().map(|run| (run.range().start, run.advance())).collect::<Vec<_>>();

        assert_eq!(line_text(&document, 0), "a, a a— a");
        assert_eq!(first.word_spacing(), 0.0, "specialized spaces use exact advances instead of one uniform line value");
        assert_eq!(advances.len(), 3);
        assert_eq!(advances.iter().map(|(index, _)| *index).collect::<Vec<_>>(), vec![2, 4, 7]);
        assert!(advances[0].1 > advances[1].1, "space after a comma should receive more expansion than an ordinary word space: {advances:?}");
        assert!(advances[1].1 > advances[2].1, "space after a dash should receive less expansion than an ordinary word space: {advances:?}");
        let total_expansion = advances.iter().map(|(_, advance)| f64::from(*advance) - 2.0).sum::<f64>();
        assert!((total_expansion - 2.85).abs() < 1.0e-6, "painted spaces must consume the same punctuation-aware capacity as the breaker");
    }

    #[test]
    fn web_justification_retains_uniform_space_adjustment() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:justify'>a, a a— a bbbbb</p></body></html>";
        let document = layout_with_native_geometry_policy(html, 21.0, vec![2.0; 15], vec![true; 16], crate::TextCompositionPolicy::WebCompatible);
        let text = document.render_view().text();
        let first = text.line(0).expect("first justified line");

        assert_eq!(line_text(&document, 0), "a, a a— a");
        assert!((first.word_spacing() - 1.0).abs() < 1.0e-9);
        assert!(text.line_glyph_advances(0).expect("line advance list").is_empty(), "web-compatible justification must not add book punctuation overrides");
    }

    #[test]
    fn book_justification_distributes_space_contraction_by_punctuation_context() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:justify'>a, a a— a bbbbb</p></body></html>";
        let document = layout_with_native_geometry_policy(html, 16.5, vec![2.0; 15], vec![true; 16], crate::TextCompositionPolicy::BookOptimized);
        let text = document.render_view().text();
        let advances = text.line_glyph_advances(0).expect("line advance overrides").iter().map(|run| run.advance()).collect::<Vec<_>>();

        assert_eq!(line_text(&document, 0), "a, a a— a");
        assert_eq!(advances.len(), 3);
        assert!(advances[1] < advances[0], "ordinary word space should accept more contraction than comma spacing: {advances:?}");
        assert!(advances[0] < advances[2], "dash-adjacent space should resist contraction most strongly: {advances:?}");
        let total_contraction = advances.iter().map(|advance| f64::from(*advance) - 2.0).sum::<f64>();
        assert!((total_contraction + 1.5).abs() < 1.0e-6, "weighted contraction must preserve the breaker's total adjustment");
    }

    #[test]
    fn emergency_justification_leaves_residual_space_on_the_right() {
        let document = layout_with_native_geometry("<html><body style='margin:0'><p style='margin:0;text-align:justify'>a a a a</p></body></html>", 8.5, vec![2.0; 7], vec![true; 8]);

        let first = document.render_view().text().line(0).expect("emergency short first line");
        assert_eq!(line_text(&document, 0), "a a");
        assert!((first.word_spacing() - 1.0).abs() < f64::EPSILON, "space expansion must stop at 50%; got {}", first.word_spacing());
        assert!((first.letter_spacing() - 0.015).abs() < 1.0e-9, "micro-tracking must stop after consuming 0.5% of the 6px natural line width; got {}", first.letter_spacing());
    }

    #[test]
    fn emergency_justification_uses_tiny_positive_cluster_tracking_before_raggedness() {
        let document = layout_with_native_geometry("<html><body style='margin:0'><p style='margin:0;text-align:justify'>aaaa aaaa aaaa</p></body></html>", 19.04, vec![2.0; 14], vec![true; 15]);

        let first = document.render_view().text().line(0).expect("micro-tracked first line");
        assert_eq!(line_text(&document, 0), "aaaa aaaa");
        assert!((first.word_spacing() - 1.0).abs() < 1.0e-9);
        assert!((first.letter_spacing() * 8.0 - 0.04).abs() < 1.0e-6, "eight shaped-cluster boundaries must share the 0.04px residual; got {}", first.letter_spacing());
    }

    #[test]
    fn emergency_justification_uses_tiny_negative_cluster_tracking_after_space_shrink() {
        let document = layout_with_native_geometry("<html><body style='margin:0'><p style='margin:0;text-align:justify'>aa aa aa</p></body></html>", 9.32, vec![2.0; 8], vec![true; 9]);

        let first = document.render_view().text().line(0).expect("micro-tracked tight first line");
        assert_eq!(line_text(&document, 0), "aa aa");
        assert!((first.word_spacing() + 0.66).abs() < 1.0e-9);
        assert!((first.letter_spacing() * 4.0 + 0.02).abs() < 1.0e-6, "four shaped-cluster boundaries must share the -0.02px residual; got {}", first.letter_spacing());
    }

    #[test]
    fn automatic_hyphenation_emits_synthetic_non_source_glyphs() {
        let document = layout_html("<html lang='en'><body style='margin:0'><p style='margin:0;text-align:justify;hyphens:auto'>internationalization internationalization</p></body></html>", 72.0);
        let text = document.render_view().text();
        let hyphenated_line = (0..document.line_count()).find(|&line_idx| text.hyphen_for_line(line_idx).is_some());

        assert!(hyphenated_line.is_some(), "a narrow English justified paragraph should select at least one dictionary breakpoint");
        let source = (0..document.line_count()).map(|line_idx| line_text(&document, line_idx)).collect::<String>();
        assert!(!source.contains('\u{2010}'), "synthetic hyphens must not enter source text, selection, or CFI ranges");
    }

    #[test]
    fn automatic_hyphenation_requires_two_characters_on_each_side() {
        assert!(!automatic_hyphen_fragments_are_long_enough(1, 6));
        assert!(automatic_hyphen_fragments_are_long_enough(2, 6));
        assert!(automatic_hyphen_fragments_are_long_enough(4, 6));
        assert!(!automatic_hyphen_fragments_are_long_enough(5, 6));
    }

    #[test]
    fn manual_soft_hyphen_is_zero_width_and_emits_a_synthetic_hyphen_when_used() {
        let document = layout_html("<html lang='en'><body style='margin:0'><p style='margin:0;hyphens:manual'>extra&shy;ordinary tail</p></body></html>", 56.0);
        let text = document.render_view().text();
        let soft_hyphen =
            (0..text.glyph_count() as u32).find(|&index| text.glyph_at(index as usize).and_then(|glyph| text.glyph_metric(glyph)).is_some_and(|metric| metric.ch() == '\u{00ad}')).expect("fixture contains a source soft hyphen");

        assert_eq!(text.character_advance(soft_hyphen), Some(0.0), "an unbroken soft hyphen must consume no inline width");
        assert!((0..document.line_count()).any(|line_idx| text.hyphen_for_line(line_idx).is_some()), "manual hyphenation should use the authored conditional break");
        let source = (0..document.line_count()).map(|line_idx| line_text(&document, line_idx)).collect::<String>();
        assert!(source.contains('\u{00ad}'), "the authored marker remains part of the underlying source");
        assert!(!source.contains('\u{2010}'), "the displayed hyphen must remain synthetic");
    }

    #[test]
    fn hyphens_none_suppresses_an_authored_soft_hyphen_break() {
        let document = layout_html("<html lang='en'><body style='margin:0'><p style='margin:0;hyphens:none'>extra&shy;ordinary tail</p></body></html>", 56.0);
        let text = document.render_view().text();
        let soft_hyphen =
            (0..text.glyph_count() as u32).find(|&index| text.glyph_at(index as usize).and_then(|glyph| text.glyph_metric(glyph)).is_some_and(|metric| metric.ch() == '\u{00ad}')).expect("fixture contains a source soft hyphen");

        assert_eq!(text.character_advance(soft_hyphen), Some(0.0), "a suppressed soft hyphen must remain invisible");
        assert!((0..document.line_count()).all(|line_idx| text.hyphen_for_line(line_idx).is_none()), "hyphens:none must suppress conditional hyphen rendering");
    }

    #[test]
    fn nowrap_suppresses_an_authored_soft_hyphen_break() {
        let document = layout_html("<html lang='en'><body style='margin:0'><p style='margin:0;white-space:nowrap;hyphens:manual'>extra&shy;ordinary tail</p></body></html>", 56.0);
        let text = document.render_view().text();

        assert_eq!(document.line_count(), 1, "a conditional hyphen is still a soft wrap opportunity and must obey nowrap");
        assert!(text.hyphen_for_line(0).is_none());
    }

    #[test]
    fn automatic_hyphenation_prefers_an_authored_soft_hyphen_in_the_word() {
        let document = layout_html("<html lang='en'><body style='margin:0'><p style='margin:0;hyphens:auto'>inter&shy;nationalization tail</p></body></html>", 56.0);
        let text = document.render_view().text();
        let hyphenated_line = (0..document.line_count()).find(|&line_idx| text.hyphen_for_line(line_idx).is_some()).expect("the authored conditional break should fit the narrow line");

        assert!(line_text(&document, hyphenated_line).ends_with('\u{00ad}'), "automatic dictionary points in a word with an authored soft hyphen must not supersede it");
    }

    fn layout_html_with_images(html: &str, width: f64, image_sizes: &[(u32, u32)], policy: crate::ImageSizingPolicy) -> LaidOutDocument {
        layout_html_with_images_in_viewport(html, width, None, image_sizes, policy)
    }

    fn layout_html_with_images_in_viewport(html: &str, width: f64, height: Option<f64>, image_sizes: &[(u32, u32)], policy: crate::ImageSizingPolicy) -> LaidOutDocument {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        let shaped = factory.parse_with_new_pipeline(html, None).shape(&mut glyph_cache).expect("test shaper must register every glyph");
        let mut metrics = crate::ImageMetrics::default();
        for (image_idx, &(image_width, image_height)) in image_sizes.iter().enumerate() {
            metrics.set(image_idx as u32, image_width, image_height);
        }
        let constraints = crate::LayoutConstraints::new(width, 16.0).unwrap().with_viewport_height(height).unwrap().with_image_sizing_policy(policy);
        shaped.layout_with_metrics(constraints, &metrics)
    }

    fn tag_size(document: &LaidOutDocument, tag: &str) -> kurbo::Size {
        let boxes = document.render_view().boxes();
        let box_idx = (0..boxes.len()).find(|&idx| boxes.tag(idx).is_some_and(|candidate| candidate.eq_ignore_ascii_case(tag))).unwrap_or_else(|| panic!("{tag} box should exist"));
        boxes.size(box_idx).expect("box index in range")
    }

    fn tag_point(document: &LaidOutDocument, tag: &str) -> kurbo::Point {
        let boxes = document.render_view().boxes();
        let box_idx = (0..boxes.len()).find(|&idx| boxes.tag(idx).is_some_and(|candidate| candidate.eq_ignore_ascii_case(tag))).unwrap_or_else(|| panic!("{tag} box should exist"));
        boxes.point(box_idx).expect("box index in range")
    }

    fn line_text(document: &LaidOutDocument, line_idx: usize) -> String {
        let line = document.render_view().text().line(line_idx).expect("line index in range");
        let text = document.render_view().text();
        text.glyph_slice(line.glyphs()).expect("line glyph range").iter().map(|&glyph| text.glyph_metric(glyph).expect("registered glyph").ch()).collect()
    }

    fn tab_advance(document: &LaidOutDocument, line_idx: usize) -> f64 {
        document.render_view().text().line_glyph_advances(line_idx).expect("advance list for line").iter().next().expect("preserved tab advance").advance()
    }

    fn alignment_token(vertical_align: VerticalAlignValue) -> (InlineToken, [InlineTokenMetrics; 1]) {
        let mut token = InlineToken::new(InlineTokenKind::Glyph { glyph_idx: 0 }, 8.0, BreakKind::None, TokenWrap::Normal, true);
        token.run_idx = 0;
        let metrics = [InlineTokenMetrics {
            owner_box_idx: u32::MAX,
            ascent: 8.0,
            descent: 2.0,
            line_height: 12.0,
            tab_interval: -1.0,
            tab_min_advance: 0.0,
            font_size: 16.0,
            vertical_align,
            white_space: WhiteSpace::Normal,
            visible: true,
            placement_required: false,
        }];
        (token, metrics)
    }

    #[test]
    fn common_inline_token_stays_cache_compact() {
        assert_eq!(std::mem::size_of::<InlineToken>(), 16, "dense inline token grew to {} bytes", std::mem::size_of::<InlineToken>());
        assert!(std::mem::size_of::<InlineTokenKind>() <= 8, "rare replaced geometry leaked back into the token discriminant");
    }

    #[test]
    fn adjacent_tokens_share_run_metrics() {
        let (first, [metrics]) = alignment_token(VerticalAlignValue::Baseline);
        let second = InlineToken::new(InlineTokenKind::Glyph { glyph_idx: 1 }, 8.0, BreakKind::None, TokenWrap::Normal, true);
        let mut tokens = InlineTokens::with_capacity(2);

        tokens.push(first, metrics);
        tokens.push(second, metrics);

        assert_eq!(tokens.runs.len(), 1);
        assert_eq!(tokens.dense[0].run_idx, tokens.dense[1].run_idx);
    }

    #[test]
    fn plain_text_uses_implicit_zero_glyph_offsets() {
        let document = layout_html("<html><body style='margin:0'><span style='line-height:12px'>ab</span><span style='line-height:18px'>cd</span></body></html>", 200.0);
        let offsets = document.render_view().text().line_glyph_offsets(0).expect("plain line offset runs").iter().collect::<Vec<_>>();

        assert!(offsets.is_empty(), "zero is the implicit glyph offset");
    }

    #[test]
    fn percentage_baseline_shift_uses_the_elements_line_height() {
        let (token, runs) = alignment_token(VerticalAlignValue::Percent(0.5));

        assert_eq!(vertical_align_offset(&token, &runs, 40.0, 10.0), 6.0);
    }

    #[test]
    fn calc_baseline_shift_keeps_absolute_and_line_height_components() {
        let (token, runs) = alignment_token(VerticalAlignValue::Calc { absolute_px: -2.0, line_height_fraction: 0.5, x_height_px: 0.0 });

        assert_eq!(vertical_align_offset(&token, &runs, 40.0, 10.0), 4.0);
    }

    #[test]
    fn top_and_bottom_alignment_include_the_inline_half_leading() {
        for alignment in [VerticalAlignValue::Top, VerticalAlignValue::Bottom] {
            let (token, runs) = alignment_token(alignment);

            assert_eq!(vertical_align_offset(&token, &runs, 12.0, 9.0), 0.0);
        }
    }

    #[test]
    fn text_top_and_bottom_align_to_parent_font_content_not_line_edges() {
        assert_eq!(super::vertical_align_metrics_offset(VerticalAlignValue::TextTop, 8.0, 2.0, 20.0, 10.0, 5.0, 40.0, 25.0, 8.0, 2.0), -5.0);
        assert_eq!(super::vertical_align_metrics_offset(VerticalAlignValue::TextBottom, 8.0, 2.0, 20.0, 10.0, 5.0, 40.0, 25.0, 8.0, 2.0), 5.0);
    }

    #[test]
    fn authored_calc_vertical_align_reaches_final_glyph_offsets() {
        let document = layout_html("<html><body><p style='font-size:16px; line-height:12px'>A<span style='vertical-align:calc(50% - 2px)'>B</span></p></body></html>", 200.0);
        let offset = document.render_view().text().line_glyph_offsets(0).expect("first line offset runs").iter().find(|run| run.range().contains(&1)).expect("the shifted glyph has an offset").offset();

        assert!((offset - 4.0).abs() < 0.001, "calc shift should preserve both components, got {offset}");
    }

    #[test]
    fn table_cell_alignment_does_not_shift_its_anonymous_inline_text() {
        let document = layout_html(
            "<html><body style='margin:0'><table><tr style='vertical-align:middle'><td>XXXXX<span style='font-size:2em;vertical-align:baseline'>XXXXX</span></td></tr></table></body></html>",
            300.0,
        );
        let text = document.render_view().text();
        let line_idx = (0..text.line_count()).find(|&line_idx| line_text(&document, line_idx) == "XXXXXXXXXX").expect("table cell text line");
        let offsets = text.line_glyph_offsets(line_idx).expect("line offset runs");
        let offset_for = |glyph| offsets.iter().find(|run| run.range().contains(&glyph)).map_or(0.0, |run| run.offset());

        assert!(offset_for(0).abs() < 0.001, "the cell's row-alignment value must not shift direct text inside its line");
        assert!(offset_for(5).abs() < 0.001, "the baseline-aligned span must share the anonymous inline's baseline");
    }

    #[test]
    fn raised_inline_expands_the_line_and_moves_the_unshifted_baseline() {
        let document = layout_html("<html><body style='margin:0'><div style='font-size:20px;line-height:20px'><span style='vertical-align:96px'>X</span>X</div></body></html>", 200.0);
        let line = document.render_view().text().line(0).expect("text produces one line");
        let offsets = document.render_view().text().line_glyph_offsets(0).expect("raised glyph requires explicit offsets");
        let offset_for = |glyph| offsets.iter().find(|run| run.range().contains(&glyph)).map_or(0.0, |run| run.offset());

        assert!((line.height() - 116.0).abs() < 0.001, "a 96px raise above a 20px strut must produce a 116px line, got {}", line.height());
        assert!((offset_for(0) - 96.0).abs() < 0.001);
        assert!(offset_for(1).abs() < 0.001);
    }

    #[test]
    fn descendant_text_does_not_erase_an_ancestor_inline_box_strut() {
        let document = layout_html(
            "<html><body style='margin:0'><div style='font-size:10px;line-height:10px'><span style='font-size:30px;line-height:30px'><span style='font-size:10px;line-height:10px'>A</span></span>B</div></body></html>",
            200.0,
        );
        let line = document.render_view().text().line(0).expect("text produces one line");

        assert!((line.height() - 30.0).abs() < 0.001, "the ancestor inline's own strut must participate even though its only glyph belongs to a smaller descendant; got {}", line.height());
    }

    #[test]
    fn nested_vertical_align_moves_descendants_without_inheriting_the_value() {
        let document = layout_html("<html><body><p style='font-size:16px;line-height:20px'>A<span style='vertical-align:5px'>B<span style='vertical-align:3px'>C</span></span></p></body></html>", 200.0);
        let offsets = document.render_view().text().line_glyph_offsets(0).expect("aligned descendants require explicit offsets");
        let offset_for = |glyph| offsets.iter().find(|run| run.range().contains(&glyph)).map_or(0.0, |run| run.offset());

        assert!((offset_for(1) - 5.0).abs() < 0.001, "the parent span applies its own alignment once");
        assert!((offset_for(2) - 8.0).abs() < 0.001, "the child moves with its parent and adds its own alignment");
    }

    #[test]
    fn top_alignment_moves_the_complete_inline_subtree_as_one_group() {
        let document = layout_html(
            "<html><body style='margin:0'><span style='vertical-align:top'><span style='font-size:80px'>A</span>x</span></body></html>",
            200.0,
        );
        let line = document.render_view().text().line(0).expect("grouped inline line");
        let offsets = document.render_view().text().line_glyph_offsets(0).expect("grouped inline offsets");
        let offset_for = |glyph| offsets.iter().find(|run| run.range().contains(&glyph)).map_or(0.0, |run| run.offset());

        assert!((line.height() - 96.0).abs() < 0.001, "the 80px descendant's normal line area determines the group height");
        assert!((offset_for(0) - offset_for(1)).abs() < 0.001, "top moves the complete subtree as one group rather than independently aligning its anonymous inlines: A={}, x={}", offset_for(0), offset_for(1));
    }

    #[test]
    fn top_and_bottom_groups_expand_opposite_line_edges() {
        let document = layout_html("<html><body style='margin:0'><div style='width:180px;font:30px/1 Ahem'>x<span style='font-size:60px;vertical-align:bottom'>X</span>x</div><div style='width:180px;font:30px/1 Ahem'>x<span style='font-size:60px;vertical-align:top'>X</span>x</div></body></html>", 300.0);
        let text = document.render_view().text();
        let bottom = text.line(0).expect("bottom-aligned line");
        let top = text.line(1).expect("top-aligned line");

        assert_eq!(bottom.height(), 60.0);
        assert_eq!(top.height(), 60.0);
        assert!((bottom.baseline() - 52.5).abs() < 0.001, "a taller bottom group expands above the ordinary strut");
        assert!((top.baseline() - 24.0).abs() < 0.001, "a taller top group expands below the ordinary strut");
    }

    #[test]
    fn authored_line_height_can_be_smaller_than_glyph_extents_and_layout_fallback() {
        let document = layout_html("<html><body style='margin:0'><div style='line-height:13px'>Text</div></body></html>", 200.0);
        let line = document.render_view().text().line(0).expect("text produces one line");

        assert!((line.height() - 13.0).abs() < 0.001, "authored line-height must not be clamped to the 16px layout fallback");
    }

    #[test]
    fn zero_line_height_preserves_negative_half_leading() {
        let document = layout_html("<html><body style='margin:0'><div style='font:20px/1 Ahem;line-height:0;width:20px'>X X</div></body></html>", 200.0);
        let text = document.render_view().text();
        let first = text.line(0).expect("first wrapped line");
        let second = text.line(1).expect("second wrapped line");

        assert_eq!(first.height(), 0.0);
        assert_eq!(second.height(), 0.0);
        assert_eq!(first.point().y, second.point().y, "zero-height lines advance by zero while their glyph ink overflows");
    }

    #[test]
    fn ex_line_height_uses_the_selected_faces_x_height() {
        let document = layout_html("<html><body style='margin:0'><div style='font-size:20px;line-height:1ex;width:20px'>X X</div></body></html>", 200.0);
        let text = document.render_view().text();

        assert_eq!(text.line(0).expect("first wrapped line").height(), 10.0);
        assert_eq!(text.line(1).expect("second wrapped line").height(), 10.0);
    }

    #[test]
    fn zero_sized_inline_text_does_not_remove_the_parent_line_strut() {
        let document = layout_html("<html><body style='margin:0'><p style='margin:0;font-size:16px;line-height:1.2'><span style='font-size:0'>zero</span></p></body></html>", 200.0);
        let line = document.render_view().text().line(0).expect("zero-sized descendant still creates the parent's line box");

        assert!((line.height() - 19.2).abs() < 0.001, "the parent strut controls the line height, got {}", line.height());
    }

    #[test]
    fn empty_float_anchor_does_not_inflate_zero_line_height() {
        let p = "style='font-size:20px;line-height:0'";
        let candidate = layout_html(&format!("<html><body><p {p}><span style='position:absolute'></span>Some paragraph</p><p {p}><span style='float:left'></span>Some paragraph</p><p {p}>Some other paragraph</p></body></html>"), 800.0);
        let reference = layout_html(&format!("<html><body><p {p}>Some paragraph</p><p {p}>Some paragraph</p><p {p}>Some other paragraph</p></body></html>"), 800.0);
        let candidate_lines = candidate.render_view().text().lines();
        let reference_lines = reference.render_view().text().lines();

        assert_eq!(candidate_lines.len(), reference_lines.len());
        for (line_idx, (candidate_line, reference_line)) in candidate_lines.iter().zip(reference_lines.iter()).enumerate() {
            assert_eq!(candidate_line.point(), reference_line.point(), "line {line_idx} point");
            assert_eq!(candidate_line.height(), reference_line.height(), "line {line_idx} height");
            assert_eq!(candidate_line.baseline(), reference_line.baseline(), "line {line_idx} baseline");
        }
    }

    #[test]
    fn collapsible_indentation_around_float_only_content_does_not_create_a_carrier_line() {
        let style = "style='float:right;width:60px;height:20px;margin:4px'";
        let indented = layout_html(&format!("<html><body style='margin:0'><div id='host'>\n <p id='b' {style}>B</p>\n <p id='a' {style}>A</p>\n</div><p id='after'>after</p></body></html>"), 300.0);
        let compact = layout_html(&format!("<html><body style='margin:0'><div id='host'><p id='b' {style}>B</p><p id='a' {style}>A</p></div><p id='after'>after</p></body></html>"), 300.0);
        let point_for = |document: &LaidOutDocument, id: &str| {
            let box_idx = (0..document.render_view().boxes().len()).find(|&idx| document.render_view().boxes().attribute(idx, "id") == Some(id)).expect("fixture box");
            document.render_view().boxes().point(box_idx).expect("box geometry")
        };

        for id in ["host", "a", "b", "after"] {
            assert_eq!(point_for(&indented, id), point_for(&compact, id), "collapsible source indentation must not change {id} geometry");
        }
        assert_eq!(indented.line_count(), compact.line_count(), "a float-only anonymous formatting context must not publish a whitespace carrier line");
    }

    #[test]
    fn relatively_positioned_inline_image_moves_without_consuming_flow_space() {
        let document = layout_html_with_images(
            "<html><body style='margin:0'><img id='shifted' src='a' style='position:relative;left:2px;width:20px;height:20px'/><img id='following' src='b' style='width:20px;height:20px'/></body></html>",
            200.0,
            &[(20, 20), (20, 20)],
            crate::ImageSizingPolicy::WebCompatible,
        );
        let view = document.render_view();
        let box_for_id = |id| (0..view.boxes().len()).find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some(id)).expect("image box");
        let shifted = view.boxes().point(box_for_id("shifted")).expect("shifted image geometry");
        let following = view.boxes().point(box_for_id("following")).expect("following image geometry");

        assert_eq!(shifted.x, 2.0);
        assert_eq!(following.x, 20.0, "relative movement must not change the next inline image's flow position");
    }

    #[test]
    fn relative_split_inline_moves_both_inline_fragments_and_block_descendant() {
        let document = layout_html(
            "<html><body><p>Test</p><div style='position:relative;width:192px;height:192px;background:yellow'>\n<div style='display:inline;position:relative;top:192px;background:blue'>\nFiller Text\n<div style='width:192px;background:orange'>Filler Text</div>\nFiller Text\n</div>\n</div></body></html>",
            800.0,
        );
        let blue = document
            .render_view()
            .fragments()
            .decorations()
            .iter()
            .filter(|fragment| fragment.color() == 0x0000ffff)
            .map(|fragment| (fragment.rect(), fragment.line_idx(), fragment.is_in_positioned_layer()))
            .collect::<Vec<_>>();
        let orange = document.render_view().fragments().decorations().iter().filter(|fragment| fragment.color() == 0xffa500ff).map(|fragment| fragment.rect()).collect::<Vec<_>>();

        assert_eq!(blue.len(), 2, "both split inline fragments must retain their background: {blue:?}");
        assert_eq!(orange.len(), 1);
        assert!(blue.iter().all(|(rect, line_idx, _)| rect.y0 >= 192.0 && line_idx.is_some()), "relative positioning moves both line-owned inline fragments: {blue:?}");
        assert!(blue.iter().all(|(_, line_idx, positioned)| {
            let line_idx = line_idx.expect("checked above");
            document.render_view().text().line(line_idx).is_some_and(|line| line.is_in_positioned_layer() == *positioned)
        }), "split decorations and their owning lines must remain in the same paint layer: {blue:?}");
        assert!(orange[0].y0 >= 192.0, "relative positioning moves the intervening block: {orange:?}");
    }

    #[test]
    fn fragmentation_css_does_not_change_continuous_web_layout_without_a_viewport_height() {
        let document = layout_html("<html><body style='margin:0'><div style='line-height:20px'>A</div><div style='line-height:20px;break-before:page;break-inside:avoid'>B</div></body></html>", 200.0);
        let text = document.render_view().text();

        assert_eq!(text.line(0).expect("A line").point().y, 0.0);
        assert_eq!(text.line(1).expect("B line").point().y, 20.0);
    }

    #[test]
    fn ahem_normal_line_height_uses_its_zero_line_gap_metrics() {
        let ahem = layout_html("<html><body style='margin:0'><div style='font:16px Ahem'>XX</div></body></html>", 200.0);
        let ordinary = layout_html("<html><body style='margin:0'><div style='font-size:16px'>XX</div></body></html>", 200.0);

        assert!((ahem.render_view().text().line(0).expect("Ahem line").height() - 16.0).abs() < 0.001);
        assert!((ordinary.render_view().text().line(0).expect("ordinary line").height() - 19.2).abs() < 0.001);
    }

    // Reduced from WPT css/css-sizing/
    // percentage-height-replaced-content-in-auto-cb.html. The cyclic
    // percentage computes as auto, so the definite width transfers through
    // the image's intrinsic aspect ratio.
    #[test]
    fn inline_replaced_percentage_height_in_auto_containing_block_uses_ratio() {
        let document = layout_html("<html><body><div><img src='x' width='60' height='60' style='width:100px;height:100%'></div></body></html>", 300.0);

        assert_eq!(tag_size(&document, "img"), kurbo::Size::new(100.0, 100.0));
    }

    #[test]
    fn inline_replaced_percentage_height_resolves_against_definite_containing_block() {
        let document = layout_html("<html><body><div style='height:120px'><img src='x' width='60' height='60' style='width:80px;height:50%'></div></body></html>", 300.0);

        assert_eq!(tag_size(&document, "img"), kurbo::Size::new(80.0, 60.0));
    }

    #[test]
    fn smart_image_policy_expands_a_standalone_inline_image() {
        let document = layout_html_with_images("<html><body style='margin:0'><div style='width:500px'><img src='graph.png'></div></body></html>", 500.0, &[(300, 150)], crate::ImageSizingPolicy::SmartStandalone);

        assert_eq!(tag_size(&document, "img"), kurbo::Size::new(500.0, 250.0));
    }

    #[test]
    fn smart_image_policy_fits_a_tall_standalone_image_to_the_viewport_height() {
        let document = layout_html_with_images_in_viewport("<html><body style='margin:0'><div style='width:500px'><img src='portrait.png'></div></body></html>", 500.0, Some(200.0), &[(300, 600)], crate::ImageSizingPolicy::SmartStandalone);

        assert_eq!(tag_size(&document, "img"), kurbo::Size::new(100.0, 200.0));
    }

    #[test]
    fn smart_image_height_fit_accounts_for_vertical_margin_padding_and_border() {
        let document = layout_html_with_images_in_viewport(
            "<html><body style='margin:0'><div style='width:500px'><img src='portrait.png' style='margin:10px 0;padding:5px 0;border-top:2px solid;border-bottom:3px solid'></div></body></html>",
            500.0,
            Some(200.0),
            &[(300, 600)],
            crate::ImageSizingPolicy::SmartStandalone,
        );

        assert_eq!(tag_size(&document, "img"), kurbo::Size::new(82.5, 180.0));
    }

    #[test]
    fn smart_image_height_fit_preserves_an_explicit_height() {
        let document = layout_html_with_images_in_viewport(
            "<html><body style='margin:0'><div style='width:500px'><img src='portrait.png' style='width:150px;height:300px'></div></body></html>",
            500.0,
            Some(200.0),
            &[(300, 600)],
            crate::ImageSizingPolicy::SmartStandalone,
        );

        assert_eq!(tag_size(&document, "img"), kurbo::Size::new(150.0, 300.0));
    }

    #[test]
    fn smart_image_policy_fills_the_column_beyond_twice_the_source_size() {
        let document = layout_html_with_images("<html><body style='margin:0'><div style='width:500px'><img src='graph.png'></div></body></html>", 500.0, &[(100, 50)], crate::ImageSizingPolicy::SmartStandalone);

        assert_eq!(tag_size(&document, "img"), kurbo::Size::new(500.0, 250.0));
    }

    #[test]
    fn smart_image_policy_ignores_whitespace_and_transparent_link_wrappers() {
        let document = layout_html_with_images("<html><body style='margin:0'><div style='width:500px'>\n<a href='#'><img src='graph.png'></a>\n</div></body></html>", 500.0, &[(300, 150)], crate::ImageSizingPolicy::SmartStandalone);

        assert_eq!(tag_size(&document, "img"), kurbo::Size::new(500.0, 250.0));
    }

    #[test]
    fn smart_image_policy_never_expands_an_image_embedded_in_text() {
        let document = layout_html_with_images("<html><body style='margin:0'><div style='width:500px'>Graph: <img src='graph.png'> caption</div></body></html>", 500.0, &[(200, 100)], crate::ImageSizingPolicy::SmartStandalone);

        assert_eq!(tag_size(&document, "img"), kurbo::Size::new(200.0, 100.0));
    }

    #[test]
    fn smart_image_policy_rejects_multiple_images_but_overrides_authored_width() {
        let multiple = layout_html_with_images("<html><body style='margin:0'><div style='width:500px'><img src='a.png'><img src='b.png'></div></body></html>", 500.0, &[(200, 100), (200, 100)], crate::ImageSizingPolicy::SmartStandalone);
        let authored = layout_html_with_images("<html><body style='margin:0'><div style='width:500px'><img src='graph.png' style='width:250px'></div></body></html>", 500.0, &[(200, 100)], crate::ImageSizingPolicy::SmartStandalone);

        assert_eq!(tag_size(&multiple, "img"), kurbo::Size::new(200.0, 100.0));
        assert_eq!(tag_size(&authored, "img"), kurbo::Size::new(500.0, 250.0));
    }

    #[test]
    fn smart_image_policy_overrides_horizontal_constraints_and_margins() {
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:300px;text-align:center'><img src='graph.png' style='width:70%;height:auto;min-width:40px;max-width:180px;margin:12px 45px 18px 35px'><p style='margin:0'>Figure caption</p></div></body></html>",
            300.0,
            &[(1586, 1023)],
            crate::ImageSizingPolicy::SmartStandalone,
        );

        assert_eq!(tag_size(&document, "img"), kurbo::Size::new(300.0, 300.0 * 1023.0 / 1586.0));
        assert_eq!(tag_point(&document, "img").x, 0.0, "used left margin should be zero");
    }

    #[test]
    fn smart_image_policy_expands_a_standalone_block_image_but_not_an_icon() {
        let block = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:500px'><img src='graph.png' style='display:block;width:70%;max-width:180px;margin:10px 30px 20px 40px'></div></body></html>",
            500.0,
            &[(300, 150)],
            crate::ImageSizingPolicy::SmartStandalone,
        );
        let icon = layout_html_with_images("<html><body style='margin:0'><div style='width:500px'><img src='icon.png'></div></body></html>", 500.0, &[(64, 64)], crate::ImageSizingPolicy::SmartStandalone);

        assert_eq!(tag_size(&block, "img"), kurbo::Size::new(500.0, 250.0));
        assert_eq!(tag_point(&block, "img").x, 0.0, "used block-image left margin should be zero");
        assert_eq!(tag_size(&icon, "img"), kurbo::Size::new(64.0, 64.0));
    }

    #[test]
    fn block_image_content_starts_at_its_own_content_edge() {
        let document = layout_html_with_images("<html><body style='margin:0'><img src='pixel.png' style='display:block;width:15px;height:15px'></body></html>", 200.0, &[(15, 15)], crate::ImageSizingPolicy::WebCompatible);
        let image_box = tag_point(&document, "img");
        let fragment = document.render_view().fragments().images().iter().next().expect("image fragment");
        let line = document.render_view().text().line(fragment.line_idx()).expect("image carrier line");

        let painted = line.point() + fragment.offset().to_vec2();
        assert!((painted.x - image_box.x).abs() < 0.001 && (painted.y - image_box.y).abs() < 0.001, "painted image {painted:?} must start at its box {image_box:?}");
        assert_eq!(fragment.size(), kurbo::Size::new(15.0, 15.0));
    }

    #[test]
    fn inline_image_paints_its_own_background_and_border_box() {
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div><img src='pixel.png' style='width:10px;height:10px;padding:3px 5px;background:#123456;border:2px solid #abcdef'></div></body></html>",
            200.0,
            &[(10, 10)],
            crate::ImageSizingPolicy::WebCompatible,
        );
        let image_point = tag_point(&document, "img");
        let image_size = tag_size(&document, "img");
        let decorations = document.render_view().fragments().decorations();
        let background = decorations.iter().find(|fragment| fragment.color() == 0x123456FF).expect("inline image background");
        let borders = decorations.iter().filter(|fragment| fragment.color() == 0xABCDEFFF).collect::<Vec<_>>();

        assert_eq!(background.rect(), kurbo::Rect::from_origin_size(image_point, image_size));
        assert!(background.is_inline());
        assert_eq!(borders.len(), 4);
        assert!(borders.iter().all(|fragment| fragment.is_inline()));
        assert!(borders.iter().all(|fragment| {
            let rect = fragment.rect();
            rect.x0 >= image_point.x && rect.y0 >= image_point.y && rect.x1 <= image_point.x + image_size.width && rect.y1 <= image_point.y + image_size.height
        }));
    }

    #[test]
    fn adjacent_full_width_inline_images_wrap_at_the_atomic_boundary() {
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:100px'><img src='a.png' style='width:100%;height:20px;vertical-align:top'><img src='b.png' style='width:100%;height:20px;vertical-align:top'></div></body></html>",
            200.0,
            &[(1, 1), (1, 1)],
            crate::ImageSizingPolicy::WebCompatible,
        );
        let fragments = document.render_view().fragments().images().iter().collect::<Vec<_>>();
        assert_eq!(fragments.len(), 2);
        assert_ne!(fragments[0].line_idx(), fragments[1].line_idx(), "adjacent atomic inline boxes have a soft wrap opportunity between them");

        let text = document.render_view().text();
        let painted = fragments.iter().map(|fragment| text.line(fragment.line_idx()).expect("image line").point() + fragment.offset().to_vec2()).collect::<Vec<_>>();
        assert!((painted[0].x - painted[1].x).abs() < 0.001);
        assert!(painted[1].y >= painted[0].y + 20.0);
    }

    #[test]
    fn top_aligned_image_does_not_add_the_text_strut_descent() {
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div><img src='a.png' width='10' height='96' style='vertical-align:top'> <img src='b.png' width='10' height='96' style='float:right'></div></body></html>",
            800.0,
            &[(10, 96), (10, 96)],
            crate::ImageSizingPolicy::WebCompatible,
        );
        let text = document.render_view().text();
        let inline_image = document.render_view().fragments().images().iter().find(|fragment| fragment.size().height == 96.0 && text.line(fragment.line_idx()).is_some_and(|line| line.point().x < 100.0)).expect("in-flow image fragment");
        let line = text.line(inline_image.line_idx()).expect("image line");

        assert!((line.height() - 96.0).abs() < 0.001, "top-aligned image should determine the line height without a phantom descender: {line:?}");
    }

    #[test]
    fn right_floated_replaced_inline_shrink_wraps_at_the_containing_edge() {
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:200px'><img src='a.png' width='10' height='20'><img src='b.png' width='10' height='20' style='float:right'></div></body></html>",
            200.0,
            &[(10, 20), (10, 20)],
            crate::ImageSizingPolicy::WebCompatible,
        );
        let fragments = document.render_view().fragments().images().iter().collect::<Vec<_>>();
        let text = document.render_view().text();
        let mut painted = fragments.iter().map(|fragment| text.line(fragment.line_idx()).expect("image line").point() + fragment.offset().to_vec2()).collect::<Vec<_>>();
        painted.sort_by(|left, right| left.x.total_cmp(&right.x));

        assert_eq!(painted.len(), 2);
        assert!((painted[0].x - 0.0).abs() < 0.001, "in-flow image starts at the inline edge: {painted:?}");
        assert!((painted[1].x - 190.0).abs() < 0.001, "right float shrink-wraps to 10px: {painted:?}");
        assert!((painted[0].y - painted[1].y).abs() < 0.001, "both images share the first row: {painted:?}");
    }

    #[test]
    fn oversized_atomic_inline_wraps_before_it_when_adjacent_to_text() {
        let document = layout_html("<html><body style='margin:0'><div style='width:100px'>x<span id='atomic' style='display:inline-block;width:200px'>y</span>z</div></body></html>", 300.0);
        let atomic_point = tag_point(&document, "span");
        let outer_lines = document.render_view().text().lines().iter().filter(|line| matches!(line_text(&document, line.index()).as_str(), "x" | "z")).map(|line| line.point()).collect::<Vec<_>>();

        assert_eq!(outer_lines.len(), 2);
        assert!(atomic_point.y > outer_lines[0].y, "the oversized atomic box must wrap after the leading text");
        assert!(outer_lines[1].y > atomic_point.y, "following text must wrap after the oversized atomic box");
    }

    #[test]
    fn baseline_inline_image_starts_at_line_top_and_reserves_font_descent() {
        let document = layout_html_with_images("<html><body style='margin:0'><div><img src='pixel.png' style='width:16px;height:16px'></div></body></html>", 200.0, &[(1, 1)], crate::ImageSizingPolicy::WebCompatible);
        let fragment = document.render_view().fragments().images().iter().next().expect("image fragment");
        let line = document.render_view().text().line(fragment.line_idx()).expect("image line");
        let painted = line.point() + fragment.offset().to_vec2();

        assert!((painted.y - line.point().y).abs() < 0.001, "baseline image should occupy the top of its line, not receive symmetric half-leading");
        assert!(line.height() > fragment.size().height, "the parent font strut should retain descender space below the image");
    }

    #[test]
    fn image_fragment_separates_object_paint_geometry_from_the_css_box() {
        let contain = layout_html_with_images(
            "<html><body style='margin:0'><img src='wide.png' style='width:100px;height:100px;object-fit:contain;object-position:center'></body></html>",
            200.0,
            &[(200, 100)],
            crate::ImageSizingPolicy::WebCompatible,
        );
        let contain = contain.render_view().fragments().images().iter().next().expect("contained image fragment");
        assert_eq!(contain.size(), kurbo::Size::new(100.0, 50.0));
        assert_eq!(contain.offset().x, contain.clip().x0);
        assert_eq!(contain.offset().y, contain.clip().y0 + 25.0);
        assert_eq!(contain.clip().size(), kurbo::Size::new(100.0, 100.0));

        let cover = layout_html_with_images(
            "<html><body style='margin:0'><img src='wide.png' style='width:100px;height:100px;object-fit:cover;object-position:center'></body></html>",
            200.0,
            &[(200, 100)],
            crate::ImageSizingPolicy::WebCompatible,
        );
        let cover = cover.render_view().fragments().images().iter().next().expect("covered image fragment");
        assert_eq!(cover.size(), kurbo::Size::new(200.0, 100.0));
        assert_eq!(cover.offset().x, cover.clip().x0 - 50.0);
        assert_eq!(cover.offset().y, cover.clip().y0);
        assert_eq!(cover.clip().size(), kurbo::Size::new(100.0, 100.0));
    }

    #[test]
    fn inline_image_line_height_does_not_resize_or_shift_the_replaced_box() {
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div><img src='square.png' style='width:96px;height:96px;line-height:192px'></div></body></html>",
            300.0,
            &[(96, 96)],
            crate::ImageSizingPolicy::WebCompatible,
        );
        let fragment = document.render_view().fragments().images().iter().next().expect("image fragment");
        let line = document.render_view().text().line(fragment.line_idx()).expect("image line");
        let painted = line.point() + fragment.offset().to_vec2();

        assert_eq!(fragment.size().height, 96.0);
        assert!((painted.y - line.point().y).abs() < 0.001, "the image's own line-height must not add leading around its box");
        assert!(line.height() < 192.0, "the image's own line-height must not become the line's minimum height");
    }

    #[test]
    fn zero_line_height_font_strut_retains_baseline_ascent_next_to_an_image() {
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div style='font:160px/0 Ahem'><img src='pixel.png' style='width:160px;height:1px'></div></body></html>",
            300.0,
            &[(1, 1)],
            crate::ImageSizingPolicy::WebCompatible,
        );
        let fragment = document.render_view().fragments().images().iter().next().expect("image fragment");
        let line = document.render_view().text().line(fragment.line_idx()).expect("image line");

        assert!((line.height() - 48.0).abs() < 0.001, "the Ahem strut contributes 0.3em above the baseline despite line-height zero; got {}", line.height());
    }

    #[test]
    fn first_letter_shapes_adjacent_css_punctuation_with_the_pseudo_style() {
        let mut factory = DocumentFactory::new();
        let mut glyphs = GlyphCache::new();
        let document = factory
            .parse_with_new_pipeline("<html><body style='margin:0'><div>)T)est</div></body></html>", Some("div::first-letter { font-size:36px; line-height:72px; color:green }"))
            .shape(&mut glyphs)
            .expect("first-letter glyphs shape")
            .layout(crate::LayoutConstraints::new(500.0, 16.0).unwrap());

        let text = document.render_view().text();
        let advances = (0..text.glyph_count()).map(|index| text.glyph_at(index).and_then(|glyph| text.glyph_metric(glyph)).expect("shaped glyph").advance()).collect::<Vec<_>>();
        assert_eq!(advances, vec![18.0, 18.0, 18.0, 8.0, 8.0, 8.0]);
        assert_eq!(text.line(0).expect("first line").height(), 72.0);
    }

    #[test]
    fn ancestor_first_letter_reaches_the_first_in_flow_block_descendant() {
        let mut factory = DocumentFactory::new();
        let mut glyphs = GlyphCache::new();
        let document = factory
            .parse_with_new_pipeline("<!doctype html><html><body style='margin:0'><div id='outer'><div>Ab</div></div></body></html>", Some("#outer { font-size:16px } #outer::first-letter { font-size:48px; color:blue }"))
            .shape(&mut glyphs)
            .expect("descendant first-letter glyphs shape")
            .layout(crate::LayoutConstraints::new(500.0, 16.0).unwrap());

        let text = document.render_view().text();
        let advances = (0..text.glyph_count()).map(|index| text.glyph_at(index).and_then(|glyph| text.glyph_metric(glyph)).expect("shaped glyph").advance()).collect::<Vec<_>>();
        assert_eq!(advances, vec![24.0, 8.0]);
    }

    #[test]
    fn sentence_per_line_inserts_logical_breaks_without_changing_text_order() {
        let html = "<html><body style='margin:0'><p style='margin:0'>First sentence. <em>Second sentence!</em> Third?</p></body></html>";
        let count = "First sentence. Second sentence! Third?".chars().count();
        let document = layout_with_native_geometry_policy(html, 1_000.0, vec![8.0; count], vec![true; count + 1], crate::TextCompositionPolicy::SentencePerLine);

        let lines = (0..document.line_count()).map(|line| line_text(&document, line)).collect::<Vec<_>>();
        assert_eq!(lines, ["First sentence.", "Second sentence!", "Third?"]);
        assert_eq!(lines.concat(), "First sentence.Second sentence!Third?");
    }

    #[test]
    fn ordinary_book_composition_does_not_force_sentence_breaks() {
        let html = "<html><body style='margin:0'><p style='margin:0'>First sentence. Second sentence!</p></body></html>";
        let count = "First sentence. Second sentence!".chars().count();
        let document = layout_with_native_geometry_policy(html, 1_000.0, vec![8.0; count], vec![true; count + 1], crate::TextCompositionPolicy::BookOptimized);

        assert_eq!(document.line_count(), 1);
        assert_eq!(line_text(&document, 0), "First sentence. Second sentence!");
    }

    #[test]
    fn web_compatible_policy_does_not_expand_standalone_images() {
        let document =
            layout_html_with_images("<html><body style='margin:0'><div style='width:500px'><img src='graph.png' style='width:70%;margin-left:20px'></div></body></html>", 500.0, &[(200, 100)], crate::ImageSizingPolicy::WebCompatible);

        let size = tag_size(&document, "img");
        assert!((size.width - 350.0).abs() < 0.001);
        assert!((size.height - 175.0).abs() < 0.001);
        assert_eq!(tag_point(&document, "img").x, 20.0);
    }

    #[test]
    fn preserved_tab_advances_to_the_next_number_based_stop() {
        let document = layout_html("<html><body><pre style='tab-size: 4'>a\tb</pre></body></html>", 300.0);
        assert_eq!(line_text(&document, 0), "a\tb");
        // Test shaper: every 16px glyph/space advances 8px. After `a`, a
        // four-space tab interval reaches the 32px stop, hence 24px.
        assert!((tab_advance(&document, 0) - 24.0).abs() < 0.001);
    }

    #[test]
    fn number_based_tab_uses_the_nearest_block_space_metric() {
        let document = layout_html("<html><body><pre style='tab-size: 4'>a<span style='font-size: 32px'>\t</span>b</pre></body></html>", 300.0);
        assert!((tab_advance(&document, 0) - 24.0).abs() < 0.001, "the inline span's larger font must not redefine the block's tab stops");
    }

    #[test]
    fn tab_stops_are_anchored_at_the_block_content_edge_before_indent() {
        let document = layout_html("<html><body><pre style='tab-size: 4; text-indent: 8px'>a\tb</pre></body></html>", 300.0);
        assert!((tab_advance(&document, 0) - 16.0).abs() < 0.001);
    }

    #[test]
    fn length_and_zero_tab_sizes_produce_exact_sparse_advances() {
        let length = layout_html("<html><body><pre style='tab-size: 20px'>a\tb</pre></body></html>", 300.0);
        assert!((tab_advance(&length, 0) - 12.0).abs() < 0.001);

        let zero = layout_html("<html><body><pre style='tab-size: 0'>a\tb</pre></body></html>", 300.0);
        assert_eq!(tab_advance(&zero, 0), 0.0);
    }

    #[test]
    fn collapsed_tabs_stay_on_the_ordinary_space_fast_path() {
        let document = layout_html("<html><body><p style='tab-size: 20px'>a\tb</p></body></html>", 300.0);
        assert_eq!(line_text(&document, 0), "a b");
        assert!(document.render_view().text().line_glyph_advances(0).expect("line advances").is_empty());
    }

    #[test]
    #[ignore]
    fn inspect_axel_chapter004_float_pipeline() {
        let html_path = std::env::var("CHAPTER_HTML_PATH").expect("set CHAPTER_HTML_PATH to the chapter fixture");
        let css_path = std::env::var("CHAPTER_CSS_PATH").expect("set CHAPTER_CSS_PATH to its stylesheet");

        fn find_dom_node_with_class(document: &html_dom::Document, node: html_dom::DomNodeId, class: &str) -> Option<html_dom::DomNodeId> {
            if document.dom_has_class(node, class) {
                return Some(node);
            }
            let element = document.element_ref(node)?;
            for child in element.children() {
                if let Some(found) = find_dom_node_with_class(document, child, class) {
                    return Some(found);
                }
            }
            None
        }

        let html = fs::read_to_string(&html_path).expect("chapter html");
        let mut factory = DocumentFactory::new();
        let mut document = factory.parse_to_dom(&html);
        document.set_root_font_size(html_dom::RootFontSize::new(16.0).unwrap());
        println!("step 2 dom: root={:?}", document.dom_root().map(|n| n.raw()));

        let author_css = fs::read_to_string(&css_path).expect("chapter css");
        let parsed_float_prop = lightningcss::properties::Property::parse_string(lightningcss::properties::PropertyId::from("float"), "left", ParserOptions::default()).expect("parse float property");
        let parsed_clear_prop = lightningcss::properties::Property::parse_string(lightningcss::properties::PropertyId::from("clear"), "both", ParserOptions::default()).expect("parse clear property");
        println!("step 2b parsed property float={parsed_float_prop:?} clear={parsed_clear_prop:?}");
        let styled = html_style::style_document(document, &[&author_css]);
        let (document, styles) = styled.into_parts();
        let prepared = crate::PreparedDocument::try_new(document, styles).expect("style resolver must produce complete styles for its document");
        let document = prepared.document();

        let dom_root = document.dom_root().expect("dom root");
        let first_letter_dom = find_dom_node_with_class(document, dom_root, "first-letter").expect("first-letter DOM node");
        let first_letter_style_idx = prepared.styles().style_for_node(first_letter_dom).expect("first-letter style");
        let first_letter_style = prepared.style_view(first_letter_style_idx);
        let first_letter_used_style = prepared.styles().used_view(first_letter_style_idx, 0.5, 0.5, 0.8).expect("valid fallback metrics");
        println!(
            "step 3 style: float={:?} display={:?} font_size={} line_height={} margin_right={} margin_bottom={}",
            first_letter_style.float(),
            first_letter_style.display(),
            first_letter_style.font_size(),
            first_letter_style.line_height(),
            first_letter_used_style.margin_right().resolve(first_letter_style.font_size() as f64),
            first_letter_used_style.margin_bottom().resolve(first_letter_style.font_size() as f64),
        );
        assert!(matches!(first_letter_style.float(), html_style_model::Float::Left | html_style_model::Float::Right));

        let float_box_idx = (0..prepared.layout_tree().box_count())
            .find(|&i| prepared.layout_tree().get_box_dom_element(i).and_then(|node| document.node_id_from_raw(node)).is_some_and(|node| document.dom_has_class(node, "first-letter")))
            .expect("float box");
        let float_box = prepared.layout_tree().box_at(float_box_idx).expect("float box exists");
        let float_mode = match float_box.layout_mode() {
            crate::layout_model::LayoutMode::Block(_) => "Block",
            crate::layout_model::LayoutMode::Table(_) => "Table",
            crate::layout_model::LayoutMode::TableRow(_) => "TableRow",
            crate::layout_model::LayoutMode::TableCell(_) => "TableCell",
            crate::layout_model::LayoutMode::Flex(_) => "Flex",
            crate::layout_model::LayoutMode::Grid(_) => "Grid",
            crate::layout_model::LayoutMode::Inline(_) => "Inline",
            crate::layout_model::LayoutMode::Anonymous(_) => "Anonymous",
        };
        println!("step 4 box tree: float box mode={float_mode}");
        assert!(matches!(float_box.layout_mode(), crate::layout_model::LayoutMode::Block(_)));

        let mut glyph_cache = GlyphCache::new();
        let shaped = prepared.shape(&mut glyph_cache).expect("test shaper must register every glyph");
        println!("step 5 shape: glyphs={}", glyph_cache.len());

        let laid_out = shaped.layout(crate::LayoutConstraints::new(220.0, 16.0).unwrap());
        println!("step 6 layout: lines={}", laid_out.line_count());

        let float_point = laid_out.render_view().boxes().point(float_box_idx).expect("box index in range");
        let float_size = laid_out.render_view().boxes().size(float_box_idx).expect("box index in range");
        for (idx, line) in laid_out.render_view().text().lines().iter().enumerate().take(20) {
            let text_view = laid_out.render_view().text();
            let text: String = text_view.glyph_slice(line.glyphs()).expect("line glyph range").iter().map(|&glyph| text_view.glyph_metric(glyph).expect("registered glyph").ch()).collect();
            println!("line {idx}: point=({:.2},{:.2}) baseline={:.2} height={:.2} text={:?}", line.point().x, line.point().y, line.baseline(), line.height(), text);
        }
        let float_line = laid_out
            .render_view()
            .text()
            .lines()
            .iter()
            .enumerate()
            .find(|(_, line)| {
                let text_view = laid_out.render_view().text();
                let text: String = text_view.glyph_slice(line.glyphs()).expect("line glyph range").iter().map(|&glyph| text_view.glyph_metric(glyph).expect("registered glyph").ch()).collect();
                text.starts_with('E') || text.starts_with('R')
            })
            .expect("line for float");
        println!("float box: point=({:.2},{:.2}) size=({:.2},{:.2})", float_point.x, float_point.y, float_size.width, float_size.height);
        println!("matched line {}: point=({:.2},{:.2}) baseline={:.2} height={:.2}", float_line.0, float_line.1.point().x, float_line.1.point().y, float_line.1.baseline(), float_line.1.height());
        assert!((float_point.y - float_line.1.point().y).abs() < 3.0, "float top should be near the first line top");
    }

    #[test]
    fn trims_collapsible_trailing_space_on_final_line() {
        let document = layout_html("<html><body><p>a </p></body></html>", 200.0);

        assert_eq!(document.line_count(), 1);
        assert_eq!(line_text(&document, 0), "a");
    }

    #[test]
    fn trims_collapsible_leading_space_on_first_greedy_line() {
        let document = layout_html("<html><body style='margin:0'><div style='margin:0'> \n <span>word</span></div></body></html>", 200.0);

        assert_eq!(document.line_count(), 1);
        assert_eq!(line_text(&document, 0), "word");
    }

    #[test]
    fn trims_collapsible_leading_space_from_knuth_plass_segment() {
        let document = layout_html("<html><body style='margin:0'><div style='margin:0;text-align:justify'> \n <span>one two three</span></div></body></html>", 200.0);

        assert_eq!(document.line_count(), 1);
        assert_eq!(line_text(&document, 0), "one two three");
    }

    #[test]
    fn preserves_leading_space_when_white_space_requires_it() {
        let document = layout_html("<html><body style='margin:0'><div style='margin:0;white-space:pre'>  word</div></body></html>", 200.0);

        assert_eq!(document.line_count(), 1);
        assert_eq!(line_text(&document, 0), "  word");
    }

    #[test]
    fn preserved_spaces_after_a_newline_establish_a_trailing_line() {
        let document = layout_html("<html><body style='margin:0'><table><tr><td><div id='target' style='margin:0;white-space:pre;font-size:20px;line-height:20px'>\nXXXX\nXXXX\nXXXX\nXXXX\nXXXX\n   </div></td></tr></table></body></html>", 500.0);

        let lines = (0..document.line_count()).map(|index| line_text(&document, index)).collect::<Vec<_>>();
        assert_eq!(document.line_count(), 7, "published lines: {lines:?}");
        assert_eq!(line_text(&document, 0), "");
        assert_eq!(line_text(&document, 6), "   ");
        let view = document.render_view();
        let target = (0..view.boxes().len()).find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some("target")).expect("target box");
        assert_eq!(view.boxes().size(target).expect("target geometry").height, 140.0);
    }

    #[test]
    fn intrinsic_grid_width_keeps_an_exact_fit_word_pair_on_one_line() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='width:min-content'>
                    <div id='grid' style='display:grid;grid-template-columns:1fr;grid-template-rows:1fr;font-size:20px;line-height:20px'>
                        <div id='item'>XXX XXXX XX X XX XXX</div>
                    </div>
                </div>
            </body></html>",
            200.0,
        );

        let view = document.render_view();
        let grid = (0..view.boxes().len()).find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some("grid")).expect("grid box");
        let item = (0..view.boxes().len()).find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some("item")).expect("item box");
        let lines = (0..document.line_count()).map(|idx| line_text(&document, idx)).collect::<Vec<_>>();
        assert_eq!(document.line_count(), 5, "the exact-width `X XX` pair should share a line; grid={:?}, item={:?}, lines={lines:?}", view.boxes().size(grid), view.boxes().size(item));
    }

    #[test]
    fn long_word_overflows_without_overflow_wrap() {
        let html = format!("<html><body><p>{}</p></body></html>", "a".repeat(40));
        let document = layout_html(&html, 100.0);

        assert_eq!(document.line_count(), 1, "an unbreakable word should overflow on one line by default");
    }

    #[test]
    fn overflow_wrap_breaks_long_word_without_losing_glyphs() {
        let html = format!("<html><body><p style=\"overflow-wrap: break-word\">{}</p></body></html>", "a".repeat(40));
        let document = layout_html(&html, 100.0);

        assert!(document.line_count() >= 2, "overflow-wrap should break the long word across lines");
        let combined: String = (0..document.line_count()).map(|i| line_text(&document, i)).collect();
        assert_eq!(combined, "a".repeat(40), "mid-word breaks must not drop glyphs");
    }

    #[test]
    fn overflow_wrap_prefers_space_break_over_mid_word_break() {
        // "aa bb": both words fit on a line, so overflow-wrap must wrap at the
        // space like normal wrapping, not mid-word.
        let html = "<html><body style=\"margin:0\"><p style=\"margin:0; overflow-wrap:break-word\">aa bb</p></body></html>";
        let document = layout_html(html, 39.0);

        assert_eq!(document.line_count(), 2);
        assert_eq!(line_text(&document, 0), "aa");
        assert_eq!(line_text(&document, 1), "bb");
    }

    #[test]
    fn word_break_break_all_breaks_anywhere() {
        let html = format!("<html><body><p style=\"word-break: break-all\">{}</p></body></html>", "a".repeat(40));
        let document = layout_html(&html, 100.0);

        assert!(document.line_count() >= 2, "word-break: break-all should wrap the long word");
        let combined: String = (0..document.line_count()).map(|i| line_text(&document, i)).collect();
        assert_eq!(combined, "a".repeat(40));
    }

    #[test]
    fn nowrap_suppresses_overflow_wrap() {
        let html = format!("<html><body><p style=\"overflow-wrap: break-word; white-space: nowrap\">{}</p></body></html>", "a".repeat(40));
        let document = layout_html(&html, 100.0);

        assert_eq!(document.line_count(), 1, "white-space: nowrap forbids wrapping even with overflow-wrap");
    }

    #[test]
    fn text_overflow_ellipsis_truncates_source_glyphs_and_emits_synthetic_marker() {
        let document = layout_html("<html><body><p style='width:24px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis'>abcdef</p></body></html>", 200.0);
        let line = document.render_view().text().line(0).expect("overflowing paragraph should emit one line");
        let marker = document.render_view().text().ellipsis_for_line(0).expect("ellipsis marker should be emitted");

        assert_eq!(line_text(&document, 0), "ab");
        assert_eq!(document.render_view().text().glyph_metric(marker.glyph()).expect("registered marker metric").ch(), '\u{2026}');
        assert!((marker.offset().x - 16.0).abs() < 0.001);
        assert_eq!(line.glyphs(), 0..2, "synthetic marker must not enter the source glyph range");
    }

    #[test]
    fn text_overflow_clip_preserves_layout_content_for_paint_clipping() {
        let document = layout_html("<html><body><p style='width:24px;white-space:nowrap;overflow:hidden;text-overflow:clip'>abcdef</p></body></html>", 200.0);

        assert_eq!(line_text(&document, 0), "abcdef");
        assert!(document.render_view().text().ellipsis_for_line(0).is_none());
        assert!(document.render_view().text().line_overflow_clip(0).is_some());
    }

    #[test]
    fn text_overflow_does_not_apply_when_inline_overflow_is_visible() {
        let document = layout_html("<html><body><p style='width:24px;white-space:nowrap;overflow:visible;text-overflow:ellipsis'>abcdef</p></body></html>", 200.0);

        assert_eq!(line_text(&document, 0), "abcdef");
        assert!(document.render_view().text().ellipsis_for_line(0).is_none());
    }

    #[test]
    fn ellipsis_is_shaped_once_and_survives_width_only_relayout() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline("<html><body style='margin:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis'>abcdef</body></html>", None);
        let mut glyph_cache = GlyphCache::new();
        let shaped = prepared.shape(&mut glyph_cache).expect("ellipsis and source glyphs should shape");
        let mut document = shaped.layout(crate::LayoutConstraints::new(200.0, 16.0).unwrap());

        assert!(document.render_view().text().ellipsis_for_line(0).is_none());
        document.relayout(crate::LayoutConstraints::new(24.0, 16.0).unwrap());
        assert!(document.render_view().text().ellipsis_for_line(0).is_some());
    }

    #[test]
    fn small_inline_image_keeps_natural_size() {
        // A 16x16 image in a 600px column must not be enlarged to fill the column.
        let document = layout_html("<html><body><p><img src=\"x.png\" width=\"16\" height=\"16\"></p></body></html>", 600.0);

        let frag = document.render_view().fragments().images().iter().next().expect("expected an image fragment");
        assert_eq!(frag.size().width, 16.0);
    }

    #[test]
    fn image_dimension_hints_transfer_through_the_natural_aspect_ratio() {
        let document = layout_html_with_images(
            "<html><body style='margin:0'><img src='wide.png' width='10'><img src='wide.png' height='10'></body></html>",
            200.0,
            &[(100, 50), (100, 50)],
            crate::ImageSizingPolicy::WebCompatible,
        );
        let sizes = document.render_view().fragments().images().iter().map(|fragment| fragment.size()).collect::<Vec<_>>();

        assert_eq!(sizes, vec![kurbo::Size::new(10.0, 5.0), kurbo::Size::new(20.0, 10.0)]);
    }

    #[test]
    fn oversized_inline_image_shrinks_to_available_width() {
        // An intrinsically 2000x1000 image with automatic CSS dimensions must
        // shrink to fit the column (the exact content width depends on UA
        // margins), preserving its 2:1 aspect ratio. Explicit HTML width and
        // height attributes are presentational hints, so they are deliberately
        // absent here.
        let document = layout_html_with_images("<html><body><p><img src=\"x.png\"></p></body></html>", 600.0, &[(2000, 1000)], crate::ImageSizingPolicy::WebCompatible);

        let frag = document.render_view().fragments().images().iter().next().expect("expected an image fragment");
        assert!(frag.size().width < 2000.0 && frag.size().width <= 600.0, "image should shrink to the column, got {}", frag.size().width);
        let ratio = frag.size().height / frag.size().width;
        assert!((ratio - 0.5).abs() < 0.01, "aspect ratio should be preserved (0.5), got {ratio}");
    }

    #[test]
    fn justified_paragraph_wraps_and_keeps_all_text() {
        let html = "<html><body><p style=\"text-align: justify\">one two three four five six seven eight nine ten</p></body></html>";
        let document = layout_html(html, 120.0);

        assert!(document.line_count() >= 2, "a narrow justified paragraph should wrap onto multiple lines");

        // Joining the per-line text (restoring the space discarded at each line
        // break) must reproduce the source in order with no glyph lost.
        let combined = (0..document.line_count()).map(|i| line_text(&document, i)).collect::<Vec<_>>().join(" ");
        let normalized = combined.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(normalized, "one two three four five six seven eight nine ten");
    }

    #[test]
    fn floated_drop_cap_distributes_its_line_height_leading_normally() {
        let html = "<html><body><p><span style=\"float:left; font-size:450%; line-height:0.9em; margin-right:0.02em; margin-bottom:-0.25em;\">T</span>ext text text text</p></body></html>";
        let document = layout_html(html, 220.0);

        let line_idx = document
            .render_view()
            .text()
            .lines()
            .iter()
            .enumerate()
            .find(|(_, line)| {
                let text = document.render_view().text();
                let chars: String = text.glyph_slice(line.glyphs()).expect("line glyph range").iter().map(|&glyph| text.glyph_metric(glyph).expect("registered glyph").ch()).collect();
                chars == "T"
            })
            .map(|(idx, _)| idx)
            .expect("drop cap line should exist");

        let line = document.render_view().text().line(line_idx).expect("line should exist");
        let glyph = document.render_view().text().glyph_slice(line.glyphs()).expect("line glyph range")[0];
        let metric = document.glyph_metrics().get(glyph);
        let baseline_offset = metric.baseline_offset() as f64;
        let half_leading = (line.height() - (metric.ascent() + metric.descent()) as f64) / 2.0;

        assert!((line.baseline() - baseline_offset - half_leading).abs() < 1.0, "a float's contents must use the same half-leading baseline placement as ordinary inline content");
    }

    #[test]
    fn pre_wrap_breaks_after_the_preserved_space_sequence() {
        let html = "<html><body><p><span>LongLongLong</span><span style=\"white-space: pre-wrap\">  x</span></p></body></html>";

        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        let document = factory.parse_with_new_pipeline(html, None).shape(&mut glyph_cache).expect("test shaper must register every glyph");

        let text: String = document.glyphs().iter().map(|&glyph| document.glyph_metrics().get(glyph).ch()).collect();
        let word_width: f64 = text.chars().zip(document.glyphs().iter()).take_while(|(c, _)| *c != ' ').map(|(_, &glyph)| document.glyph_metrics().get(glyph).advance() as f64).sum();

        let document = document.layout(crate::LayoutConstraints::new(word_width + 1.0, 16.0).unwrap());

        assert!(document.line_count() >= 2, "expected wrapping to produce multiple lines");
        assert!(line_text(&document, 0).ends_with("  "), "the preserved spaces stay before the break");
        assert_eq!(line_text(&document, 1), "x");
    }

    #[test]
    fn wpt_break_spaces_preserves_segment_breaks() {
        // Semantic adaptation of WPT
        // css/css-text/white-space/break-spaces-newline-011.html. Visible
        // markers replace the reftest's colored boxes so this remains a
        // deterministic framework-neutral assertion:
        // https://github.com/web-platform-tests/wpt/blob/master/css/css-text/white-space/break-spaces-newline-011.html
        let document = layout_html("<html><body><div style=\"white-space: break-spaces\">A\nB</div></body></html>", 200.0);

        assert_eq!(document.line_count(), 2);
        assert_eq!(line_text(&document, 0), "A");
        assert_eq!(line_text(&document, 1), "B");
    }

    #[test]
    fn pre_line_normalizes_crlf_to_one_segment_break() {
        let document = layout_html("<html><body><div style='white-space:pre-line'>XX&#13;&#10;</div></body></html>", 200.0);
        let nonempty = document.render_view().text().lines().iter().filter(|line| !line.glyphs().is_empty()).map(|line| line_text(&document, line.index())).collect::<Vec<_>>();

        assert_eq!(nonempty, ["XX"]);
    }

    #[test]
    fn collapsed_spaces_are_pending_across_inline_styles_but_not_blocks() {
        let document = layout_html("<html><body><div><span style='white-space:normal'>X </span><span style='white-space:pre'> X</span></div><div>A </div><div>B</div></body></html>", 200.0);
        let lines = document.render_view().text().lines().iter().filter(|line| !line.glyphs().is_empty()).map(|line| line_text(&document, line.index())).collect::<Vec<_>>();

        assert_eq!(lines, ["X  X", "A", "B"]);
    }

    #[test]
    fn collapsed_space_before_preserved_spaces_does_not_split_the_pre_sequence() {
        let document = layout_html("<html><body><div style='width:20px'><span style='white-space:pre'>X</span><span style='white-space:normal'> </span><span style='white-space:pre'>   X</span></div></body></html>", 200.0);
        let lines = document.render_view().text().lines().iter().filter(|line| !line.glyphs().is_empty()).map(|line| line_text(&document, line.index())).collect::<Vec<_>>();

        assert_eq!(lines, ["X    X"]);
    }

    #[test]
    fn collapsed_segment_break_before_atomic_inline_is_preserved() {
        let document = layout_html("<html><body><span>left\n<span style='display:table-cell'>right</span></span></body></html>", 200.0);
        let lines = document.render_view().text().lines().iter().map(|line| (line_text(&document, line.index()), line.point())).collect::<Vec<_>>();

        assert!(lines.iter().any(|(text, _)| text.contains(' ')), "the collapsed segment break must remain in the outer inline before its atomic child: {lines:?}");
    }

    #[test]
    fn nowrap_discards_collapsible_spaces_at_line_edges() {
        let leading = layout_html("<html><body><div style='white-space:nowrap'> X</div></body></html>", 200.0);
        let trailing = layout_html("<html><body><div style='white-space:nowrap'>X </div></body></html>", 200.0);

        assert_eq!(line_text(&leading, 0), "X");
        assert_eq!(line_text(&trailing, 0), "X");
    }

    #[test]
    fn collapsed_sequence_keeps_wrap_opportunity_from_normal_ancestor() {
        let document = layout_html("<html><body><div style='width:24px'><span style='white-space:nowrap'>XX </span> XX</div></body></html>", 200.0);
        let lines = document.render_view().text().lines().iter().filter(|line| !line.glyphs().is_empty()).map(|line| line_text(&document, line.index())).collect::<Vec<_>>();

        assert_eq!(lines, ["XX", "XX"]);
    }



    #[test]
    fn zero_width_space_is_an_invisible_wrap_opportunity() {
        let document = layout_with_native_geometry("<html><body style='margin:0'><p style='margin:0'>aa&#x200B;bb</p></body></html>", 5.0, vec![2.0; 5], vec![true; 6]);
        let lines = document.render_view().text().lines().iter().map(|line| line_text(&document, line.index())).collect::<Vec<_>>();

        assert_eq!(lines, ["aa", "bb"]);
    }

    #[test]
    fn out_of_flow_descendant_glyphs_are_not_repainted_in_the_parent_line() {
        let document = layout_html("<html><body style='margin:0'><div style='position:relative'><span>AAA<div style='position:absolute;right:0;top:0'>BBB</div>CCCC</span></div></body></html>", 200.0);
        let text = document.render_view().text();
        let lines = text
            .lines()
            .iter()
            .map(|line| {
                text.line_text_fragments(line.index())
                    .expect("line fragments")
                    .flat_map(|fragment| text.glyph_slice(fragment.glyphs()).expect("fragment glyph range").iter().copied())
                    .map(|glyph| text.glyph_metric(glyph).expect("registered glyph").ch())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(lines.iter().any(|line| line == "AAACCCC"), "the parent line must skip nested absolute glyph storage: {lines:?}");
        assert!(lines.iter().any(|line| line == "BBB"), "the absolute child still paints in its own formatting context: {lines:?}");
    }

    #[test]
    fn text_indent_percentage_resolves_against_containing_block_width() {
        // `text-indent: 50%` in a fixed 200px block indents the first line by
        // 100px (half the containing-block width), not by a fraction of the font
        // size. Lay out in a wide viewport so the inner div keeps its 200px width.
        let html = "<html><body style=\"margin:0\"><div style=\"width:200px\"><p style=\"margin:0; text-indent:50%\">hello world this is a longer line</p></div></body></html>";
        let document = layout_html(html, 600.0);

        let first = document.render_view().text().line(0).expect("line should exist");
        assert!((first.point().x - 100.0).abs() < 1.0, "expected first line indented ~100px (50% of 200px), got {}", first.point().x);
    }

    #[test]
    fn each_line_indent_is_preserved_around_float_exclusions() {
        let html = "<html><body style='margin:0'><p style='margin:0; white-space:pre-line; text-indent:20px each-line'><span style='float:left; width:20px; height:100px'></span>first\nsecond</p></body></html>";
        let document = layout_html(html, 200.0);
        let text_lines = document.render_view().text().lines().iter().filter(|line| !line.glyphs().is_empty()).collect::<Vec<_>>();

        assert_eq!(text_lines.len(), 2);
        assert_eq!(line_text(&document, text_lines[0].index()), "first");
        assert_eq!(line_text(&document, text_lines[1].index()), "second");
        assert!((text_lines[0].point().x - text_lines[1].point().x).abs() < 0.001, "a forced line break around the same float must reapply each-line indentation");
    }

    #[test]
    fn margin_left_percentage_resolves_against_containing_block_width() {
        // `margin-left: 25%` on a block resolves against its containing block's
        // width (200px) → 50px, regardless of font size.
        let html = "<html><body style=\"margin:0\"><div style=\"width:200px\"><p style=\"margin:0; margin-left:25%\">x</p></div></body></html>";
        let document = layout_html(html, 600.0);

        let first = document.render_view().text().line(0).expect("line should exist");
        assert!((first.point().x - 50.0).abs() < 1.0, "expected first line offset ~50px (25% of 200px), got {}", first.point().x);
    }
}
