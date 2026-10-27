#[cfg(test)]
mod tests {
    use crate::{GlyphRequest, ShapedGlyph};

    use crate::GlyphResourceStore;

    use super::{
        BreakKind, InlineToken, InlineTokenKind, InlineTokenMetrics, InlineTokens,
        InlineVerticalAlign, TokenWrap, automatic_hyphen_fragments_are_long_enough,
        hyphenation_language, vertical_align_offset,
    };
    use crate::parser::DocumentFactory;
    use crate::test_support::TestGlyphShaper as GlyphCache;
    use crate::{
        GlyphShaper, LaidOutDocument, ShapeError, ShapedTextRun, TextRunShapeRequest,
        TextShapeRequest,
    };
    use html_style_model::{Hyphens, VerticalAlignValue, WhiteSpace};
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

        fn resource_generation(&self) -> crate::GlyphResourceGeneration {
            self.fallback.resource_generation()
        }

        fn begin_append_shaping(&mut self) -> Result<(), ShapeError> {
            self.fallback.begin_append_shaping()
        }

        fn commit_append_shaping(&mut self) {
            self.fallback.commit_append_shaping();
        }

        fn rollback_append_shaping(&mut self) {
            self.fallback.rollback_append_shaping();
        }

        fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
            self.fallback.shape_glyph(request)
        }

        fn shape_text_run(
            &mut self,
            request: TextRunShapeRequest<'_>,
        ) -> Result<Option<ShapedTextRun>, ShapeError> {
            if request.text().chars().count() != self.advances.len() {
                return Ok(None);
            }
            Ok(ShapedTextRun::new(
                Arc::from(self.advances.clone()),
                Arc::from(self.boundaries.clone()),
            ))
        }

        fn measure_line(
            &mut self,
            request: TextShapeRequest<'_>,
        ) -> Result<Option<ShapedTextRun>, ShapeError> {
            let Some(advance) = self.exact_line_advance else {
                return Ok(None);
            };
            self.measured_texts.push(request.text().to_owned());
            let character_count = request.text().chars().count();
            Ok(ShapedTextRun::new(
                Arc::from(vec![advance; character_count]),
                Arc::from(vec![true; character_count + 1]),
            ))
        }
    }

    struct OscillatingBoundaryShaper {
        fallback: GlyphCache,
    }

    impl GlyphShaper for OscillatingBoundaryShaper {
        fn reset(&mut self) {
            self.fallback.reset();
        }

        fn resource_generation(&self) -> crate::GlyphResourceGeneration {
            self.fallback.resource_generation()
        }

        fn begin_append_shaping(&mut self) -> Result<(), ShapeError> {
            self.fallback.begin_append_shaping()
        }

        fn commit_append_shaping(&mut self) {
            self.fallback.commit_append_shaping();
        }

        fn rollback_append_shaping(&mut self) {
            self.fallback.rollback_append_shaping();
        }

        fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
            self.fallback.shape_glyph(request)
        }

        fn shape_text_run(
            &mut self,
            request: TextRunShapeRequest<'_>,
        ) -> Result<Option<ShapedTextRun>, ShapeError> {
            let count = request.text().chars().count();
            Ok(ShapedTextRun::new(
                Arc::from(vec![4.0; count]),
                Arc::from(vec![true; count + 1]),
            ))
        }

        fn measure_line(
            &mut self,
            request: TextShapeRequest<'_>,
        ) -> Result<Option<ShapedTextRun>, ShapeError> {
            let count = request.text().chars().count();
            let advance = if count == 1 { 2.0 } else { 6.0 };
            Ok(ShapedTextRun::new(
                Arc::from(vec![advance; count]),
                Arc::from(vec![true; count + 1]),
            ))
        }
    }

    fn layout_with_native_geometry(
        html: &str,
        width: f64,
        advances: Vec<f32>,
        boundaries: Vec<bool>,
        resources: &mut GlyphResourceStore,
    ) -> LaidOutDocument<'static> {
        layout_with_native_geometry_policy(
            html,
            width,
            advances,
            boundaries,
            crate::TextCompositionPolicy::WebCompatible,
            resources,
        )
    }

    fn layout_with_native_geometry_policy(
        html: &str,
        width: f64,
        advances: Vec<f32>,
        boundaries: Vec<bool>,
        policy: crate::TextCompositionPolicy,
        resources: &mut GlyphResourceStore,
    ) -> LaidOutDocument<'static> {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = NativeGeometryShaper {
            fallback: GlyphCache::new(),
            advances,
            boundaries,
            exact_line_advance: None,
            measured_texts: Vec::new(),
        };
        factory
            .parse_with_new_pipeline(html, None)
            .shape(&mut glyph_cache, resources)
            .expect("native geometry must be valid")
            .layout(
                crate::LayoutConstraints::new(width, 16.0)
                    .unwrap()
                    .with_text_composition_policy(policy),
                resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles")
    }

    #[test]
    fn width_only_layout_reuses_authoritative_document_shaping() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = NativeGeometryShaper {
            fallback: GlyphCache::new(),
            advances: vec![4.0; 4],
            boundaries: vec![true; 5],
            exact_line_advance: Some(6.0),
            measured_texts: Vec::new(),
        };
        let mut shaped_resources = GlyphResourceStore::default();
        let shaped = factory.parse_with_new_pipeline("<html><body style='margin:0'><p style='margin:0;overflow-wrap:anywhere'>abcd</p></body></html>", None).shape(&mut glyph_cache, &mut shaped_resources).expect("native geometry must be valid");
        let constraints = crate::LayoutConstraints::new(9.0, 16.0).unwrap();
        let mut document = shaped
            .layout_with_metrics_and_shaper(
                constraints,
                &crate::ImageMetrics::default(),
                &mut glyph_cache,
                &mut shaped_resources,
            )
            .expect("exact boundary shaping must succeed");

        assert_eq!(document.line_count(), 2);
        for (line_index, expected) in ["ab", "cd"].iter().enumerate() {
            assert_eq!(
                line_text(&document, line_index, &shaped_resources),
                *expected
            );
        }

        assert!(
            glyph_cache.measured_texts.is_empty(),
            "layout must not invoke backend line reshaping"
        );
        document
            .relayout_with_metrics_and_shaper(
                constraints,
                &crate::ImageMetrics::default(),
                &mut glyph_cache,
                &mut shaped_resources,
            )
            .expect("width-only relayout must remain valid");
        assert!(
            glyph_cache.measured_texts.is_empty(),
            "relayout must reuse authoritative shaped geometry"
        );

        let wider = crate::LayoutConstraints::new(13.0, 16.0).unwrap();
        document
            .relayout_with_metrics_and_shaper(
                wider,
                &crate::ImageMetrics::default(),
                &mut glyph_cache,
                &mut shaped_resources,
            )
            .expect("changed exact boundaries must remain valid");
        assert_eq!(document.line_count(), 2);
    }

    #[test]
    fn authoritative_line_geometry_is_fragmented_around_inline_images() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = NativeGeometryShaper {
            fallback: GlyphCache::new(),
            advances: vec![8.0],
            boundaries: vec![true; 2],
            exact_line_advance: Some(8.0),
            measured_texts: Vec::new(),
        };
        let mut shaped_resources = GlyphResourceStore::default();
        let shaped = factory
            .parse_with_new_pipeline("<html><body style='margin:0'><p style='margin:0;white-space:nowrap'>a<img src='x.png' style='width:20px;height:10px'>b</p></body></html>", None)
            .shape(&mut glyph_cache, &mut shaped_resources)
            .expect("fixture shaping must succeed");
        let constraints = crate::LayoutConstraints::new(200.0, 16.0).unwrap();
        let mut image_metrics = crate::ImageMetrics::default();
        image_metrics.set(0, 20, 10);
        let document = shaped
            .layout_with_metrics_and_shaper(
                constraints,
                &image_metrics,
                &mut glyph_cache,
                &mut shaped_resources,
            )
            .expect("fragmented exact shaping must succeed");

        assert!(
            glyph_cache.measured_texts.is_empty(),
            "inline-image fragmentation must not trigger backend line reshaping"
        );

        assert_eq!(document.line_count(), 1);
        let fragments = document
            .render_view(&shaped_resources)
            .text()
            .line_text_fragments(0)
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(fragments.len(), 2);
        assert_eq!(fragments[0].glyphs(), 0..1);
        assert_eq!(fragments[1].glyphs(), 1..2);
        assert_eq!(fragments[1].offset_x(), 28.0);
    }

    #[test]
    fn nested_atomic_text_has_unambiguous_line_ownership() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><p style='margin:0;white-space:nowrap'>A<span style='display:inline-flex'>INNER</span>B</p></body></html>",
            200.0,
            &mut document_resources,
        );
        let view = document.render_view(&document_resources).text();
        let glyph_for = |target| {
            (0..view.glyph_count() as u32)
                .find(|index| {
                    view.glyph_at(*index as usize)
                        .and_then(|glyph| view.glyph_metric(glyph))
                        .is_some_and(|metric| metric.ch() == target)
                })
                .expect("fixture character must be shaped")
        };
        let before = glyph_for('A');
        let nested = glyph_for('I');
        let after = glyph_for('B');
        let outer_line = view
            .line_index_for_glyph(before)
            .expect("leading outer text must own a line");

        assert_eq!(
            view.line_index_for_glyph(after),
            Some(outer_line),
            "text after an atomic inline must remain owned by its outer line"
        );
        assert_ne!(
            view.line_index_for_glyph(nested),
            Some(outer_line),
            "nested atomic text must retain its own line identity"
        );
        assert_eq!(
            view.line(outer_line).map(|line| line.index()),
            Some(outer_line)
        );
    }

    #[test]
    fn inline_border_fragments_retain_the_outer_line_across_an_atomic_table() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><span style='font-size:15px;border:3px solid'>left <span style='display:table-cell'>right</span></span></body></html>",
            400.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let leading = (0..text.glyph_count() as u32)
            .find(|index| {
                text.glyph_at(*index as usize)
                    .and_then(|glyph| text.glyph_metric(glyph))
                    .is_some_and(|metric| metric.ch() == 'l')
            })
            .expect("outer text must be shaped");
        let outer_line = text
            .line_index_for_glyph(leading)
            .expect("outer text must own a line");
        let borders = document
            .render_view(&document_resources)
            .fragments()
            .decorations()
            .iter()
            .filter(|fragment| fragment.is_inline() && fragment.color() == 0x000000ff)
            .collect::<Vec<_>>();

        let owned = borders
            .iter()
            .filter(|fragment| fragment.line_idx().is_some())
            .collect::<Vec<_>>();

        assert_eq!(
            owned.len(),
            4,
            "the inline segment must retain all four physical edges: {owned:?}"
        );
        assert!(
            owned
                .iter()
                .all(|fragment| fragment.line_idx() == Some(outer_line)),
            "every segment edge must project through the outer line rather than infer ownership from y geometry"
        );
    }

    #[test]
    fn inline_border_uses_font_content_box_without_half_leading() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><span style='font:10px/30px sans-serif;padding:2px 0;border:10px solid lime'>x</span></body></html>",
            200.0,
            &mut document_resources,
        );
        let borders = document
            .render_view(&document_resources)
            .fragments()
            .decorations()
            .iter()
            .filter(|decoration| decoration.is_inline() && decoration.color() == 0x00ff00ff)
            .collect::<Vec<_>>();

        let top = borders
            .iter()
            .map(|border| border.rect().y0)
            .fold(f64::INFINITY, f64::min);
        let bottom = borders
            .iter()
            .map(|border| border.rect().y1)
            .fold(f64::NEG_INFINITY, f64::max);
        assert_eq!(borders.len(), 4);
        assert_eq!(
            bottom - top,
            34.0,
            "10px font content + 4px padding + 20px border; line-height must not enlarge the border box"
        );
    }

    #[test]
    fn inline_background_paint_order_precedes_its_text_after_an_atomic_inline() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><span style='display:inline-block;width:15px;height:15px'></span> <span style='background:red;color:blue'>X</span></body></html>",
            200.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let x = (0..text.glyph_count() as u32)
            .find(|index| {
                text.glyph_at(*index as usize)
                    .and_then(|glyph| text.glyph_metric(glyph))
                    .is_some_and(|metric| metric.ch() == 'X')
            })
            .expect("fixture X");
        let line_idx = text.line_index_for_glyph(x).expect("X line");
        let text_fragment = text
            .line_text_fragments(line_idx)
            .expect("fragmented line")
            .find(|fragment| fragment.glyphs().contains(&x))
            .expect("X fragment");
        let background = document
            .render_view(&document_resources)
            .fragments()
            .decorations_for_line(line_idx)
            .iter()
            .find(|decoration| decoration.color() == 0xff0000ff)
            .expect("red inline background");

        assert!(
            background.paint_order() <= text_fragment.paint_order(),
            "inline background must not paint after its descendant text"
        );
        assert!(
            text_fragment.offset_x() >= 15.0,
            "text after an atomic inline must retain its placed x position: {}",
            text_fragment.offset_x()
        );
        assert_eq!(
            text_fragment.glyphs(),
            x..x + 1,
            "a fragment must not merge text from a differently styled owner; otherwise its earlier paint key can place the inline background above X"
        );
    }

    #[test]
    fn layout_does_not_enter_backend_boundary_refinement() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = OscillatingBoundaryShaper {
            fallback: GlyphCache::new(),
        };
        let mut shaped_resources = GlyphResourceStore::default();
        let shaped = factory.parse_with_new_pipeline("<html><body style='margin:0'><p style='margin:0;overflow-wrap:anywhere'>abcd</p></body></html>", None).shape(&mut glyph_cache, &mut shaped_resources).expect("fixture shaping must succeed");
        let document = shaped
            .layout_with_metrics_and_shaper(
                crate::LayoutConstraints::new(9.0, 16.0).unwrap(),
                &crate::ImageMetrics::default(),
                &mut glyph_cache,
                &mut shaped_resources,
            )
            .expect("layout uses authoritative document shaping");
        assert_eq!(document.line_count(), 2);
    }

    #[test]
    fn first_line_font_metrics_participate_in_line_breaking() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        let prepared = factory.parse_with_new_pipeline("<html><body><p>aa aa aa</p></body></html>", Some("html,body,p { margin:0 } p { font-size:10px } p::first-line { font-size:20px; font-weight:700; font-family:serif }"));
        let mut shaped_resources = GlyphResourceStore::default();
        let shaped = prepared
            .shape(&mut glyph_cache, &mut shaped_resources)
            .expect("fixture shaping must succeed");
        let document = shaped
            .layout_with_metrics_and_shaper(
                crate::LayoutConstraints::new(28.0, 16.0).unwrap(),
                &crate::ImageMetrics::default(),
                &mut glyph_cache,
                &mut shaped_resources,
            )
            .expect("first-line refinement must shape successfully");

        assert_eq!(
            line_text(&document, 0, &shaped_resources).trim_end(),
            "aa",
            "20px pseudo text must not retain the 10px-font boundary"
        );
        let first = document
            .render_view(&shaped_resources)
            .text()
            .line(0)
            .expect("first line");
        let first_glyph = document
            .render_view(&shaped_resources)
            .text()
            .glyph_slice(first.glyphs())
            .expect("first-line glyphs")[0];
        assert_eq!(
            document
                .render_view(&shaped_resources)
                .text()
                .glyph_metric(first_glyph)
                .expect("first glyph metric")
                .advance(),
            10.0
        );
    }

    #[test]
    fn ancestor_first_line_reaches_the_first_in_flow_block_descendant() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        let mut document_resources = GlyphResourceStore::default();
        let document = factory
            .parse_with_new_pipeline("<!doctype html><html><body style='margin:0'><div id='outer'><p style='margin:0'>First line</p><p>Second block</p></div></body></html>", Some("#outer::first-line { color:green }"))
            .shape(&mut glyph_cache, &mut document_resources)
            .expect("descendant first-line glyphs shape")
            .layout_with_shaper(crate::LayoutConstraints::new(500.0, 16.0).unwrap(), &mut glyph_cache, &mut document_resources).expect("layout requires the shaper-aware API for first-line styles");

        assert_eq!(
            document
                .render_view(&document_resources)
                .text()
                .line(0)
                .expect("first line")
                .paint_color(),
            Some(0x008000ff)
        );
        assert_eq!(
            document
                .render_view(&document_resources)
                .text()
                .line(1)
                .expect("second block line")
                .paint_color(),
            None
        );
    }

    #[test]
    fn first_line_spacing_participates_in_line_breaking() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        let prepared = factory.parse_with_new_pipeline("<html><body><p>aa aa aa</p></body></html>", Some("html,body,p { margin:0 } p { font-size:10px } p::first-line { letter-spacing:5px; word-spacing:5px }"));
        let mut shaped_resources = GlyphResourceStore::default();
        let shaped = prepared
            .shape(&mut glyph_cache, &mut shaped_resources)
            .expect("fixture shaping must succeed");
        let document = shaped
            .layout_with_metrics_and_shaper(
                crate::LayoutConstraints::new(31.0, 16.0).unwrap(),
                &crate::ImageMetrics::default(),
                &mut glyph_cache,
                &mut shaped_resources,
            )
            .expect("first-line spacing refinement must shape successfully");

        assert_eq!(
            line_text(&document, 0, &shaped_resources).trim_end(),
            "aa",
            "pseudo spacing must be visible to the line breaker"
        );
    }

    #[test]
    fn first_line_relayout_discards_the_previous_widths_pseudo_extent() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        let prepared = factory.parse_with_new_pipeline(
            "<html><body><p>aa aa aa aa</p></body></html>",
            Some("html,body,p { margin:0 } p { font-size:10px } p::first-line { font-size:20px }"),
        );
        let mut shaped_resources = GlyphResourceStore::default();
        let shaped = prepared
            .shape(&mut glyph_cache, &mut shaped_resources)
            .expect("fixture shaping must succeed");
        let mut document = shaped
            .layout_with_metrics_and_shaper(
                crate::LayoutConstraints::new(28.0, 16.0).unwrap(),
                &crate::ImageMetrics::default(),
                &mut glyph_cache,
                &mut shaped_resources,
            )
            .expect("initial first-line refinement must succeed");
        assert_eq!(line_text(&document, 0, &shaped_resources).trim_end(), "aa");

        document
            .relayout_with_metrics_and_shaper(
                crate::LayoutConstraints::new(55.0, 16.0).unwrap(),
                &crate::ImageMetrics::default(),
                &mut glyph_cache,
                &mut shaped_resources,
            )
            .expect("width-dependent first-line refinement must succeed");
        assert_eq!(
            line_text(&document, 0, &shaped_resources).trim_end(),
            "aa aa"
        );

        let second = document
            .render_view(&shaped_resources)
            .text()
            .line(1)
            .expect("second line");
        let second_glyph = document
            .render_view(&shaped_resources)
            .text()
            .glyph_slice(second.glyphs())
            .expect("second-line glyphs")[0];
        assert_eq!(
            document
                .render_view(&shaped_resources)
                .text()
                .glyph_metric(second_glyph)
                .expect("second-line metric")
                .advance(),
            5.0,
            "text outside the new first line must use the base 10px shape"
        );
    }

    fn layout_html(
        html: &str,
        width: f64,
        resources: &mut GlyphResourceStore,
    ) -> LaidOutDocument<'static> {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        factory
            .parse_with_new_pipeline(html, None)
            .shape(&mut glyph_cache, resources)
            .expect("test shaper must register every glyph")
            .layout(
                crate::LayoutConstraints::new(width, 16.0).unwrap(),
                resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles")
    }

    #[test]
    fn nested_inline_left_padding_separates_opening_borders() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><div style='display:inline;border-left:5px solid blue;padding-left:50px'>\n  <div style='display:inline;border-left:5px solid orange'>Filler Text</div>\n</div></body></html>",
            400.0,
            &mut document_resources,
        );
        let fragments = document
            .render_view(&document_resources)
            .text()
            .line_text_fragments(0)
            .expect("inline text line")
            .collect::<Vec<_>>();

        assert_eq!(fragments.len(), 1);
        assert_eq!(
            fragments[0].offset_x(),
            60.0,
            "5px outer border + 50px padding + 5px nested border must precede the text"
        );
        let borders = document
            .render_view(&document_resources)
            .fragments()
            .decorations()
            .iter()
            .filter(|decoration| decoration.is_inline())
            .map(|decoration| (decoration.color(), decoration.rect()))
            .collect::<Vec<_>>();
        assert!(
            borders
                .iter()
                .any(|(color, rect)| *color == 0x0000ffff && rect.x0 == 0.0 && rect.x1 == 5.0),
            "outer opening border must paint before its padding: {borders:?}"
        );
        assert!(
            borders
                .iter()
                .any(|(color, rect)| *color == 0xffa500ff && rect.x0 == 55.0 && rect.x1 == 60.0),
            "nested opening border must paint after the outer padding: {borders:?}"
        );
    }

    #[test]
    fn empty_inline_edges_are_published_as_line_fragments() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><span style='padding:0 7px;border:3px solid blue'></span>x<span style='padding:0 5px;border:2px solid orange'></span></body></html>",
            200.0,
            &mut document_resources,
        );
        let decorations = document
            .render_view(&document_resources)
            .fragments()
            .decorations()
            .iter()
            .filter(|fragment| fragment.is_inline())
            .map(|fragment| (fragment.color(), fragment.rect(), fragment.line_idx()))
            .collect::<Vec<_>>();
        let blue = decorations
            .iter()
            .filter(|(color, _, _)| *color == 0x0000ffff)
            .collect::<Vec<_>>();
        let orange = decorations
            .iter()
            .filter(|(color, _, _)| *color == 0xffa500ff)
            .collect::<Vec<_>>();

        assert_eq!(
            blue.len(),
            4,
            "the first empty inline must retain four border edges: {decorations:?}"
        );
        assert_eq!(
            orange.len(),
            4,
            "the second empty inline must retain four border edges: {decorations:?}"
        );
        assert!(
            blue.iter().all(|(_, _, line)| line.is_some())
                && orange.iter().all(|(_, _, line)| line.is_some()),
            "empty inline borders must be owned by their line"
        );
        let blue_right = blue
            .iter()
            .map(|(_, rect, _)| rect.x1)
            .fold(f64::NEG_INFINITY, f64::max);
        let orange_left = orange
            .iter()
            .map(|(_, rect, _)| rect.x0)
            .fold(f64::INFINITY, f64::min);
        assert!(
            orange_left >= blue_right,
            "successive empty fragments must retain source-order geometry: {decorations:?}"
        );
    }

    #[test]
    fn empty_inline_line_height_uses_font_ascent_plus_half_leading() {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::with_font_relative_metrics(0.5, 0.9);
        let mut candidate_resources = GlyphResourceStore::default();
        let candidate = factory
            .parse_with_new_pipeline("<html><body style='margin:0'><div style='font-size:16px;line-height:1'><span style='line-height:5'></span>X</div></body></html>", None)
            .shape(&mut glyph_cache, &mut candidate_resources)
            .expect("candidate shaping")
            .layout(crate::LayoutConstraints::new(200.0, 16.0).unwrap(), &candidate_resources).expect("layout requires the shaper-aware API for first-line styles");
        let mut reference_resources = GlyphResourceStore::default();
        let reference = factory
            .parse_with_new_pipeline("<html><body style='margin:0'><div style='font-size:16px;line-height:5'>X</div></body></html>", None)
            .shape(&mut glyph_cache, &mut reference_resources)
            .expect("reference shaping")
            .layout(crate::LayoutConstraints::new(200.0, 16.0).unwrap(), &reference_resources).expect("layout requires the shaper-aware API for first-line styles");
        let candidate_line = candidate
            .render_view(&candidate_resources)
            .text()
            .line(0)
            .expect("candidate line");
        let reference_line = reference
            .render_view(&reference_resources)
            .text()
            .line(0)
            .expect("reference line");

        assert!(
            (candidate_line.height() - reference_line.height()).abs() < 1e-5,
            "candidate={candidate_line:?}, reference={reference_line:?}"
        );
        assert!(
            (candidate_line.baseline() - reference_line.baseline()).abs() < 1e-5,
            "candidate={candidate_line:?}, reference={reference_line:?}"
        );
    }

    fn layout_with_word_spacing(
        text: &str,
        css: &str,
        width: f64,
        factor: f32,
        policy: crate::TextCompositionPolicy,
        resources: &mut GlyphResourceStore,
    ) -> LaidOutDocument<'static> {
        let html = format!(
            "<html><body style='margin:0'><p style='margin:0;{css}'>{text}</p></body></html>"
        );
        let document = html_parse::parse_dom_document(&html).unwrap();
        let (document, styles) = html_style::style_document(document, &[]).into_parts();
        let styles = styles
            .with_reader_overrides(
                &document,
                &crate::ReaderStyleOverrides {
                    word_spacing_factor: Some(factor),
                    ..Default::default()
                },
            )
            .unwrap();
        let count = text.chars().count();
        let mut shaper = NativeGeometryShaper {
            fallback: GlyphCache::new(),
            advances: vec![2.0; count],
            boundaries: vec![true; count + 1],
            exact_line_advance: None,
            measured_texts: Vec::new(),
        };
        crate::PreparedDocument::try_new(document, styles)
            .unwrap()
            .shape(&mut shaper, resources)
            .unwrap()
            .layout(
                crate::LayoutConstraints::new(width, 16.0)
                    .unwrap()
                    .with_text_composition_policy(policy),
                resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles")
    }

    #[test]
    fn reader_word_spacing_scales_natural_and_authored_gaps_and_publishes_geometry() {
        for (factor, expected) in [(0.5, 2.0), (1.0, 4.0), (1.5, 6.0), (2.0, 8.0)] {
            let mut document_resources = GlyphResourceStore::default();
            let document = layout_with_word_spacing(
                "a a",
                "word-spacing:2px",
                100.0,
                factor,
                crate::TextCompositionPolicy::WebCompatible,
                &mut document_resources,
            );
            let text = document.render_view(&document_resources).text();
            let fragments = text.line_text_fragments(0).unwrap().collect::<Vec<_>>();
            let last = fragments
                .iter()
                .find(|fragment| fragment.glyphs().contains(&2))
                .unwrap();
            assert!(
                (last.offset_x() - (2.0 + expected)).abs() < 1.0e-6,
                "the next word's painted origin must use the scaled gap: {factor}"
            );
            if factor != 1.0 {
                let space = text
                    .line_glyph_advances(0)
                    .unwrap()
                    .iter()
                    .find(|run| run.range().contains(&1))
                    .unwrap();
                assert!(
                    (space.advance() - expected).abs() < 1.0e-6,
                    "selection must use the painted gap width"
                );
            }
        }
    }

    #[test]
    fn reader_word_spacing_changes_wraps_and_scales_nonbreaking_spaces() {
        let mut normal_resources = GlyphResourceStore::default();
        let normal = layout_with_word_spacing(
            "a a a",
            "text-align:left",
            6.5,
            1.0,
            crate::TextCompositionPolicy::WebCompatible,
            &mut normal_resources,
        );
        let mut wider_resources = GlyphResourceStore::default();
        let wider = layout_with_word_spacing(
            "a a a",
            "text-align:left",
            6.5,
            2.0,
            crate::TextCompositionPolicy::WebCompatible,
            &mut wider_resources,
        );
        assert_eq!(line_text(&normal, 0, &normal_resources), "a a");
        assert_eq!(line_text(&wider, 0, &wider_resources), "a");
        let mut nonbreaking_resources = GlyphResourceStore::default();
        let nonbreaking = layout_with_word_spacing(
            "a\u{00a0}a",
            "text-align:left",
            6.5,
            2.0,
            crate::TextCompositionPolicy::WebCompatible,
            &mut nonbreaking_resources,
        );
        assert_eq!(
            nonbreaking.line_count(),
            1,
            "a wider nonbreaking space must remain nonbreaking"
        );
        assert_eq!(
            nonbreaking
                .render_view(&nonbreaking_resources)
                .text()
                .line_glyph_advances(0)
                .unwrap()
                .iter()
                .next()
                .unwrap()
                .advance(),
            4.0
        );
    }

    #[test]
    fn reader_word_spacing_justifies_scaled_gaps_and_keeps_last_lines_natural() {
        for policy in [
            crate::TextCompositionPolicy::WebCompatible,
            crate::TextCompositionPolicy::BookOptimized,
        ] {
            let mut document_resources = GlyphResourceStore::default();
            let document = layout_with_word_spacing(
                "a a a",
                "text-align:justify",
                7.0,
                1.2,
                policy,
                &mut document_resources,
            );
            assert_eq!(line_text(&document, 0, &document_resources), "a a");
            let text = document.render_view(&document_resources).text();
            let space = text
                .line_glyph_advances(0)
                .unwrap()
                .iter()
                .find(|run| run.range().contains(&1))
                .unwrap();
            assert!(
                (space.advance() - 3.0).abs() < 1.0e-6,
                "justification must fill the column after scaling the preferred gap: {policy:?}, actual {}, line adjustment {}",
                space.advance(),
                text.line(0).unwrap().word_spacing()
            );
            let last_word = text
                .line_text_fragments(0)
                .unwrap()
                .find(|fragment| fragment.glyphs().contains(&2))
                .unwrap();
            assert!(
                (last_word.offset_x() - 5.0).abs() < 1.0e-6,
                "painted word positions must include justification"
            );
            assert_eq!(text.line(1).unwrap().word_spacing(), 0.0);
        }
        let mut last_resources = GlyphResourceStore::default();
        let last = layout_with_word_spacing(
            "a a",
            "text-align:justify",
            100.0,
            1.5,
            crate::TextCompositionPolicy::BookOptimized,
            &mut last_resources,
        );
        assert_eq!(
            last.render_view(&last_resources)
                .text()
                .line_glyph_advances(0)
                .unwrap()
                .iter()
                .next()
                .unwrap()
                .advance(),
            3.0
        );
    }

    #[test]
    fn line_breaking_uses_native_run_advances_instead_of_character_fallbacks() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_with_native_geometry(
            "<html><body style='margin:0'><p style='margin:0;overflow-wrap:anywhere'>abcd</p></body></html>",
            7.0,
            vec![3.0; 4],
            vec![true; 5],
            &mut document_resources,
        );

        assert_eq!(document.line_count(), 2);
        assert_eq!(line_text(&document, 0, &document_resources), "ab");
        assert_eq!(line_text(&document, 1, &document_resources), "cd");
    }

    #[test]
    fn emergency_wrapping_never_splits_a_native_cluster() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_with_native_geometry(
            "<html><body style='margin:0'><p style='margin:0;word-break:break-all'>abc</p></body></html>",
            4.0,
            vec![3.0; 3],
            vec![true, false, true, true],
            &mut document_resources,
        );

        assert_eq!(document.line_count(), 2);
        assert_eq!(line_text(&document, 0, &document_resources), "ab");
        assert_eq!(line_text(&document, 1, &document_resources), "c");
    }

    #[test]
    fn justification_uses_the_native_shaped_line_width() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_with_native_geometry(
            "<html><body style='margin:0'><p style='margin:0;text-align:justify'>a a a</p></body></html>",
            7.0,
            vec![2.0; 5],
            vec![true; 6],
            &mut document_resources,
        );

        let first = document
            .render_view(&document_resources)
            .text()
            .line(0)
            .expect("wrapped first line");
        assert_eq!(line_text(&document, 0, &document_resources), "a a");
        assert!((first.word_spacing() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn book_composition_optimizes_left_aligned_paragraphs_without_changing_web_compatible_breaks() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:left'>aa aa b c</p></body></html>";
        let advances = vec![10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 5.0, 10.0, 5.0];
        let boundaries = vec![true; advances.len() + 1];
        let mut web_resources = GlyphResourceStore::default();
        let web = layout_with_native_geometry_policy(
            html,
            70.0,
            advances.clone(),
            boundaries.clone(),
            crate::TextCompositionPolicy::WebCompatible,
            &mut web_resources,
        );
        let mut book_resources = GlyphResourceStore::default();
        let book = layout_with_native_geometry_policy(
            html,
            70.0,
            advances,
            boundaries,
            crate::TextCompositionPolicy::BookOptimized,
            &mut book_resources,
        );

        assert_eq!(
            (
                line_text(&web, 0, &web_resources),
                line_text(&web, 1, &web_resources)
            ),
            ("aa aa b".to_owned(), "c".to_owned()),
            "web-compatible layout must retain greedy first-fit wrapping"
        );
        assert_eq!(
            (
                line_text(&book, 0, &book_resources),
                line_text(&book, 1, &book_resources)
            ),
            ("aa aa".to_owned(), "b c".to_owned()),
            "book composition should improve the paragraph shape globally"
        );
        for line_idx in 0..book.line_count() {
            let line = book
                .render_view(&book_resources)
                .text()
                .line(line_idx)
                .expect("book line");
            assert_eq!(
                line.word_spacing(),
                0.0,
                "ragged composition must preserve natural word spacing"
            );
            assert_eq!(
                line.letter_spacing(),
                0.0,
                "ragged composition must not justify through tracking"
            );
        }
    }

    #[test]
    fn book_composition_hangs_opening_punctuation_without_changing_web_geometry() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:left'>“opening</p></body></html>";
        let advances = vec![10.0; 8];
        let boundaries = vec![true; advances.len() + 1];
        let mut web_resources = GlyphResourceStore::default();
        let web = layout_with_native_geometry_policy(
            html,
            200.0,
            advances.clone(),
            boundaries.clone(),
            crate::TextCompositionPolicy::WebCompatible,
            &mut web_resources,
        );
        let mut book_resources = GlyphResourceStore::default();
        let book = layout_with_native_geometry_policy(
            html,
            200.0,
            advances,
            boundaries,
            crate::TextCompositionPolicy::BookOptimized,
            &mut book_resources,
        );

        assert_eq!(
            web.render_view(&web_resources)
                .text()
                .line(0)
                .expect("web line")
                .optical_offset_x(),
            0.0
        );
        assert!(
            book.render_view(&book_resources)
                .text()
                .line(0)
                .expect("book line")
                .optical_offset_x()
                < 0.0,
            "the opening quote should protrude into the left margin"
        );
        assert_eq!(
            line_text(&web, 0, &web_resources),
            line_text(&book, 0, &book_resources),
            "optical alignment must not alter source ranges or line breaking"
        );
    }

    #[test]
    fn book_composition_hangs_terminal_punctuation_on_a_right_aligned_line() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:right'>ending.”</p></body></html>";
        let advances = vec![10.0; 8];
        let boundaries = vec![true; advances.len() + 1];
        let mut book_resources = GlyphResourceStore::default();
        let book = layout_with_native_geometry_policy(
            html,
            200.0,
            advances,
            boundaries,
            crate::TextCompositionPolicy::BookOptimized,
            &mut book_resources,
        );

        assert!(
            book.render_view(&book_resources)
                .text()
                .line(0)
                .expect("book line")
                .optical_offset_x()
                > 0.0,
            "the closing quote should protrude into the right margin"
        );
    }

    #[test]
    fn book_justification_reserves_optical_room_for_terminal_punctuation() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:justify'>a a.” bbbbb</p></body></html>";
        let advances = vec![10.0; 11];
        let boundaries = vec![true; advances.len() + 1];
        let mut book_resources = GlyphResourceStore::default();
        let book = layout_with_native_geometry_policy(
            html,
            50.0,
            advances,
            boundaries,
            crate::TextCompositionPolicy::BookOptimized,
            &mut book_resources,
        );

        let first = book
            .render_view(&book_resources)
            .text()
            .line(0)
            .expect("first justified line");
        assert_eq!(line_text(&book, 0, &book_resources), "a a.”");
        assert!(
            first.word_spacing() > 0.0 || first.letter_spacing() > 0.0,
            "terminal punctuation should receive bounded optical room beyond the logical measure"
        );
        assert_eq!(
            first.optical_offset_x(),
            0.0,
            "right protrusion must not disturb the aligned left edge"
        );
    }

    #[test]
    fn book_composition_hangs_terminal_dashes_on_right_aligned_lines() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:right'>ending—</p></body></html>";
        let advances = vec![10.0; 7];
        let boundaries = vec![true; advances.len() + 1];
        let mut book_resources = GlyphResourceStore::default();
        let book = layout_with_native_geometry_policy(
            html,
            200.0,
            advances,
            boundaries,
            crate::TextCompositionPolicy::BookOptimized,
            &mut book_resources,
        );

        assert!(
            book.render_view(&book_resources)
                .text()
                .line(0)
                .expect("book line")
                .optical_offset_x()
                > 0.0,
            "the terminal dash should protrude into the right margin"
        );
    }

    #[test]
    fn centered_text_is_not_optically_shifted() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:center'>“centered”</p></body></html>";
        let advances = vec![10.0; 10];
        let boundaries = vec![true; advances.len() + 1];
        let mut book_resources = GlyphResourceStore::default();
        let book = layout_with_native_geometry_policy(
            html,
            200.0,
            advances,
            boundaries,
            crate::TextCompositionPolicy::BookOptimized,
            &mut book_resources,
        );

        assert_eq!(
            book.render_view(&book_resources)
                .text()
                .line(0)
                .expect("book line")
                .optical_offset_x(),
            0.0
        );
    }

    #[test]
    fn justification_shrinks_spaces_when_knuth_plass_selects_a_tight_line() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_with_native_geometry(
            "<html><body style='margin:0'><p style='margin:0;text-align:justify'>a a a</p></body></html>",
            5.5,
            vec![2.0; 5],
            vec![true; 6],
            &mut document_resources,
        );

        let first = document
            .render_view(&document_resources)
            .text()
            .line(0)
            .expect("wrapped first line");
        assert_eq!(line_text(&document, 0, &document_resources), "a a");
        assert!(
            (first.word_spacing() + 0.5).abs() < f64::EPSILON,
            "tight line must shrink its one space by 0.5px, got {}",
            first.word_spacing()
        );
    }

    #[test]
    fn book_justification_distributes_space_by_punctuation_context() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:justify'>a, a a— a bbbbb</p></body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_with_native_geometry_policy(
            html,
            20.85,
            vec![2.0; 15],
            vec![true; 16],
            crate::TextCompositionPolicy::BookOptimized,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let first = text.line(0).expect("first justified line");
        let advances = text
            .line_glyph_advances(0)
            .expect("line advance overrides")
            .iter()
            .map(|run| (run.range().start, run.advance()))
            .collect::<Vec<_>>();

        assert_eq!(line_text(&document, 0, &document_resources), "a, a a— a");
        assert_eq!(
            first.word_spacing(),
            0.0,
            "specialized spaces use exact advances instead of one uniform line value"
        );
        assert_eq!(advances.len(), 3);
        assert_eq!(
            advances.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            vec![2, 4, 7]
        );
        assert!(
            advances[0].1 > advances[1].1,
            "space after a comma should receive more expansion than an ordinary word space: {advances:?}"
        );
        assert!(
            advances[1].1 > advances[2].1,
            "space after a dash should receive less expansion than an ordinary word space: {advances:?}"
        );
        let total_expansion = advances
            .iter()
            .map(|(_, advance)| f64::from(*advance) - 2.0)
            .sum::<f64>();
        assert!(
            (total_expansion - 2.85).abs() < 1.0e-6,
            "painted spaces must consume the same punctuation-aware capacity as the breaker"
        );
    }

    #[test]
    fn web_justification_retains_uniform_space_adjustment() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:justify'>a, a a— a bbbbb</p></body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_with_native_geometry_policy(
            html,
            21.0,
            vec![2.0; 15],
            vec![true; 16],
            crate::TextCompositionPolicy::WebCompatible,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let first = text.line(0).expect("first justified line");

        assert_eq!(line_text(&document, 0, &document_resources), "a, a a— a");
        assert!((first.word_spacing() - 1.0).abs() < 1.0e-9);
        assert!(
            text.line_glyph_advances(0)
                .expect("line advance list")
                .is_empty(),
            "web-compatible justification must not add book punctuation overrides"
        );
    }

    #[test]
    fn book_justification_distributes_space_contraction_by_punctuation_context() {
        let html = "<html><body style='margin:0'><p style='margin:0;text-align:justify'>a, a a— a bbbbb</p></body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_with_native_geometry_policy(
            html,
            16.5,
            vec![2.0; 15],
            vec![true; 16],
            crate::TextCompositionPolicy::BookOptimized,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let advances = text
            .line_glyph_advances(0)
            .expect("line advance overrides")
            .iter()
            .map(|run| run.advance())
            .collect::<Vec<_>>();

        assert_eq!(line_text(&document, 0, &document_resources), "a, a a— a");
        assert_eq!(advances.len(), 3);
        assert!(
            advances[1] < advances[0],
            "ordinary word space should accept more contraction than comma spacing: {advances:?}"
        );
        assert!(
            advances[0] < advances[2],
            "dash-adjacent space should resist contraction most strongly: {advances:?}"
        );
        let total_contraction = advances
            .iter()
            .map(|advance| f64::from(*advance) - 2.0)
            .sum::<f64>();
        assert!(
            (total_contraction + 1.5).abs() < 1.0e-6,
            "weighted contraction must preserve the breaker's total adjustment"
        );
    }

    #[test]
    fn emergency_justification_leaves_residual_space_on_the_right() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_with_native_geometry(
            "<html><body style='margin:0'><p style='margin:0;text-align:justify'>a a a a</p></body></html>",
            8.5,
            vec![2.0; 7],
            vec![true; 8],
            &mut document_resources,
        );

        let first = document
            .render_view(&document_resources)
            .text()
            .line(0)
            .expect("emergency short first line");
        assert_eq!(line_text(&document, 0, &document_resources), "a a");
        assert!(
            (first.word_spacing() - 1.0).abs() < f64::EPSILON,
            "space expansion must stop at 50%; got {}",
            first.word_spacing()
        );
        assert!(
            (first.letter_spacing() - 0.015).abs() < 1.0e-9,
            "micro-tracking must stop after consuming 0.5% of the 6px natural line width; got {}",
            first.letter_spacing()
        );
    }

    #[test]
    fn justification_uses_tiny_positive_cluster_tracking_before_raggedness() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_with_native_geometry(
            "<html><body style='margin:0'><p style='margin:0;text-align:justify'>aaaa aaaa aaaa</p></body></html>",
            19.04,
            vec![2.0; 14],
            vec![true; 15],
            &mut document_resources,
        );

        let first = document
            .render_view(&document_resources)
            .text()
            .line(0)
            .expect("micro-tracked first line");
        assert_eq!(line_text(&document, 0, &document_resources), "aaaa aaaa");
        assert!((first.word_spacing() - 1.0).abs() < 1.0e-9);
        assert!(
            (first.letter_spacing() * 8.0 - 0.04).abs() < 1.0e-6,
            "eight shaped-cluster boundaries must share the 0.04px residual; got {}",
            first.letter_spacing()
        );
    }

    #[test]
    fn justification_uses_tiny_negative_cluster_tracking_after_space_shrink() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_with_native_geometry(
            "<html><body style='margin:0'><p style='margin:0;text-align:justify'>aa aa aa</p></body></html>",
            9.32,
            vec![2.0; 8],
            vec![true; 9],
            &mut document_resources,
        );

        let first = document
            .render_view(&document_resources)
            .text()
            .line(0)
            .expect("micro-tracked tight first line");
        assert_eq!(line_text(&document, 0, &document_resources), "aa aa");
        assert!((first.word_spacing() + 0.66).abs() < 1.0e-9);
        assert!(
            (first.letter_spacing() * 4.0 + 0.02).abs() < 1.0e-6,
            "four shaped-cluster boundaries must share the -0.02px residual; got {}",
            first.letter_spacing()
        );
    }

    #[cfg(feature = "profiling")]
    #[test]
    fn normal_pass_admits_tracking_only_within_final_cluster_capacity() {
        use crate::profiling::capture;
        use std::time::Duration;

        for (text, width, glyphs, adjustment) in [
            ("aaaa aaaa aaaa", 19.04, 14, 0.04),
            ("aa aa aa", 9.32, 8, -0.02),
            ("aaaa aaaa aaaa<br>aaaa aaaa aaaa", 19.04, 14, 0.04),
        ] {
            let html = format!(
                "<html><body style='margin:0'><p style='margin:0;text-align:justify'>{text}</p></body></html>"
            );
            let mut entries_resources = GlyphResourceStore::default();
            let (document, entries) = capture(Duration::default, || {
                layout_with_native_geometry(
                    &html,
                    width,
                    vec![2.0; glyphs],
                    vec![true; glyphs + 1],
                    &mut entries_resources,
                )
            });
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.name == "lines.justified.normal")
            );
            assert!(
                !entries
                    .iter()
                    .any(|entry| entry.name == "lines.justified.emergency"),
                "tracking-feasible text must finish in the normal pass"
            );
            let first = document.render_view().text().line(0).unwrap();
            let boundaries = if adjustment > 0.0 { 8.0 } else { 4.0 };
            assert!((first.letter_spacing() * boundaries - adjustment).abs() < 1.0e-6);
        }

        // The same requested expansion fits eight boundaries but exceeds
        // two native word-cluster boundaries at a 1px line height.
        let html = "<html><body style='margin:0'><p style='margin:0;font:1px/1px sans-serif;text-align:justify'>aaaa aaaa aaaa</p></body></html>";
        for clustered in [false, true] {
            let boundaries = if clustered {
                (0..15)
                    .map(|index| [0, 4, 5, 9, 10, 14].contains(&index))
                    .collect()
            } else {
                vec![true; 15]
            };
            let mut entries_resources = GlyphResourceStore::default();
            let (document, entries) = capture(Duration::default, || {
                layout_with_native_geometry(
                    html,
                    19.03,
                    vec![2.0; 14],
                    boundaries,
                    &mut entries_resources,
                )
            });
            assert_eq!(
                entries
                    .iter()
                    .any(|entry| entry.name == "lines.justified.emergency"),
                clustered
            );
            assert_eq!(line_text(&document, 0, &entries_resources), "aaaa aaaa");
            let first = document.render_view().text().line(0).unwrap();
            assert!(
                first.letter_spacing().abs() <= 0.005 + 1.0e-9,
                "placement must retain its per-boundary tracking limit"
            );
        }
    }

    #[test]
    fn hyphenation_language_prefers_explicit_tags_then_publication_then_english() {
        use html_style_model::{ContentLanguage, LanguageTag};
        use hypher::Lang;
        let tag = |value| ContentLanguage::Tag(LanguageTag::parse(value).unwrap());
        let english = tag("en-GB");
        let french = tag("fr");
        let swedish = tag("sv");
        let unsupported = tag("zz");
        let malformed = ContentLanguage::Unrecognized("en_US".into());
        assert_eq!(hyphenation_language(None, None), Some(Lang::English));
        assert_eq!(
            hyphenation_language(None, Some(&french)),
            Some(Lang::French)
        );
        assert_eq!(
            hyphenation_language(Some(&swedish), Some(&french)),
            Some(Lang::Swedish)
        );
        assert_eq!(
            hyphenation_language(Some(&english), Some(&french)),
            Some(Lang::English)
        );
        assert_eq!(
            hyphenation_language(Some(&unsupported), Some(&english)),
            None
        );
        assert_eq!(hyphenation_language(Some(&malformed), Some(&english)), None);
        assert_eq!(hyphenation_language(None, Some(&unsupported)), None);
    }

    #[test]
    fn inline_language_annotations_control_hyphenation_in_multilingual_text() {
        let html = "<html lang='en'><body style='margin:0;hyphens:auto'><p id='english' style='margin:0'>internationalization internationalization</p><p lang='zz' style='margin:0'>internationalization internationalization</p><p lang='zz' style='margin:0'><span lang='en'>internationalization internationalization</span></p></body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(html, 72.0, &mut document_resources);
        let text = document.render_view(&document_resources).text();
        let mut paragraphs = vec![false; 3];
        for index in 0..document.line_count() {
            let line = text.line(index).unwrap();
            let paragraph_length =
                "internationalization internationalization".chars().count() as u32;
            let paragraph = (line.start() / paragraph_length) as usize;
            paragraphs[paragraph] |= text.hyphen_for_line(index).is_some();
        }
        assert_eq!(
            paragraphs,
            [true, false, true],
            "inline tags must override inherited chapter tags independently for each passage"
        );
    }

    fn layout_with_reader_hyphenation(
        author_rule: &str,
        language: &str,
        text: &str,
        selection: Option<Hyphens>,
        resources: &mut GlyphResourceStore,
    ) -> LaidOutDocument<'static> {
        let html = format!(
            "<html {language}><body style='margin:0'><p style='margin:0;hyphens:{author_rule}'>{text}</p></body></html>"
        );
        let (document, styles) =
            html_style::style_document(html_parse::parse_dom_document(&html).unwrap(), &[])
                .into_parts();
        let styles = styles
            .with_reader_overrides(
                &document,
                &crate::ReaderStyleOverrides {
                    hyphens: selection,
                    ..Default::default()
                },
            )
            .unwrap();
        crate::PreparedDocument::try_new(document, styles)
            .unwrap()
            .shape(&mut GlyphCache::new(), resources)
            .unwrap()
            .layout(
                crate::LayoutConstraints::new(72.0, 16.0).unwrap(),
                resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles")
    }

    #[test]
    fn reader_hyphenation_on_and_off_override_publication_rules_and_book_preserves_them() {
        let mut comparison_resources_45183 = GlyphResourceStore::default();

        let mut comparison_resources_45023 = GlyphResourceStore::default();

        let mut comparison_resources_44910 = GlyphResourceStore::default();

        let mut comparison_resources_44811 = GlyphResourceStore::default();

        let mut comparison_resources_44698 = GlyphResourceStore::default();

        let mut comparison_resources_44600 = GlyphResourceStore::default();

        let content = "internationalization internationalization";
        let has_hyphen = |document: &LaidOutDocument, resources: &GlyphResourceStore| {
            (0..document.line_count()).any(|index| {
                document
                    .render_view(resources)
                    .text()
                    .hyphen_for_line(index)
                    .is_some()
            })
        };
        assert!(!has_hyphen(
            &layout_with_reader_hyphenation(
                "none",
                "lang='en'",
                content,
                None,
                &mut comparison_resources_44600
            ),
            &comparison_resources_44600
        ));
        assert!(has_hyphen(
            &layout_with_reader_hyphenation(
                "none",
                "lang='en'",
                content,
                Some(Hyphens::Auto),
                &mut comparison_resources_44698
            ),
            &comparison_resources_44698
        ));
        assert!(has_hyphen(
            &layout_with_reader_hyphenation(
                "auto",
                "lang='en'",
                content,
                None,
                &mut comparison_resources_44811
            ),
            &comparison_resources_44811
        ));
        assert!(!has_hyphen(
            &layout_with_reader_hyphenation(
                "auto",
                "lang='en'",
                content,
                Some(Hyphens::None),
                &mut comparison_resources_44910
            ),
            &comparison_resources_44910
        ));
        assert!(
            has_hyphen(
                &layout_with_reader_hyphenation(
                    "none",
                    "",
                    content,
                    Some(Hyphens::Auto),
                    &mut comparison_resources_45023
                ),
                &comparison_resources_45023
            ),
            "On defaults to English when no language is declared"
        );
        assert!(
            !has_hyphen(
                &layout_with_reader_hyphenation(
                    "none",
                    "lang='zz'",
                    content,
                    Some(Hyphens::Auto),
                    &mut comparison_resources_45183
                ),
                &comparison_resources_45183
            ),
            "unsupported languages must remain unhyphenated"
        );
    }

    #[test]
    fn reader_hyphenation_off_suppresses_soft_hyphens_and_preserves_literal_hyphens() {
        let content = "inter&shy;nationalization state-of-the-art";
        let mut book_resources = GlyphResourceStore::default();
        let book = layout_with_reader_hyphenation(
            "manual",
            "lang='en'",
            content,
            None,
            &mut book_resources,
        );
        let mut off_resources = GlyphResourceStore::default();
        let off = layout_with_reader_hyphenation(
            "manual",
            "lang='en'",
            content,
            Some(Hyphens::None),
            &mut off_resources,
        );
        assert!((0..book.line_count()).any(|index| {
            book.render_view(&book_resources)
                .text()
                .hyphen_for_line(index)
                .is_some()
        }));
        assert!((0..off.line_count()).all(|index| {
            off.render_view(&off_resources)
                .text()
                .hyphen_for_line(index)
                .is_none()
        }));
        let source = (0..off.line_count())
            .map(|index| line_text(&off, index, &off_resources))
            .collect::<String>();
        assert!(source.contains("state-of-the-art"));
    }

    #[test]
    fn automatic_hyphenation_emits_synthetic_non_source_glyphs() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html lang='en'><body style='margin:0'><p style='margin:0;text-align:justify;hyphens:auto'>internationalization internationalization</p></body></html>",
            72.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let hyphenated_line =
            (0..document.line_count()).find(|&line_idx| text.hyphen_for_line(line_idx).is_some());

        assert!(
            hyphenated_line.is_some(),
            "a narrow English justified paragraph should select at least one dictionary breakpoint"
        );
        let source = (0..document.line_count())
            .map(|line_idx| line_text(&document, line_idx, &document_resources))
            .collect::<String>();
        assert!(
            !source.contains('\u{2010}'),
            "synthetic hyphens must not enter source text, selection, or CFI ranges"
        );
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
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html lang='en'><body style='margin:0'><p style='margin:0;hyphens:manual'>extra&shy;ordinary tail</p></body></html>",
            56.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let soft_hyphen = (0..text.glyph_count() as u32)
            .find(|&index| {
                text.glyph_at(index as usize)
                    .and_then(|glyph| text.glyph_metric(glyph))
                    .is_some_and(|metric| metric.ch() == '\u{00ad}')
            })
            .expect("fixture contains a source soft hyphen");

        assert_eq!(
            text.character_advance(soft_hyphen),
            Some(0.0),
            "an unbroken soft hyphen must consume no inline width"
        );
        assert!(
            (0..document.line_count()).any(|line_idx| text.hyphen_for_line(line_idx).is_some()),
            "manual hyphenation should use the authored conditional break"
        );
        let source = (0..document.line_count())
            .map(|line_idx| line_text(&document, line_idx, &document_resources))
            .collect::<String>();
        assert!(
            source.contains('\u{00ad}'),
            "the authored marker remains part of the underlying source"
        );
        assert!(
            !source.contains('\u{2010}'),
            "the displayed hyphen must remain synthetic"
        );
    }

    #[test]
    fn hyphens_none_suppresses_an_authored_soft_hyphen_break() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html lang='en'><body style='margin:0'><p style='margin:0;hyphens:none'>extra&shy;ordinary tail</p></body></html>",
            56.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let soft_hyphen = (0..text.glyph_count() as u32)
            .find(|&index| {
                text.glyph_at(index as usize)
                    .and_then(|glyph| text.glyph_metric(glyph))
                    .is_some_and(|metric| metric.ch() == '\u{00ad}')
            })
            .expect("fixture contains a source soft hyphen");

        assert_eq!(
            text.character_advance(soft_hyphen),
            Some(0.0),
            "a suppressed soft hyphen must remain invisible"
        );
        assert!(
            (0..document.line_count()).all(|line_idx| text.hyphen_for_line(line_idx).is_none()),
            "hyphens:none must suppress conditional hyphen rendering"
        );
    }

    #[test]
    fn nowrap_suppresses_an_authored_soft_hyphen_break() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html lang='en'><body style='margin:0'><p style='margin:0;white-space:nowrap;hyphens:manual'>extra&shy;ordinary tail</p></body></html>",
            56.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();

        assert_eq!(
            document.line_count(),
            1,
            "a conditional hyphen is still a soft wrap opportunity and must obey nowrap"
        );
        assert!(text.hyphen_for_line(0).is_none());
    }

    #[test]
    fn automatic_hyphenation_prefers_an_authored_soft_hyphen_in_the_word() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html lang='en'><body style='margin:0'><p style='margin:0;hyphens:auto'>inter&shy;nationalization tail</p></body></html>",
            56.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let hyphenated_line = (0..document.line_count())
            .find(|&line_idx| text.hyphen_for_line(line_idx).is_some())
            .expect("the authored conditional break should fit the narrow line");

        assert!(
            line_text(&document, hyphenated_line, &document_resources).ends_with('\u{00ad}'),
            "automatic dictionary points in a word with an authored soft hyphen must not supersede it"
        );
    }

    #[test]
    fn automatic_hyphenation_joins_words_across_inline_formatting() {
        let plain = "internationalization internationalization";
        let styled = "in<span style='color:red'>terna</span>tionalization inter<span style='color:blue'>national</span>ization";
        for width in [48.0, 64.0, 72.0, 88.0, 104.0, 128.0] {
            let mut layout_resources = GlyphResourceStore::default();
            let layout = |content, resources: &mut GlyphResourceStore| {
                layout_html(
                    &format!(
                        "<html lang='en'><body style='margin:0;hyphens:auto'>{content}</body></html>"
                    ),
                    width,
                    resources,
                )
            };
            let mut expected_resources = GlyphResourceStore::default();
            let expected = layout(plain, &mut expected_resources);
            let actual = layout(styled, &mut layout_resources);
            let lines = |document: &LaidOutDocument, resources: &GlyphResourceStore| {
                (0..document.line_count())
                    .map(|index| {
                        (
                            line_text(document, index, resources),
                            document
                                .render_view(resources)
                                .text()
                                .hyphen_for_line(index)
                                .is_some(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                lines(&actual, &layout_resources),
                lines(&expected, &expected_resources),
                "inline color boundaries changed hyphenation at width {width}"
            );
        }
    }

    #[test]
    fn automatic_hyphenation_respects_soft_hyphens_across_inline_formatting() {
        let html = "<html lang='en'><body style='margin:0;hyphens:auto'>inter<span style='color:red'>&shy;national</span>ization tail</body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(html, 56.0, &mut document_resources);
        let hyphenated = (0..document.line_count())
            .filter(|&index| {
                document
                    .render_view(&document_resources)
                    .text()
                    .hyphen_for_line(index)
                    .is_some()
            })
            .collect::<Vec<_>>();
        assert!(!hyphenated.is_empty());
        assert!(hyphenated.into_iter().all(|index| line_text(&document, index, &document_resources).ends_with('\u{00ad}')));
    }

    #[test]
    fn automatic_hyphenation_does_not_split_words_at_policy_boundaries() {
        for attributes in [
            "lang='zz'",
            "lang='fr'",
            "lang='en-GB'",
            "style='hyphens:none'",
            "style='white-space:nowrap'",
        ] {
            let html = format!(
                "<html lang='en'><body style='margin:0;hyphens:auto'>inter<span {attributes}>national</span>ization</body></html>"
            );
            let mut document_resources = GlyphResourceStore::default();
            let document = layout_html(&html, 48.0, &mut document_resources);
            assert!(
                (0..document.line_count()).all(|index| document
                    .render_view(&document_resources)
                    .text()
                    .hyphen_for_line(index)
                    .is_none()),
                "a word with {attributes} must not be hyphenated as separate fragments"
            );
        }
    }

    #[test]
    fn automatic_hyphenation_apostrophe_variants_have_identical_layout() {
        let mut layout_resources = GlyphResourceStore::default();
        let layout = |apostrophe: char, resources: &mut GlyphResourceStore| {
            layout_html(
                &format!(
                    "<html lang='en'><body style='margin:0;hyphens:auto'>internationalization{apostrophe}s internationalization{apostrophe}s</body></html>"
                ),
                72.0,
                resources,
            )
        };
        let lines = |document: &LaidOutDocument, resources: &GlyphResourceStore| {
            (0..document.line_count())
                .map(|index| {
                    (
                        line_text(document, index, resources)
                            .replace(['\u{2019}', '\u{02bc}'], "'"),
                        document
                            .render_view(resources)
                            .text()
                            .hyphen_for_line(index)
                            .is_some(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let expected = lines(&layout('\'', &mut layout_resources), &layout_resources);
        assert_eq!(
            lines(
                &layout('\u{2019}', &mut layout_resources),
                &layout_resources
            ),
            expected
        );
        assert_eq!(
            lines(
                &layout('\u{02bc}', &mut layout_resources),
                &layout_resources
            ),
            expected
        );
    }

    #[test]
    fn automatic_hyphenation_respects_native_shaping_clusters() {
        let word = "internationalization";
        let count = word.chars().count();
        let mut boundaries = vec![false; count + 1];
        boundaries[0] = true;
        boundaries[count] = true;
        let html =
            format!("<html lang='en'><body style='margin:0;hyphens:auto'>{word}</body></html>");
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_with_native_geometry(
            &html,
            48.0,
            vec![8.0; count],
            boundaries,
            &mut document_resources,
        );
        assert!((0..document.line_count()).all(|index| {
            document
                .render_view(&document_resources)
                .text()
                .hyphen_for_line(index)
                .is_none()
        }));
    }

    #[test]
    fn automatic_hyphenation_preserves_decomposed_source_positions_in_layout() {
        use unicode_categories::UnicodeCategories;
        use unicode_normalization::UnicodeNormalization;

        let composed = "électroencéphalographie";
        let decomposed = composed.nfd().collect::<String>();
        let mut layout_resources = GlyphResourceStore::default();
        let layout = |word: &str, resources: &mut GlyphResourceStore| {
            let advances = word
                .chars()
                .map(|ch| if ch.is_mark() { 0.0 } else { 8.0 })
                .collect();
            let mut boundaries = word.chars().map(|ch| !ch.is_mark()).collect::<Vec<_>>();
            boundaries.push(true);
            layout_with_native_geometry(
                &format!(
                    "<html lang='fr'><body style='margin:0;hyphens:auto'>{word}</body></html>"
                ),
                72.0,
                advances,
                boundaries,
                resources,
            )
        };
        let mut expected_resources = GlyphResourceStore::default();
        let expected = layout(composed, &mut expected_resources);
        let actual = layout(&decomposed, &mut layout_resources);
        let lines = |document: &LaidOutDocument, resources: &GlyphResourceStore| {
            (0..document.line_count())
                .map(|index| {
                    (
                        line_text(document, index, resources)
                            .nfc()
                            .collect::<String>(),
                        document
                            .render_view(resources)
                            .text()
                            .hyphen_for_line(index)
                            .is_some(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            lines(&actual, &layout_resources),
            lines(&expected, &expected_resources)
        );
        assert!((0..actual.line_count()).any(|index| {
            actual
                .render_view(&layout_resources)
                .text()
                .hyphen_for_line(index)
                .is_some()
        }));
        let original_source = (0..actual.line_count())
            .map(|index| line_text(&actual, index, &layout_resources))
            .collect::<String>();
        assert_eq!(
            original_source, decomposed,
            "normalization must not rewrite selection/source ranges"
        );
    }

    fn layout_html_with_images(
        html: &str,
        width: f64,
        image_sizes: &[(u32, u32)],
        policy: crate::ImageSizingPolicy,
        resources: &mut GlyphResourceStore,
    ) -> LaidOutDocument<'static> {
        layout_html_with_images_in_viewport(html, width, None, image_sizes, policy, resources)
    }

    fn layout_html_with_images_in_viewport(
        html: &str,
        width: f64,
        height: Option<f64>,
        image_sizes: &[(u32, u32)],
        policy: crate::ImageSizingPolicy,
        resources: &mut GlyphResourceStore,
    ) -> LaidOutDocument<'static> {
        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        let shaped = factory
            .parse_with_new_pipeline(html, None)
            .shape(&mut glyph_cache, resources)
            .expect("test shaper must register every glyph");
        let mut metrics = crate::ImageMetrics::default();
        for (image_idx, &(image_width, image_height)) in image_sizes.iter().enumerate() {
            metrics.set(image_idx as u32, image_width, image_height);
        }
        let constraints = crate::LayoutConstraints::new(width, 16.0)
            .unwrap()
            .with_viewport_height(height)
            .unwrap()
            .with_image_sizing_policy(policy);
        shaped
            .layout_with_metrics(constraints, &metrics, resources)
            .expect("layout requires the shaper-aware API for first-line styles")
    }

    fn tag_size(
        document: &LaidOutDocument,
        tag: &str,
        resources: &GlyphResourceStore,
    ) -> kurbo::Size {
        let boxes = document.render_view(resources).boxes();
        let box_idx = (0..boxes.len())
            .find(|&idx| {
                boxes
                    .tag(idx)
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(tag))
            })
            .unwrap_or_else(|| panic!("{tag} box should exist"));
        boxes.size(box_idx).expect("box index in range")
    }

    fn tag_point(
        document: &LaidOutDocument,
        tag: &str,
        resources: &GlyphResourceStore,
    ) -> kurbo::Point {
        let boxes = document.render_view(resources).boxes();
        let box_idx = (0..boxes.len())
            .find(|&idx| {
                boxes
                    .tag(idx)
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(tag))
            })
            .unwrap_or_else(|| panic!("{tag} box should exist"));
        boxes.point(box_idx).expect("box index in range")
    }

    fn line_text(
        document: &LaidOutDocument,
        line_idx: usize,
        resources: &GlyphResourceStore,
    ) -> String {
        let line = document
            .render_view(resources)
            .text()
            .line(line_idx)
            .expect("line index in range");
        let text = document.render_view(resources).text();
        text.glyph_slice(line.glyphs())
            .expect("line glyph range")
            .iter()
            .map(|&glyph| text.glyph_metric(glyph).expect("registered glyph").ch())
            .collect()
    }

    fn tab_advance(
        document: &LaidOutDocument,
        line_idx: usize,
        resources: &GlyphResourceStore,
    ) -> f64 {
        document
            .render_view(resources)
            .text()
            .line_glyph_advances(line_idx)
            .expect("advance list for line")
            .iter()
            .next()
            .expect("preserved tab advance")
            .advance()
    }

    fn alignment_token(
        vertical_align: VerticalAlignValue,
    ) -> (InlineToken, [InlineTokenMetrics; 1]) {
        let mut token = InlineToken::new(
            InlineTokenKind::Glyph { glyph_idx: 0 },
            8.0,
            BreakKind::None,
            TokenWrap::Normal,
            true,
        );
        token.set_run_idx(0);
        let metrics = [InlineTokenMetrics {
            owner_box_idx: u32::MAX,
            ascent: 8.0,
            descent: 2.0,
            line_height: 12.0,
            font_size: 16.0,
            vertical_align: InlineVerticalAlign::resolve(vertical_align, 12.0, 16.0),
            white_space: WhiteSpace::Normal,
            visible: true,
            placement_required: false,
        }];
        (token, metrics)
    }

    #[test]
    fn common_inline_token_stays_cache_compact() {
        assert_eq!(
            std::mem::size_of::<InlineToken>(),
            12,
            "dense inline token grew to {} bytes",
            std::mem::size_of::<InlineToken>()
        );
        assert_eq!(std::mem::align_of::<InlineToken>(), 4);
        assert_eq!(std::mem::offset_of!(InlineToken, width), 8);
        assert_eq!(std::mem::size_of::<InlineVerticalAlign>(), 8);
        assert_eq!(std::mem::size_of::<InlineTokenMetrics>(), 32);
        assert!(
            std::mem::size_of::<InlineTokenKind>() <= 8,
            "rare replaced geometry leaked back into the token discriminant"
        );
    }

    #[test]
    fn packed_token_header_preserves_fields_at_their_bit_limits() {
        use super::justification::SpaceGlueClass;
        let index = InlineToken::MAX_DATA;
        let kinds = [
            InlineTokenKind::Glyph { glyph_idx: index },
            InlineTokenKind::Ellipsis { glyph: index },
            InlineTokenKind::Image { payload_idx: index },
            InlineTokenKind::AtomicBox { payload_idx: index },
            InlineTokenKind::Break {
                clear: html_style_model::Clear::Both,
            },
            InlineTokenKind::FloatAnchor { box_idx: index },
            InlineTokenKind::InlineBoundary { payload_idx: index },
            InlineTokenKind::Discretionary { glyph: index },
            InlineTokenKind::Opportunity,
            InlineTokenKind::AbsoluteAnchor { box_idx: index },
        ];
        for kind in kinds {
            for wrap in [TokenWrap::Normal, TokenWrap::BreakWord, TokenWrap::Anywhere] {
                for break_kind in [
                    BreakKind::None,
                    BreakKind::Soft,
                    BreakKind::Hard,
                    BreakKind::Discretionary,
                ] {
                    let mut token = InlineToken::new(kind, 12.5, break_kind, wrap, true);
                    token.set_run_idx(InlineToken::MAX_RUN_IDX);
                    token.set_bidi_level(u8::MAX);
                    token.set_is_space();
                    token.set_is_tab();
                    token.set_space_glue_class(SpaceGlueClass::Clause);
                    assert!(token.kind() == kind);
                    assert!(token.wrap() == wrap);
                    assert!(token.break_kind() == break_kind);
                    assert!(token.is_cluster_boundary() && token.is_space() && token.is_tab());
                    assert_eq!(token.space_glue_class(), SpaceGlueClass::Clause);
                    assert_eq!(token.run_idx(), InlineToken::MAX_RUN_IDX);
                    assert_eq!(token.bidi_level(), u8::MAX);
                    assert_eq!(f64::from(token.width), 12.5);

                    token.set_run_idx(0);
                    token.set_bidi_level(0);
                    assert!(token.kind() == kind);
                    assert!(token.break_kind() == break_kind && token.wrap() == wrap);
                    assert_eq!(token.run_idx(), 0);
                    assert_eq!(token.bidi_level(), 0);
                    assert_eq!(token.space_glue_class(), SpaceGlueClass::Clause);
                    assert!(token.is_cluster_boundary() && token.is_space() && token.is_tab());
                }
            }
        }
    }

    #[test]
    #[should_panic(expected = "glyph/payload index exceeds 24-bit capacity")]
    fn packed_token_rejects_indices_above_24_bits() {
        InlineToken::new(
            InlineTokenKind::Glyph {
                glyph_idx: InlineToken::MAX_DATA + 1,
            },
            8.0,
            BreakKind::None,
            TokenWrap::Normal,
            true,
        );
    }

    #[test]
    #[should_panic(expected = "metrics index exceeds 18-bit capacity")]
    fn packed_token_reserves_the_unbound_metrics_index() {
        let mut token = InlineToken::new(
            InlineTokenKind::Opportunity,
            0.0,
            BreakKind::None,
            TokenWrap::Normal,
            true,
        );
        token.set_run_idx(InlineToken::MAX_RUN_IDX + 1);
    }

    #[test]
    fn adjacent_tokens_share_run_metrics() {
        let (first, [metrics]) = alignment_token(VerticalAlignValue::Baseline);
        let second = InlineToken::new(
            InlineTokenKind::Glyph { glyph_idx: 1 },
            8.0,
            BreakKind::None,
            TokenWrap::Normal,
            true,
        );
        let mut tokens = InlineTokens::with_capacity(2);

        tokens.push(first, metrics);
        tokens.push(second, metrics);

        assert_eq!(tokens.runs.len(), 1);
        assert_eq!(tokens.dense[0].run_idx(), tokens.dense[1].run_idx());
    }

    #[test]
    fn plain_text_uses_implicit_zero_glyph_offsets() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><span style='line-height:12px'>ab</span><span style='line-height:18px'>cd</span></body></html>",
            200.0,
            &mut document_resources,
        );
        let offsets = document
            .render_view(&document_resources)
            .text()
            .line_glyph_offsets(0)
            .expect("plain line offset runs")
            .iter()
            .collect::<Vec<_>>();

        assert!(offsets.is_empty(), "zero is the implicit glyph offset");
    }

    #[test]
    fn percentage_baseline_shift_uses_the_elements_line_height() {
        let (token, runs) = alignment_token(VerticalAlignValue::Percent(0.5));

        assert_eq!(vertical_align_offset(&token, &runs, 40.0, 10.0), 6.0);
    }

    #[test]
    fn calc_baseline_shift_keeps_absolute_and_line_height_components() {
        let (token, runs) = alignment_token(VerticalAlignValue::Calc {
            absolute_px: -2.0,
            line_height_fraction: 0.5,
            x_height_px: 0.0,
        });

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
        assert_eq!(
            super::vertical_align_metrics_offset(
                VerticalAlignValue::TextTop,
                8.0,
                2.0,
                20.0,
                10.0,
                5.0,
                40.0,
                25.0,
                8.0,
                2.0
            ),
            -5.0
        );
        assert_eq!(
            super::vertical_align_metrics_offset(
                VerticalAlignValue::TextBottom,
                8.0,
                2.0,
                20.0,
                10.0,
                5.0,
                40.0,
                25.0,
                8.0,
                2.0
            ),
            5.0
        );
    }

    #[test]
    fn authored_calc_vertical_align_reaches_final_glyph_offsets() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><p style='font-size:16px; line-height:12px'>A<span style='vertical-align:calc(50% - 2px)'>B</span></p></body></html>",
            200.0,
            &mut document_resources,
        );
        let offset = document
            .render_view(&document_resources)
            .text()
            .line_glyph_offsets(0)
            .expect("first line offset runs")
            .iter()
            .find(|run| run.range().contains(&1))
            .expect("the shifted glyph has an offset")
            .offset();

        assert!(
            (offset - 4.0).abs() < 0.001,
            "calc shift should preserve both components, got {offset}"
        );
    }

    #[test]
    fn numeric_alignment_uses_a_compact_offset_without_losing_script_identity() {
        for (value, expected) in [
            (VerticalAlignValue::Length(-3.5), -3.5),
            (VerticalAlignValue::Percent(0.25), 3.0),
            (
                VerticalAlignValue::Calc {
                    absolute_px: -2.0,
                    line_height_fraction: 0.5,
                    x_height_px: 0.0,
                },
                4.0,
            ),
            (VerticalAlignValue::Sub, -4.0),
            (VerticalAlignValue::Super, 8.0),
        ] {
            let (token, runs) = alignment_token(value);
            assert_eq!(vertical_align_offset(&token, &runs, 40.0, 10.0), expected);
        }
        assert_ne!(
            InlineVerticalAlign::resolve(VerticalAlignValue::Super, 12.0, 16.0),
            InlineVerticalAlign::Offset(8.0)
        );
        assert_ne!(
            InlineVerticalAlign::resolve(VerticalAlignValue::Sub, 12.0, 16.0),
            InlineVerticalAlign::Offset(-4.0)
        );
        assert_ne!(
            InlineVerticalAlign::Baseline,
            InlineVerticalAlign::Offset(0.0)
        );
    }

    #[test]
    fn first_line_height_reresolves_numeric_alignment() {
        for (alignment, expected) in [("50%", 20.0), ("calc(50% - 2px)", 18.0)] {
            let mut factory = DocumentFactory::new();
            let mut glyphs = GlyphCache::new();
            let markup = format!(
                "<html><body style='margin:0'><div style='font-size:16px;line-height:20px'>A<span style='vertical-align:{alignment}'>B</span></div></body></html>"
            );
            let mut document_resources = GlyphResourceStore::default();
            let document = factory
                .parse_with_new_pipeline(&markup, Some("div::first-line { line-height:40px }"))
                .shape(&mut glyphs, &mut document_resources)
                .expect("first-line glyphs shape")
                .layout_with_shaper(
                    crate::LayoutConstraints::new(200.0, 16.0).unwrap(),
                    &mut glyphs,
                    &mut document_resources,
                )
                .expect("layout requires the shaper-aware API for first-line styles");
            let offset = document
                .render_view(&document_resources)
                .text()
                .line_glyph_offsets(0)
                .expect("aligned first line")
                .iter()
                .find(|run| run.range().contains(&1))
                .expect("shifted B")
                .offset();
            assert!(
                (offset - expected).abs() < 0.001,
                "{alignment} should resolve against the 40px first-line height, got {offset}"
            );
        }
    }

    #[test]
    fn table_cell_alignment_does_not_shift_its_anonymous_inline_text() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><table><tr style='vertical-align:middle'><td>XXXXX<span style='font-size:2em;vertical-align:baseline'>XXXXX</span></td></tr></table></body></html>",
            300.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let line_idx = (0..text.line_count())
            .find(|&line_idx| line_text(&document, line_idx, &document_resources) == "XXXXXXXXXX")
            .expect("table cell text line");
        let offsets = text.line_glyph_offsets(line_idx).expect("line offset runs");
        let offset_for = |glyph| {
            offsets
                .iter()
                .find(|run| run.range().contains(&glyph))
                .map_or(0.0, |run| run.offset())
        };

        assert!(
            offset_for(0).abs() < 0.001,
            "the cell's row-alignment value must not shift direct text inside its line"
        );
        assert!(
            offset_for(5).abs() < 0.001,
            "the baseline-aligned span must share the anonymous inline's baseline"
        );
    }

    #[test]
    fn raised_inline_expands_the_line_and_moves_the_unshifted_baseline() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><div style='font-size:20px;line-height:20px'><span style='vertical-align:96px'>X</span>X</div></body></html>",
            200.0,
            &mut document_resources,
        );
        let line = document
            .render_view(&document_resources)
            .text()
            .line(0)
            .expect("text produces one line");
        let offsets = document
            .render_view(&document_resources)
            .text()
            .line_glyph_offsets(0)
            .expect("raised glyph requires explicit offsets");
        let offset_for = |glyph| {
            offsets
                .iter()
                .find(|run| run.range().contains(&glyph))
                .map_or(0.0, |run| run.offset())
        };

        assert!(
            (line.height() - 116.0).abs() < 0.001,
            "a 96px raise above a 20px strut must produce a 116px line, got {}",
            line.height()
        );
        assert!((offset_for(0) - 96.0).abs() < 0.001);
        assert!(offset_for(1).abs() < 0.001);
    }

    #[test]
    fn descendant_text_does_not_erase_an_ancestor_inline_box_strut() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><div style='font-size:10px;line-height:10px'><span style='font-size:30px;line-height:30px'><span style='font-size:10px;line-height:10px'>A</span></span>B</div></body></html>",
            200.0,
            &mut document_resources,
        );
        let line = document
            .render_view(&document_resources)
            .text()
            .line(0)
            .expect("text produces one line");

        assert!(
            (line.height() - 30.0).abs() < 0.001,
            "the ancestor inline's own strut must participate even though its only glyph belongs to a smaller descendant; got {}",
            line.height()
        );
    }

    #[test]
    fn nested_vertical_align_moves_descendants_without_inheriting_the_value() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><p style='font-size:16px;line-height:20px'>A<span style='vertical-align:5px'>B<span style='vertical-align:3px'>C</span></span></p></body></html>",
            200.0,
            &mut document_resources,
        );
        let offsets = document
            .render_view(&document_resources)
            .text()
            .line_glyph_offsets(0)
            .expect("aligned descendants require explicit offsets");
        let offset_for = |glyph| {
            offsets
                .iter()
                .find(|run| run.range().contains(&glyph))
                .map_or(0.0, |run| run.offset())
        };

        assert!(
            (offset_for(1) - 5.0).abs() < 0.001,
            "the parent span applies its own alignment once"
        );
        assert!(
            (offset_for(2) - 8.0).abs() < 0.001,
            "the child moves with its parent and adds its own alignment"
        );
    }

    #[test]
    fn top_alignment_moves_the_complete_inline_subtree_as_one_group() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><span style='vertical-align:top'><span style='font-size:80px'>A</span>x</span></body></html>",
            200.0,
            &mut document_resources,
        );
        let line = document
            .render_view(&document_resources)
            .text()
            .line(0)
            .expect("grouped inline line");
        let offsets = document
            .render_view(&document_resources)
            .text()
            .line_glyph_offsets(0)
            .expect("grouped inline offsets");
        let offset_for = |glyph| {
            offsets
                .iter()
                .find(|run| run.range().contains(&glyph))
                .map_or(0.0, |run| run.offset())
        };

        assert!(
            (line.height() - 96.0).abs() < 0.001,
            "the 80px descendant's normal line area determines the group height"
        );
        assert!(
            (offset_for(0) - offset_for(1)).abs() < 0.001,
            "top moves the complete subtree as one group rather than independently aligning its anonymous inlines: A={}, x={}",
            offset_for(0),
            offset_for(1)
        );
    }

    #[test]
    fn top_and_bottom_groups_expand_opposite_line_edges() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><div style='width:180px;font:30px/1 Ahem'>x<span style='font-size:60px;vertical-align:bottom'>X</span>x</div><div style='width:180px;font:30px/1 Ahem'>x<span style='font-size:60px;vertical-align:top'>X</span>x</div></body></html>",
            300.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let bottom = text.line(0).expect("bottom-aligned line");
        let top = text.line(1).expect("top-aligned line");

        assert_eq!(bottom.height(), 60.0);
        assert_eq!(top.height(), 60.0);
        assert!(
            (bottom.baseline() - 52.5).abs() < 0.001,
            "a taller bottom group expands above the ordinary strut"
        );
        assert!(
            (top.baseline() - 24.0).abs() < 0.001,
            "a taller top group expands below the ordinary strut"
        );
    }

    #[test]
    fn authored_line_height_can_be_smaller_than_glyph_extents_and_layout_fallback() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><div style='line-height:13px'>Text</div></body></html>",
            200.0,
            &mut document_resources,
        );
        let line = document
            .render_view(&document_resources)
            .text()
            .line(0)
            .expect("text produces one line");

        assert!(
            (line.height() - 13.0).abs() < 0.001,
            "authored line-height must not be clamped to the 16px layout fallback"
        );
    }

    #[test]
    fn zero_line_height_preserves_negative_half_leading() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><div style='font:20px/1 Ahem;line-height:0;width:20px'>X X</div></body></html>",
            200.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let first = text.line(0).expect("first wrapped line");
        let second = text.line(1).expect("second wrapped line");

        assert_eq!(first.height(), 0.0);
        assert_eq!(second.height(), 0.0);
        assert_eq!(
            first.point().y,
            second.point().y,
            "zero-height lines advance by zero while their glyph ink overflows"
        );
    }

    #[test]
    fn ex_line_height_uses_the_selected_faces_x_height() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><div style='font-size:20px;line-height:1ex;width:20px'>X X</div></body></html>",
            200.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();

        assert_eq!(text.line(0).expect("first wrapped line").height(), 10.0);
        assert_eq!(text.line(1).expect("second wrapped line").height(), 10.0);
    }

    #[test]
    fn zero_sized_inline_text_does_not_remove_the_parent_line_strut() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><p style='margin:0;font-size:16px;line-height:1.2'><span style='font-size:0'>zero</span></p></body></html>",
            200.0,
            &mut document_resources,
        );
        let line = document
            .render_view(&document_resources)
            .text()
            .line(0)
            .expect("zero-sized descendant still creates the parent's line box");

        assert!(
            (line.height() - 19.2).abs() < 0.001,
            "the parent strut controls the line height, got {}",
            line.height()
        );
    }

    #[test]
    fn empty_float_anchor_does_not_inflate_zero_line_height() {
        let p = "style='font-size:20px;line-height:0'";
        let mut candidate_resources = GlyphResourceStore::default();
        let candidate = layout_html(
            &format!(
                "<html><body><p {p}><span style='position:absolute'></span>Some paragraph</p><p {p}><span style='float:left'></span>Some paragraph</p><p {p}>Some other paragraph</p></body></html>"
            ),
            800.0,
            &mut candidate_resources,
        );
        let mut reference_resources = GlyphResourceStore::default();
        let reference = layout_html(
            &format!(
                "<html><body><p {p}>Some paragraph</p><p {p}>Some paragraph</p><p {p}>Some other paragraph</p></body></html>"
            ),
            800.0,
            &mut reference_resources,
        );
        let candidate_lines = candidate.render_view(&candidate_resources).text().lines();
        let reference_lines = reference.render_view(&reference_resources).text().lines();

        assert_eq!(candidate_lines.len(), reference_lines.len());
        for (line_idx, (candidate_line, reference_line)) in candidate_lines
            .iter()
            .zip(reference_lines.iter())
            .enumerate()
        {
            assert_eq!(
                candidate_line.point(),
                reference_line.point(),
                "line {line_idx} point"
            );
            assert_eq!(
                candidate_line.height(),
                reference_line.height(),
                "line {line_idx} height"
            );
            assert_eq!(
                candidate_line.baseline(),
                reference_line.baseline(),
                "line {line_idx} baseline"
            );
        }
    }

    #[test]
    fn collapsible_indentation_around_float_only_content_does_not_create_a_carrier_line() {
        let style = "style='float:right;width:60px;height:20px;margin:4px'";
        let mut indented_resources = GlyphResourceStore::default();
        let indented = layout_html(
            &format!(
                "<html><body style='margin:0'><div id='host'>\n <p id='b' {style}>B</p>\n <p id='a' {style}>A</p>\n</div><p id='after'>after</p></body></html>"
            ),
            300.0,
            &mut indented_resources,
        );
        let mut compact_resources = GlyphResourceStore::default();
        let compact = layout_html(
            &format!(
                "<html><body style='margin:0'><div id='host'><p id='b' {style}>B</p><p id='a' {style}>A</p></div><p id='after'>after</p></body></html>"
            ),
            300.0,
            &mut compact_resources,
        );
        let point_for = |document: &LaidOutDocument, id: &str, resources: &GlyphResourceStore| {
            let box_idx = (0..document.render_view(resources).boxes().len())
                .find(|&idx| {
                    document.render_view(resources).boxes().attribute(idx, "id") == Some(id)
                })
                .expect("fixture box");
            document
                .render_view(resources)
                .boxes()
                .point(box_idx)
                .expect("box geometry")
        };

        for id in ["host", "a", "b", "after"] {
            assert_eq!(
                point_for(&indented, id, &indented_resources),
                point_for(&compact, id, &compact_resources),
                "collapsible source indentation must not change {id} geometry"
            );
        }
        assert_eq!(
            indented.line_count(),
            compact.line_count(),
            "a float-only anonymous formatting context must not publish a whitespace carrier line"
        );
    }

    #[test]
    fn relatively_positioned_inline_image_moves_without_consuming_flow_space() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><img id='shifted' src='a' style='position:relative;left:2px;width:20px;height:20px'/><img id='following' src='b' style='width:20px;height:20px'/></body></html>",
            200.0,
            &[(20, 20), (20, 20)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut document_resources,
        );
        let view = document.render_view(&document_resources);
        let box_for_id = |id| {
            (0..view.boxes().len())
                .find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some(id))
                .expect("image box")
        };
        let shifted = view
            .boxes()
            .point(box_for_id("shifted"))
            .expect("shifted image geometry");
        let following = view
            .boxes()
            .point(box_for_id("following"))
            .expect("following image geometry");

        assert_eq!(shifted.x, 2.0);
        assert_eq!(
            following.x, 20.0,
            "relative movement must not change the next inline image's flow position"
        );
    }

    #[test]
    fn relative_split_inline_moves_both_inline_fragments_and_block_descendant() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><p>Test</p><div style='position:relative;width:192px;height:192px;background:yellow'>\n<div style='display:inline;position:relative;top:192px;background:blue'>\nFiller Text\n<div style='width:192px;background:orange'>Filler Text</div>\nFiller Text\n</div>\n</div></body></html>",
            800.0,
            &mut document_resources,
        );
        let blue = document
            .render_view(&document_resources)
            .fragments()
            .decorations()
            .iter()
            .filter(|fragment| fragment.color() == 0x0000ffff)
            .map(|fragment| {
                (
                    fragment.rect(),
                    fragment.line_idx(),
                    fragment.is_in_positioned_layer(),
                )
            })
            .collect::<Vec<_>>();
        let orange = document
            .render_view(&document_resources)
            .fragments()
            .decorations()
            .iter()
            .filter(|fragment| fragment.color() == 0xffa500ff)
            .map(|fragment| fragment.rect())
            .collect::<Vec<_>>();

        assert_eq!(
            blue.len(),
            2,
            "both split inline fragments must retain their background: {blue:?}"
        );
        assert_eq!(orange.len(), 1);
        assert!(
            blue.iter()
                .all(|(rect, line_idx, _)| rect.y0 >= 192.0 && line_idx.is_some()),
            "relative positioning moves both line-owned inline fragments: {blue:?}"
        );
        assert!(
            blue.iter().all(|(_, line_idx, positioned)| {
                let line_idx = line_idx.expect("checked above");
                document
                    .render_view(&document_resources)
                    .text()
                    .line(line_idx)
                    .is_some_and(|line| line.is_in_positioned_layer() == *positioned)
            }),
            "split decorations and their owning lines must remain in the same paint layer: {blue:?}"
        );
        assert!(
            orange[0].y0 >= 192.0,
            "relative positioning moves the intervening block: {orange:?}"
        );
    }

    #[test]
    fn fragmentation_css_does_not_change_continuous_web_layout_without_a_viewport_height() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><div style='line-height:20px'>A</div><div style='line-height:20px;break-before:page;break-inside:avoid'>B</div></body></html>",
            200.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();

        assert_eq!(text.line(0).expect("A line").point().y, 0.0);
        assert_eq!(text.line(1).expect("B line").point().y, 20.0);
    }

    #[test]
    fn text_run_metrics_keep_varying_glyph_extents_and_tab_transitions() {
        use crate::{GlyphMetric, GlyphResourceGeneration, GlyphResourceStore};

        struct VaryingMetrics(GlyphResourceGeneration);
        impl GlyphShaper for VaryingMetrics {
            fn reset(&mut self) {
                self.0 = GlyphResourceGeneration::default();
            }
            fn resource_generation(&self) -> GlyphResourceGeneration {
                self.0
            }
            fn shape_glyph(
                &mut self,
                request: GlyphRequest<'_>,
            ) -> Result<ShapedGlyph, ShapeError> {
                let (ascent, descent) = if request.character == 'Y' {
                    (16.0, 4.0)
                } else {
                    (8.0, 2.0)
                };
                Ok(ShapedGlyph::from_metric(
                    request.character as u32,
                    GlyphMetric::try_new(request.character, 10.0, ascent, descent).unwrap(),
                ))
            }
        }
        let mut factory = DocumentFactory::new();
        let mut shaper = VaryingMetrics(GlyphResourceGeneration::default());
        let mut document_resources = GlyphResourceStore::default();
        let document = factory.parse_with_new_pipeline(
            "<html><body style='margin:0'><div style='font:10px Ahem;white-space:pre;tab-size:4;text-decoration:underline'>X\tYX\nX</div></body></html>", None,
        ).shape(&mut shaper, &mut document_resources).unwrap().layout(crate::LayoutConstraints::new(200.0, 10.0).unwrap(), &document_resources).expect("layout requires the shaper-aware API for first-line styles");
        let text = document.render_view(&document_resources).text();
        assert_eq!(line_text(&document, 0, &document_resources), "X\tYX");
        assert_eq!(line_text(&document, 1, &document_resources), "X");
        assert_eq!(text.line(0).unwrap().height(), 20.0);
        assert_eq!(text.line(1).unwrap().height(), 10.0);
        assert_eq!(tab_advance(&document, 0, &document_resources), 30.0);
    }

    #[test]
    fn inline_fragment_ancestor_geometry_tracks_each_line_baseline() {
        for (direction, content) in [
            ("ltr", "one two three four five six seven eight"),
            ("rtl", "אבג דהו זחט יכל מנס עפק"),
        ] {
            let split: String = content
                .chars()
                .map(|character| format!("<span>{character}</span>"))
                .collect();
            let source = |text: &str| {
                format!(
                    "<html><body style='margin:0'><p style='direction:{direction};font-size:16px'><span style='border-bottom:2px solid blue;position:relative;left:5px;top:3px'>{text}</span></p></body></html>"
                )
            };
            for source_text in [content, &split] {
                let mut document_resources = GlyphResourceStore::default();
                let document = layout_html(&source(source_text), 90.0, &mut document_resources);
                let view = document.render_view(&document_resources);
                let text = view.text();
                assert!(text.line_count() > 1);
                let borders: Vec<_> = view
                    .fragments()
                    .decorations()
                    .iter()
                    .filter(|fragment| fragment.is_inline() && fragment.color() == 0x0000ffff)
                    .collect();
                assert!(!borders.is_empty());
                for border in borders {
                    let line = text.line(border.line_idx().unwrap() as usize).unwrap();
                    // The owner's 16px font has 0.2em descent. Its relative
                    // top offset is 3px, independently of descendant owners.
                    let expected = line.point().y + line.baseline() + 3.0 + 16.0 * 0.2;
                    assert!(
                        (border.rect().y0 - expected).abs() < 0.001,
                        "{direction}: border {:?}, line {line:?}",
                        border.rect()
                    );
                }
            }
        }
    }

    #[test]
    fn ahem_normal_line_height_uses_its_zero_line_gap_metrics() {
        let mut ahem_resources = GlyphResourceStore::default();
        let ahem = layout_html(
            "<html><body style='margin:0'><div style='font:16px Ahem'>XX</div></body></html>",
            200.0,
            &mut ahem_resources,
        );
        let mut ordinary_resources = GlyphResourceStore::default();
        let ordinary = layout_html(
            "<html><body style='margin:0'><div style='font-size:16px'>XX</div></body></html>",
            200.0,
            &mut ordinary_resources,
        );

        assert!(
            (ahem
                .render_view(&ahem_resources)
                .text()
                .line(0)
                .expect("Ahem line")
                .height()
                - 16.0)
                .abs()
                < 0.001
        );
        assert!(
            (ordinary
                .render_view(&ordinary_resources)
                .text()
                .line(0)
                .expect("ordinary line")
                .height()
                - 19.2)
                .abs()
                < 0.001
        );
    }

    // Reduced from WPT css/css-sizing/
    // percentage-height-replaced-content-in-auto-cb.html. The cyclic
    // percentage computes as auto, so the definite width transfers through
    // the image's intrinsic aspect ratio.
    #[test]
    fn inline_replaced_percentage_height_in_auto_containing_block_uses_ratio() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><div><img src='x' width='60' height='60' style='width:100px;height:100%'></div></body></html>",
            300.0,
            &mut document_resources,
        );

        assert_eq!(
            tag_size(&document, "img", &document_resources),
            kurbo::Size::new(100.0, 100.0)
        );
    }

    #[test]
    fn inline_replaced_percentage_height_resolves_against_definite_containing_block() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><div style='height:120px'><img src='x' width='60' height='60' style='width:80px;height:50%'></div></body></html>",
            300.0,
            &mut document_resources,
        );

        assert_eq!(
            tag_size(&document, "img", &document_resources),
            kurbo::Size::new(80.0, 60.0)
        );
    }

    #[test]
    fn smart_image_policy_expands_a_standalone_inline_image() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:500px'><img src='graph.png'></div></body></html>",
            500.0,
            &[(300, 150)],
            crate::ImageSizingPolicy::SmartStandalone,
            &mut document_resources,
        );

        assert_eq!(
            tag_size(&document, "img", &document_resources),
            kurbo::Size::new(500.0, 250.0)
        );
    }

    #[test]
    fn smart_image_policy_fits_a_tall_standalone_image_to_the_viewport_height() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images_in_viewport(
            "<html><body style='margin:0'><div style='width:500px'><img src='portrait.png'></div></body></html>",
            500.0,
            Some(200.0),
            &[(300, 600)],
            crate::ImageSizingPolicy::SmartStandalone,
            &mut document_resources,
        );

        assert_eq!(
            tag_size(&document, "img", &document_resources),
            kurbo::Size::new(100.0, 200.0)
        );
    }

    #[test]
    fn smart_image_height_fit_accounts_for_vertical_margin_padding_and_border() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images_in_viewport(
            "<html><body style='margin:0'><div style='width:500px'><img src='portrait.png' style='margin:10px 0;padding:5px 0;border-top:2px solid;border-bottom:3px solid'></div></body></html>",
            500.0,
            Some(200.0),
            &[(300, 600)],
            crate::ImageSizingPolicy::SmartStandalone,
            &mut document_resources,
        );

        assert_eq!(
            tag_size(&document, "img", &document_resources),
            kurbo::Size::new(82.5, 180.0)
        );
    }

    #[test]
    fn smart_image_height_fit_preserves_an_explicit_height() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images_in_viewport(
            "<html><body style='margin:0'><div style='width:500px'><img src='portrait.png' style='width:150px;height:300px'></div></body></html>",
            500.0,
            Some(200.0),
            &[(300, 600)],
            crate::ImageSizingPolicy::SmartStandalone,
            &mut document_resources,
        );

        assert_eq!(
            tag_size(&document, "img", &document_resources),
            kurbo::Size::new(150.0, 300.0)
        );
    }

    #[test]
    fn smart_image_policy_fills_the_column_beyond_twice_the_source_size() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:500px'><img src='graph.png'></div></body></html>",
            500.0,
            &[(100, 50)],
            crate::ImageSizingPolicy::SmartStandalone,
            &mut document_resources,
        );

        assert_eq!(
            tag_size(&document, "img", &document_resources),
            kurbo::Size::new(500.0, 250.0)
        );
    }

    #[test]
    fn smart_image_policy_ignores_whitespace_and_transparent_link_wrappers() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:500px'>\n<a href='#'><img src='graph.png'></a>\n</div></body></html>",
            500.0,
            &[(300, 150)],
            crate::ImageSizingPolicy::SmartStandalone,
            &mut document_resources,
        );

        assert_eq!(
            tag_size(&document, "img", &document_resources),
            kurbo::Size::new(500.0, 250.0)
        );
    }

    #[test]
    fn smart_image_policy_never_expands_an_image_embedded_in_text() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:500px'>Graph: <img src='graph.png'> caption</div></body></html>",
            500.0,
            &[(200, 100)],
            crate::ImageSizingPolicy::SmartStandalone,
            &mut document_resources,
        );

        assert_eq!(
            tag_size(&document, "img", &document_resources),
            kurbo::Size::new(200.0, 100.0)
        );
    }

    #[test]
    fn smart_image_policy_rejects_multiple_images_but_overrides_authored_width() {
        let mut multiple_resources = GlyphResourceStore::default();
        let multiple = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:500px'><img src='a.png'><img src='b.png'></div></body></html>",
            500.0,
            &[(200, 100), (200, 100)],
            crate::ImageSizingPolicy::SmartStandalone,
            &mut multiple_resources,
        );
        let mut authored_resources = GlyphResourceStore::default();
        let authored = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:500px'><img src='graph.png' style='width:250px'></div></body></html>",
            500.0,
            &[(200, 100)],
            crate::ImageSizingPolicy::SmartStandalone,
            &mut authored_resources,
        );

        assert_eq!(
            tag_size(&multiple, "img", &multiple_resources),
            kurbo::Size::new(200.0, 100.0)
        );
        assert_eq!(
            tag_size(&authored, "img", &authored_resources),
            kurbo::Size::new(500.0, 250.0)
        );
    }

    #[test]
    fn smart_image_policy_overrides_horizontal_constraints_and_margins() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:300px;text-align:center'><img src='graph.png' style='width:70%;height:auto;min-width:40px;max-width:180px;margin:12px 45px 18px 35px'><p style='margin:0'>Figure caption</p></div></body></html>",
            300.0,
            &[(1586, 1023)],
            crate::ImageSizingPolicy::SmartStandalone,
            &mut document_resources,
        );

        assert_eq!(
            tag_size(&document, "img", &document_resources),
            kurbo::Size::new(300.0, 300.0 * 1023.0 / 1586.0)
        );
        assert_eq!(
            tag_point(&document, "img", &document_resources).x,
            0.0,
            "used left margin should be zero"
        );
    }

    #[test]
    fn smart_image_policy_expands_a_standalone_block_image_but_not_an_icon() {
        let mut block_resources = GlyphResourceStore::default();
        let block = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:500px'><img src='graph.png' style='display:block;width:70%;max-width:180px;margin:10px 30px 20px 40px'></div></body></html>",
            500.0,
            &[(300, 150)],
            crate::ImageSizingPolicy::SmartStandalone,
            &mut block_resources,
        );
        let mut icon_resources = GlyphResourceStore::default();
        let icon = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:500px'><img src='icon.png'></div></body></html>",
            500.0,
            &[(64, 64)],
            crate::ImageSizingPolicy::SmartStandalone,
            &mut icon_resources,
        );

        assert_eq!(
            tag_size(&block, "img", &block_resources),
            kurbo::Size::new(500.0, 250.0)
        );
        assert_eq!(
            tag_point(&block, "img", &block_resources).x,
            0.0,
            "used block-image left margin should be zero"
        );
        assert_eq!(
            tag_size(&icon, "img", &icon_resources),
            kurbo::Size::new(64.0, 64.0)
        );
    }

    #[test]
    fn block_image_content_starts_at_its_own_content_edge() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><img src='pixel.png' style='display:block;width:15px;height:15px'></body></html>",
            200.0,
            &[(15, 15)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut document_resources,
        );
        let image_box = tag_point(&document, "img", &document_resources);
        let fragment = document
            .render_view(&document_resources)
            .fragments()
            .images()
            .iter()
            .next()
            .expect("image fragment");
        let line = document
            .render_view(&document_resources)
            .text()
            .line(fragment.line_idx())
            .expect("image carrier line");

        let painted = line.point() + fragment.offset().to_vec2();
        assert!(
            (painted.x - image_box.x).abs() < 0.001 && (painted.y - image_box.y).abs() < 0.001,
            "painted image {painted:?} must start at its box {image_box:?}"
        );
        assert_eq!(fragment.size(), kurbo::Size::new(15.0, 15.0));
    }

    #[test]
    fn inline_image_paints_its_own_background_and_border_box() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div><img src='pixel.png' style='width:10px;height:10px;padding:3px 5px;background:#123456;border:2px solid #abcdef'></div></body></html>",
            200.0,
            &[(10, 10)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut document_resources,
        );
        let image_point = tag_point(&document, "img", &document_resources);
        let image_size = tag_size(&document, "img", &document_resources);
        let decorations = document
            .render_view(&document_resources)
            .fragments()
            .decorations();
        let background = decorations
            .iter()
            .find(|fragment| fragment.color() == 0x123456FF)
            .expect("inline image background");
        let borders = decorations
            .iter()
            .filter(|fragment| fragment.color() == 0xABCDEFFF)
            .collect::<Vec<_>>();

        assert_eq!(
            background.rect(),
            kurbo::Rect::from_origin_size(image_point, image_size)
        );
        assert!(background.is_inline());
        assert_eq!(borders.len(), 4);
        assert!(borders.iter().all(|fragment| fragment.is_inline()));
        assert!(borders.iter().all(|fragment| {
            let rect = fragment.rect();
            rect.x0 >= image_point.x
                && rect.y0 >= image_point.y
                && rect.x1 <= image_point.x + image_size.width
                && rect.y1 <= image_point.y + image_size.height
        }));
    }

    #[test]
    fn adjacent_full_width_inline_images_wrap_at_the_atomic_boundary() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:100px'><img src='a.png' style='width:100%;height:20px;vertical-align:top'><img src='b.png' style='width:100%;height:20px;vertical-align:top'></div></body></html>",
            200.0,
            &[(1, 1), (1, 1)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut document_resources,
        );
        let fragments = document
            .render_view(&document_resources)
            .fragments()
            .images()
            .iter()
            .collect::<Vec<_>>();
        assert_eq!(fragments.len(), 2);
        assert_ne!(
            fragments[0].line_idx(),
            fragments[1].line_idx(),
            "adjacent atomic inline boxes have a soft wrap opportunity between them"
        );

        let text = document.render_view(&document_resources).text();
        let painted = fragments
            .iter()
            .map(|fragment| {
                text.line(fragment.line_idx()).expect("image line").point()
                    + fragment.offset().to_vec2()
            })
            .collect::<Vec<_>>();
        assert!((painted[0].x - painted[1].x).abs() < 0.001);
        assert!(painted[1].y >= painted[0].y + 20.0);
    }

    #[test]
    fn top_aligned_image_does_not_add_the_text_strut_descent() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div><img src='a.png' width='10' height='96' style='vertical-align:top'> <img src='b.png' width='10' height='96' style='float:right'></div></body></html>",
            800.0,
            &[(10, 96), (10, 96)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let inline_image = document
            .render_view(&document_resources)
            .fragments()
            .images()
            .iter()
            .find(|fragment| {
                fragment.size().height == 96.0
                    && text
                        .line(fragment.line_idx())
                        .is_some_and(|line| line.point().x < 100.0)
            })
            .expect("in-flow image fragment");
        let line = text.line(inline_image.line_idx()).expect("image line");

        assert!(
            (line.height() - 96.0).abs() < 0.001,
            "top-aligned image should determine the line height without a phantom descender: {line:?}"
        );
    }

    #[test]
    fn right_floated_replaced_inline_shrink_wraps_at_the_containing_edge() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:200px'><img src='a.png' width='10' height='20'><img src='b.png' width='10' height='20' style='float:right'></div></body></html>",
            200.0,
            &[(10, 20), (10, 20)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut document_resources,
        );
        let fragments = document
            .render_view(&document_resources)
            .fragments()
            .images()
            .iter()
            .collect::<Vec<_>>();
        let text = document.render_view(&document_resources).text();
        let mut painted = fragments
            .iter()
            .map(|fragment| {
                text.line(fragment.line_idx()).expect("image line").point()
                    + fragment.offset().to_vec2()
            })
            .collect::<Vec<_>>();
        painted.sort_by(|left, right| left.x.total_cmp(&right.x));

        assert_eq!(painted.len(), 2);
        assert!(
            (painted[0].x - 0.0).abs() < 0.001,
            "in-flow image starts at the inline edge: {painted:?}"
        );
        assert!(
            (painted[1].x - 190.0).abs() < 0.001,
            "right float shrink-wraps to 10px: {painted:?}"
        );
        assert!(
            (painted[0].y - painted[1].y).abs() < 0.001,
            "both images share the first row: {painted:?}"
        );
    }

    #[test]
    fn oversized_atomic_inline_wraps_before_it_when_adjacent_to_text() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><div style='width:100px'>x<span id='atomic' style='display:inline-block;width:200px'>y</span>z</div></body></html>",
            300.0,
            &mut document_resources,
        );
        let atomic_point = tag_point(&document, "span", &document_resources);
        let outer_lines = document
            .render_view(&document_resources)
            .text()
            .lines()
            .iter()
            .filter(|line| {
                matches!(
                    line_text(&document, line.index(), &document_resources).as_str(),
                    "x" | "z"
                )
            })
            .map(|line| line.point())
            .collect::<Vec<_>>();

        assert_eq!(outer_lines.len(), 2);
        assert!(
            atomic_point.y > outer_lines[0].y,
            "the oversized atomic box must wrap after the leading text"
        );
        assert!(
            outer_lines[1].y > atomic_point.y,
            "following text must wrap after the oversized atomic box"
        );
    }

    #[test]
    fn baseline_inline_image_starts_at_line_top_and_reserves_font_descent() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div><img src='pixel.png' style='width:16px;height:16px'></div></body></html>",
            200.0,
            &[(1, 1)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut document_resources,
        );
        let fragment = document
            .render_view(&document_resources)
            .fragments()
            .images()
            .iter()
            .next()
            .expect("image fragment");
        let line = document
            .render_view(&document_resources)
            .text()
            .line(fragment.line_idx())
            .expect("image line");
        let painted = line.point() + fragment.offset().to_vec2();

        assert!(
            (painted.y - line.point().y).abs() < 0.001,
            "baseline image should occupy the top of its line, not receive symmetric half-leading"
        );
        assert!(
            line.height() > fragment.size().height,
            "the parent font strut should retain descender space below the image"
        );
    }

    #[test]
    fn image_fragment_separates_object_paint_geometry_from_the_css_box() {
        let mut contain_resources = GlyphResourceStore::default();
        let contain = layout_html_with_images(
            "<html><body style='margin:0'><img src='wide.png' style='width:100px;height:100px;object-fit:contain;object-position:center'></body></html>",
            200.0,
            &[(200, 100)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut contain_resources,
        );
        let contain = contain
            .render_view(&contain_resources)
            .fragments()
            .images()
            .iter()
            .next()
            .expect("contained image fragment");
        assert_eq!(contain.size(), kurbo::Size::new(100.0, 50.0));
        assert_eq!(contain.offset().x, contain.clip().x0);
        assert_eq!(contain.offset().y, contain.clip().y0 + 25.0);
        assert_eq!(contain.clip().size(), kurbo::Size::new(100.0, 100.0));

        let mut cover_resources = GlyphResourceStore::default();
        let cover = layout_html_with_images(
            "<html><body style='margin:0'><img src='wide.png' style='width:100px;height:100px;object-fit:cover;object-position:center'></body></html>",
            200.0,
            &[(200, 100)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut cover_resources,
        );
        let cover = cover
            .render_view(&cover_resources)
            .fragments()
            .images()
            .iter()
            .next()
            .expect("covered image fragment");
        assert_eq!(cover.size(), kurbo::Size::new(200.0, 100.0));
        assert_eq!(cover.offset().x, cover.clip().x0 - 50.0);
        assert_eq!(cover.offset().y, cover.clip().y0);
        assert_eq!(cover.clip().size(), kurbo::Size::new(100.0, 100.0));
    }

    #[test]
    fn inline_image_line_height_does_not_resize_or_shift_the_replaced_box() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div><img src='square.png' style='width:96px;height:96px;line-height:192px'></div></body></html>",
            300.0,
            &[(96, 96)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut document_resources,
        );
        let fragment = document
            .render_view(&document_resources)
            .fragments()
            .images()
            .iter()
            .next()
            .expect("image fragment");
        let line = document
            .render_view(&document_resources)
            .text()
            .line(fragment.line_idx())
            .expect("image line");
        let painted = line.point() + fragment.offset().to_vec2();

        assert_eq!(fragment.size().height, 96.0);
        assert!(
            (painted.y - line.point().y).abs() < 0.001,
            "the image's own line-height must not add leading around its box"
        );
        assert!(
            line.height() < 192.0,
            "the image's own line-height must not become the line's minimum height"
        );
    }

    #[test]
    fn zero_line_height_font_strut_retains_baseline_ascent_next_to_an_image() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div style='font:160px/0 Ahem'><img src='pixel.png' style='width:160px;height:1px'></div></body></html>",
            300.0,
            &[(1, 1)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut document_resources,
        );
        let fragment = document
            .render_view(&document_resources)
            .fragments()
            .images()
            .iter()
            .next()
            .expect("image fragment");
        let line = document
            .render_view(&document_resources)
            .text()
            .line(fragment.line_idx())
            .expect("image line");

        assert!(
            (line.height() - 48.0).abs() < 0.001,
            "the Ahem strut contributes 0.3em above the baseline despite line-height zero; got {}",
            line.height()
        );
    }

    #[test]
    fn first_letter_shapes_adjacent_css_punctuation_with_the_pseudo_style() {
        let mut factory = DocumentFactory::new();
        let mut glyphs = GlyphCache::new();
        let mut document_resources = GlyphResourceStore::default();
        let document = factory
            .parse_with_new_pipeline(
                "<html><body style='margin:0'><div>)T)est</div></body></html>",
                Some("div::first-letter { font-size:36px; line-height:72px; color:green }"),
            )
            .shape(&mut glyphs, &mut document_resources)
            .expect("first-letter glyphs shape")
            .layout(
                crate::LayoutConstraints::new(500.0, 16.0).unwrap(),
                &document_resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles");

        let text = document.render_view(&document_resources).text();
        let advances = (0..text.glyph_count())
            .map(|index| {
                text.glyph_at(index)
                    .and_then(|glyph| text.glyph_metric(glyph))
                    .expect("shaped glyph")
                    .advance()
            })
            .collect::<Vec<_>>();
        assert_eq!(advances, vec![18.0, 18.0, 18.0, 8.0, 8.0, 8.0]);
        assert_eq!(text.line(0).expect("first line").height(), 72.0);
    }

    #[test]
    fn ancestor_first_letter_reaches_the_first_in_flow_block_descendant() {
        let mut factory = DocumentFactory::new();
        let mut glyphs = GlyphCache::new();
        let mut document_resources = GlyphResourceStore::default();
        let document = factory
            .parse_with_new_pipeline("<!doctype html><html><body style='margin:0'><div id='outer'><div>Ab</div></div></body></html>", Some("#outer { font-size:16px } #outer::first-letter { font-size:48px; color:blue }"))
            .shape(&mut glyphs, &mut document_resources)
            .expect("descendant first-letter glyphs shape")
            .layout(crate::LayoutConstraints::new(500.0, 16.0).unwrap(), &document_resources).expect("layout requires the shaper-aware API for first-line styles");

        let text = document.render_view(&document_resources).text();
        let advances = (0..text.glyph_count())
            .map(|index| {
                text.glyph_at(index)
                    .and_then(|glyph| text.glyph_metric(glyph))
                    .expect("shaped glyph")
                    .advance()
            })
            .collect::<Vec<_>>();
        assert_eq!(advances, vec![24.0, 8.0]);
    }

    #[test]
    fn ordinary_book_composition_does_not_force_sentence_breaks() {
        let html = "<html><body style='margin:0'><p style='margin:0'>First sentence. Second sentence!</p></body></html>";
        let count = "First sentence. Second sentence!".chars().count();
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_with_native_geometry_policy(
            html,
            1_000.0,
            vec![8.0; count],
            vec![true; count + 1],
            crate::TextCompositionPolicy::BookOptimized,
            &mut document_resources,
        );

        assert_eq!(document.line_count(), 1);
        assert_eq!(
            line_text(&document, 0, &document_resources),
            "First sentence. Second sentence!"
        );
    }

    #[test]
    fn web_compatible_policy_does_not_expand_standalone_images() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><div style='width:500px'><img src='graph.png' style='width:70%;margin-left:20px'></div></body></html>",
            500.0,
            &[(200, 100)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut document_resources,
        );

        let size = tag_size(&document, "img", &document_resources);
        assert!((size.width - 350.0).abs() < 0.001);
        assert!((size.height - 175.0).abs() < 0.001);
        assert_eq!(tag_point(&document, "img", &document_resources).x, 20.0);
    }

    #[test]
    fn preserved_tab_advances_to_the_next_number_based_stop() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><pre style='tab-size: 4'>a\tb</pre></body></html>",
            300.0,
            &mut document_resources,
        );
        assert_eq!(line_text(&document, 0, &document_resources), "a\tb");
        // Test shaper: every 16px glyph/space advances 8px. After `a`, a
        // four-space tab interval reaches the 32px stop, hence 24px.
        assert!((tab_advance(&document, 0, &document_resources) - 24.0).abs() < 0.001);
    }

    #[test]
    fn preserved_tabs_share_metrics_with_adjacent_text() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><pre style='tab-size:4'>a\tb\tc</pre></body></html>",
            300.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let advances: Vec<_> = text
            .line_glyph_advances(0)
            .unwrap()
            .iter()
            .map(|run| (run.range(), run.advance()))
            .collect();
        assert_eq!(advances, [(1..2, 24.0), (3..4, 24.0)]);
        let metrics_entries: usize = document
            .memory_usage_report()
            .entries
            .iter()
            .filter(|entry| entry.label.ends_with("inline_plans.run_metrics"))
            .map(|entry| entry.count)
            .sum();
        assert_eq!(
            metrics_entries, 1,
            "tabs and letters should share the same ordinary metrics"
        );
    }

    #[test]
    fn preserved_tab_wrap_opportunities_do_not_advance_twice() {
        for white_space in ["pre-wrap", "break-spaces"] {
            let html = format!(
                "<html><body style='margin:0'><div style='white-space:{white_space};tab-size:4;width:90px'>a\tb\tc</div></body></html>"
            );
            let mut document_resources = GlyphResourceStore::default();
            let document = layout_html(&html, 300.0, &mut document_resources);
            let text = document.render_view(&document_resources).text();
            assert_eq!(
                text.line_count(),
                1,
                "zero-width opportunities must not expand tabs for {white_space}"
            );
            assert_eq!(line_text(&document, 0, &document_resources), "a\tb\tc");
            let advances: Vec<_> = text
                .line_glyph_advances(0)
                .unwrap()
                .iter()
                .map(|run| (run.range(), run.advance()))
                .collect();
            assert_eq!(advances, [(1..2, 24.0), (3..4, 24.0)]);
        }
    }

    #[test]
    fn preserved_tabs_resolve_each_inline_owners_style() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><pre><span style='tab-size:20px'>a\tb</span><span style='tab-size:40px'>c\td</span></pre></body></html>",
            300.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let advances: Vec<_> = text
            .line_glyph_advances(0)
            .unwrap()
            .iter()
            .map(|run| (run.range(), run.advance()))
            .collect();
        assert_eq!(advances, [(1..2, 12.0), (4..5, 4.0)]);
    }

    #[test]
    fn preserved_tab_skips_a_stop_closer_than_the_minimum_advance() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><pre style='tab-size:18px'>aa\tb</pre></body></html>",
            300.0,
            &mut document_resources,
        );
        // At x=16 the next stop is only 2px away, below the 4px minimum.
        assert_eq!(tab_advance(&document, 0, &document_resources), 20.0);
    }

    #[test]
    fn number_based_tab_uses_the_nearest_block_space_metric() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><pre style='tab-size: 4'>a<span style='font-size: 32px'>\t</span>b</pre></body></html>",
            300.0,
            &mut document_resources,
        );
        assert!(
            (tab_advance(&document, 0, &document_resources) - 24.0).abs() < 0.001,
            "the inline span's larger font must not redefine the block's tab stops"
        );
    }

    #[test]
    fn tab_stops_are_anchored_at_the_block_content_edge_before_indent() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><pre style='tab-size: 4; text-indent: 8px'>a\tb</pre></body></html>",
            300.0,
            &mut document_resources,
        );
        assert!((tab_advance(&document, 0, &document_resources) - 16.0).abs() < 0.001);
    }

    #[test]
    fn length_and_zero_tab_sizes_produce_exact_sparse_advances() {
        let mut length_resources = GlyphResourceStore::default();
        let length = layout_html(
            "<html><body><pre style='tab-size: 20px'>a\tb</pre></body></html>",
            300.0,
            &mut length_resources,
        );
        assert!((tab_advance(&length, 0, &length_resources) - 12.0).abs() < 0.001);

        let mut zero_resources = GlyphResourceStore::default();
        let zero = layout_html(
            "<html><body><pre style='tab-size: 0'>a\tb</pre></body></html>",
            300.0,
            &mut zero_resources,
        );
        assert_eq!(tab_advance(&zero, 0, &zero_resources), 0.0);
    }

    #[test]
    fn collapsed_tabs_stay_on_the_ordinary_space_fast_path() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><p style='tab-size: 20px'>a\tb</p></body></html>",
            300.0,
            &mut document_resources,
        );
        assert_eq!(line_text(&document, 0, &document_resources), "a b");
        assert!(
            document
                .render_view(&document_resources)
                .text()
                .line_glyph_advances(0)
                .expect("line advances")
                .is_empty()
        );
    }

    #[test]
    #[ignore]
    fn inspect_axel_chapter004_float_pipeline() {
        let html_path = std::env::var("CHAPTER_HTML_PATH")
            .expect("set CHAPTER_HTML_PATH to the chapter fixture");
        let css_path =
            std::env::var("CHAPTER_CSS_PATH").expect("set CHAPTER_CSS_PATH to its stylesheet");

        fn find_dom_node_with_class(
            document: &html_dom::Document,
            node: html_dom::DomNodeId,
            class: &str,
        ) -> Option<html_dom::DomNodeId> {
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
        println!(
            "step 2 dom: root={:?}",
            document.dom_root().map(|n| n.raw())
        );

        let author_css = fs::read_to_string(&css_path).expect("chapter css");
        let parsed_float_prop = lightningcss::properties::Property::parse_string(
            lightningcss::properties::PropertyId::from("float"),
            "left",
            ParserOptions::default(),
        )
        .expect("parse float property");
        let parsed_clear_prop = lightningcss::properties::Property::parse_string(
            lightningcss::properties::PropertyId::from("clear"),
            "both",
            ParserOptions::default(),
        )
        .expect("parse clear property");
        println!("step 2b parsed property float={parsed_float_prop:?} clear={parsed_clear_prop:?}");
        let styled = html_style::style_document(document, &[&author_css]);
        let (document, styles) = styled.into_parts();
        let prepared = crate::PreparedDocument::try_new(document, styles)
            .expect("style resolver must produce complete styles for its document");
        let document = prepared.document();

        let dom_root = document.dom_root().expect("dom root");
        let first_letter_dom = find_dom_node_with_class(document, dom_root, "first-letter")
            .expect("first-letter DOM node");
        let first_letter_style_idx = prepared
            .test_styles()
            .style_for_node(first_letter_dom)
            .expect("first-letter style");
        let first_letter_style = prepared.style_view(first_letter_style_idx);
        let first_letter_used_style = prepared
            .test_styles()
            .used_view(first_letter_style_idx, 0.5, 0.5, 0.8)
            .expect("valid fallback metrics");
        println!(
            "step 3 style: float={:?} display={:?} font_size={} line_height={} margin_right={} margin_bottom={}",
            first_letter_style.float(),
            first_letter_style.display(),
            first_letter_style.font_size(),
            first_letter_style.line_height(),
            first_letter_used_style
                .margin_right()
                .resolve(first_letter_style.font_size() as f64),
            first_letter_used_style
                .margin_bottom()
                .resolve(first_letter_style.font_size() as f64),
        );
        assert!(matches!(
            first_letter_style.float(),
            html_style_model::Float::Left | html_style_model::Float::Right
        ));

        let float_box_idx = (0..prepared.layout_tree().box_count())
            .find(|&i| {
                prepared
                    .layout_tree()
                    .get_box_dom_element(i)
                    .and_then(|node| document.node_id_from_raw(node))
                    .is_some_and(|node| document.dom_has_class(node, "first-letter"))
            })
            .expect("float box");
        let float_box = prepared
            .layout_tree()
            .box_at(float_box_idx)
            .expect("float box exists");
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
        assert!(matches!(
            float_box.layout_mode(),
            crate::layout_model::LayoutMode::Block(_)
        ));

        let mut glyph_cache = GlyphCache::new();
        let mut shaped_resources = GlyphResourceStore::default();
        let shaped = prepared
            .shape(&mut glyph_cache, &mut shaped_resources)
            .expect("test shaper must register every glyph");
        println!("step 5 shape: glyphs={}", glyph_cache.len());

        let laid_out = shaped
            .layout(
                crate::LayoutConstraints::new(220.0, 16.0).unwrap(),
                &shaped_resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles");
        println!("step 6 layout: lines={}", laid_out.line_count());

        let float_point = laid_out
            .render_view(&shaped_resources)
            .boxes()
            .point(float_box_idx)
            .expect("box index in range");
        let float_size = laid_out
            .render_view(&shaped_resources)
            .boxes()
            .size(float_box_idx)
            .expect("box index in range");
        for (idx, line) in laid_out
            .render_view(&shaped_resources)
            .text()
            .lines()
            .iter()
            .enumerate()
            .take(20)
        {
            let text_view = laid_out.render_view(&shaped_resources).text();
            let text: String = text_view
                .glyph_slice(line.glyphs())
                .expect("line glyph range")
                .iter()
                .map(|&glyph| {
                    text_view
                        .glyph_metric(glyph)
                        .expect("registered glyph")
                        .ch()
                })
                .collect();
            println!(
                "line {idx}: point=({:.2},{:.2}) baseline={:.2} height={:.2} text={:?}",
                line.point().x,
                line.point().y,
                line.baseline(),
                line.height(),
                text
            );
        }
        let float_line = laid_out
            .render_view(&shaped_resources)
            .text()
            .lines()
            .iter()
            .enumerate()
            .find(|(_, line)| {
                let text_view = laid_out.render_view(&shaped_resources).text();
                let text: String = text_view
                    .glyph_slice(line.glyphs())
                    .expect("line glyph range")
                    .iter()
                    .map(|&glyph| {
                        text_view
                            .glyph_metric(glyph)
                            .expect("registered glyph")
                            .ch()
                    })
                    .collect();
                text.starts_with('E') || text.starts_with('R')
            })
            .expect("line for float");
        println!(
            "float box: point=({:.2},{:.2}) size=({:.2},{:.2})",
            float_point.x, float_point.y, float_size.width, float_size.height
        );
        println!(
            "matched line {}: point=({:.2},{:.2}) baseline={:.2} height={:.2}",
            float_line.0,
            float_line.1.point().x,
            float_line.1.point().y,
            float_line.1.baseline(),
            float_line.1.height()
        );
        assert!(
            (float_point.y - float_line.1.point().y).abs() < 3.0,
            "float top should be near the first line top"
        );
    }

    #[test]
    fn trims_collapsible_trailing_space_on_final_line() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><p>a </p></body></html>",
            200.0,
            &mut document_resources,
        );

        assert_eq!(document.line_count(), 1);
        assert_eq!(line_text(&document, 0, &document_resources), "a");
    }

    #[test]
    fn trims_collapsible_leading_space_on_first_greedy_line() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><div style='margin:0'> \n <span>word</span></div></body></html>",
            200.0,
            &mut document_resources,
        );

        assert_eq!(document.line_count(), 1);
        assert_eq!(line_text(&document, 0, &document_resources), "word");
    }

    #[test]
    fn trims_collapsible_leading_space_from_knuth_plass_segment() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><div style='margin:0;text-align:justify'> \n <span>one two three</span></div></body></html>",
            200.0,
            &mut document_resources,
        );

        assert_eq!(document.line_count(), 1);
        assert_eq!(
            line_text(&document, 0, &document_resources),
            "one two three"
        );
    }

    #[test]
    fn preserves_leading_space_when_white_space_requires_it() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><div style='margin:0;white-space:pre'>  word</div></body></html>",
            200.0,
            &mut document_resources,
        );

        assert_eq!(document.line_count(), 1);
        assert_eq!(line_text(&document, 0, &document_resources), "  word");
    }

    #[test]
    fn preserved_spaces_after_a_newline_establish_a_trailing_line() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><table><tr><td><div id='target' style='margin:0;white-space:pre;font-size:20px;line-height:20px'>\nXXXX\nXXXX\nXXXX\nXXXX\nXXXX\n   </div></td></tr></table></body></html>",
            500.0,
            &mut document_resources,
        );

        let lines = (0..document.line_count())
            .map(|index| line_text(&document, index, &document_resources))
            .collect::<Vec<_>>();
        assert_eq!(document.line_count(), 7, "published lines: {lines:?}");
        assert_eq!(line_text(&document, 0, &document_resources), "");
        assert_eq!(line_text(&document, 6, &document_resources), "   ");
        let view = document.render_view(&document_resources);
        let target = (0..view.boxes().len())
            .find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some("target"))
            .expect("target box");
        assert_eq!(
            view.boxes().size(target).expect("target geometry").height,
            140.0
        );
    }

    #[test]
    fn intrinsic_grid_width_keeps_an_exact_fit_word_pair_on_one_line() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='width:min-content'>
                    <div id='grid' style='display:grid;grid-template-columns:1fr;grid-template-rows:1fr;font-size:20px;line-height:20px'>
                        <div id='item'>XXX XXXX XX X XX XXX</div>
                    </div>
                </div>
            </body></html>",
            200.0,
         &mut document_resources);

        let view = document.render_view(&document_resources);
        let grid = (0..view.boxes().len())
            .find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some("grid"))
            .expect("grid box");
        let item = (0..view.boxes().len())
            .find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some("item"))
            .expect("item box");
        let lines = (0..document.line_count())
            .map(|idx| line_text(&document, idx, &document_resources))
            .collect::<Vec<_>>();
        assert_eq!(
            document.line_count(),
            5,
            "the exact-width `X XX` pair should share a line; grid={:?}, item={:?}, lines={lines:?}",
            view.boxes().size(grid),
            view.boxes().size(item)
        );
    }

    #[test]
    fn long_word_overflows_without_overflow_wrap() {
        let html = format!("<html><body><p>{}</p></body></html>", "a".repeat(40));
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(&html, 100.0, &mut document_resources);

        assert_eq!(
            document.line_count(),
            1,
            "an unbreakable word should overflow on one line by default"
        );
    }

    #[test]
    fn overflow_wrap_breaks_long_word_without_losing_glyphs() {
        let html = format!(
            "<html><body><p style=\"overflow-wrap: break-word\">{}</p></body></html>",
            "a".repeat(40)
        );
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(&html, 100.0, &mut document_resources);

        assert!(
            document.line_count() >= 2,
            "overflow-wrap should break the long word across lines"
        );
        let combined: String = (0..document.line_count())
            .map(|i| line_text(&document, i, &document_resources))
            .collect();
        assert_eq!(
            combined,
            "a".repeat(40),
            "mid-word breaks must not drop glyphs"
        );
    }

    #[test]
    fn overflow_wrap_prefers_space_break_over_mid_word_break() {
        // "aa bb": both words fit on a line, so overflow-wrap must wrap at the
        // space like normal wrapping, not mid-word.
        let html = "<html><body style=\"margin:0\"><p style=\"margin:0; overflow-wrap:break-word\">aa bb</p></body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(html, 39.0, &mut document_resources);

        assert_eq!(document.line_count(), 2);
        assert_eq!(line_text(&document, 0, &document_resources), "aa");
        assert_eq!(line_text(&document, 1, &document_resources), "bb");
    }

    #[test]
    fn word_break_break_all_breaks_anywhere() {
        let html = format!(
            "<html><body><p style=\"word-break: break-all\">{}</p></body></html>",
            "a".repeat(40)
        );
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(&html, 100.0, &mut document_resources);

        assert!(
            document.line_count() >= 2,
            "word-break: break-all should wrap the long word"
        );
        let combined: String = (0..document.line_count())
            .map(|i| line_text(&document, i, &document_resources))
            .collect();
        assert_eq!(combined, "a".repeat(40));
    }

    #[test]
    fn nowrap_suppresses_overflow_wrap() {
        let html = format!(
            "<html><body><p style=\"overflow-wrap: break-word; white-space: nowrap\">{}</p></body></html>",
            "a".repeat(40)
        );
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(&html, 100.0, &mut document_resources);

        assert_eq!(
            document.line_count(),
            1,
            "white-space: nowrap forbids wrapping even with overflow-wrap"
        );
    }

    #[test]
    fn text_overflow_ellipsis_truncates_source_glyphs_and_emits_synthetic_marker() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><p style='width:24px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis'>abcdef</p></body></html>",
            200.0,
            &mut document_resources,
        );
        let line = document
            .render_view(&document_resources)
            .text()
            .line(0)
            .expect("overflowing paragraph should emit one line");
        let marker = document
            .render_view(&document_resources)
            .text()
            .ellipsis_for_line(0)
            .expect("ellipsis marker should be emitted");

        assert_eq!(line_text(&document, 0, &document_resources), "ab");
        assert_eq!(
            document
                .render_view(&document_resources)
                .text()
                .glyph_metric(marker.glyph())
                .expect("registered marker metric")
                .ch(),
            '\u{2026}'
        );
        assert!((marker.offset().x - 16.0).abs() < 0.001);
        assert_eq!(
            line.glyphs(),
            0..2,
            "synthetic marker must not enter the source glyph range"
        );
    }

    #[test]
    fn text_overflow_clip_preserves_layout_content_for_paint_clipping() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><p style='width:24px;white-space:nowrap;overflow:hidden;text-overflow:clip'>abcdef</p></body></html>",
            200.0,
            &mut document_resources,
        );

        assert_eq!(line_text(&document, 0, &document_resources), "abcdef");
        assert!(
            document
                .render_view(&document_resources)
                .text()
                .ellipsis_for_line(0)
                .is_none()
        );
        assert!(
            document
                .render_view(&document_resources)
                .text()
                .line_overflow_clip(0)
                .is_some()
        );
    }

    #[test]
    fn text_overflow_does_not_apply_when_inline_overflow_is_visible() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><p style='width:24px;white-space:nowrap;overflow:visible;text-overflow:ellipsis'>abcdef</p></body></html>",
            200.0,
            &mut document_resources,
        );

        assert_eq!(line_text(&document, 0, &document_resources), "abcdef");
        assert!(
            document
                .render_view(&document_resources)
                .text()
                .ellipsis_for_line(0)
                .is_none()
        );
    }

    #[test]
    fn ellipsis_is_shaped_once_and_survives_width_only_relayout() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline("<html><body style='margin:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis'>abcdef</body></html>", None);
        let mut glyph_cache = GlyphCache::new();
        let mut shaped_resources = GlyphResourceStore::default();
        let shaped = prepared
            .shape(&mut glyph_cache, &mut shaped_resources)
            .expect("ellipsis and source glyphs should shape");
        let mut document = shaped
            .layout(
                crate::LayoutConstraints::new(200.0, 16.0).unwrap(),
                &shaped_resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles");

        assert!(
            document
                .render_view(&shaped_resources)
                .text()
                .ellipsis_for_line(0)
                .is_none()
        );
        document
            .relayout(
                crate::LayoutConstraints::new(24.0, 16.0).unwrap(),
                &shaped_resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles");
        assert!(
            document
                .render_view(&shaped_resources)
                .text()
                .ellipsis_for_line(0)
                .is_some()
        );
    }

    #[test]
    fn small_inline_image_keeps_natural_size() {
        // A 16x16 image in a 600px column must not be enlarged to fill the column.
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><p><img src=\"x.png\" width=\"16\" height=\"16\"></p></body></html>",
            600.0,
            &mut document_resources,
        );

        let frag = document
            .render_view(&document_resources)
            .fragments()
            .images()
            .iter()
            .next()
            .expect("expected an image fragment");
        assert_eq!(frag.size().width, 16.0);
    }

    #[test]
    fn image_dimension_hints_transfer_through_the_natural_aspect_ratio() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body style='margin:0'><img src='wide.png' width='10'><img src='wide.png' height='10'></body></html>",
            200.0,
            &[(100, 50), (100, 50)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut document_resources,
        );
        let sizes = document
            .render_view(&document_resources)
            .fragments()
            .images()
            .iter()
            .map(|fragment| fragment.size())
            .collect::<Vec<_>>();

        assert_eq!(
            sizes,
            vec![kurbo::Size::new(10.0, 5.0), kurbo::Size::new(20.0, 10.0)]
        );
    }

    #[test]
    fn oversized_inline_image_shrinks_to_available_width() {
        // An intrinsically 2000x1000 image with automatic CSS dimensions must
        // shrink to fit the column (the exact content width depends on UA
        // margins), preserving its 2:1 aspect ratio. Explicit HTML width and
        // height attributes are presentational hints, so they are deliberately
        // absent here.
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html_with_images(
            "<html><body><p><img src=\"x.png\"></p></body></html>",
            600.0,
            &[(2000, 1000)],
            crate::ImageSizingPolicy::WebCompatible,
            &mut document_resources,
        );

        let frag = document
            .render_view(&document_resources)
            .fragments()
            .images()
            .iter()
            .next()
            .expect("expected an image fragment");
        assert!(
            frag.size().width < 2000.0 && frag.size().width <= 600.0,
            "image should shrink to the column, got {}",
            frag.size().width
        );
        let ratio = frag.size().height / frag.size().width;
        assert!(
            (ratio - 0.5).abs() < 0.01,
            "aspect ratio should be preserved (0.5), got {ratio}"
        );
    }

    #[test]
    fn justified_paragraph_wraps_and_keeps_all_text() {
        let html = "<html><body><p style=\"text-align: justify\">one two three four five six seven eight nine ten</p></body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(html, 120.0, &mut document_resources);

        assert!(
            document.line_count() >= 2,
            "a narrow justified paragraph should wrap onto multiple lines"
        );

        // Joining the per-line text (restoring the space discarded at each line
        // break) must reproduce the source in order with no glyph lost.
        let combined = (0..document.line_count())
            .map(|i| line_text(&document, i, &document_resources))
            .collect::<Vec<_>>()
            .join(" ");
        let normalized = combined.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(
            normalized,
            "one two three four five six seven eight nine ten"
        );
    }

    #[test]
    fn floated_drop_cap_distributes_its_line_height_leading_normally() {
        let html = "<html><body><p><span style=\"float:left; font-size:450%; line-height:0.9em; margin-right:0.02em; margin-bottom:-0.25em;\">T</span>ext text text text</p></body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(html, 220.0, &mut document_resources);

        let line_idx = document
            .render_view(&document_resources)
            .text()
            .lines()
            .iter()
            .enumerate()
            .find(|(_, line)| {
                let text = document.render_view(&document_resources).text();
                let chars: String = text
                    .glyph_slice(line.glyphs())
                    .expect("line glyph range")
                    .iter()
                    .map(|&glyph| text.glyph_metric(glyph).expect("registered glyph").ch())
                    .collect();
                chars == "T"
            })
            .map(|(idx, _)| idx)
            .expect("drop cap line should exist");

        let line = document
            .render_view(&document_resources)
            .text()
            .line(line_idx)
            .expect("line should exist");
        let glyph = document
            .render_view(&document_resources)
            .text()
            .glyph_slice(line.glyphs())
            .expect("line glyph range")[0];
        let metric = document.glyph_metrics(&document_resources).get(glyph);
        let baseline_offset = metric.ascent() as f64;
        let half_leading = (line.height() - (metric.ascent() + metric.descent()) as f64) / 2.0;

        assert!(
            (line.baseline() - baseline_offset - half_leading).abs() < 1.0,
            "a float's contents must use the same half-leading baseline placement as ordinary inline content"
        );
    }

    #[test]
    fn pre_wrap_breaks_after_the_preserved_space_sequence() {
        let html = "<html><body><p><span>LongLongLong</span><span style=\"white-space: pre-wrap\">  x</span></p></body></html>";

        let mut factory = DocumentFactory::new();
        let mut glyph_cache = GlyphCache::new();
        let mut document_resources = GlyphResourceStore::default();
        let document = factory
            .parse_with_new_pipeline(html, None)
            .shape(&mut glyph_cache, &mut document_resources)
            .expect("test shaper must register every glyph");

        let text: String = document
            .glyphs()
            .iter()
            .map(|&glyph| document.glyph_metrics(&document_resources).get(glyph).ch())
            .collect();
        let word_width: f64 = text
            .chars()
            .zip(document.glyphs().iter())
            .take_while(|(c, _)| *c != ' ')
            .map(|(_, &glyph)| {
                document
                    .glyph_metrics(&document_resources)
                    .get(glyph)
                    .advance() as f64
            })
            .sum();

        let document = document
            .layout(
                crate::LayoutConstraints::new(word_width + 1.0, 16.0).unwrap(),
                &document_resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles");

        assert!(
            document.line_count() >= 2,
            "expected wrapping to produce multiple lines"
        );
        assert!(
            line_text(&document, 0, &document_resources).ends_with("  "),
            "the preserved spaces stay before the break"
        );
        assert_eq!(line_text(&document, 1, &document_resources), "x");
    }

    #[test]
    fn wpt_break_spaces_preserves_segment_breaks() {
        // Semantic adaptation of WPT
        // css/css-text/white-space/break-spaces-newline-011.html. Visible
        // markers replace the reftest's colored boxes so this remains a
        // deterministic framework-neutral assertion:
        // https://github.com/web-platform-tests/wpt/blob/master/css/css-text/white-space/break-spaces-newline-011.html
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><div style=\"white-space: break-spaces\">A\nB</div></body></html>",
            200.0,
            &mut document_resources,
        );

        assert_eq!(document.line_count(), 2);
        assert_eq!(line_text(&document, 0, &document_resources), "A");
        assert_eq!(line_text(&document, 1, &document_resources), "B");
    }

    #[test]
    fn pre_line_normalizes_crlf_to_one_segment_break() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><div style='white-space:pre-line'>XX&#13;&#10;</div></body></html>",
            200.0,
            &mut document_resources,
        );
        let nonempty = document
            .render_view(&document_resources)
            .text()
            .lines()
            .iter()
            .filter(|line| !line.glyphs().is_empty())
            .map(|line| line_text(&document, line.index(), &document_resources))
            .collect::<Vec<_>>();

        assert_eq!(nonempty, ["XX"]);
    }

    #[test]
    fn collapsed_spaces_are_pending_across_inline_styles_but_not_blocks() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><div><span style='white-space:normal'>X </span><span style='white-space:pre'> X</span></div><div>A </div><div>B</div></body></html>",
            200.0,
            &mut document_resources,
        );
        let lines = document
            .render_view(&document_resources)
            .text()
            .lines()
            .iter()
            .filter(|line| !line.glyphs().is_empty())
            .map(|line| line_text(&document, line.index(), &document_resources))
            .collect::<Vec<_>>();

        assert_eq!(lines, ["X  X", "A", "B"]);
    }

    #[test]
    fn collapsed_space_before_preserved_spaces_does_not_split_the_pre_sequence() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><div style='width:20px'><span style='white-space:pre'>X</span><span style='white-space:normal'> </span><span style='white-space:pre'>   X</span></div></body></html>",
            200.0,
            &mut document_resources,
        );
        let lines = document
            .render_view(&document_resources)
            .text()
            .lines()
            .iter()
            .filter(|line| !line.glyphs().is_empty())
            .map(|line| line_text(&document, line.index(), &document_resources))
            .collect::<Vec<_>>();

        assert_eq!(lines, ["X    X"]);
    }

    #[test]
    fn collapsed_segment_break_before_atomic_inline_is_preserved() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><span>left\n<span style='display:table-cell'>right</span></span></body></html>",
            200.0,
            &mut document_resources,
        );
        let lines = document
            .render_view(&document_resources)
            .text()
            .lines()
            .iter()
            .map(|line| {
                (
                    line_text(&document, line.index(), &document_resources),
                    line.point(),
                )
            })
            .collect::<Vec<_>>();

        assert!(
            lines.iter().any(|(text, _)| text.contains(' ')),
            "the collapsed segment break must remain in the outer inline before its atomic child: {lines:?}"
        );
    }

    #[test]
    fn nowrap_discards_collapsible_spaces_at_line_edges() {
        let mut leading_resources = GlyphResourceStore::default();
        let leading = layout_html(
            "<html><body><div style='white-space:nowrap'> X</div></body></html>",
            200.0,
            &mut leading_resources,
        );
        let mut trailing_resources = GlyphResourceStore::default();
        let trailing = layout_html(
            "<html><body><div style='white-space:nowrap'>X </div></body></html>",
            200.0,
            &mut trailing_resources,
        );

        assert_eq!(line_text(&leading, 0, &leading_resources), "X");
        assert_eq!(line_text(&trailing, 0, &trailing_resources), "X");
    }

    #[test]
    fn collapsed_sequence_keeps_wrap_opportunity_from_normal_ancestor() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body><div style='width:24px'><span style='white-space:nowrap'>XX </span> XX</div></body></html>",
            200.0,
            &mut document_resources,
        );
        let lines = document
            .render_view(&document_resources)
            .text()
            .lines()
            .iter()
            .filter(|line| !line.glyphs().is_empty())
            .map(|line| line_text(&document, line.index(), &document_resources))
            .collect::<Vec<_>>();

        assert_eq!(lines, ["XX", "XX"]);
    }

    #[test]
    fn zero_width_space_is_an_invisible_wrap_opportunity() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_with_native_geometry(
            "<html><body style='margin:0'><p style='margin:0'>aa&#x200B;bb</p></body></html>",
            5.0,
            vec![2.0; 5],
            vec![true; 6],
            &mut document_resources,
        );
        let lines = document
            .render_view(&document_resources)
            .text()
            .lines()
            .iter()
            .map(|line| line_text(&document, line.index(), &document_resources))
            .collect::<Vec<_>>();

        assert_eq!(lines, ["aa", "bb"]);
    }

    #[test]
    fn out_of_flow_descendant_glyphs_are_not_repainted_in_the_parent_line() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<html><body style='margin:0'><div style='position:relative'><span>AAA<div style='position:absolute;right:0;top:0'>BBB</div>CCCC</span></div></body></html>",
            200.0,
            &mut document_resources,
        );
        let text = document.render_view(&document_resources).text();
        let lines = text
            .lines()
            .iter()
            .map(|line| {
                text.line_text_fragments(line.index())
                    .expect("line fragments")
                    .flat_map(|fragment| {
                        text.glyph_slice(fragment.glyphs())
                            .expect("fragment glyph range")
                            .iter()
                            .copied()
                    })
                    .map(|glyph| text.glyph_metric(glyph).expect("registered glyph").ch())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            lines.iter().any(|line| line == "AAACCCC"),
            "the parent line must skip nested absolute glyph storage: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == "BBB"),
            "the absolute child still paints in its own formatting context: {lines:?}"
        );
    }

    fn paragraph_geometry(
        document: &html_dom::Document,
        styles: &html_style_model::ComputedStyles,
        id: &str,
    ) -> (f64, f64, f64, bool, bool) {
        let node = document
            .node_ids()
            .find(|&node| document.get_dom_id(node) == Some(id))
            .unwrap();
        let style = styles
            .used_view(styles.style_for_node(node).unwrap(), 0.5, 0.5, 0.7)
            .unwrap();
        (
            style.margin_top().resolve(300.0),
            style.margin_bottom().resolve(300.0),
            style.text_indent().resolve(300.0),
            style.text_indent_hanging(),
            style.text_indent_each_line(),
        )
    }

    #[test]
    fn authored_break_marker_preserves_clear_bidi_and_metrics() {
        for clear in [html_style_model::Clear::None, html_style_model::Clear::Both] {
            let mut token = InlineToken::new(
                InlineTokenKind::Break { clear },
                0.0,
                BreakKind::Hard,
                TokenWrap::Normal,
                true,
            );
            token.set_run_idx(InlineToken::MAX_RUN_IDX);
            assert!(!token.is_authored_break());
            token.mark_authored_break();
            for level in [0, 1, u8::MAX] {
                token.set_bidi_level(level);
                assert!(token.is_authored_break());
                assert!(token.kind() == InlineTokenKind::Break { clear });
                assert_eq!(token.run_idx(), InlineToken::MAX_RUN_IDX);
                assert_eq!(token.bidi_level(), level);
            }
        }
    }

    #[test]
    fn reader_paragraph_controls_preserve_authored_styles() {
        let html = "<html><body><p id='p' style='font-size:20px;margin:7px 0 11px;text-indent:15% hanging each-line'>Text</p><p id='small' style='font-size:10px'>Small</p><div id='div' style='font-size:20px;margin:7px 0 11px;text-indent:15% hanging each-line'>Div</div></body></html>";
        let (document, book) =
            html_style::style_document(html_parse::parse_dom_document(html).unwrap(), &[])
                .into_parts();
        let controls = crate::ReaderStyleOverrides {
            paragraph_spacing_em: Some(0.5),
            first_line_indent_em: Some(1.5),
            ..Default::default()
        };
        assert!(!controls.same_layout_preferences(&crate::ReaderStyleOverrides::default()));
        let adjusted = book.with_reader_overrides(&document, &controls).unwrap();
        assert_eq!(adjusted.reader_paragraph_spacing_em(), Some(0.5));
        assert_eq!(adjusted.reader_first_line_indent_em(), Some(1.5));
        assert_eq!(
            paragraph_geometry(&document, &adjusted, "p"),
            paragraph_geometry(&document, &book, "p")
        );
        let small = paragraph_geometry(&document, &book, "small");
        assert_eq!(paragraph_geometry(&document, &adjusted, "small"), small);
        let paragraph = document
            .node_ids()
            .find(|&node| document.get_dom_id(node) == Some("p"))
            .unwrap();
        let anonymous = adjusted
            .anonymous_box_indices(adjusted.style_for_node(paragraph).unwrap())
            .unwrap();
        let anonymous = adjusted.used_view(anonymous, 0.5, 0.5, 0.7).unwrap();
        assert!((anonymous.text_indent().resolve(300.0) - 45.0).abs() < 1e-5);
        assert_eq!(anonymous.margin_top().resolve(300.0), 0.0);
        assert_eq!(anonymous.margin_bottom().resolve(300.0), 0.0);
        assert_eq!(
            paragraph_geometry(&document, &adjusted, "div"),
            paragraph_geometry(&document, &book, "div")
        );
        let repeated = adjusted
            .with_reader_overrides(&document, &controls)
            .unwrap();
        assert_eq!(
            paragraph_geometry(&document, &repeated, "p"),
            paragraph_geometry(&document, &adjusted, "p")
        );
        let restored = adjusted
            .with_reader_overrides(&document, &crate::ReaderStyleOverrides::default())
            .unwrap();
        assert_eq!(
            paragraph_geometry(&document, &restored, "p"),
            paragraph_geometry(&document, &book, "p")
        );
        assert_eq!(restored.reader_paragraph_spacing_em(), None);
        let indent_only = adjusted
            .with_reader_overrides(
                &document,
                &crate::ReaderStyleOverrides {
                    first_line_indent_em: Some(0.0),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(indent_only.reader_first_line_indent_em(), Some(0.0));
        assert_eq!(
            paragraph_geometry(&document, &indent_only, "p"),
            paragraph_geometry(&document, &book, "p")
        );
        let spacing_only = adjusted
            .with_reader_overrides(
                &document,
                &crate::ReaderStyleOverrides {
                    paragraph_spacing_em: Some(0.0),
                    ..Default::default()
                },
            )
            .unwrap();
        let authored = paragraph_geometry(&document, &book, "p");
        assert_eq!(paragraph_geometry(&document, &spacing_only, "p"), authored);
        for invalid in [-1.0, f32::NAN, f32::INFINITY] {
            let invalid = adjusted
                .with_reader_overrides(
                    &document,
                    &crate::ReaderStyleOverrides {
                        paragraph_spacing_em: Some(invalid),
                        first_line_indent_em: Some(invalid),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert_eq!(paragraph_geometry(&document, &invalid, "p"), authored);
        }
    }

    #[test]
    fn reader_paragraph_spacing_and_indentation_use_reader_base_font() {
        for font_size in [12.0, 24.0] {
            for (spacing, indent) in [(0.0, 0.0), (0.5, 1.5)] {
                let html = format!(
                    "<html><body style='margin:0'><p style='font-size:{font_size}px;line-height:{font_size}px;margin:7px 0 11px;white-space:pre-line;text-indent:4px hanging each-line'><span>one</span>\nsecond</p><p style='font-size:{font_size}px;line-height:{font_size}px;margin:7px 0 11px;white-space:pre-line;text-indent:4px hanging each-line'>three\nfour</p></body></html>"
                );
                let (document, styles) =
                    html_style::style_document(html_parse::parse_dom_document(&html).unwrap(), &[])
                        .into_parts();
                let styles = styles
                    .with_reader_overrides(
                        &document,
                        &crate::ReaderStyleOverrides {
                            paragraph_spacing_em: Some(spacing),
                            first_line_indent_em: Some(indent),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                let mut layout_resources = GlyphResourceStore::default();
                let layout = crate::PreparedDocument::try_new(document, styles)
                    .unwrap()
                    .shape(&mut GlyphCache::new(), &mut layout_resources)
                    .unwrap()
                    .layout(
                        crate::LayoutConstraints::new(300.0, 16.0).unwrap(),
                        &layout_resources,
                    )
                    .expect("layout requires the shaper-aware API for first-line styles");
                let lines = layout
                    .render_view(&layout_resources)
                    .text()
                    .lines()
                    .iter()
                    .collect::<Vec<_>>();
                assert_eq!(lines.len(), 4);
                assert!((lines[0].point().x - f64::from(16.0 * indent)).abs() < 1e-5);
                assert!((lines[2].point().x - f64::from(16.0 * indent)).abs() < 1e-5);
                assert_eq!(
                    lines[1].point().x,
                    0.0,
                    "forced breaks must not reapply first-line indentation"
                );
                assert_eq!(lines[3].point().x, 0.0);
                let gap = lines[2].point().y - lines[1].point().y - lines[1].height();
                assert!(
                    (gap - f64::from(16.0 * spacing)).abs() < 1e-5,
                    "paragraph gap was {gap} for {spacing} em at base 16px and paragraph {font_size}px"
                );
            }
        }
    }

    #[test]
    fn text_indent_percentage_resolves_against_containing_block_width() {
        // `text-indent: 50%` in a fixed 200px block indents the first line by
        // 100px (half the containing-block width), not by a fraction of the font
        // size. Lay out in a wide viewport so the inner div keeps its 200px width.
        let html = "<html><body style=\"margin:0\"><div style=\"width:200px\"><p style=\"margin:0; text-indent:50%\">hello world this is a longer line</p></div></body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(html, 600.0, &mut document_resources);

        let first = document
            .render_view(&document_resources)
            .text()
            .line(0)
            .expect("line should exist");
        assert!(
            (first.point().x - 100.0).abs() < 1.0,
            "expected first line indented ~100px (50% of 200px), got {}",
            first.point().x
        );
    }

    #[test]
    fn each_line_indent_is_preserved_around_float_exclusions() {
        let html = "<html><body style='margin:0'><p style='margin:0; white-space:pre-line; text-indent:20px each-line'><span style='float:left; width:20px; height:100px'></span>first\nsecond</p></body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(html, 200.0, &mut document_resources);
        let text_lines = document
            .render_view(&document_resources)
            .text()
            .lines()
            .iter()
            .filter(|line| !line.glyphs().is_empty())
            .collect::<Vec<_>>();

        assert_eq!(text_lines.len(), 2);
        assert_eq!(
            line_text(&document, text_lines[0].index(), &document_resources),
            "first"
        );
        assert_eq!(
            line_text(&document, text_lines[1].index(), &document_resources),
            "second"
        );
        assert!(
            (text_lines[0].point().x - text_lines[1].point().x).abs() < 0.001,
            "a forced line break around the same float must reapply each-line indentation"
        );
    }

    #[test]
    fn margin_left_percentage_resolves_against_containing_block_width() {
        // `margin-left: 25%` on a block resolves against its containing block's
        // width (200px) → 50px, regardless of font size.
        let html = "<html><body style=\"margin:0\"><div style=\"width:200px\"><p style=\"margin:0; margin-left:25%\">x</p></div></body></html>";
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(html, 600.0, &mut document_resources);

        let first = document
            .render_view(&document_resources)
            .text()
            .line(0)
            .expect("line should exist");
        assert!(
            (first.point().x - 50.0).abs() < 1.0,
            "expected first line offset ~50px (25% of 200px), got {}",
            first.point().x
        );
    }
}
