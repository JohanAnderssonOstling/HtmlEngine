#[cfg(test)]
mod stage_tests {
    use crate::ShapeError;
    use crate::layout_model::{GlyphId, GlyphMetric, GlyphMetricError};
    use crate::test_support::{DocumentFactory, TestGlyphShaper};
    use crate::{
        FontSlant, GlyphResourceStore, GlyphShaper, ImageMetrics, LaidOutDocument,
        LayoutConstraintError, LayoutConstraints, PreparedDocument, ReaderStyleOverrides,
        TextCompositionPolicy,
    };
    use std::sync::Arc;
    use html_style_model::{ComputedStylesBuildError, ComputedStylesBuilder, ComputedStylesValidationError};

    fn paragraph_geometry_snapshot(document: &LaidOutDocument) -> Vec<String> {
        let mut result = Vec::new();
        let output = &document.layout_state.line_output;
        for (index, line) in output.lines.iter().enumerate() {
            result.push(format!("{:?}", (&line.glyphs, line.owner_box_idx, line.point, line.height, line.baseline, line.word_spacing, line.letter_spacing,
                output.line_glyph_offsets.get(index).unwrap_or_default().iter().map(|run| (&run.range,run.offset)).collect::<Vec<_>>(), output.line_glyph_advances.get(index).unwrap_or_default().iter().map(|run| (&run.range,run.advance)).collect::<Vec<_>>(), document.render_view().text().line_overflow_clip(index))));
        }
        result.push(format!("{:?}", (&output.text_fragments, &output.inline_box_fragments, &output.prepared_text_run_fragments)));
        for index in 0..document.box_count() {
            result.push(format!("{:?}", (document.geometry.point(index), document.geometry.size(index))));
        }
        result.extend(document.render_view().fragments().decorations().iter().map(|decoration| format!("{decoration:?}")));
        result.extend(document.render_view().fragments().images().iter().map(|image| format!("{image:?}")));
        result.push(format!("{:?}", (0..document.glyphs().len()).map(|glyph| document.render_view().text().line_index_for_glyph(glyph as u32)).collect::<Vec<_>>()));
        result
    }

    #[test]
    fn image_height_changes_reposition_unchanged_paragraphs_without_recomposing_them() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline("<img src='top' style='display:block;width:40px'><p>one two three four five six seven eight nine ten</p><p>more words across several lines of text</p>", None);
        let constraints = LayoutConstraints::new(160.0, 20.0).unwrap();
        let mut metrics = ImageMetrics::default(); metrics.set(0, 40, 10);
        let mut document = prepared.shape(&mut TestGlyphShaper::new()).unwrap().layout_with_metrics(constraints, &metrics);
        let old_y = document.layout_state.line_output.lines.last().unwrap().point.y;
        let before = document.layout_scratch.paragraph_reuse_counts();
        metrics.set(0, 40, 60);
        document.relayout_with_metrics(constraints, &metrics);
        let after = document.layout_scratch.paragraph_reuse_counts();
        assert!(after.0 >= before.0 + 2, "both unchanged text paragraphs must reuse local line layout");
        assert!(after.1 > before.1, "the changed image paragraph must be measured again");
        assert!(document.layout_state.line_output.lines.last().unwrap().point.y > old_y);
        let mut reference = document.clone();
        reference.relayout_with_metrics(constraints, &metrics);
        assert_eq!(paragraph_geometry_snapshot(&document), paragraph_geometry_snapshot(&reference));
    }

    #[test]
    fn paragraph_reuse_and_repaint_preserve_reading_exclusions_and_source_ranges() {
        let source = "<img src='top' style='display:block;width:40px'><section><h2>Contents</h2><p>Chapter One ... 1</p></section><p>Hello<br>world<a class='fnanchor' href='#fn1'>[1]</a> <span style='visibility:hidden'>secret </span>again.</p><div class='footnotes'><p id='fn1'>Note body.</p></div>";
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(source, None);
        let constraints = LayoutConstraints::new(160.0, 20.0).unwrap();
        let mut metrics = ImageMetrics::default();
        metrics.set(0, 40, 10);
        let mut document = prepared
            .clone()
            .shape(&mut TestGlyphShaper::new())
            .unwrap()
            .layout_with_metrics(constraints, &metrics);
        let reading_snapshot = |document: &LaidOutDocument| {
            document.render_view().addressing().reading_fragments(0)
                .into_iter()
                .map(|fragment| (fragment.text, fragment.target.element, fragment.target.glyph_ranges))
                .collect::<Vec<_>>()
        };
        let original = reading_snapshot(&document);
        assert_eq!(original.len(), 1);
        assert_eq!(original[0].0, "Hello world again.");
        let hits = document.layout_scratch.paragraph_reuse_counts().0;

        metrics.set(0, 40, 60);
        document.relayout_with_metrics(constraints, &metrics);
        assert!(document.layout_scratch.paragraph_reuse_counts().0 > hits);
        assert_eq!(reading_snapshot(&document), original);

        let painted = prepared.with_reader_paint_overrides(&ReaderStyleOverrides {
            foreground: Some(0xeeeeeeff),
            ..Default::default()
        }).unwrap();
        assert!(document.repaint_from(&painted));
        assert_eq!(reading_snapshot(&document), original);
        document.relayout_with_metrics(constraints, &metrics);
        assert_eq!(reading_snapshot(&document), original);
        assert!(document.repaint_from(&prepared));
        assert_eq!(reading_snapshot(&document), original);
    }

    #[test]
    fn translated_float_contexts_reuse_lines_but_changed_exclusion_heights_recompose() {
        let mut factory = DocumentFactory::new();
        let source = "<img src='top' style='display:block;width:40px'><section style='display:flow-root'><img src='float' style='float:left;width:60px'><p>one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen</p></section>";
        let constraints = LayoutConstraints::new(180.0, 20.0).unwrap();
        let mut metrics = ImageMetrics::default();metrics.set(0,40,10);metrics.set(1,60,70);
        let mut document = factory.parse_with_new_pipeline(source,None).shape(&mut TestGlyphShaper::new()).unwrap().layout_with_metrics(constraints,&metrics);
        let before = document.layout_scratch.paragraph_reuse_counts();
        metrics.set(0,40,50);
        document.relayout_with_metrics(constraints,&metrics);
        let translated = document.layout_scratch.paragraph_reuse_counts();
        assert!(translated.0 > before.0, "translating the whole context preserves its relative float profile");
        let mut reference = document.clone();reference.relayout_with_metrics(constraints,&metrics);
        assert_eq!(paragraph_geometry_snapshot(&document),paragraph_geometry_snapshot(&reference));
        metrics.set(1,60,150);
        document.relayout_with_metrics(constraints,&metrics);
        let changed = document.layout_scratch.paragraph_reuse_counts();
        assert!(changed.1 > translated.1,"changed float lanes must invalidate dependent line layout");
        let mut reference = document.clone();reference.relayout_with_metrics(constraints,&metrics);
        assert_eq!(paragraph_geometry_snapshot(&document),paragraph_geometry_snapshot(&reference));
    }

    #[test]
    fn retained_paragraph_layout_matches_uncached_tables_pseudos_overflow_and_width_changes() {
        let sources = [
            "<table style='width:100%'><tr><td><p>table text with <img src='x' style='width:40%'> a changing image</p></td><td>another cell with several words that wrap</td></tr></table>",
            "<p>before <span style='display:inline-block;width:50%'><table><tr><td>nested table words</td></tr></table></span> after <img src='x' style='width:30px'></p>",
            "<style>p::first-line{font-size:24px;line-height:1.4}p::first-letter{font-size:28px}p::before{content:'G'}</style><img src='x' style='display:block;width:20px'><p>one two three four five six seven eight nine ten</p>",
            "<p style='white-space:nowrap;overflow:hidden;text-overflow:ellipsis'>some text that overflows</p><p style='white-space:pre-wrap'>a\tb c\nnext line</p>",
        ];
        for source in sources {
            let mut factory=DocumentFactory::new();let mut shaper=TestGlyphShaper::new();let mut metrics=ImageMetrics::default();metrics.set(0,40,30);
            let initial=LayoutConstraints::new(180.0,20.0).unwrap();
            let mut document=factory.parse_with_new_pipeline(source,None).shape(&mut shaper).unwrap().layout_with_metrics_and_shaper(initial,&metrics,&mut shaper).unwrap();
            let before=document.layout_scratch.paragraph_reuse_counts();
            for (width,height,policy) in [(180.0,60,TextCompositionPolicy::WebCompatible),(120.0,60,TextCompositionPolicy::BookOptimized),(180.0,60,TextCompositionPolicy::WebCompatible),(180.0,60,TextCompositionPolicy::WebCompatible)] {
                let constraints=LayoutConstraints::new(width,20.0).unwrap().with_text_composition_policy(policy);
                metrics.set(0,40,height);
                document.relayout_with_metrics_and_shaper(constraints,&metrics,&mut shaper).unwrap();
                let mut reference=document.clone();reference.layout_scratch.clear_paragraph_layouts();
                reference.relayout_with_metrics_and_shaper(constraints,&metrics,&mut shaper).unwrap();
                assert_eq!(paragraph_geometry_snapshot(&document),paragraph_geometry_snapshot(&reference),"{source}");
            }
            assert!(document.layout_scratch.paragraph_reuse_counts().0>before.0,"repeated dependencies must reuse paragraph results: {source}");
        }
    }

    #[test]
    fn intrinsic_table_probes_do_not_displace_published_paragraph_results() {
        let mut factory=DocumentFactory::new();
        let source="<img src='top' style='display:block;width:40px'><table style='width:160px'><tr><td><p id='cell'>some words that wrap inside this table cell</p></td></tr></table>";
        let constraints=LayoutConstraints::new(200.0,20.0).unwrap();
        let mut metrics=ImageMetrics::default();metrics.set(0,40,10);
        let mut document=factory.parse_with_new_pipeline(source,None).shape(&mut TestGlyphShaper::new()).unwrap().layout_with_metrics(constraints,&metrics);
        let owner=(0..document.box_count()).find(|&index| document.inputs.layout_tree.get_box_dom_element(index).and_then(|raw| document.inputs.document.node_id_from_raw(raw)).is_some_and(|node| document.inputs.document.get_dom_id(node)==Some("cell"))).unwrap();
        let before=document.layout_scratch.paragraph_published_hits(owner);
        metrics.set(0,40,60);
        document.relayout_with_metrics(constraints,&metrics);
        assert!(document.layout_scratch.paragraph_published_hits(owner)>before,"measurement widths must not evict the cell's actual paragraph layout");
        let mut reference=document.clone();reference.relayout_with_metrics(constraints,&metrics);
        assert_eq!(paragraph_geometry_snapshot(&document),paragraph_geometry_snapshot(&reference));
    }

    #[test]
    fn composition_policy_switches_match_fresh_shaping_and_layout() {
        let mut factory=DocumentFactory::new();
        let prepared=factory.parse_with_new_pipeline("<p>First sentence. Second sentence. Third sentence.</p>",None);
        let base=LayoutConstraints::new(600.0,20.0).unwrap();
        let mut document=prepared.clone().shape(&mut TestGlyphShaper::new()).unwrap().layout(base);
        for policy in [TextCompositionPolicy::BookOptimized,TextCompositionPolicy::SentencePerLine,TextCompositionPolicy::BookOptimizedUnrestrictedHyphenation,TextCompositionPolicy::WebCompatible,TextCompositionPolicy::SentencePerLine] {
            let constraints=base.with_text_composition_policy(policy);
            document.relayout(constraints);
            let reference=prepared.clone().shape(&mut TestGlyphShaper::new()).unwrap().layout(constraints);
            assert_eq!(paragraph_geometry_snapshot(&document),paragraph_geometry_snapshot(&reference),"{policy:?}");
            if policy==TextCompositionPolicy::SentencePerLine { assert!(document.line_count()>1); }
        }
    }

    #[test]
    fn paragraph_result_retention_stays_bounded_across_many_width_variants() {
        let paragraph = format!("<p>{}</p>", "one two three four five six seven eight ".repeat(12));
        let source = paragraph.repeat(1000);
        let mut factory = DocumentFactory::new();
        let mut document = factory.parse_with_new_pipeline(&source, None).shape(&mut TestGlyphShaper::new()).unwrap().layout(LayoutConstraints::new(120.0,20.0).unwrap());
        for width in [160.0,120.0,200.0,120.0] {
            document.relayout(LayoutConstraints::new(width,20.0).unwrap());
            assert!(document.layout_scratch.paragraph_layout_bytes() < 5 * 1024 * 1024,
                "retained results have a 4 MiB budget plus bounded slot metadata");
        }
        let mut reference=document.clone();reference.relayout(document.last_constraints);
        assert_eq!(document.line_count(),reference.line_count());
        assert_eq!(document.layout_state.line_output.lines.last().unwrap().point,reference.layout_state.line_output.lines.last().unwrap().point);
    }

    #[test]
    fn resizing_switches_glyph_line_width_without_losing_ownership_or_snapshots() {
        const ROWS: usize = 32_800;
        let content = "xx\n".repeat(ROWS - 1) + "xx";
        let html = format!("<pre>{content}</pre>");
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(&html, Some("html,body,pre {margin:0} pre {font-size:10px;white-space:pre-wrap;overflow-wrap:anywhere}"));
        let mut shaper = TestGlyphShaper::new();
        let shaped = prepared.shape(&mut shaper).unwrap();
        let mut document = shaped.layout(LayoutConstraints::new(12.0, 16.0).unwrap());
        let last = document.render_view().text().glyph_count() as u32 - 1;
        assert_eq!(document.line_count(), ROWS);
        assert!(document.layout_state.line_output.glyph_line_indices.is_narrow());
        assert_eq!(document.render_view().text().line_index_for_glyph(last), Some(ROWS - 1));
        let snapshot = document.clone();
        let glyphs = document.glyphs().as_ptr();
        document.relayout_with_metrics(LayoutConstraints::new(6.0, 16.0).unwrap(), &ImageMetrics::default());
        assert_eq!(document.line_count(), ROWS * 2);
        assert!(!document.layout_state.line_output.glyph_line_indices.is_narrow());
        assert_eq!(document.render_view().text().line_index_for_glyph(last), Some(ROWS * 2 - 1));
        assert_eq!(document.glyphs().as_ptr(), glyphs);
        assert!(snapshot.layout_state.line_output.glyph_line_indices.is_narrow());
        assert_eq!(snapshot.render_view().text().line_index_for_glyph(last), Some(ROWS - 1));
        document.relayout_with_metrics(LayoutConstraints::new(12.0, 16.0).unwrap(), &ImageMetrics::default());
        assert_eq!(document.line_count(), ROWS);
        assert!(document.layout_state.line_output.glyph_line_indices.is_narrow());
        assert_eq!(document.render_view().text().line_index_for_glyph(last), Some(ROWS - 1));
    }


    #[test]
    fn compact_source_offsets_preserve_collapsed_spaces_and_unicode_expansion() {
        let mut factory = DocumentFactory::new();
        let prepared =
            factory.parse_with_new_pipeline("<p>a   😀ß</p>", Some("p {text-transform:uppercase}"));
        let document = prepared
            .shape(&mut TestGlyphShaper::new())
            .unwrap()
            .layout(LayoutConstraints::new(600.0, 20.0).unwrap());
        let addressing = document.render_view().addressing();
        let offsets = [0, 1, 4, 6, 6];
        let after_offsets = [1, 2, 6, 7, 7];
        assert_eq!(document.glyphs().len(), offsets.len());
        for (glyph, &offset) in offsets.iter().enumerate() {
            assert_eq!(document.glyph_source_offset(glyph as u32), Some(offset));
            let position = addressing
                .source_position_for_glyph(glyph as u32, false)
                .unwrap();
            assert_eq!(position.utf16_offset(), offset as usize);
            let elements: Vec<_> = position.elements().map(|element| element.step()).collect();
            assert_eq!(
                addressing.resolve_source_position(
                    &elements,
                    Some(position.text_step()),
                    Some(position.utf16_offset()),
                    false
                ),
                Some(glyph.min(3) as u32)
            );
            assert_eq!(
                addressing
                    .source_position_for_glyph(glyph as u32, true)
                    .unwrap()
                    .utf16_offset(),
                after_offsets[glyph]
            );
        }
    }

    #[test]
    fn source_offsets_above_u16_preserve_reading_positions() {
        let html = format!("<p>{}😀Z</p>", "a".repeat(65_535));
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(&html, None);
        let document = prepared
            .shape(&mut TestGlyphShaper::new())
            .unwrap()
            .layout(LayoutConstraints::new(600.0, 20.0).unwrap());
        let addressing = document.render_view().addressing();
        for (glyph, offset, after) in [
            (65_534, 65_534, 65_535),
            (65_535, 65_535, 65_537),
            (65_536, 65_537, 65_538),
        ] {
            assert_eq!(document.glyph_source_offset(glyph), Some(offset));
            let position = addressing.source_position_for_glyph(glyph, false).unwrap();
            assert_eq!(position.utf16_offset(), offset as usize);
            let elements: Vec<_> = position.elements().map(|element| element.step()).collect();
            assert_eq!(
                addressing.resolve_source_position(
                    &elements,
                    Some(position.text_step()),
                    Some(position.utf16_offset()),
                    false
                ),
                Some(glyph)
            );
            assert_eq!(
                addressing
                    .source_position_for_glyph(glyph, true)
                    .unwrap()
                    .utf16_offset(),
                after
            );
        }
    }

    #[test]
    fn narrow_node_offsets_support_cfi_text_chunks_larger_than_u16() {
        let html = format!(
            "<p>{}<!--split-->{}</p>",
            "a".repeat(40_000),
            "b".repeat(40_000)
        );
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(&html, None);
        let document = prepared
            .shape(&mut TestGlyphShaper::new())
            .unwrap()
            .layout(LayoutConstraints::new(600.0, 20.0).unwrap());
        let addressing = document.render_view().addressing();
        assert_eq!(document.glyph_source_offset(79_999), Some(39_999));
        let position = addressing.source_position_for_glyph(79_999, false).unwrap();
        assert_eq!(position.utf16_offset(), 79_999);
        let elements: Vec<_> = position.elements().map(|element| element.step()).collect();
        assert_eq!(
            addressing.resolve_source_position(
                &elements,
                Some(position.text_step()),
                Some(position.utf16_offset()),
                false
            ),
            Some(79_999)
        );
    }

    #[test]
    fn compact_source_offsets_keep_generated_markers_without_source_positions() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(
            "<ol><li>😀ß</li></ol>",
            Some("li {display:list-item;list-style-type:decimal;text-transform:uppercase}"),
        );
        let document = prepared
            .shape(&mut TestGlyphShaper::new())
            .unwrap()
            .layout(LayoutConstraints::new(600.0, 20.0).unwrap());
        let text = document.render_view().text();
        let addressing = document.render_view().addressing();
        let mut marker_count = 0;
        let mut source_offsets = Vec::new();
        for run in text.text_runs().chain(text.marker_runs()) {
            for glyph in run.glyphs() {
                if run.is_marker() {
                    marker_count += 1;
                    assert_eq!(document.glyph_source_offset(glyph), None);
                    assert_eq!(addressing.source_position_for_glyph(glyph, false), None);
                } else {
                    source_offsets.push(document.glyph_source_offset(glyph).unwrap());
                }
            }
        }
        assert!(marker_count > 0);
        assert_eq!(source_offsets, [0, 2, 2]);
    }

    #[test]
    fn prepared_logical_text_survives_replacing_font_resources() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(
            "<p>  ß😀  a</p>",
            Some("p {text-transform:uppercase} p::first-letter {font-size:32px}"),
        );
        let text = prepared.inputs.text.clone();
        let characters: String = text.content().glyphs().iter().copied().filter_map(char::from_u32).collect();
        assert_eq!(characters, "SS😀 A");
        assert!(text.first_letter_styles().style_at(0).is_some());
        assert_eq!(text.first_letter_styles().style_at(0), text.first_letter_styles().style_at(1));
        assert!(text.first_letter_styles().style_at(2).is_none());
        let mut first_generation = None;
        for _ in 0..2 {
            let mut shaper = TestGlyphShaper::new();
            let shaped = prepared.shape(&mut shaper).unwrap();
            assert!(Arc::ptr_eq(&text, &shaped.inputs.text));
            let generation = shaper.glyph_resources().generation();
            if let Some(previous) = first_generation.as_ref() { assert_ne!(previous, &generation); }
            first_generation = Some(generation);
            let metrics = &shaped.shaped.glyph_metrics;
            let glyphs = shaped.glyphs();
            assert_eq!(metrics.get(glyphs[0]).advance(), metrics.get(glyphs[1]).advance());
            assert!(metrics.get(glyphs[0]).advance() > metrics.get(glyphs[2]).advance());
            let mut document = shaped.layout(LayoutConstraints::new(600.0, 20.0).unwrap());
            document.relayout(LayoutConstraints::new(50.0, 20.0).unwrap());
            assert!(Arc::ptr_eq(&text, &document.inputs.text));
            let offsets: Vec<_> = (0..5).map(|glyph| document.glyph_source_offset(glyph)).collect();
            assert_eq!(offsets, [Some(2), Some(2), Some(3), Some(5), Some(7)]);
        }
    }

    #[test]
    fn prepared_first_letter_storage_does_not_grow_with_paragraph_length() {
        let mut bytes = Vec::new();
        for length in [10, 40_000] {
            let mut factory = DocumentFactory::new();
            let prepared = factory.parse_with_new_pipeline(
                &format!("<p>{}</p>", "a".repeat(length)),
                Some("p::first-letter {font-size:32px}"),
            );
            let report = prepared.memory_usage_report();
            let storage = report.entries.iter().find(|entry| entry.label.ends_with("PreparedText.first_letter_styles.storage")).unwrap();
            assert!(storage.bytes > 0 && storage.bytes <= 128);
            bytes.push(storage.bytes);
        }
        assert_eq!(bytes[0], bytes[1]);
    }

    #[test]
    fn paint_variants_share_logical_text_and_retarget_sparse_pseudo_styles() {
        for generated in [false, true] {
            let mut factory = DocumentFactory::new();
            let css = format!("p {{text-transform:uppercase}} p::first-letter {{font-size:32px;color:red}} {}", if generated {"p::before {content:'ß'}"} else {""});
            let prepared = factory.parse_with_new_pipeline("<p>ßa</p>", Some(&css));
            let themed = prepared.with_reader_paint_overrides(&crate::ReaderStyleOverrides {
                foreground: Some(0xddddddff), ..Default::default()
            }).unwrap();
            assert!(std::ptr::eq(prepared.inputs.text.content(), themed.inputs.text.content()));
            let first = themed.inputs.text.first_letter_styles().style_at(0).unwrap();
            assert!(themed.inputs.styles.view(first).is_some(), "cached pseudo style belongs to the new paint store");
            assert_eq!(Some(first), themed.inputs.text.first_letter_styles().style_at(1));
            let shaped = themed.shape(&mut TestGlyphShaper::new()).unwrap();
            let metrics = &shaped.shaped.glyph_metrics;
            assert_eq!(metrics.get(shaped.glyphs()[0]).advance(), metrics.get(shaped.glyphs()[1]).advance());
        }
    }

    #[test]
    fn first_line_refinement_retains_prepared_first_letter_selection() {
        for (source, second_offset) in [("<p>ß word</p>", 0), ("<p>“Apple” word</p>", 1)] {
            let mut factory = DocumentFactory::new();
            let prepared = factory.parse_with_new_pipeline(source, Some("p{text-transform:uppercase}p::first-letter{font-size:32px;color:purple}p::first-line{font-size:20px;color:blue}"));
            let mut shaper = TestGlyphShaper::new();
            let mut document = prepared.shape(&mut shaper).unwrap().layout_with_metrics_and_shaper(LayoutConstraints::new(400.0,20.0).unwrap(), &ImageMetrics::default(), &mut shaper).unwrap();
            let glyphs = document.glyphs();
            assert_eq!(document.glyph_metrics().get(glyphs[0]).advance(), document.glyph_metrics().get(glyphs[1]).advance());
            assert!(document.glyph_metrics().get(glyphs[0]).advance() > document.glyph_metrics().get(glyphs[2]).advance());
            let theme = crate::ThemeAdaptation {ink:0xeeeeeeff,surface:0x222222ff};
            let painted = prepared.with_reader_paint_overrides(&crate::ReaderStyleOverrides {theme:Some(theme),..Default::default()}).unwrap();
            assert!(document.repaint_from(&painted));
            let text = document.render_view().text();
            assert_eq!(text.paint_color_at(0), Some(theme.adapt(0x800080ff)));
            assert_eq!(text.paint_color_at(1), Some(theme.adapt(0x800080ff)));
            assert_eq!(document.glyph_source_offset(0),Some(0));
            assert_eq!(document.glyph_source_offset(1),Some(second_offset));
        }
    }

    #[derive(Default)]
    struct InvalidGlyphShaper(crate::GlyphResourceStore);

    impl GlyphShaper for InvalidGlyphShaper {
        fn reset(&mut self) {}

        fn glyph_resources(&mut self) -> &mut crate::GlyphResourceStore { &mut self.0 }

        fn shape_glyph(&mut self, _ch: char, _font_size: f32, _font_weight: u16, _font_slant: crate::FontSlant, _color: u32, _family: Option<&str>) -> Result<GlyphId, ShapeError> {
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
        let error = prepared.shape(&mut InvalidGlyphShaper::default()).err().expect("invalid shaper must be rejected");

        assert_eq!(error.glyph_id(), Some(17));
        assert_eq!(error.metrics_len(), Some(0));
    }

    #[derive(Default)]
    struct InvalidMetricShaper(crate::GlyphResourceStore);

    impl GlyphShaper for InvalidMetricShaper {
        fn reset(&mut self) {}

        fn glyph_resources(&mut self) -> &mut crate::GlyphResourceStore { &mut self.0 }

        fn shape_glyph(&mut self, ch: char, font_size: f32, _font_weight: u16, _font_slant: crate::FontSlant, _color: u32, _family: Option<&str>) -> Result<GlyphId, ShapeError> {
            let metric = GlyphMetric::try_new(ch, font_size, -font_size, font_size * 0.25, font_size * 0.75).map_err(ShapeError::rejected_metric)?;
            self.0.register(metric)
        }
    }

    #[test]
    fn shaping_rejects_invalid_glyph_metrics() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline("<html><body>text</body></html>", None);
        let error = prepared.shape(&mut InvalidMetricShaper::default()).err().expect("invalid metric must be rejected");
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
    fn inline_descendants_and_empty_blocks_do_not_reserve_paragraph_plans() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline("<p>words <em>emphasis</em> <span>span</span> <a href='#a'>link</a></p><div></div><p></p>", None);
        let mut shaper = TestGlyphShaper::new();
        let mut document = prepared.shape(&mut shaper).unwrap().layout(LayoutConstraints::new(120.0, 16.0).unwrap());
        let (boxes, reserved, filled) = document.shaped.inline_plans.storage_counts();
        assert!(boxes > reserved);
        assert_eq!((reserved, filled), (1, 1));
        let glyph_pointer = document.glyphs().as_ptr();
        document.relayout(LayoutConstraints::new(100.0, 16.0).unwrap());
        assert_eq!(document.shaped.inline_plans.storage_counts(), (boxes, reserved, filled));
        assert_eq!(document.glyphs().as_ptr(), glyph_pointer);
    }

    #[test]
    fn ordinary_lines_omit_optional_storage_and_preserve_snapshot_geometry() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline("<p>plain text continues through several lines without inline elements or special paint data</p>", None);
        let mut shaper = TestGlyphShaper::new();
        let mut document = prepared.shape(&mut shaper).unwrap().layout(LayoutConstraints::new(100.0, 16.0).unwrap());
        assert!(document.layout_state.line_output.lines.len() > 1);
        assert!(document.layout_state.line_output.lines.iter().all(|line| line.details.is_none()));
        assert!(document.layout_state.line_output.line_details.is_empty());
        assert!(document.layout_state.line_output.text_fragments.is_empty());
        assert!(std::mem::size_of::<crate::layout_model::Line>() <= 80);
        let snapshot = document.clone();
        let original = snapshot.render_view().text().line(1).unwrap();
        document.relayout(LayoutConstraints::new(200.0, 16.0).unwrap());
        assert_eq!(snapshot.render_view().text().line(1).unwrap().point(), original.point());
        assert_eq!(snapshot.render_view().text().line(1).unwrap().glyphs(), original.glyphs());
    }

    #[test]
    fn paragraph_scratch_is_retained_between_layouts_and_not_copied_to_snapshots() {
        let mut factory = DocumentFactory::new();
        let source = format!("<p>{}</p><p>second paragraph</p>", "Words continue through this paragraph with punctuation and possible hyphenation. ".repeat(20));
        let prepared = factory.parse_with_new_pipeline(&source, None);
        let mut shaper = TestGlyphShaper::new();
        let constraints = LayoutConstraints::new(180.0, 16.0).unwrap().with_text_composition_policy(TextCompositionPolicy::BookOptimized);
        let mut document = prepared.shape(&mut shaper).unwrap().layout(constraints);
        let capacities = document.layout_scratch.paragraph_capacities();
        assert_eq!(capacities.len(), 1);
        assert!(capacities[0].0 > 0 && capacities[0].1.0 > 0 && capacities[0].1.1 > 0 && capacities[0].1.2 > 0);
        document.relayout(constraints);
        assert_eq!(document.layout_scratch.paragraph_capacities(), capacities);
        assert!(document.clone().layout_scratch.paragraph_capacities().is_empty());
    }

    #[test]
    fn shaper_relayout_reuses_buffers_and_matches_fresh_layout() {
        for policy in [
            TextCompositionPolicy::WebCompatible,
            TextCompositionPolicy::BookOptimized,
        ] {
            let mut factory = DocumentFactory::new();
            let prepared = factory.parse_with_new_pipeline(
                "<html><body><p id='first'>one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen</p><p id='second'>another paragraph with an <em>inline span</em> and more words</p></body></html>",
                Some("p::first-letter { font-size:2em }"),
            );
            let mut shaper = TestGlyphShaper::new();
            let shaped = prepared.shape(&mut shaper).unwrap();
            let constraints = LayoutConstraints::new(120.0, 20.0)
                .unwrap()
                .with_text_composition_policy(policy);
            let metrics = ImageMetrics::default();
            let mut document = shaped
                .clone()
                .layout_with_metrics_and_shaper(constraints, &metrics, &mut shaper)
                .unwrap();
            let lines = document.layout_state.line_output.lines.as_ptr();
            let offsets = document
                .layout_state
                .line_output
                .line_glyph_offsets
                .as_ptr();
            let advances = document
                .layout_state
                .line_output
                .line_glyph_advances
                .as_ptr();
            let glyph_lines = document
                .layout_state
                .line_output
                .glyph_line_indices
                .as_ptr();
            let scratch = document.layout_scratch.allocation_capacities();
            let glyphs = document.glyphs().as_ptr();

            for width in [240.0, 180.0, 300.0, 120.0] {
                let constraints = LayoutConstraints::new(width, 20.0)
                    .unwrap()
                    .with_text_composition_policy(policy);
                document
                    .relayout_with_metrics_and_shaper(constraints, &metrics, &mut shaper)
                    .unwrap();
                let fresh = shaped.clone().layout(constraints);
                assert_eq!(document.glyphs().as_ptr(), glyphs);
                assert_eq!(
                    document.layout_state.line_output.lines.as_ptr(),
                    lines,
                    "resize must recycle line storage"
                );
                assert_eq!(
                    document
                        .layout_state
                        .line_output
                        .line_glyph_offsets
                        .as_ptr(),
                    offsets
                );
                assert_eq!(
                    document
                        .layout_state
                        .line_output
                        .line_glyph_advances
                        .as_ptr(),
                    advances
                );
                assert_eq!(
                    document
                        .layout_state
                        .line_output
                        .glyph_line_indices
                        .as_ptr(),
                    glyph_lines
                );
                let retained = document.layout_scratch.allocation_capacities();
                assert_eq!(retained.0.0 + retained.1.2, scratch.0.0 + scratch.1.2);
                assert_eq!(retained.1.0, scratch.1.0);
                assert_eq!(retained.1.1, scratch.1.1);
                assert_eq!(document.last_constraints, constraints);
                let lines = |doc: &LaidOutDocument| {
                    doc.render_view()
                        .text()
                        .lines()
                        .iter()
                        .map(|line| (line.glyphs(), line.point(), line.height(), line.baseline()))
                        .collect::<Vec<_>>()
                };
                assert_eq!(lines(&document), lines(&fresh));
                for index in 0..document.box_count() {
                    assert_eq!(document.geometry.point(index), fresh.geometry.point(index));
                    assert_eq!(document.geometry.size(index), fresh.geometry.size(index));
                }
                for (id, position) in document
                    .render_view()
                    .addressing()
                    .anchor_positions()
                    .iter()
                {
                    let expected = fresh
                        .render_view()
                        .addressing()
                        .anchor_position(id)
                        .unwrap();
                    assert_eq!(
                        (position.y(), position.order()),
                        (expected.y(), expected.order())
                    );
                }
            }
        }
    }

    #[test]
    fn shaper_relayout_preserves_auxiliary_glyphs_and_retained_snapshots() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(
            "<html><body><p>one two three four five six seven eight nine ten</p></body></html>",
            None,
        );
        let mut shaper = TestGlyphShaper::new();
        let mut document = prepared
            .shape(&mut shaper)
            .unwrap()
            .layout(LayoutConstraints::new(120.0, 20.0).unwrap());
        let snapshot = document.clone();
        let old_lines = snapshot
            .render_view()
            .text()
            .lines()
            .iter()
            .map(|line| (line.glyphs(), line.point()))
            .collect::<Vec<_>>();
        let metric = GlyphMetric::try_new('#', 4.0, 3.0, 1.0, 0.0).unwrap();
        let appended = shaper.glyph_resources().register(metric).unwrap();
        assert!(
            snapshot
                .render_view()
                .text()
                .glyph_metric(appended)
                .is_none()
        );

        document
            .relayout_with_metrics_and_shaper(
                LayoutConstraints::new(240.0, 20.0).unwrap(),
                &ImageMetrics::default(),
                &mut shaper,
            )
            .unwrap();

        assert_eq!(
            document.render_view().text().glyph_metric(appended),
            Some(metric)
        );
        assert!(
            snapshot
                .render_view()
                .text()
                .glyph_metric(appended)
                .is_none()
        );
        assert_eq!(
            snapshot
                .render_view()
                .text()
                .lines()
                .iter()
                .map(|line| (line.glyphs(), line.point()))
                .collect::<Vec<_>>(),
            old_lines
        );
        assert!(
            !Arc::ptr_eq(&snapshot.placement, &document.placement),
            "retained placement snapshots must not be mutated by resize"
        );
        let shaped = document.shaped.clone();
        document
            .relayout_with_metrics_and_shaper(
                LayoutConstraints::new(120.0, 20.0).unwrap(),
                &ImageMetrics::default(),
                &mut shaper,
            )
            .unwrap();
        assert!(
            Arc::ptr_eq(&document.shaped, &shaped),
            "unchanged glyph resources must not copy the shaped chapter again"
        );
        assert_eq!(
            document.render_view().text().glyph_metric(appended),
            Some(metric)
        );
    }

    #[test]
    fn shaper_relayout_rejects_a_stale_generation_before_mutating_layout() {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(
            "<html><body><p>one two three four five six</p></body></html>",
            None,
        );
        let mut shaper = TestGlyphShaper::new();
        let constraints = LayoutConstraints::new(120.0, 20.0).unwrap();
        let mut document = prepared.shape(&mut shaper).unwrap().layout(constraints);
        let lines = document.layout_state.line_output.lines.as_ptr();
        let before = document
            .render_view()
            .text()
            .lines()
            .iter()
            .map(|line| (line.glyphs(), line.point()))
            .collect::<Vec<_>>();
        shaper.reset();

        let error = document
            .relayout_with_metrics_and_shaper(
                LayoutConstraints::new(240.0, 20.0).unwrap(),
                &ImageMetrics::default(),
                &mut shaper,
            )
            .unwrap_err();

        assert_eq!(error, ShapeError::resource_generation_mismatch());
        assert_eq!(document.last_constraints, constraints);
        assert_eq!(document.layout_state.line_output.lines.as_ptr(), lines);
        assert_eq!(
            document
                .render_view()
                .text()
                .lines()
                .iter()
                .map(|line| (line.glyphs(), line.point()))
                .collect::<Vec<_>>(),
            before
        );
    }

    #[test]
    fn failed_first_line_append_keeps_the_published_layout() {
        struct RejectAppend<'a>(&'a mut TestGlyphShaper);
        impl GlyphShaper for RejectAppend<'_> {
            fn reset(&mut self) {
                self.0.reset();
            }
            fn glyph_resources(&mut self) -> &mut GlyphResourceStore {
                self.0.glyph_resources()
            }
            fn shape_glyph(
                &mut self,
                ch: char,
                size: f32,
                weight: u16,
                slant: FontSlant,
                color: u32,
                family: Option<&str>,
            ) -> Result<GlyphId, ShapeError> {
                self.0.shape_glyph(ch, size, weight, slant, color, family)
            }
        }
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(
            "<html><body><p>one two three four five six</p></body></html>",
            Some("p::first-line {font-size:24px}"),
        );
        let mut shaper = TestGlyphShaper::new();
        let constraints = LayoutConstraints::new(120.0, 20.0).unwrap();
        let mut document = prepared.shape(&mut shaper).unwrap().layout(constraints);
        let before = document
            .render_view()
            .text()
            .lines()
            .iter()
            .map(|line| (line.glyphs(), line.point()))
            .collect::<Vec<_>>();
        let lines = document.layout_state.line_output.lines.as_ptr();
        let metrics = shaper.glyph_resources().len();

        let error = document
            .relayout_with_metrics_and_shaper(
                LayoutConstraints::new(240.0, 20.0).unwrap(),
                &ImageMetrics::default(),
                &mut RejectAppend(&mut shaper),
            )
            .unwrap_err();

        assert_eq!(error, ShapeError::append_transaction_unsupported());
        assert_eq!(document.last_constraints, constraints);
        assert_eq!(document.layout_state.line_output.lines.as_ptr(), lines);
        assert_eq!(
            document
                .render_view()
                .text()
                .lines()
                .iter()
                .map(|line| (line.glyphs(), line.point()))
                .collect::<Vec<_>>(),
            before
        );
        assert_eq!(shaper.glyph_resources().len(), metrics);
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
        let appended = shaper.glyph_resources().register(metric).expect("append auxiliary glyph");
        document.refresh_glyph_metrics_from(&mut shaper).expect("same resource generation");

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
