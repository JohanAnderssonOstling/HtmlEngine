#[cfg(test)]
mod tests {
    use html_engine_model::shaping;

    use crate::{GlyphRequest, ShapedGlyph, ShapedRun};

    use super::{
        BoxFontMetrics, FontSlant, RequiredFontMetrics, ResolvedBidi, ShapedFontMetrics,
        ShapedLine, ShapedTextGeometry, ShapedTextRun, TextRunShapeRequest, collapse_whitespace,
        first_letter_overrides_with_characters, first_letter_style_overrides,
        is_css_collapsible_space, plan_shaping_spans, transform_text,
    };
    use crate::parser::DocumentFactory;
    use std::cell::Cell;
    use std::sync::Arc;

    fn planned_spans(html: &str, css: Option<&str>) -> Vec<(String, Vec<u32>)> {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(html, css);
        let mut content = prepared.inline_content().clone();
        collapse_whitespace(prepared.test_styles(), prepared.layout_tree(), &mut content);
        let overrides = vec![None; content.characters().len()];
        let metrics = ShapedFontMetrics::new(
            BoxFontMetrics::NotRequired,
            RequiredFontMetrics::NotRequired,
            prepared.box_count(),
            0.0,
            0.0,
            0.0,
        );
        plan_shaping_spans(
            prepared.test_styles(),
            prepared.layout_tree(),
            &content,
            &overrides,
            &metrics,
            &ResolvedBidi::new(prepared.test_styles(), prepared.layout_tree(), &content),
        )
        .into_iter()
        .map(|span| {
            let text = span
                .character_indices()
                .filter_map(|index| content.character_at(index as usize))
                .collect();
            let (colors, _) =
                span.paint_and_placement(prepared.test_styles(), prepared.layout_tree(), &metrics);
            let colors = colors.unwrap_or_else(|| {
                vec![
                    span.style(prepared.test_styles(), prepared.layout_tree())
                        .color();
                    span.character_count()
                ]
            });
            (text, colors)
        })
        .collect()
    }

    fn transformed_text(html: &str) -> (String, Vec<u32>) {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(html, None);
        let mut content = prepared.inline_content().clone();
        collapse_whitespace(prepared.test_styles(), prepared.layout_tree(), &mut content);
        let mut first_letter = first_letter_style_overrides(
            prepared.document(),
            prepared.test_styles(),
            prepared.layout_tree(),
            &content,
        );
        transform_text(
            prepared.test_styles(),
            prepared.layout_tree(),
            &mut content,
            &mut first_letter,
        );
        let text = content
            .characters()
            .iter()
            .copied()
            .collect();
        let offsets = (0..content.characters().len())
            .map(|index| content.raw_glyph_source_offset(index))
            .collect();
        (text, offsets)
    }

    #[test]
    fn absent_first_letter_skips_character_access_even_with_generated_text() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(
            "<p>text <em>inside</em></p>",
            Some("p::before {content:'prefix'} p::first-line {color:blue}"),
        );
        assert!(!prepared.test_styles().has_first_letter_styles());
        assert!(prepared.test_styles().has_first_line_styles());
        let overrides = first_letter_overrides_with_characters(
            prepared.document(),
            prepared.test_styles(),
            prepared.layout_tree(),
            prepared.inline_content(),
            |_| panic!("first-letter scan must be skipped"),
        );
        assert!(overrides.is_empty());
        assert_eq!(overrides.capacity(), 0);
    }

    #[test]
    fn first_letter_selection_stops_reading_completed_roots() {
        for length in [10, 40_000] {
            let tail = "x".repeat(length);
            let mut factory = DocumentFactory::new();
            let prepared = factory.parse_with_new_pipeline(
                &format!(
                    "<p>“<span>A</span>!<em>\u{301}</em>{tail}</p><p><span>B</span>{tail}</p>"
                ),
                Some("p::first-letter {font-size:2em}"),
            );
            let content = prepared.inline_content();
            let reads = Cell::new(0);
            let overrides = first_letter_overrides_with_characters(
                prepared.document(),
                prepared.test_styles(),
                prepared.layout_tree(),
                content,
                |index| {
                    reads.set(reads.get() + 1);
                    content.character_at(index as usize).unwrap()
                },
            );
            let selected: String = content
                .characters()
                .iter()
                .zip(overrides)
                .filter_map(|(&character, style)| style.map(|_| character))
                .collect();
            assert_eq!(selected, "“A!\u{301}B");
            assert!(
                reads.get() <= 7,
                "completed roots must not scan paragraph tails"
            );
        }
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
            collapse_whitespace(prepared.test_styles(), prepared.layout_tree(), &mut content);
            let metrics = ShapedFontMetrics::new(
                BoxFontMetrics::NotRequired,
                RequiredFontMetrics::NotRequired,
                prepared.box_count(),
                0.0,
                0.0,
                0.0,
            );
            let dense = vec![None; content.characters().len()];
            let original = plan_shaping_spans(
                prepared.test_styles(),
                prepared.layout_tree(),
                &content,
                &dense,
                &metrics,
                &ResolvedBidi::new(prepared.test_styles(), prepared.layout_tree(), &content),
            );
            let empty = plan_shaping_spans(
                prepared.test_styles(),
                prepared.layout_tree(),
                &content,
                &[],
                &metrics,
                &ResolvedBidi::new(prepared.test_styles(), prepared.layout_tree(), &content),
            );
            assert_eq!(format!("{empty:?}"), format!("{original:?}"), "{html}");
            for (before, after) in original.iter().zip(empty.iter()) {
                assert_eq!(
                    before.paint_and_placement(prepared.test_styles(), prepared.layout_tree(), &metrics),
                    after.paint_and_placement(prepared.test_styles(), prepared.layout_tree(), &metrics)
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
                prepared.test_styles(),
                prepared.layout_tree(),
                &mut empty_content,
            );
            let mut dense_content = empty_content.clone();
            let mut empty_overrides = Vec::new();
            let mut dense_overrides = vec![None; dense_content.characters().len()];
            transform_text(
                prepared.test_styles(),
                prepared.layout_tree(),
                &mut empty_content,
                &mut empty_overrides,
            );
            transform_text(
                prepared.test_styles(),
                prepared.layout_tree(),
                &mut dense_content,
                &mut dense_overrides,
            );
            assert!(empty_overrides.is_empty(), "{html}");
            assert_eq!(empty_overrides.capacity(), 0);
            assert_eq!(empty_content.characters(), dense_content.characters());
            for index in 0..empty_content.characters().len() {
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
                prepared.test_styles(),
                prepared.layout_tree(),
                &empty_content,
                &empty_overrides,
                &metrics,
                &ResolvedBidi::new(prepared.test_styles(), prepared.layout_tree(), &empty_content),
            );
            let dense = plan_shaping_spans(
                prepared.test_styles(),
                prepared.layout_tree(),
                &dense_content,
                &dense_overrides,
                &metrics,
                &ResolvedBidi::new(prepared.test_styles(), prepared.layout_tree(), &dense_content),
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
            assert!(prepared.test_styles().has_first_letter_styles(), "{css}");
            let overrides = first_letter_style_overrides(
                prepared.document(),
                prepared.test_styles(),
                prepared.layout_tree(),
                prepared.inline_content(),
            );
            assert_eq!(
                overrides.len(),
                prepared.inline_content().characters().len(),
                "{css}"
            );
        }
    }

    fn shaped_line() -> ShapedLine {
        ShapedLine {
            line_index: 3,
            text_range: 10..14,
            run: 0,
            ascent: 12.0,
            caret_stops: [0.0, 8.0, 15.0, 22.0, 30.0].into(),
            cluster_boundaries: Arc::from([true, true, true, true, true]),
        }
    }

    #[test]
    fn native_ascii_boundaries_preserve_crlf_and_ligatures() {
        let mut run = ShapedTextRun::new(
            Arc::from([4.0; 4]),
            Arc::from([true, false, true, true, true]),
        );
        shaping::preserve_graphemes(&mut run, "fi\r\n");
        assert_eq!(run.cluster_boundaries(), &[true, false, true, false, true]);
    }

    #[test]
    fn graphemes_are_segmented_once_per_run_and_keep_native_ligatures() {
        use crate::test_support::TestGlyphShaper;
        use crate::{
            GlyphResourceGeneration, GlyphResourceStore, GlyphShaper, ImageMetrics,
            LayoutConstraints, ShapeError,
        };

        struct Shaper {
            fallback: TestGlyphShaper,
            native: bool,
            run_calls: usize,
        }
        impl GlyphShaper for Shaper {
            fn reset(&mut self) {
                self.fallback.reset();
                self.run_calls = 0;
            }
            fn resource_generation(&self) -> GlyphResourceGeneration {
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
            fn shape_glyph(
                &mut self,
                request: GlyphRequest<'_>,
            ) -> Result<ShapedGlyph, ShapeError> {
                self.fallback.shape_glyph(request)
            }
            fn shape_run(
                &mut self,
                request: TextRunShapeRequest<'_>,
            ) -> Result<Option<ShapedRun>, ShapeError> {
                self.run_calls += 1;
                let size = request.style().font_size;
                let ligature_boundary = request
                    .text()
                    .find("fi")
                    .map(|byte| request.text()[..byte].chars().count() + 1);
                let count = request.text().chars().count();
                if !self.native {
                    return Ok(None);
                }
                // The backend merges "fi" but leaves combining/emoji clusters
                // too permissive, so both native and Unicode constraints matter.
                let mut boundaries = vec![true; count + 1];
                if let Some(boundary) = ligature_boundary {
                    boundaries[boundary] = false;
                }
                Ok(Some(ShapedRun::geometry(ShapedTextRun::new(
                    vec![size * 0.5; count].into(),
                    boundaries.into(),
                ))))
            }
        }

        for native in [false, true] {
            for first_line in [false, true] {
                let css = first_line.then_some("p::first-line { font-size:20px }");
                let prepared = DocumentFactory::new()
                    .parse_with_new_pipeline("<p>e\u{301}fi 👩\u{200d}💻 🇸🇪</p>", css);
                let mut shaper = Shaper {
                    fallback: TestGlyphShaper::new(),
                    native,
                    run_calls: 0,
                };
                let constraints = LayoutConstraints::new(1000.0, 16.0).unwrap();
                let metrics = ImageMetrics::default();
                let mut document_resources = GlyphResourceStore::default();
                #[cfg(feature = "profiling")]
                let document = {
                    let (result, entries) =
                        crate::profiling::capture(std::time::Duration::default, || {
                            prepared.shape_and_layout_with_metrics_and_shaper(
                                constraints,
                                &metrics,
                                &mut shaper,
                                &mut document_resources,
                            )
                        });
                    let passes = entries
                        .iter()
                        .find(|entry| entry.name == "shape.grapheme_boundaries")
                        .unwrap()
                        .calls;
                    assert_eq!(
                        passes, shaper.run_calls,
                        "native={native}, first_line={first_line}"
                    );
                    result.unwrap()
                };
                #[cfg(not(feature = "profiling"))]
                let document = prepared
                    .shape_and_layout_with_metrics_and_shaper(
                        constraints,
                        &metrics,
                        &mut shaper,
                        &mut document_resources,
                    )
                    .unwrap();
                let text = document.render_view(&document_resources).text();
                let mut expected = [
                    true, false, true, true, true, true, false, false, true, true, false, true,
                ];
                if native {
                    expected[3] = false;
                }
                assert_eq!(text.glyph_count(), expected.len() - 1);
                for (index, expected) in expected.into_iter().enumerate() {
                    assert_eq!(
                        text.is_character_cluster_boundary(index as u32),
                        expected,
                        "boundary {index}, native={native}, first_line={first_line}"
                    );
                }
            }
        }
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
        let (text, offsets) = transformed_text(
            "<html><body><span style='text-transform:uppercase'>ß</span></body></html>",
        );

        assert_eq!(text, "SS");
        assert_eq!(offsets, [0, 0]);
    }

    #[test]
    fn capitalization_uses_words_across_inline_style_boundaries() {
        let (text, _) = transformed_text(
            "<html><body>T<span style='text-transform:capitalize'>his text</span></body></html>",
        );
        let (nested_none, _) = transformed_text(
            "<html><body><span style='text-transform:capitalize'>i ask <span style='text-transform:none'>q</span>uestions</span></body></html>",
        );

        assert_eq!(text, "This Text");
        assert_eq!(nested_none, "I Ask questions");
    }

    #[test]
    fn capitalization_observes_unicode_words_for_punctuation_breaks_and_nbsp() {
        let (text, _) = transformed_text(
            "<html><body><span style='text-transform:capitalize'>i ask &quot;questions&quot;<br>questions&nbsp;questions</span></body></html>",
        );

        assert_eq!(text, "I Ask \"Questions\"Questions\u{a0}Questions");
    }

    #[test]
    fn capitalization_ignores_empty_out_of_flow_content() {
        let (text, _) = transformed_text(
            "<html><body><span style='text-transform:capitalize'>p<span style='position:absolute'></span>ass</span></body></html>",
        );

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

        assert_eq!(
            text.chars().filter(|character| *character == 'A').count(),
            8,
            "{text:?}"
        );
    }

    #[test]
    fn css2_unicase_georgian_is_not_changed_by_uppercase() {
        let (text, _) = transformed_text(
            "<html><body><span style='text-transform:uppercase'>&#4304;</span></body></html>",
        );

        assert_eq!(text, "ა");
    }

    #[test]
    fn geometry_change_detection_uses_cumulative_caret_deltas() {
        let mut geometry = ShapedTextGeometry::with_len(200);
        let initial = ShapedTextRun::new(Arc::from(vec![1.0; 200]), Arc::from(vec![true; 201]));
        assert!(geometry.update_range(0..200, &initial));

        let sub_epsilon_per_character =
            ShapedTextRun::new(Arc::from(vec![1.009; 200]), Arc::from(vec![true; 201]));
        assert!(
            geometry.update_range(0..200, &sub_epsilon_per_character),
            "small scalar changes that accumulate across a line must trigger relayout"
        );
    }

    #[test]
    fn shared_geometry_memory_counts_each_retained_allocation_once() {
        let run = || {
            ShapedTextRun::with_backend_run(
                Arc::from([2.0, 3.0]),
                Arc::from([true; 3]),
                7,
                [0.0, 2.0, 5.0].into(),
                8.0,
            )
        };
        let geometry = |shared: bool| {
            let first = run();
            let second = if shared { first.clone() } else { run() };
            let mut geometry = ShapedTextGeometry::with_len(6);
            for (indices, run) in [([0, 1], first), ([4, 5], second)] {
                geometry.push_run(&indices, run, false);
            }
            geometry.finish_runs();
            geometry
        };
        let shared = geometry(true);
        let independent = geometry(false);
        let one_run_bytes = 2 * size_of::<f32>() + 3 * size_of::<bool>() + 3 * size_of::<f32>();
        assert_eq!(
            independent.memory_usage_bytes() - shared.memory_usage_bytes(),
            one_run_bytes
        );
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
            let widths = (0..indices.len())
                .map(|index| index as f32 + 0.25)
                .collect::<Arc<[f32]>>();
            let mut clusters = vec![true; indices.len() + 1];
            clusters[1..indices.len()].fill(false);
            let shaped = ShapedTextRun::new(widths, Arc::from(clusters));
            for (offset, &index) in indices.iter().enumerate() {
                advances[index as usize] = shaped.advances()[offset];
                boundaries[index as usize] &= shaped.cluster_boundaries()[offset];
            }
            boundaries[*indices.last().unwrap() as usize + 1] &=
                shaped.cluster_boundaries()[indices.len()];
            geometry.push_run(&indices, shaped, false);
        }
        geometry.finish_runs();
        let check = |geometry: &ShapedTextGeometry, advances: &[f32], boundaries: &[bool]| {
            for (index, expected) in advances.iter().enumerate() {
                assert_eq!(geometry.advance(index), Some(*expected), "advance {index}");
            }
            for (index, expected) in boundaries.iter().enumerate() {
                assert_eq!(
                    geometry.is_cluster_boundary(index),
                    *expected,
                    "boundary {index}"
                );
            }
            assert_eq!(geometry.advance(LEN), None);
            let view = super::TextGeometryView::new(geometry, None);
            let mut geometry_hint = 0;
            for index in (0..=LEN + 1).chain((0..=LEN).rev()) {
                assert_eq!(
                    view.character_geometry_with_hint(index, &mut geometry_hint),
                    (geometry.advance(index), geometry.is_cluster_boundary(index)),
                    "cursor {index}"
                );
            }
            let mut advance_hint = usize::MAX;
            let mut boundary_hint = usize::MAX;
            for index in (0..LEN).chain((0..LEN).rev()) {
                assert_eq!(
                    geometry.advance_with_hint(index, &mut advance_hint),
                    Some(advances[index])
                );
                assert_eq!(
                    geometry.boundary_with_hint(index, &mut boundary_hint),
                    boundaries[index]
                );
            }
        };
        check(&geometry, &advances, &boundaries);
        // Dynamic first-line styling replaces partial ranges. Check retained
        // prefixes/suffixes as well as entirely overwritten and empty spans.
        for range in [1..3, 42..51, 19..23, 0..2, 2..80, 0..80] {
            let count = (range.end - range.start) as usize;
            let mut clusters = vec![true; count + 1];
            if count > 1 {
                clusters[1] = false;
            }
            let shaped = ShapedTextRun::new(Arc::from(vec![7.5; count]), Arc::from(clusters));
            advances[range.start as usize..range.end as usize].copy_from_slice(shaped.advances());
            boundaries[range.start as usize..=range.end as usize]
                .copy_from_slice(shaped.cluster_boundaries());
            geometry.replace_range(range, shaped, false);
            check(&geometry, &advances, &boundaries);
        }
    }

    #[test]
    fn combined_geometry_matches_scalar_queries_across_refinement_edges_and_gaps() {
        use super::TextGeometryView;

        const LEN: usize = 32;
        let assert_cursor = |view: TextGeometryView<'_>| {
            let mut geometry_hint = 0;
            let indices = (0..=LEN + 2)
                .chain((0..=LEN + 2).rev())
                .chain((0..200).map(|index| (index * 17) % (LEN + 3)))
                .chain([usize::MAX, 0, LEN, 2]);
            for index in indices {
                assert_eq!(
                    view.character_geometry_with_hint(index, &mut geometry_hint),
                    (view.advance(index), view.is_cluster_boundary(index)),
                    "index {index}"
                );
            }
        };
        let empty = ShapedTextGeometry::with_len(0);
        assert_cursor(TextGeometryView::new(&empty, None));
        let mut base = ShapedTextGeometry::with_len(LEN);
        assert_cursor(TextGeometryView::new(&base, None));
        for indices in [
            vec![0, 1, 2, 3],
            vec![6, 7, 9],
            vec![16, 17, 18],
            vec![25, 26, 27, 28, 29, 30, 31],
        ] {
            let mut boundaries = vec![true; indices.len() + 1];
            boundaries[1..indices.len()].fill(false);
            let advances = (0..indices.len())
                .map(|index| index as f32 + 0.25)
                .collect::<Arc<[f32]>>();
            base.push_run(
                &indices,
                ShapedTextRun::new(advances, boundaries.into()),
                false,
            );
        }
        base.finish_runs();
        assert_cursor(TextGeometryView::new(&base, None));
        let mut refined = ShapedTextGeometry::with_len(LEN);
        assert_cursor(TextGeometryView::new(&base, Some(&refined)));
        for range in [1..3, 4..7, 7..9, 17..19, 18..21, 24..32] {
            let length = (range.end - range.start) as usize;
            let mut boundaries = vec![true; length + 1];
            boundaries[1..length].fill(false);
            refined.replace_range(
                range,
                ShapedTextRun::new(vec![7.5; length].into(), boundaries.into()),
                false,
            );
            assert_cursor(TextGeometryView::new(&base, Some(&refined)));
        }
    }

    #[test]
    fn run_shape_request_requires_one_full_nonempty_style_span() {
        let full = shaping::style(16.0, 400, FontSlant::Normal, 0, None)
            .and_then(|resolved_style| shaping::span(0..2, resolved_style))
            .unwrap();
        let partial = shaping::style(16.0, 400, FontSlant::Normal, 0, None)
            .and_then(|resolved_style| shaping::span(0..1, resolved_style))
            .unwrap();
        assert!(shaping::run("fi", full, None).is_some());
        assert!(shaping::run("fi", partial, None).is_none());
    }

    #[test]
    fn run_shape_request_keeps_paint_colors_out_of_style_segmentation() {
        let style = shaping::style(16.0, 400, FontSlant::Normal, 0, None)
            .and_then(|resolved_style| shaping::span(0..2, resolved_style))
            .unwrap();
        let colors = [0x112233ff, 0x445566ff];
        let request = shaping::run("fi", style, Some(&colors)).unwrap();
        assert_eq!(request.paint_colors(), Some(colors.as_slice()));
    }

    #[test]
    fn shaping_requests_preserve_uniform_and_mixed_colors_across_bidi_splits() {
        use crate::test_support::TestGlyphShaper;
        use crate::{GlyphResourceGeneration, GlyphResourceStore, GlyphShaper, ShapeError};

        struct Recorder {
            fallback: TestGlyphShaper,
            requests: Vec<(String, u32, Option<Vec<u32>>)>,
        }
        impl GlyphShaper for Recorder {
            fn reset(&mut self) {
                self.fallback.reset();
                self.requests.clear();
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
                self.requests.push((
                    request.text().to_owned(),
                    request.style().color,
                    request.paint_colors().map(|colors| colors.to_vec()),
                ));
                Ok(None)
            }
        }

        let black = 0x000000ff;
        let red = 0xff0000ff;
        for (source, expected) in [
            (
                "<p style='color:#123456'>of<span>fi</span>ce</p>",
                vec![("office".to_owned(), 0x123456ff, None)],
            ),
            (
                "<p style='color:black'>of<span style='color:red'>fi</span>ce</p>",
                vec![(
                    "office".to_owned(),
                    black,
                    Some(vec![black, black, red, red, black, black]),
                )],
            ),
            (
                "<p style='color:black'>a<span style='color:red'>אב</span>b</p>",
                vec![
                    ("a".to_owned(), black, None),
                    ("אב".to_owned(), black, Some(vec![red, red])),
                    ("b".to_owned(), black, None),
                ],
            ),
        ] {
            let prepared = DocumentFactory::new().parse_with_new_pipeline(source, None);
            let mut shaper = Recorder {
                fallback: TestGlyphShaper::new(),
                requests: Vec::new(),
            };
            let mut resources = GlyphResourceStore::default();
            prepared.shape(&mut shaper, &mut resources).unwrap();
            assert_eq!(shaper.requests, expected, "{source}");
        }
    }

    #[test]
    fn shaping_spans_cross_dom_and_paint_boundaries() {
        let spans = planned_spans(
            "<html><body><p>of<span style='color:red'>fi</span>ce</p></body></html>",
            None,
        );
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].0, "office");
        assert_eq!(spans[0].1.len(), 6);
        assert_ne!(
            spans[0].1[1], spans[0].1[2],
            "the shared shaping span must retain its independent paint colors"
        );
    }

    #[test]
    fn out_of_flow_anchors_do_not_split_shaping_spans() {
        let absolute = planned_spans(
            "<html><body><p>of<span style='position:absolute'></span>fice</p></body></html>",
            None,
        );
        let float = planned_spans(
            "<html><body><p>of<span style='float:left'></span>fice</p></body></html>",
            None,
        );

        assert_eq!(
            absolute
                .iter()
                .map(|span| span.0.as_str())
                .collect::<Vec<_>>(),
            ["office"]
        );
        assert_eq!(
            float.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(),
            ["office"]
        );
    }

    #[test]
    fn nonempty_out_of_flow_content_is_shaped_separately_without_splitting_surrounding_text() {
        let spans = planned_spans(
            "<html><body><p>of<span style='position:absolute'>x</span>fice</p></body></html>",
            None,
        );
        let texts = spans.iter().map(|span| span.0.as_str()).collect::<Vec<_>>();

        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts.contains(&"x"), "{texts:?}");
        assert!(texts.contains(&"office"), "{texts:?}");
    }

    #[test]
    fn inline_clear_does_not_create_a_font_shaping_boundary() {
        let spans = planned_spans(
            "<html><body><div><div style='float:left'>P</div><span style='clear:left'>A</span>SS</div></body></html>",
            None,
        );

        assert_eq!(
            spans.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(),
            ["P", "ASS"]
        );
    }

    #[test]
    fn font_shaping_changes_split_spans_but_spacing_and_paint_do_not() {
        let font = planned_spans(
            "<html><body><p>of<span style='font-weight:bold'>fi</span>ce</p></body></html>",
            None,
        );
        let placement_and_paint = planned_spans(
            "<html><body><p>of<span style='letter-spacing:2px;color:red'>fi</span>ce</p></body></html>",
            None,
        );

        assert_eq!(
            font.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(),
            ["of", "fi", "ce"]
        );
        assert_eq!(
            placement_and_paint
                .iter()
                .map(|span| span.0.as_str())
                .collect::<Vec<_>>(),
            ["office"]
        );
    }

    #[test]
    fn inline_edge_geometry_splits_only_at_the_affected_edge() {
        let start_padding = planned_spans(
            "<html><body><p>of<span style='padding-left:10px'>f</span>ice</p></body></html>",
            None,
        );
        let end_margin = planned_spans(
            "<html><body><p>of<span style='margin-right:10px'>f</span>ice</p></body></html>",
            None,
        );
        let end_border = planned_spans(
            "<html><body><p>of<span style='border-right:10px solid transparent'>f</span>ice</p></body></html>",
            None,
        );

        assert_eq!(
            start_padding
                .iter()
                .map(|span| span.0.as_str())
                .collect::<Vec<_>>(),
            ["of", "fice"]
        );
        assert_eq!(
            end_margin
                .iter()
                .map(|span| span.0.as_str())
                .collect::<Vec<_>>(),
            ["off", "ice"]
        );
        assert_eq!(
            end_border
                .iter()
                .map(|span| span.0.as_str())
                .collect::<Vec<_>>(),
            ["off", "ice"]
        );
    }

    #[test]
    fn non_baseline_vertical_alignment_splits_both_inline_edges() {
        let spans = planned_spans(
            "<html><body><p>of<span style='vertical-align:1em'>f</span>ice</p></body></html>",
            None,
        );

        assert_eq!(
            spans.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(),
            ["of", "f", "ice"]
        );
    }

    #[test]
    fn atomic_inline_items_are_hard_shaping_boundaries_even_when_empty() {
        let spans = planned_spans(
            "<html><body><p>f<span style='display:inline-block'></span>i</p></body></html>",
            None,
        );
        assert_eq!(
            spans.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(),
            ["f", "i"]
        );
    }

    #[test]
    fn preserved_newlines_split_runs_without_forcing_surrounding_text_to_scalar_shaping() {
        let cases: &[(&str, &[&str])] = &[
            ("of\nfice", &["of", "\n", "fice"]),
            ("\n\nof\n\nfice\n", &["\n", "\n", "of", "\n", "\n", "fice", "\n"]),
            ("", &[]),
        ];
        for (text, expected) in cases {
            let spans = planned_spans(
                &format!("<html><body><p style='white-space:pre'>{text}</p></body></html>"),
                None,
            );
            assert_eq!(
                spans.iter().map(|span| span.0.as_str()).collect::<Vec<_>>(),
                *expected,
                "{text:?}",
            );
        }
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
    fn text_backend_contract_rejects_invalid_geometry_at_construction() {
        assert!(
            shaping::style(0.0, 400, FontSlant::Normal, 0, None)
                .and_then(|resolved_style| shaping::span(0..1, resolved_style))
                .is_some()
        );
        assert!(
            shaping::style(-1.0, 400, FontSlant::Normal, 0, None)
                .and_then(|resolved_style| shaping::span(0..1, resolved_style))
                .is_none()
        );
    }
}
