#[cfg(test)]
mod tests {
    use super::{BoxFontMetrics, CharacterPlacement, FontSlant, RequiredFontMetrics, ShapedFontMetrics, ShapedLine, ShapedTextGeometry, ShapedTextRun, TextRunShapeRequest, TextShapeRequest, TextStyleSpan, collapse_whitespace, first_letter_style_overrides, first_letter_overrides_with_characters, is_css_collapsible_space, plan_shaping_spans, transform_text};
    use crate::parser::DocumentFactory;
    use std::sync::Arc;

    fn planned_spans(html: &str, css: Option<&str>) -> Vec<(String, Vec<u32>)> {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(html, css);
        let mut content = prepared.inline_content().clone();
        collapse_whitespace(prepared.styles(), prepared.layout_tree(), &mut content);
        let overrides = vec![None; content.glyphs().len()];
        let metrics = ShapedFontMetrics::new(BoxFontMetrics::NotRequired, RequiredFontMetrics::NotRequired, prepared.box_count(), 0.0, 0.0, 0.0);
        plan_shaping_spans(prepared.styles(), prepared.layout_tree(), &content, &overrides, &metrics)
            .into_iter()
            .map(|span| {
                let text = span.character_indices().filter_map(|index| content.glyph_at(index as usize).and_then(char::from_u32)).collect();
                let colors = span.paint_colors(prepared.styles(), prepared.layout_tree());
                (text, colors)
            })
            .collect()
    }

    fn transformed_text(html: &str) -> (String, Vec<u32>) {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(html, None);
        let mut content = prepared.inline_content().clone();
        collapse_whitespace(prepared.styles(), prepared.layout_tree(), &mut content);
        let mut first_letter = first_letter_style_overrides(prepared.document(), prepared.styles(), prepared.layout_tree(), &content);
        transform_text(prepared.styles(), prepared.layout_tree(), &mut content, &mut first_letter);
        let text = content.glyphs().iter().filter_map(|glyph| char::from_u32(*glyph)).collect();
        let offsets = (0..content.glyphs().len()).map(|index| content.raw_glyph_source_offset(index)).collect();
        (text, offsets)
    }

    #[test]
    fn absent_first_letter_skips_character_access_even_with_generated_text() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(
            "<p>text <em>inside</em></p>",
            Some("p::before {content:'prefix'} p::first-line {color:blue}"),
        );
        assert!(!prepared.styles().has_first_letter_styles());
        assert!(prepared.styles().has_first_line_styles());
        let overrides = first_letter_overrides_with_characters(
            prepared.document(),
            prepared.styles(),
            prepared.layout_tree(),
            prepared.inline_content(),
            |_| panic!("first-letter scan must be skipped"),
        );
        assert!(overrides.is_empty());
        assert_eq!(overrides.capacity(), 0);
    }

    #[test]
    fn empty_first_letter_overrides_match_dense_none_shaping_spans() {
        for (html, css) in [
            ("<p>of<span style='color:red'>fi</span>ce</p>", None),
            ("<pre>one\ntwo\tthree</pre>", None),
            (
                "<p>of<span style='position:absolute'>x</span>fice<br>end</p>",
                None,
            ),
            (
                "<p>A<span style='display:inline-block'>nested</span>B<img src='x'>C</p>",
                None,
            ),
            ("<p>A<span style='vertical-align:super'>b</span>C</p>", None),
            (
                "<p dir='rtl'>السلام <em>عليكم</em> a\u{301} fi</p>",
                Some("p::before {content:'prefix'}"),
            ),
            ("<p></p>", None),
        ] {
            let mut factory = DocumentFactory::new();
            let prepared = factory.parse_with_new_pipeline(html, css);
            let mut content = prepared.inline_content().clone();
            collapse_whitespace(prepared.styles(), prepared.layout_tree(), &mut content);
            let metrics = ShapedFontMetrics::new(
                BoxFontMetrics::NotRequired,
                RequiredFontMetrics::NotRequired,
                prepared.box_count(),
                0.0,
                0.0,
                0.0,
            );
            let dense = vec![None; content.glyphs().len()];
            let original = plan_shaping_spans(
                prepared.styles(),
                prepared.layout_tree(),
                &content,
                &dense,
                &metrics,
            );
            let empty = plan_shaping_spans(
                prepared.styles(),
                prepared.layout_tree(),
                &content,
                &[],
                &metrics,
            );
            assert_eq!(format!("{empty:?}"), format!("{original:?}"), "{html}");
            for (before, after) in original.iter().zip(empty.iter()) {
                assert_eq!(
                    before.paint_colors(prepared.styles(), prepared.layout_tree()),
                    after.paint_colors(prepared.styles(), prepared.layout_tree())
                );
            }
        }
    }

    #[test]
    fn text_transforms_preserve_empty_first_letter_overrides_and_source_mapping() {
        for html in [
            "<p style='text-transform:uppercase'>Straße <em>ß</em> a\u{301}</p>",
            "<p style='text-transform:lowercase'>İSTANBUL</p>",
            "<p style='text-transform:capitalize'>one <em>two</em><br>three</p>",
            "<p style='text-transform:uppercase'>ა</p>",
        ] {
            let mut factory = DocumentFactory::new();
            let prepared = factory.parse_with_new_pipeline(html, None);
            let mut empty_content = prepared.inline_content().clone();
            collapse_whitespace(
                prepared.styles(),
                prepared.layout_tree(),
                &mut empty_content,
            );
            let mut dense_content = empty_content.clone();
            let mut empty_overrides = Vec::new();
            let mut dense_overrides = vec![None; dense_content.glyphs().len()];
            transform_text(
                prepared.styles(),
                prepared.layout_tree(),
                &mut empty_content,
                &mut empty_overrides,
            );
            transform_text(
                prepared.styles(),
                prepared.layout_tree(),
                &mut dense_content,
                &mut dense_overrides,
            );
            assert!(empty_overrides.is_empty(), "{html}");
            assert_eq!(empty_overrides.capacity(), 0);
            assert_eq!(empty_content.glyphs(), dense_content.glyphs());
            for index in 0..empty_content.glyphs().len() {
                assert_eq!(
                    empty_content.raw_glyph_source_offset(index),
                    dense_content.raw_glyph_source_offset(index)
                );
                assert_eq!(
                    empty_content.whitespace_wrap_before(index),
                    dense_content.whitespace_wrap_before(index)
                );
            }
            let metrics = ShapedFontMetrics::new(
                BoxFontMetrics::NotRequired,
                RequiredFontMetrics::NotRequired,
                prepared.box_count(),
                0.0,
                0.0,
                0.0,
            );
            let empty = plan_shaping_spans(
                prepared.styles(),
                prepared.layout_tree(),
                &empty_content,
                &empty_overrides,
                &metrics,
            );
            let dense = plan_shaping_spans(
                prepared.styles(),
                prepared.layout_tree(),
                &dense_content,
                &dense_overrides,
                &metrics,
            );
            assert_eq!(format!("{empty:?}"), format!("{dense:?}"), "{html}");
        }
    }

    #[test]
    fn present_first_letter_styles_keep_dense_override_selection() {
        for (html, css) in [
            (
                "<p>“Apple” a\u{301}</p>",
                "p::first-letter {font-size:2em; color:purple}",
            ),
            (
                "<div><p>Apple</p></div>",
                "div::first-letter {font-size:2em}",
            ),
            (
                "<p>Apple</p>",
                "p::before {content:'“Hello” '} p::first-letter {font-size:2em}",
            ),
            (
                "<p>Apple</p>",
                "p::first-letter {float:left; font-size:2em}",
            ),
        ] {
            let mut factory = DocumentFactory::new();
            let prepared = factory.parse_with_new_pipeline(html, Some(css));
            assert!(prepared.styles().has_first_letter_styles(), "{css}");
            let overrides = first_letter_style_overrides(
                prepared.document(),
                prepared.styles(),
                prepared.layout_tree(),
                prepared.inline_content(),
            );
            assert_eq!(
                overrides.len(),
                prepared.inline_content().glyphs().len(),
                "{css}"
            );
        }
    }

    fn shaped_line() -> ShapedLine {
        ShapedLine { line_index: 3, text_range: 10..14, run: 0, ascent: 12.0, caret_stops: Arc::from([0.0, 8.0, 15.0, 22.0, 30.0]), cluster_boundaries: Arc::from([true, true, true, true, true]) }
    }

    #[test]
    fn shaped_run_requires_complete_valid_cluster_geometry() {
        assert!(ShapedTextRun::new(Arc::from([4.0, 4.0]), Arc::from([true, false, true])).is_some());
        assert!(ShapedTextRun::new(Arc::from([4.0, 4.0]), Arc::from([true, true])).is_none());
        assert!(ShapedTextRun::new(Arc::from([4.0]), Arc::from([false, true])).is_none());
        assert!(ShapedTextRun::new(Arc::from([f32::NAN]), Arc::from([true, true])).is_none());
    }

    #[test]
    fn css_collapsing_does_not_consume_typographic_unicode_spaces() {
        assert!(is_css_collapsible_space(' '));
        assert!(is_css_collapsible_space('\t'));
        assert!(!is_css_collapsible_space('\u{00a0}'));
        assert!(!is_css_collapsible_space('\u{2000}'));
        assert!(!is_css_collapsible_space('\u{3000}'));
    }

    #[test]
    fn uppercase_expansion_retains_one_source_position_for_all_generated_scalars() {
        let (text, offsets) = transformed_text("<html><body><span style='text-transform:uppercase'>ß</span></body></html>");

        assert_eq!(text, "SS");
        assert_eq!(offsets, [0, 0]);
    }

    #[test]
    fn capitalization_uses_words_across_inline_style_boundaries() {
        let (text, _) = transformed_text("<html><body>T<span style='text-transform:capitalize'>his text</span></body></html>");
        let (nested_none, _) = transformed_text("<html><body><span style='text-transform:capitalize'>i ask <span style='text-transform:none'>q</span>uestions</span></body></html>");

        assert_eq!(text, "This Text");
        assert_eq!(nested_none, "I Ask questions");
    }

    #[test]
    fn capitalization_observes_unicode_words_for_punctuation_breaks_and_nbsp() {
        let (text, _) = transformed_text("<html><body><span style='text-transform:capitalize'>i ask &quot;questions&quot;<br>questions&nbsp;questions</span></body></html>");

        assert_eq!(text, "I Ask \"Questions\"Questions\u{a0}Questions");
    }

    #[test]
    fn capitalization_ignores_empty_out_of_flow_content() {
        let (text, _) = transformed_text("<html><body><span style='text-transform:capitalize'>p<span style='position:absolute'></span>ass</span></body></html>");

        assert_eq!(text, "Pass");
    }

    #[test]
    fn capitalization_ignores_out_of_flow_siblings_with_or_without_space() {
        let (text, _) = transformed_text(
            "<html><body>\
             <span style='text-transform:capitalize'>abc</span><span style='position:absolute'></span>\n<span style='text-transform:capitalize'>abc</span><br>\
             <span style='text-transform:capitalize'>abc<span style='position:absolute'></span></span>\n<span style='text-transform:capitalize'>abc</span><br>\
             <span style='text-transform:capitalize'>abc</span> <span style='position:absolute'></span>\n<span style='text-transform:capitalize'>abc</span><br>\
             <span style='text-transform:capitalize'>abc <span style='position:absolute'></span></span>\n<span style='text-transform:capitalize'>abc</span>\
             </body></html>",
        );

        assert_eq!(text.chars().filter(|character| *character == 'A').count(), 8, "{text:?}");
    }

    #[test]
    fn css2_unicase_georgian_is_not_changed_by_uppercase() {
        let (text, _) = transformed_text("<html><body><span style='text-transform:uppercase'>&#4304;</span></body></html>");

        assert_eq!(text, "ა");
    }

    #[test]
    fn geometry_change_detection_uses_cumulative_caret_deltas() {
        let mut geometry = ShapedTextGeometry::with_len(200);
        let initial = ShapedTextRun::new(Arc::from(vec![1.0; 200]), Arc::from(vec![true; 201])).unwrap();
        assert!(geometry.update_range(0..200, &initial));

        let sub_epsilon_per_character = ShapedTextRun::new(Arc::from(vec![1.009; 200]), Arc::from(vec![true; 201])).unwrap();
        assert!(geometry.update_range(0..200, &sub_epsilon_per_character), "small scalar changes that accumulate across a line must trigger relayout");
    }

    #[test]
    fn run_metric_registration_shares_advances_and_preserves_snapshots() {
        use crate::{GlyphMetric, GlyphResourceStore};
        let mut resources = GlyphResourceStore::default();
        let single = GlyphMetric::try_new('x', 3.0, 8.0, 2.0, 8.0).unwrap();
        assert_eq!(resources.register(single).unwrap(), 0);
        let advances: Arc<[f32]> = Arc::from([2.0, 0.0, 4.0, 5.0]);
        let run = ShapedTextRun::new(advances.clone(), Arc::from([true, false, true, true, true])).unwrap();
        assert_eq!(resources.register_run("a\u{301}😀ב", &run, 8.0, 2.0, 8.0).unwrap(), 1..5);
        // Both the run and ledger retain the original advance allocation.
        assert_eq!(Arc::strong_count(&advances), 3);
        assert_eq!(resources.record_count(), 2);
        for (id, (ch, advance)) in "a\u{301}😀ב".chars().zip(advances.iter().copied()).enumerate() {
            assert_eq!(resources.get(id as u32 + 1), Some(GlyphMetric::try_new(ch, advance, 8.0, 2.0, 8.0).unwrap()));
        }
        let retained = resources.clone();
        let snapshot = resources.snapshot();
        resources.register(single).unwrap();
        resources.register(single).unwrap();
        assert_eq!(resources.record_count(), 3);
        assert_eq!(retained.len(), 5);
        assert_eq!(snapshot.len(), 5);
        assert_eq!(snapshot.get_checked(4).unwrap().ch(), 'ב');
        assert!(snapshot.get_checked(5).is_none());
        let mut hint = usize::MAX;
        for id in [4, 0, 2, 3, 1, 5, u32::MAX] {
            assert_eq!(snapshot.get_with_hint(id, &mut hint), snapshot.get_checked(id));
        }
        resources.clear();
        assert_eq!(retained.get(0), Some(single));
        assert_eq!(retained.get(4).unwrap().ch(), 'ב');
        assert!(resources.is_empty());
    }

    #[test]
    fn run_metric_registration_rejects_invalid_input_without_allocating_ids() {
        use crate::GlyphResourceStore;
        let mut resources = GlyphResourceStore::default();
        let run = ShapedTextRun::new(Arc::from([1.0, 2.0]), Arc::from([true; 3])).unwrap();
        assert!(resources.register_run("x", &run, 8.0, 2.0, 8.0).is_err());
        for (ascent, descent, baseline) in [(f32::NAN, 2.0, 8.0), (8.0, -1.0, 8.0), (8.0, 2.0, f32::INFINITY)] {
            assert!(resources.register_run("ab", &run, ascent, descent, baseline).is_err());
        }
        assert_eq!(resources.len(), 0);
        assert_eq!(resources.record_count(), 0);
        let empty = ShapedTextRun::new(Arc::from([]), Arc::from([true])).unwrap();
        assert_eq!(resources.register_run("", &empty, 0.0, 0.0, 0.0).unwrap(), 0..0);
        assert_eq!(resources.record_count(), 0);
    }

    #[test]
    fn shared_geometry_matches_dense_geometry_for_scattered_and_replaced_runs() {
        const LEN: usize = 80;
        let mut geometry = ShapedTextGeometry::with_len(LEN);
        let mut advances = vec![0.0; LEN];
        let mut boundaries = vec![true; LEN + 1];
        // Include out-of-source-order runs, gaps, and cluster boundaries that
        // land at the edge of a discontiguous DOM slice.
        for indices in [vec![40, 41, 48, 49, 50], vec![0, 1, 2, 3], vec![20, 21, 22]] {
            let widths = (0..indices.len()).map(|index| index as f32 + 0.25).collect::<Arc<[f32]>>();
            let mut clusters = vec![true; indices.len() + 1];
            clusters[1..indices.len()].fill(false);
            let shaped = ShapedTextRun::new(widths, Arc::from(clusters)).unwrap();
            for (offset, &index) in indices.iter().enumerate() {
                advances[index as usize] = shaped.advances()[offset];
                boundaries[index as usize] &= shaped.cluster_boundaries()[offset];
            }
            boundaries[*indices.last().unwrap() as usize + 1] &= shaped.cluster_boundaries()[indices.len()];
            geometry.push_run(&indices, shaped);
        }
        geometry.finish_runs();
        let check = |geometry: &ShapedTextGeometry, advances: &[f32], boundaries: &[bool]| {
            for (index, expected) in advances.iter().enumerate() { assert_eq!(geometry.advance(index), Some(*expected), "advance {index}"); }
            for (index, expected) in boundaries.iter().enumerate() { assert_eq!(geometry.is_cluster_boundary(index), *expected, "boundary {index}"); }
            assert_eq!(geometry.advance(LEN), None);
            let mut advance_hint = usize::MAX;
            let mut boundary_hint = usize::MAX;
            for index in (0..LEN).chain((0..LEN).rev()) {
                assert_eq!(geometry.advance_with_hint(index, &mut advance_hint), Some(advances[index]));
                assert_eq!(geometry.boundary_with_hint(index, &mut boundary_hint), boundaries[index]);
            }
        };
        check(&geometry, &advances, &boundaries);
        // Dynamic first-line styling replaces partial ranges. Check retained
        // prefixes/suffixes as well as entirely overwritten and empty spans.
        for range in [1..3, 42..51, 19..23, 0..2, 2..80, 0..80] {
            let count = (range.end - range.start) as usize;
            let mut clusters = vec![true; count + 1];
            if count > 1 { clusters[1] = false; }
            let shaped = ShapedTextRun::new(Arc::from(vec![7.5; count]), Arc::from(clusters)).unwrap();
            advances[range.start as usize..range.end as usize].copy_from_slice(shaped.advances());
            boundaries[range.start as usize..=range.end as usize].copy_from_slice(shaped.cluster_boundaries());
            geometry.replace_range(range, shaped);
            check(&geometry, &advances, &boundaries);
        }
    }

    #[test]
    fn run_backed_metrics_preserve_layout_and_source_geometry() {
        use crate::{GlyphId, GlyphMetric, GlyphResourceStore, GlyphShaper, ImageMetrics, LayoutConstraints, ShapeError};
        use crate::test_support::TestGlyphShaper;
        struct LedgerShaper { fallback: TestGlyphShaper, compact: bool }
        impl GlyphShaper for LedgerShaper {
            fn reset(&mut self) { self.fallback.reset(); }
            fn glyph_resources(&mut self) -> &mut GlyphResourceStore { self.fallback.glyph_resources() }
            fn begin_append_shaping(&mut self) -> Result<(), ShapeError> { self.fallback.begin_append_shaping() }
            fn commit_append_shaping(&mut self) { self.fallback.commit_append_shaping(); }
            fn rollback_append_shaping(&mut self) { self.fallback.rollback_append_shaping(); }
            fn shape_glyph(&mut self, ch: char, size: f32, weight: u16, slant: FontSlant, color: u32, family: Option<&str>) -> Result<GlyphId, ShapeError> {
                self.fallback.shape_glyph(ch, size, weight, slant, color, family)
            }
            fn shape_glyph_run(&mut self, request: TextRunShapeRequest<'_>, glyphs: &mut [GlyphId]) -> Result<Option<ShapedTextRun>, ShapeError> {
                let size = request.style().font_size();
                let geometry = ShapedTextRun::new(Arc::from(vec![size * 0.5; glyphs.len()]), Arc::from(vec![true; glyphs.len() + 1])).unwrap();
                if self.compact {
                    let ids = self.glyph_resources().register_run(request.text(), &geometry, size * 0.75, size * 0.25, size * 0.75)?;
                    for (slot, id) in glyphs.iter_mut().zip(ids) { *slot = id; }
                } else {
                    for (slot, ch) in glyphs.iter_mut().zip(request.text().chars()) {
                        *slot = self.glyph_resources().register(GlyphMetric::try_new(ch, size * 0.5, size * 0.75, size * 0.25, size * 0.75).unwrap())?;
                    }
                }
                Ok(Some(geometry))
            }
        }
        for (html, css) in [
            ("<p>office a\u{301} 😀 אבג soft\u{ad}hyphen with more words</p>", None),
            ("<p>of<span style='position:absolute'>nested</span>fice <em>again</em></p><pre>a\nb\tc</pre>", None),
            ("<p>aa aa aa aa aa aa aa</p><p>second paragraph</p>", Some("p {font-size:10px} p::first-line {font-size:20px;letter-spacing:1px;word-spacing:2px}")),
            ("<p>first <span style='font-size:24px'>large</span> end</p><ul><li>marker</li></ul>", None),
        ] {
            let mut factory = DocumentFactory::new();
            let prepared = factory.parse_with_new_pipeline(html, css);
            let mut dense = LedgerShaper { fallback: TestGlyphShaper::new(), compact: false };
            let mut compact = LedgerShaper { fallback: TestGlyphShaper::new(), compact: true };
            let dense_shaped = prepared.shape(&mut dense).unwrap();
            let compact_shaped = prepared.shape(&mut compact).unwrap();
            for width in [40.0, 120.0, 350.0] {
                let constraints = LayoutConstraints::new(width, 16.0).unwrap();
                let dense_doc = dense_shaped.clone().layout_with_metrics_and_shaper(constraints, &ImageMetrics::default(), &mut dense).unwrap();
                let compact_doc = compact_shaped.clone().layout_with_metrics_and_shaper(constraints, &ImageMetrics::default(), &mut compact).unwrap();
                let expected = dense_doc.render_view().text();
                let actual = compact_doc.render_view().text();
                assert_eq!(actual.glyph_count(), expected.glyph_count());
                assert_eq!(compact_doc.line_count(), dense_doc.line_count(), "{html} at {width}");
                for line in 0..dense_doc.line_count() {
                    assert_eq!(format!("{:?}", actual.line(line)), format!("{:?}", expected.line(line)), "{html} at {width}");
                }
                for index in 0..expected.glyph_count() {
                    assert_eq!(actual.glyph_metric(actual.glyph_at(index).unwrap()), expected.glyph_metric(expected.glyph_at(index).unwrap()));
                    assert_eq!(actual.character_advance(index as u32), expected.character_advance(index as u32));
                    assert_eq!(actual.is_character_cluster_boundary(index as u32), expected.is_character_cluster_boundary(index as u32));
                    assert_eq!(compact_doc.glyph_source_offset(index as u32), dense_doc.glyph_source_offset(index as u32));
                }
            }
        }
    }

    #[test]
    fn run_shape_request_requires_one_full_nonempty_style_span() {
        let full = TextStyleSpan::new(0..2, 16.0, 400, FontSlant::Normal, 0, None).unwrap();
        let partial = TextStyleSpan::new(0..1, 16.0, 400, FontSlant::Normal, 0, None).unwrap();
        assert!(TextRunShapeRequest::new("fi", full).is_some());
        assert!(TextRunShapeRequest::new("fi", partial).is_none());
    }

    #[test]
    fn run_shape_request_keeps_paint_colors_out_of_style_segmentation() {
        let style = TextStyleSpan::new(0..2, 16.0, 400, FontSlant::Normal, 0, None).unwrap();
        let colors = [0x112233ff, 0x445566ff];
        let request = TextRunShapeRequest::new("fi", style).unwrap().with_paint_colors(&colors).unwrap();
        assert_eq!(request.paint_colors(), Some(colors.as_slice()));
    }

    #[test]
    fn shaping_spans_cross_dom_and_paint_boundaries() {
        let spans = planned_spans("<html><body><p>of<span style='color:red'>fi</span>ce</p></body></html>", None);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].0, "office");
        assert_eq!(spans[0].1.len(), 6);
        assert_ne!(spans[0].1[1], spans[0].1[2], "the shared shaping span must retain its independent paint colors");
    }

    #[test]
    fn out_of_flow_anchors_do_not_split_shaping_spans() {
        let absolute = planned_spans("<html><body><p>of<span style='position:absolute'></span>fice</p></body></html>", None);
        let float = planned_spans("<html><body><p>of<span style='float:left'></span>fice</p></body></html>", None);

        assert_eq!(absolute.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(), ["office"]);
        assert_eq!(float.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(), ["office"]);
    }

    #[test]
    fn nonempty_out_of_flow_content_is_shaped_separately_without_splitting_surrounding_text() {
        let spans = planned_spans("<html><body><p>of<span style='position:absolute'>x</span>fice</p></body></html>", None);
        let texts = spans.iter().map(|span| span.0.as_str()).collect::<Vec<_>>();

        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts.contains(&"x"), "{texts:?}");
        assert!(texts.contains(&"office"), "{texts:?}");
    }

    #[test]
    fn inline_clear_does_not_create_a_font_shaping_boundary() {
        let spans = planned_spans("<html><body><div><div style='float:left'>P</div><span style='clear:left'>A</span>SS</div></body></html>", None);

        assert_eq!(spans.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(), ["P", "ASS"]);
    }

    #[test]
    fn font_shaping_changes_split_spans_but_spacing_and_paint_do_not() {
        let font = planned_spans("<html><body><p>of<span style='font-weight:bold'>fi</span>ce</p></body></html>", None);
        let placement_and_paint = planned_spans("<html><body><p>of<span style='letter-spacing:2px;color:red'>fi</span>ce</p></body></html>", None);

        assert_eq!(font.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(), ["of", "fi", "ce"]);
        assert_eq!(placement_and_paint.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(), ["office"]);
    }

    #[test]
    fn inline_edge_geometry_splits_only_at_the_affected_edge() {
        let start_padding = planned_spans("<html><body><p>of<span style='padding-left:10px'>f</span>ice</p></body></html>", None);
        let end_margin = planned_spans("<html><body><p>of<span style='margin-right:10px'>f</span>ice</p></body></html>", None);
        let end_border = planned_spans("<html><body><p>of<span style='border-right:10px solid transparent'>f</span>ice</p></body></html>", None);

        assert_eq!(start_padding.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(), ["of", "fice"]);
        assert_eq!(end_margin.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(), ["off", "ice"]);
        assert_eq!(end_border.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(), ["off", "ice"]);
    }

    #[test]
    fn non_baseline_vertical_alignment_splits_both_inline_edges() {
        let spans = planned_spans("<html><body><p>of<span style='vertical-align:1em'>f</span>ice</p></body></html>", None);

        assert_eq!(spans.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(), ["of", "f", "ice"]);
    }

    #[test]
    fn atomic_inline_items_are_hard_shaping_boundaries_even_when_empty() {
        let spans = planned_spans("<html><body><p>f<span style='display:inline-block'></span>i</p></body></html>", None);
        assert_eq!(spans.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(), ["f", "i"]);
    }

    #[test]
    fn preserved_newlines_split_runs_without_forcing_surrounding_text_to_scalar_shaping() {
        let spans = planned_spans("<html><body><p style='white-space:pre'>of\nfice</p></body></html>", None);
        assert_eq!(spans.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(), ["of", "\n", "fice"]);
    }

    #[test]
    fn shaped_line_maps_logical_positions_through_shared_caret_stops() {
        let line = shaped_line();
        assert_eq!(line.x_for_text_position(10), Some(0.0));
        assert_eq!(line.x_for_text_position(12), Some(15.0));
        assert_eq!(line.x_for_text_position(14), Some(30.0));
        assert_eq!(line.x_for_text_position(15), None);
    }

    #[test]
    fn shaped_line_hit_testing_uses_shared_caret_stops() {
        let line = shaped_line();
        assert_eq!(line.closest_text_position(14.0), 12);
        assert_eq!(line.closest_text_position(21.5), 13);
        assert_eq!(line.closest_text_position(29.5), 14);
    }

    #[test]
    fn placements_add_or_replace_natural_character_advances() {
        let placements = [CharacterPlacement::new(2.0, None, true).unwrap(), CharacterPlacement::new(99.0, Some(4.0), false).unwrap()];
        let styles = [TextStyleSpan::new(0..2, 16.0, 400, FontSlant::Normal, 0, None).unwrap()];
        let request = TextShapeRequest::new(0, 0..2, "ab", &styles, &placements).unwrap();
        assert_eq!(request.adjusted_caret_stops(&[0.0, 8.0, 15.0]).as_deref(), Some([0.0, 10.0, 14.0].as_slice()));
    }

    #[test]
    fn placements_preserve_the_direction_of_natural_advances() {
        let placements = [CharacterPlacement::new(2.0, None, true).unwrap()];
        let styles = [TextStyleSpan::new(0..1, 16.0, 400, FontSlant::Normal, 0, None).unwrap()];
        let request = TextShapeRequest::new(0, 0..1, "a", &styles, &placements).unwrap();
        assert_eq!(request.adjusted_caret_stops(&[10.0, 4.0]).as_deref(), Some([10.0, 2.0].as_slice()));
    }

    #[test]
    fn text_backend_contract_rejects_invalid_geometry_at_construction() {
        assert!(CharacterPlacement::new(f32::NAN, None, true).is_none());
        assert!(CharacterPlacement::new(0.0, Some(-1.0), true).is_none());
        assert!(CharacterPlacement::with_baseline_shift(0.0, None, f32::NAN, true).is_none());
        assert!(TextStyleSpan::new(0..1, 0.0, 400, FontSlant::Normal, 0, None).is_some());
        assert!(TextStyleSpan::new(0..1, -1.0, 400, FontSlant::Normal, 0, None).is_none());

        let placement = [CharacterPlacement::default()];
        let gap = [TextStyleSpan::new(1..2, 16.0, 400, FontSlant::Normal, 0, None).unwrap()];
        assert!(TextShapeRequest::new(0, 0..1, "a", &gap, &placement).is_none());

        let complete = [TextStyleSpan::new(0..1, 16.0, 400, FontSlant::Normal, 0, None).unwrap()];
        assert!(TextShapeRequest::new(0, 0..2, "a", &complete, &placement).is_none());
    }
}
