#[cfg(test)]
mod tests {
    use super::{BoxFontMetrics, CharacterPlacement, FontSlant, RequiredFontMetrics, ShapedFontMetrics, ShapedLine, ShapedTextGeometry, ShapedTextRun, TextRunShapeRequest, TextShapeRequest, TextStyleSpan, collapse_whitespace, first_letter_style_overrides, is_css_collapsible_space, plan_shaping_spans, transform_text};
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
