#[cfg(test)]
mod tests {
    use super::MatchedRule;
    use crate::document::{BorderCollapseMode, BorderStyle, CaptionSide, Clear, Display, Document, ElementRef, EmptyCellsMode, Float, FontRelativeLength, LengthPct, PositionMode, PreferredSize, TableLayoutMode, TextAlign, TextDirection, WhiteSpace};
    use crate::parser::DocumentFactory;

    #[test]
    fn matched_rule_hot_path_record_stays_compact() {
        assert!(std::mem::size_of::<MatchedRule>() <= 12);
    }

    #[test]
    fn inline_style_attribute_overrides_computed_style() {
        let html = "<html><body><p style=\"display: inline; color: red;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.box_model_style(style).expect("validated style handle").display, Display::Inline);
        assert_eq!(document.text_style(style).expect("validated style handle").color, 0xFF0000FF);
    }

    #[test]
    fn has_selector_participates_in_candidate_lookup_and_cascade() {
        let html = "<html><body><div id='match'><span></span></div><div id='miss'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("div { background: blue } :has(> span) { background: green }"));
        let background_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).expect("element by id");
            document.background_style(document.style_for_node(node).expect("computed style")).expect("background").background_color
        };

        assert_eq!(background_for("match"), 0x008000FF);
        assert_eq!(background_for("miss"), 0x0000FFFF);
    }

    #[test]
    fn revert_layer_restores_the_previous_normal_layer() {
        let html = "<html><body><div id='target'></div></body></html>";
        let css = "@layer base, theme; @layer base { #target { background-color: green } } @layer theme { #target { background-color: red; background-color: revert-layer } }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn revert_restores_the_previous_cascade_origin() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { display: inline; display: revert }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box style").display, Display::Block);
    }

    #[test]
    fn inline_revert_discards_stylesheet_declarations_from_the_author_origin() {
        let html = "<html><body><div id='target' style='display: revert'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { display: inline }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box style").display, Display::Block);
    }

    #[test]
    fn important_revert_uses_the_normal_previous_origin_boundary() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { display: inline !important; display: revert !important }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box style").display, Display::Block);
    }

    #[test]
    fn custom_property_revert_restores_the_inherited_origin_value() {
        let html = "<html><body style='--tone: green'><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { --tone: red; --tone: revert; background-color: var(--tone) }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn self_referential_custom_property_is_invalid_even_with_an_inner_fallback() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { --tone: var(--tone, red); background-color: var(--tone, green) }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn every_member_of_a_multi_property_cycle_becomes_invalid() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { --a: var(--b); --b: var(--a); background-color: var(--a, green) }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn acyclic_dependent_can_fallback_from_a_cyclic_property() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { --a: var(--a); --b: var(--a, green); background-color: var(--b) }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn child_declarations_do_not_revive_an_invalid_inherited_custom_property() {
        let html = "<html><body id='parent'><div id='target'></div></body></html>";
        let css = "#parent { --tone: var(--missing) } #target { --missing: red; background-color: var(--tone, green) }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn custom_property_cycle_and_inheritance_wpt_cases_resolve_to_green() {
        let html = "<html><body><p id='multi'></p><section><p id='invalid-inherited'></p></section><article><p id='resolved-inherited'></p></article></body></html>";
        let css = "
            body { color: green }
            #multi { color: crimson; --a: red var(--b); --b: var(--c); --c: var(--d); --d: var(--e); --e: var(--a); --f: var(--e); color: var(--f) }
            section { --c: var(--missing) }
            #invalid-inherited { color: red; --a: var(--b); --b: var(--c, green); color: var(--a) }
            article { --c: var(--missing, green) }
            #resolved-inherited { color: red; --a: var(--b); --b: var(--c, crimson); color: var(--a) }
        ";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        for id in ["multi", "invalid-inherited", "resolved-inherited"] {
            let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).expect("target");
            let style = document.style_for_node(node).expect("computed style");
            assert_eq!(document.text_style(style).expect("text style").color, 0x008000FF, "{id}");
        }
    }

    #[test]
    fn substituted_inherit_is_finalized_before_dependent_custom_properties() {
        let html = "<html><body id='parent'><div id='target'></div></body></html>";
        let css = "#parent { --a: green } #target { --a: var(--missing, inherit); --b: var(--a); background-color: var(--b) }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn var_substitution_preserves_boundary_before_an_adjacent_identifier() {
        let html = "<html><body><p id='target'></p></body></html>";
        let css = "body { color: green } #target { color: red; --a: var(--b)red; --b: orange; color: var(--a) }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.text_style(style).expect("text style").color, 0x008000FF);
    }

    #[test]
    fn adjacent_var_substitutions_remain_distinct_tokens() {
        let html = "<html><body><p id='target'></p></body></html>";
        let css = "body { color: green } #target { color: red; --a: orange; --b: red; color: var(--a)var(--b) }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.text_style(style).expect("text style").color, 0x008000FF);
    }

    #[test]
    fn all_revert_restores_every_property_from_the_previous_origin() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { display: inline; width: 200px; background-color: red; all: revert }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");
        let box_style = document.box_model_style(style).expect("box style");

        assert_eq!(box_style.display, Display::Block);
        assert_eq!(box_style.width, PreferredSize::Auto);
        assert_eq!(document.background_style(style).expect("background").background_color, 0x00000000);
    }

    #[test]
    fn all_initial_resets_supported_properties_but_preserves_direction() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { direction: rtl; display: block; width: 200px; color: red; background-color: red; all: initial }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");
        let box_style = document.box_model_style(style).expect("box style");

        assert_eq!(box_style.display, Display::Inline);
        assert_eq!(box_style.width, PreferredSize::Auto);
        assert_eq!(document.text_style(style).expect("text style").direction, TextDirection::Rtl);
        assert_eq!(document.text_style(style).expect("text style").color, 0x000000FF);
        assert_eq!(document.background_style(style).expect("background").background_color, 0x00000000);
    }

    #[test]
    fn all_inherit_copies_reset_and_inherited_properties_from_the_parent() {
        let html = "<html><body><div id='parent'><div id='target'></div></div></body></html>";
        let css = "#parent { display: inline; width: 123px; color: green; background-color: green } #target { direction: rtl; all: inherit }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");
        let box_style = document.box_model_style(style).expect("box style");

        assert_eq!(box_style.display, Display::Inline);
        assert_eq!(box_style.width, PreferredSize::Px(123.0));
        assert_eq!(document.text_style(style).expect("text style").direction, TextDirection::Rtl);
        assert_eq!(document.text_style(style).expect("text style").color, 0x008000FF);
        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn all_inherit_uses_the_dom_parent_across_inline_block_nesting() {
        let html = "<html><body><span id='parent'><div id='target'></div></span></body></html>";
        let css = "#parent { display: inline } #target { all: inherit }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box style").display, Display::Inline);
    }

    #[test]
    fn all_unset_inherits_inherited_properties_and_initializes_reset_properties() {
        let html = "<html><body><div id='parent'><div id='target'></div></div></body></html>";
        let css = "#parent { color: green; font-size: 20px; background-color: green } #target { display: block; width: 123px; background-color: red; all: unset }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");
        let box_style = document.box_model_style(style).expect("box style");

        assert_eq!(box_style.display, Display::Inline);
        assert_eq!(box_style.width, PreferredSize::Auto);
        assert_eq!(document.font_style(style).expect("font style").font_size, 20.0);
        assert_eq!(document.text_style(style).expect("text style").color, 0x008000FF);
        assert_eq!(document.background_style(style).expect("background").background_color, 0x00000000);
    }

    #[test]
    fn unlayered_revert_layer_falls_back_to_the_previous_origin() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { display: inline; display: revert-layer }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box style").display, Display::Block);
    }

    #[test]
    fn important_revert_layer_uses_the_corresponding_normal_layer_boundary() {
        let html = "<html><body><div id='target'></div></body></html>";
        let css = "@layer { #target { background-color: green } } @layer { #target { background-color: red !important; background-color: revert-layer !important } } @layer { #target { background-color: red !important } }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn inline_revert_layer_restores_stylesheet_declarations() {
        let html = "<html><body><div id='target' style='background-color: red; background-color: revert-layer'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { background-color: green }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn important_inline_revert_layer_keeps_author_stylesheet_rules() {
        let html = "<html><body><div id='target' style='background-color: red !important; background-color: revert-layer !important'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { background-color: green !important }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn all_revert_layer_restores_every_property_from_the_previous_layer() {
        let html = "<html><body><div id='target'></div></body></html>";
        let css = "@layer { #target { width: 100px; height: 100px; background-color: green } } @layer { #target { width: 200px; height: 200px; background-color: red } #target { all: revert-layer } }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");
        let box_style = document.box_model_style(style).expect("box style");

        assert_eq!(box_style.width, PreferredSize::Px(100.0));
        assert_eq!(box_style.height, PreferredSize::Px(100.0));
        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn custom_property_revert_layer_restores_the_previous_token_value() {
        let html = "<html><body><div id='target'></div></body></html>";
        let css = "@layer base, theme; @layer base { #target { --tone: green } } @layer theme { #target { --tone: red; --tone: revert-layer } } #target { background-color: var(--tone) }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn open_type_controls_resolve_to_normalized_feature_tags() {
        let html = r#"<html><body><p style='font-variant-caps: all-small-caps;
            font-variant-numeric: oldstyle-nums tabular-nums;
            font-variant-ligatures: no-common-ligatures discretionary-ligatures;
            font-kerning: none;
            font-feature-settings: "liga" on, "ss03" 2'>office 123</p></body></html>"#;
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("paragraph");
        let indices = document.style_for_node(node).expect("computed style");
        let font = document.font_style(indices).expect("font style");
        let features = font.open_type_features();
        let value = |tag| features.iter().find(|feature| feature.tag() == tag).map(|feature| feature.value());

        assert_eq!(value(*b"smcp"), Some(1));
        assert_eq!(value(*b"c2sc"), Some(1));
        assert_eq!(value(*b"onum"), Some(1));
        assert_eq!(value(*b"tnum"), Some(1));
        assert_eq!(value(*b"dlig"), Some(1));
        assert_eq!(value(*b"kern"), Some(0));
        assert_eq!(value(*b"ss03"), Some(2));
        assert_eq!(value(*b"liga"), Some(1), "explicit feature settings override the variant longhand");
    }

    #[test]
    fn inherited_font_feature_settings_reach_inline_descendants() {
        let html = r#"<html><body><div><span id="target">A</span>SS</div></body></html>"#;
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(r#"html { font-kerning: none; font-feature-settings: "kern" off; }"#));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target span");
        let indices = document.style_for_node(node).expect("computed style");
        let features = document.font_style(indices).expect("font style").open_type_features();

        assert_eq!(features.iter().find(|feature| feature.tag() == *b"kern").map(|feature| feature.value()), Some(0));
    }

    #[test]
    fn inline_block_is_preserved_as_an_atomic_flow_root() {
        let html = "<html><body><div style='display:inline-block'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("inline-block element");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box model").display, Display::InlineBlock);
    }

    #[test]
    fn block_flow_root_is_preserved_as_a_distinct_formatting_context() {
        let html = "<html><body><div style='display:flow-root'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("flow-root element");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box model").display, Display::FlowRoot);
    }

    #[test]
    fn flow_root_list_item_preserves_both_inner_display_and_marker_role() {
        let html = "<html><body><div style='display:flow-root list-item'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("flow-root list item");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box model").display, Display::FlowRootListItem);
    }

    #[test]
    fn inline_style_recovers_after_an_invalid_declaration() {
        let html = "<html><body><div style='grid; display:grid; width:300px; height:200px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let node = find_body(document.document()).children().next().expect("grid should be child of body");
        let indices = document.style_for_node(node).expect("grid style should exist");
        let box_model = document.box_model_style(indices).expect("validated box style");

        assert_eq!(box_model.display, Display::Grid);
        assert_eq!(box_model.width, PreferredSize::Px(300.0));
        assert_eq!(box_model.height, PreferredSize::Px(200.0));
    }

    #[test]
    fn invalid_numeric_longhands_keep_the_previous_cascaded_value() {
        let html = "<html><body><div style='font-size:18px;font-size:-1px;font-weight:400;font-weight:9000;border-bottom:7px solid;border-bottom-width:-10px;line-height:24px;line-height:-2'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("test element");
        let indices = document.style_for_node(node).expect("computed style");

        let font = document.font_style(indices).expect("font style");
        let text = document.text_style(indices).expect("text style");
        let border = document.border_style(indices).expect("border style");
        assert_eq!(font.font_size, 18.0);
        assert_eq!(font.font_weight, 400);
        assert_eq!(text.line_height, 24.0);
        assert_eq!(border.border_bottom_width, FontRelativeLength::px(7.0).unwrap());
    }

    #[test]
    fn invalid_border_width_shorthand_is_atomic() {
        let html = "<html><body><div style='border-width:1px 2px 3px 4px;border-width:5px -1px 7px 8px;border-style:solid'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("test element");
        let indices = document.style_for_node(node).expect("computed style");
        let border = document.border_style(indices).expect("border style");

        assert_eq!(
            (border.border_top_width, border.border_right_width, border.border_bottom_width, border.border_left_width),
            (FontRelativeLength::px(1.0).unwrap(), FontRelativeLength::px(2.0).unwrap(), FontRelativeLength::px(3.0).unwrap(), FontRelativeLength::px(4.0).unwrap())
        );
    }

    #[test]
    fn border_none_preserves_computed_width_for_inheritance() {
        let html = "<html><body><div style='border:16px solid blue;border-top:none'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("test element");
        let border = document.border_style(document.style_for_node(node).expect("computed style")).expect("border style");

        assert_eq!(border.border_top_style, BorderStyle::None);
        assert_eq!(border.border_top_width, FontRelativeLength::px(3.0).unwrap());
        assert_eq!(border.border_right_width, FontRelativeLength::px(16.0).unwrap());
    }

    #[test]
    fn border_inherit_copies_the_parents_computed_current_color() {
        let html = "<html><body><p style='color:black;border:medium solid'><em style='color:red;border:inherit'></em></p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let paragraph = find_body(document.document()).children().next().expect("paragraph");
        let emphasis = document.document().element_ref(paragraph).expect("paragraph element").children().find(|&child| document.document().element_ref(child).is_some_and(|element| element.tag() == "em")).expect("emphasis element");
        let indices = document.style_for_node(emphasis).expect("computed emphasis style");
        let text = document.text_style(indices).expect("text style");
        let border = document.border_style(indices).expect("border style");

        assert_eq!(text.color, 0xff0000ff);
        assert_eq!(border.border_top_color, 0x000000ff);
        assert_eq!(border.border_right_color, 0x000000ff);
        assert_eq!(border.border_bottom_color, 0x000000ff);
        assert_eq!(border.border_left_color, 0x000000ff);
    }

    #[test]
    fn border_shorthand_components_survive_a_later_style_longhand() {
        let html = "<html><body><div style='border:1in blue;border-style:solid'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("test element");
        let border = document.border_style(document.style_for_node(node).expect("computed style")).expect("border style");

        assert_eq!(border.border_top_width, FontRelativeLength::px(96.0).unwrap());
        assert_eq!(border.border_top_style, BorderStyle::Solid);
        assert_eq!(border.border_top_color, 0x0000ffff);
    }

    #[test]
    fn negative_physical_padding_is_ignored_and_shorthands_are_atomic() {
        let html = "<html><body><div style='padding:1px 2px 3px 4px;padding:5px -1px 7px 8px'></div><div style='padding-top:9px;padding-top:-1px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let nodes = find_body(document.document()).children().collect::<Vec<_>>();
        let shorthand = document.box_model_style(document.style_for_node(nodes[0]).expect("computed style")).expect("box model");
        let longhand = document.box_model_style(document.style_for_node(nodes[1]).expect("computed style")).expect("box model");

        assert_eq!((shorthand.padding_top, shorthand.padding_right, shorthand.padding_bottom, shorthand.padding_left), (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(3.0), LengthPct::Px(4.0)));
        assert_eq!(longhand.padding_top, LengthPct::Px(9.0));
    }

    #[test]
    fn margin_shorthand_expands_one_through_four_values() {
        let html = "<html><body><div style='margin:1px'></div><div style='margin:1px 2px'></div><div style='margin:1px 2px 3px'></div><div style='margin:1px 2px 3px 4px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let nodes = find_body(document.document()).children().collect::<Vec<_>>();
        let sides = nodes
            .iter()
            .map(|&node| {
                let style = document.style_for_node(node).expect("computed margin style");
                let box_model = document.box_model_style(style).expect("box model");
                (box_model.margin_top, box_model.margin_right, box_model.margin_bottom, box_model.margin_left)
            })
            .collect::<Vec<_>>();

        assert_eq!(sides[0], (LengthPct::Px(1.0), LengthPct::Px(1.0), LengthPct::Px(1.0), LengthPct::Px(1.0)));
        assert_eq!(sides[1], (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(1.0), LengthPct::Px(2.0)));
        assert_eq!(sides[2], (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(3.0), LengthPct::Px(2.0)));
        assert_eq!(sides[3], (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(3.0), LengthPct::Px(4.0)));
    }

    #[test]
    fn padding_shorthand_expands_and_inherits_one_through_four_values() {
        let html = "<html><body><div style='padding:1px'><span style='padding:inherit'></span></div><div style='padding:1px 2px'><span style='padding:inherit'></span></div><div style='padding:1px 2px 3px'><span style='padding:inherit'></span></div><div style='padding:1px 2px 3px 4px'><span style='padding:inherit'></span></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let parents = find_body(document.document()).children().collect::<Vec<_>>();
        let expected = [
            (LengthPct::Px(1.0), LengthPct::Px(1.0), LengthPct::Px(1.0), LengthPct::Px(1.0)),
            (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(1.0), LengthPct::Px(2.0)),
            (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(3.0), LengthPct::Px(2.0)),
            (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(3.0), LengthPct::Px(4.0)),
        ];

        for (parent, expected_sides) in parents.into_iter().zip(expected) {
            let child = document.document().element_ref(parent).expect("padding parent").children().next().expect("padding child");
            for node in [parent, child] {
                let indices = document.style_for_node(node).expect("computed padding style");
                let box_model = document.box_model_style(indices).expect("box model");
                assert_eq!((box_model.padding_top, box_model.padding_right, box_model.padding_bottom, box_model.padding_left), expected_sides);
            }
        }
    }

    #[test]
    fn relative_position_and_physical_insets_cross_as_typed_values() {
        let html = "<html><body><div style='position:relative;position:absolute;top:11px;right:auto;bottom:3%;left:-7px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("positioned element");
        let indices = document.style_for_node(node).expect("computed positioned style");
        let layout = document.styles().layout_style(indices).expect("layout style");

        assert_eq!(layout.position, PositionMode::Absolute);
        assert_eq!(layout.inset_top, Some(LengthPct::Px(11.0)));
        assert_eq!(layout.inset_right, None);
        assert_eq!(layout.inset_bottom, Some(LengthPct::Pct(0.03)));
        assert_eq!(layout.inset_left, Some(LengthPct::Px(-7.0)));
    }

    #[test]
    fn image_dimension_attributes_are_presentational_hints_below_author_css() {
        let html = "<html><body><img id='percentage' src='x' width='50%' height='25%'><img id='pixels' src='x' width='80' height='3'><img id='overridden' src='x' width='50%' style='width:75%'><svg id='svg' width='40' height='50%'></svg></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let style_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("image by id");
            document.box_model_style(document.style_for_node(node).expect("computed image style")).expect("box model")
        };

        let percentage = style_for("percentage");
        assert_eq!(percentage.width, PreferredSize::Percent(0.5));
        assert_eq!(percentage.height, PreferredSize::Percent(0.25));
        let pixels = style_for("pixels");
        assert_eq!(pixels.width, PreferredSize::Px(80.0));
        assert_eq!(pixels.height, PreferredSize::Px(3.0));
        assert_eq!(style_for("overridden").width, PreferredSize::Percent(0.75));
        assert_eq!(style_for("svg").width, PreferredSize::Px(40.0));
        assert_eq!(style_for("svg").height, PreferredSize::Percent(0.5));
    }

    #[test]
    fn image_spacing_attributes_preserve_pixel_and_percentage_margins() {
        let html = "<html><body><img id='hinted' src='x' hspace='10%' vspace='7'><img id='overridden' src='x' hspace='10%' style='margin-left:3px'></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let style_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("image by id");
            document.box_model_style(document.style_for_node(node).expect("computed image style")).expect("box model")
        };

        let hinted = style_for("hinted");
        assert_eq!((hinted.margin_left, hinted.margin_right), (LengthPct::Pct(0.1), LengthPct::Pct(0.1)));
        assert_eq!((hinted.margin_top, hinted.margin_bottom), (LengthPct::Px(7.0), LengthPct::Px(7.0)));
        let overridden = style_for("overridden");
        assert_eq!(overridden.margin_left, LengthPct::Px(3.0));
        assert_eq!(overridden.margin_right, LengthPct::Pct(0.1));
    }

    #[test]
    fn hidden_attribute_is_a_cascading_html_presentational_hint() {
        let html = "<html><body><div id='hidden' hidden>A</div><div id='overridden' hidden style='display:block'>B</div><div id='until-found' hidden='until-found'>C</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let display_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).expect("element by id");
            document.box_model_style(document.style_for_node(node).expect("computed style")).expect("box model").display
        };

        assert_eq!(display_for("hidden"), Display::None);
        assert_eq!(display_for("overridden"), Display::Block, "author CSS must override the presentational hint");
        assert_eq!(display_for("until-found"), Display::Block, "hidden-until-found retains its principal box");
    }

    #[test]
    fn html_presentational_hints_do_not_leak_into_foreign_xml_elements() {
        let parsed = html_parse::parse_xml_document("<root><div id='target' hidden=''/></root>").expect("valid XML");
        let document = crate::style_document(parsed.build_dom(), &[]);
        let target = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target element");
        let style = document.box_model_style(document.style_for_node(target).expect("computed style")).expect("box model");

        assert_eq!(style.display, Display::Block);
    }

    #[test]
    fn legacy_html_color_attributes_are_presentational_hints_below_author_css() {
        let html = "<html><body id='body' text='green' bgcolor='#ffff00'><table><tr><td id='cell' bgcolor='red' style='background-color:blue'>Cell</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("body { color: #123456; }"));
        let node_by_id = |id| document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");

        let body_style = document.style_for_node(node_by_id("body")).expect("computed body style");
        assert_eq!(document.text_style(body_style).expect("body text style").color, 0x123456FF, "author CSS should override the text hint");
        assert_eq!(document.background_style(body_style).expect("body background style").background_color, 0xFFFF00FF);

        let cell_style = document.style_for_node(node_by_id("cell")).expect("computed cell style");
        assert_eq!(document.background_style(cell_style).expect("cell background style").background_color, 0x0000FFFF, "author CSS should override the bgcolor hint");
    }

    #[test]
    fn author_revert_also_rolls_back_the_presentational_hint_origin() {
        let html = "<html><body><table><tr><td id='target' bgcolor='red' style='background-color:revert'>Cell</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x00000000);
    }

    #[test]
    fn revert_layer_can_expose_the_presentational_hint_origin() {
        let html = "<html><body><img id='target' width='123' style='width:revert-layer'></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box style").width, PreferredSize::Px(123.0));
    }

    #[test]
    fn all_revert_removes_hints_except_direction() {
        let html = "<html><body><table><tr><td id='target' dir='rtl' bgcolor='red' style='all:revert'>Cell</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x00000000);
        assert_eq!(document.text_style(style).expect("text style").direction, TextDirection::Rtl);
    }

    #[test]
    fn legacy_table_spacing_and_padding_are_presentational_hints_below_author_css() {
        let html = "<html><body><table id='hinted' cellspacing='0' cellpadding='0'><tr><td id='hinted-cell'>A</td></tr></table><table id='overridden' cellspacing='7' cellpadding='9' style='border-spacing:3px'><tr><td id='overridden-cell' style='padding:4px'>B</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let box_style_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");
            document.box_model_style(document.style_for_node(node).expect("computed style")).expect("box model")
        };

        let hinted = box_style_for("hinted");
        assert_eq!((hinted.border_spacing_horizontal, hinted.border_spacing_vertical), (0.0, 0.0));
        let hinted_cell = box_style_for("hinted-cell");
        assert_eq!((hinted_cell.padding_top, hinted_cell.padding_right, hinted_cell.padding_bottom, hinted_cell.padding_left), (LengthPct::Px(0.0), LengthPct::Px(0.0), LengthPct::Px(0.0), LengthPct::Px(0.0)));

        let overridden = box_style_for("overridden");
        assert_eq!((overridden.border_spacing_horizontal, overridden.border_spacing_vertical), (3.0, 3.0));
        let overridden_cell = box_style_for("overridden-cell");
        assert_eq!((overridden_cell.padding_top, overridden_cell.padding_right, overridden_cell.padding_bottom, overridden_cell.padding_left), (LengthPct::Px(4.0), LengthPct::Px(4.0), LengthPct::Px(4.0), LengthPct::Px(4.0)));
    }

    #[test]
    fn legacy_table_rules_and_frame_values_are_ascii_case_insensitive() {
        let html = "<html><body><table id='lower-rules' rules='rows'><tr id='lower-row'><td>X</td></tr></table><table id='upper-rules' rules='RoWs'><tr id='upper-row'><td>X</td></tr></table><table id='invalid-rules' rules='rowſ'><tr id='invalid-row'><td>X</td></tr></table><table id='lower-frame' frame='hsides'><tr><td>X</td></tr></table><table id='upper-frame' frame='HsIdEs'><tr><td>X</td></tr></table><table id='invalid-frame' frame='hſideſ'><tr><td>X</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let style_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");
            document.style_for_node(node).expect("computed style")
        };

        let lower_row = document.border_style(style_for("lower-row")).expect("row border");
        let upper_row = document.border_style(style_for("upper-row")).expect("row border");
        let invalid_row = document.border_style(style_for("invalid-row")).expect("row border");
        assert_eq!(lower_row, upper_row);
        assert_ne!(lower_row, invalid_row);

        let lower_frame = document.border_style(style_for("lower-frame")).expect("table border");
        let upper_frame = document.border_style(style_for("upper-frame")).expect("table border");
        let invalid_frame = document.border_style(style_for("invalid-frame")).expect("table border");
        assert_eq!(lower_frame, upper_frame);
        assert_ne!(lower_frame, invalid_frame);
    }

    #[test]
    fn legacy_cell_nowrap_is_a_presentational_hint_below_author_css() {
        let html = "<html><body><table><tr><td id='hinted' nowrap>text</td><td id='overridden' nowrap style='white-space:normal'>text</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let white_space_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");
            document.text_style(document.style_for_node(node).expect("computed style")).expect("text style").white_space
        };

        assert_eq!(white_space_for("hinted"), WhiteSpace::NoWrap);
        assert_eq!(white_space_for("overridden"), WhiteSpace::Normal);
    }

    #[test]
    fn inherited_table_model_properties_cross_the_computed_style_boundary() {
        let html = "<html><body><table id='table' style='border-collapse:collapse;border-spacing:7px 9px;caption-side:bottom;empty-cells:hide'><tbody><tr><td id='cell'>A</td></tr></tbody></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let by_id = |id| document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");
        let table_style = document.box_model_style(document.style_for_node(by_id("table")).expect("computed table style")).expect("table box model");
        assert_eq!(table_style.border_collapse, BorderCollapseMode::Collapse);
        let cell = by_id("cell");
        let style = document.box_model_style(document.style_for_node(cell).expect("computed cell style")).expect("cell box model");

        assert_eq!(style.border_collapse, BorderCollapseMode::Collapse);
        assert_eq!((style.border_spacing_horizontal, style.border_spacing_vertical), (7.0, 9.0));
        assert_eq!(style.caption_side, CaptionSide::Bottom);
        assert_eq!(style.empty_cells, EmptyCellsMode::Hide);
    }

    #[test]
    fn custom_represented_table_keywords_obey_the_normal_cascade() {
        let html = "<html><body><table id='important' style='border-collapse:separate'><tr><td>A</td></tr></table><table id='ordered' style='border-collapse:collapse;border-collapse:separate'><tr><td>B</td></tr></table><table id='variable' style='--mode:collapse;border-collapse:var(--mode)'><tr><td>C</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#important { border-collapse: collapse !important; }"));
        let by_id = |id| document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("table by id");
        let collapse = |id| document.box_model_style(document.style_for_node(by_id(id)).expect("computed table style")).expect("table box model").border_collapse;

        assert_eq!(collapse("important"), BorderCollapseMode::Collapse);
        assert_eq!(collapse("ordered"), BorderCollapseMode::Separate);
        assert_eq!(collapse("variable"), BorderCollapseMode::Collapse);
    }

    #[test]
    fn table_layout_is_resolved_from_stylesheets_and_is_not_inherited_by_default() {
        let html = "<html><body><table id='table'><tr style='table-layout:fixed'><td id='reset'>A</td><td id='inherited' style='table-layout:inherit'>B</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#table { table-layout: fixed; }"));
        let by_id = |id| document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");
        let mode = |id| document.box_model_style(document.style_for_node(by_id(id)).expect("computed style")).expect("box model").table_layout;

        assert_eq!(mode("table"), TableLayoutMode::Fixed);
        assert_eq!(mode("reset"), TableLayoutMode::Auto);
        assert_eq!(mode("inherited"), TableLayoutMode::Fixed);
    }

    #[test]
    fn size_containment_is_resolved_as_a_reset_property() {
        let html = "<html><body><div id='parent' style='contain:size'><div id='reset'></div><div id='inherited' style='contain:inherit'></div></div><div id='strict' style='contain:strict'></div><div id='content' style='contain:content'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let by_id = |id| document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");
        let contained = |id| document.box_model_style(document.style_for_node(by_id(id)).expect("computed style")).expect("box model").size_containment;

        assert!(contained("parent"));
        assert!(!contained("reset"));
        assert!(contained("inherited"));
        assert!(contained("strict"));
        assert!(!contained("content"));
    }

    #[test]
    fn absolute_positioning_blockifies_table_internal_displays() {
        let displays = ["table-row-group", "table-header-group", "table-footer-group", "table-row", "table-column-group", "table-column", "table-cell", "table-caption"];
        let children = displays.iter().enumerate().map(|(index, display)| format!("<div id='d{index}' style='display:{display};position:absolute'></div>")).collect::<String>();
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(&format!("<html><body>{children}</body></html>"), None);

        for index in 0..displays.len() {
            let id = format!("d{index}");
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id.as_str())).expect("positioned table-internal display");
            let box_model = document.box_model_style(document.style_for_node(node).expect("computed positioned style")).expect("box model");
            assert_eq!(box_model.display, Display::Block, "{} must blockify", displays[index]);
        }
    }

    #[test]
    fn floating_blockifies_inline_and_table_internal_displays() {
        let displays = ["inline", "inline-block", "table-row-group", "table-header-group", "table-footer-group", "table-row", "table-column-group", "table-column", "table-cell", "table-caption"];
        let children = displays.iter().enumerate().map(|(index, display)| format!("<div id='d{index}' style='display:{display};float:left'></div>")).collect::<String>();
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(&format!("<html><body>{children}</body></html>"), None);

        for index in 0..displays.len() {
            let id = format!("d{index}");
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id.as_str())).expect("floated display");
            let box_model = document.box_model_style(document.style_for_node(node).expect("computed floated style")).expect("box model");
            assert_eq!(box_model.display, Display::Block, "{} must blockify", displays[index]);
        }
    }

    #[test]
    fn z_index_crosses_the_typed_style_boundary() {
        let html = "<html><body><div id='integer' style='position:relative;z-index:-7'></div><div id='auto' style='position:absolute;z-index:auto'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let z_index_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");
            document.styles().layout_style(document.style_for_node(node).expect("computed style")).expect("layout style").z_index
        };
        assert_eq!(z_index_for("integer"), Some(-7));
        assert_eq!(z_index_for("auto"), None);
    }

    #[test]
    fn physical_padding_inherit_copies_the_parent_computed_value() {
        let html = "<html><body><div id='parent' style='padding-top:1in'><div id='child' style='padding-top:inherit'></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        for id in ["parent", "child"] {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("padding test element");
            let indices = document.style_for_node(node).expect("computed padding style");
            assert_eq!(document.box_model_style(indices).expect("box style").padding_top, LengthPct::Px(96.0));
        }
    }

    #[test]
    fn inherited_em_and_percentage_padding_keep_their_computed_semantics() {
        let html = "<html><body><div id='em-parent' style='font-size:24px;padding:2em 3em 1em 4em'><div id='em-child' style='font-size:40px;padding:inherit'></div></div><div id='pct-parent' style='padding:20%'><div id='pct-child' style='padding:inherit'></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let box_model = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("padding test element");
            document.box_model_style(document.style_for_node(node).expect("computed padding style")).expect("box style")
        };

        let expected_em = (LengthPct::Px(48.0), LengthPct::Px(72.0), LengthPct::Px(24.0), LengthPct::Px(96.0));
        for id in ["em-parent", "em-child"] {
            let style = box_model(id);
            assert_eq!((style.padding_top, style.padding_right, style.padding_bottom, style.padding_left), expected_em);
        }
        for id in ["pct-parent", "pct-child"] {
            let style = box_model(id);
            assert_eq!((style.padding_top, style.padding_right, style.padding_bottom, style.padding_left), (LengthPct::Pct(0.2), LengthPct::Pct(0.2), LengthPct::Pct(0.2), LengthPct::Pct(0.2)));
        }
    }

    #[test]
    fn invalid_font_shorthand_is_atomic() {
        let html = "<html><body><div style='font:italic 700 20px/30px serif;font:4em/-2em monospace'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("test element");
        let indices = document.style_for_node(node).expect("computed style");
        let font = document.font_style(indices).expect("font style");
        let text = document.text_style(indices).expect("text style");

        assert_eq!(font.font_size, 20.0);
        assert_eq!(font.font_weight, 700);
        assert_eq!(font.font_style, crate::document::FontStyle::Italic);
        assert_eq!(text.line_height, 30.0);
        let family = font.font_family.and_then(|id| document.styles().string(id)).expect("font family");
        assert!(family.contains("serif"));
    }

    #[test]
    fn italic_and_oblique_remain_distinct_computed_font_styles() {
        let html = "<html><body><span id='italic' style='font-style:italic'></span><span id='oblique' style='font-style:oblique 12deg'></span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let font_style_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("font style test element");
            document.font_style(document.style_for_node(node).expect("computed style")).expect("font style").font_style
        };

        assert_eq!(font_style_for("italic"), crate::document::FontStyle::Italic);
        assert_eq!(font_style_for("oblique"), crate::document::FontStyle::Oblique);
    }

    #[test]
    fn positive_infinite_flex_factor_is_normalized_before_storage() {
        let html = "<html><body><div style='display:flex'><div style='flex:calc(infinity) 0 0px'></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let container = find_body(document.document()).children().next().expect("flex container");
        let child = document.document().element_ref(container).expect("container element").children().next().expect("flex item");
        let indices = document.style_for_node(child).expect("computed style");
        let layout = document.styles().layout_style(indices).expect("layout style");

        assert!(layout.flex_grow.is_finite());
        assert_eq!(layout.flex_grow, super::MAX_COMPUTED_FLEX_FACTOR);
    }

    #[test]
    fn invalid_var_substitution_uses_unset_instead_of_the_losing_declaration() {
        let html = "<html><body style='font-size:22px'><div style='--bad:-1px;font-size:30px;font-size:var(--bad)'></div><div style='font-size:31px;font-size:var(--missing)'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let sizes = find_body(document.document())
            .children()
            .map(|node| {
                let indices = document.style_for_node(node).expect("computed style");
                document.font_style(indices).expect("font style").font_size
            })
            .collect::<Vec<_>>();

        assert_eq!(sizes, vec![22.0, 22.0]);
    }

    #[test]
    fn custom_property_wide_keywords_resolve_before_var_fallbacks() {
        let html = "<html><body>
            <div id='initial' style='color:orange;--tone:initial;color:var(--tone,green)'></div>
            <div style='--tone:green'><div id='inherit' style='color:orange;--tone:inherit;color:var(--tone)'></div></div>
            <div style='--tone:green'><div id='unset' style='color:orange;--tone:unset;color:var(--tone)'></div></div>
            <div style='--tone:green'><div style='--tone:var(--missing,unset)'><div id='fallback' style='color:var(--tone)'></div></div></div>
        </body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let color = |id| {
            let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).expect("element by id");
            document.text_style(document.style_for_node(node).expect("computed style")).expect("text style").color
        };

        assert_eq!(color("initial"), 0x008000FF);
        assert_eq!(color("inherit"), 0x008000FF);
        assert_eq!(color("unset"), 0x008000FF);
        assert_eq!(color("fallback"), 0x008000FF);
    }

    #[test]
    fn css_wide_keywords_from_var_fallbacks_keep_their_semantics() {
        let html = "<html><body><div id='outer'><div id='inner'></div></div></body></html>";
        let css = "#outer { color: transparent; border-style: solid } #inner { color: var(--missing, initial); border-style: var(--missing, inherit) }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("inner")).expect("inner");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.text_style(style).expect("text style").color, 0x000000FF);
        assert_eq!(document.border_style(style).expect("border style").border_top_style, BorderStyle::Solid);
    }

    #[test]
    fn first_line_and_first_letter_styles_are_resolved_separately() {
        let mut factory = DocumentFactory::new();
        let document = factory
            .parse_with_new_pipeline("<html><body style='line-height:20px'><div id='line'>Line</div><div id='letter'>Letter</div></body></html>", Some("#line::first-line { line-height:100px } #letter::first-letter { line-height:80px }"));
        let children = find_body(document.document()).children().collect::<Vec<_>>();
        let [line, letter] = children.as_slice() else { panic!("expected two body children") };
        let line_style = document.styles().first_line_style_for_node(*line).expect("first-line style");
        let letter_style = document.styles().first_letter_style_for_node(*letter).expect("first-letter style");

        assert_eq!(document.text_style(line_style).expect("first-line text style").line_height, 100.0);
        assert_eq!(document.text_style(letter_style).expect("first-letter text style").line_height, 80.0);
        assert!(document.styles().first_letter_style_for_node(*line).is_none());
        assert!(document.styles().first_line_style_for_node(*letter).is_none());
    }

    #[test]
    fn first_letter_retains_the_inheritance_path_of_generated_before_text() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline("<html><body><div style='color:red'></div></body></html>", Some("div::first-line { color:blue } div::before { content:'Filler'; color:green } div::first-letter { font-size:98px }"));
        let div = find_body(document.document()).children().next().expect("generated-content origin");
        let ordinary = document.styles().first_letter_style_for_node(div).expect("ordinary first-letter style");
        let generated = document.styles().before_first_letter_style_for_node(div).expect("generated first-letter style");

        assert_eq!(document.text_style(ordinary).expect("ordinary first-letter text style").color, 0x0000FFFF);
        assert_eq!(document.text_style(generated).expect("generated first-letter text style").color, 0x008000FF);
        assert_eq!(document.font_style(generated).expect("generated first-letter font style").font_size, 98.0);
    }

    #[test]
    fn horizontal_rtl_logical_properties_resolve_to_physical_style_values() {
        let html = "<html><body><div style='direction:rtl;inline-size:120px;block-size:30px;margin-inline:10px 20px;padding-inline:3px 4px;border-inline-width:5px 7px;border-inline-style:solid dashed;border-inline-color:red blue;text-align:start;float:inline-start;clear:inline-end'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("logical box");
        let indices = document.style_for_node(node).expect("computed logical style");
        let text = document.text_style(indices).expect("text style");
        let box_model = document.box_model_style(indices).expect("box style");
        let border = document.border_style(indices).expect("border style");

        assert_eq!(text.direction, TextDirection::Rtl);
        assert_eq!(text.text_align, TextAlign::Right);
        assert_eq!(box_model.width, PreferredSize::Px(120.0));
        assert_eq!(box_model.height, PreferredSize::Px(30.0));
        assert_eq!((box_model.margin_right, box_model.margin_left), (LengthPct::Px(10.0), LengthPct::Px(20.0)));
        assert_eq!((box_model.padding_right, box_model.padding_left), (LengthPct::Px(3.0), LengthPct::Px(4.0)));
        assert_eq!((border.border_right_width, border.border_left_width), (FontRelativeLength::px(5.0).unwrap(), FontRelativeLength::px(7.0).unwrap()));
        assert_eq!((border.border_right_style, border.border_left_style), (BorderStyle::Solid, BorderStyle::Dashed));
        assert_eq!((border.border_right_color, border.border_left_color), (0xFF0000FF, 0x0000FFFF));
        assert_eq!(box_model.float, Float::Right);
        assert_eq!(box_model.clear, Clear::Left);
    }

    #[test]
    fn logical_text_alignment_re_resolves_when_descendant_direction_changes() {
        let html = "<html><body dir='rtl' id='initial'><div style='text-align:start'><span dir='ltr' id='logical'></span></div><div style='text-align:left'><span dir='rtl' id='physical'></span></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let alignment = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("alignment test element");
            document.text_style(document.style_for_node(node).expect("computed style")).expect("text style").text_align
        };

        assert_eq!(alignment("initial"), TextAlign::Right, "the initial logical start follows an HTML rtl direction hint");
        assert_eq!(alignment("logical"), TextAlign::Left, "inherited text-align:start re-resolves against the descendant direction");
        assert_eq!(alignment("physical"), TextAlign::Left, "an explicitly physical inherited alignment does not flip");
    }

    #[test]
    fn intrinsic_size_keywords_reach_computed_box_style() {
        let html = "<html><body><div style='width:min-content;min-width:max-content;max-width:fit-content'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("intrinsically sized box");
        let indices = document.style_for_node(node).expect("computed box style");
        let box_model = document.box_model_style(indices).expect("validated box style");

        assert_eq!(box_model.width, PreferredSize::MinContent);
        assert_eq!(box_model.min_width, PreferredSize::MaxContent);
        assert_eq!(box_model.max_width, PreferredSize::FitContent);
    }

    #[test]
    fn stretch_size_keyword_reaches_computed_box_style() {
        let html = "<html><body><div style='width:stretch;height:-webkit-fill-available;min-height:stretch;max-height:stretch'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("stretched box");
        let indices = document.style_for_node(node).expect("computed box style");
        let box_model = document.box_model_style(indices).expect("validated box style");

        assert_eq!(box_model.width, PreferredSize::Stretch);
        assert_eq!(box_model.height, PreferredSize::Stretch);
        assert_eq!(box_model.min_height, PreferredSize::Stretch);
        assert_eq!(box_model.max_height, PreferredSize::Stretch);
    }

    #[test]
    fn border_radius_crosses_style_boundary_as_physical_length_percentages() {
        let html = "<html><body><div style='direction:rtl;border-radius:10px 20% / 30px 40%;border-start-start-radius:7px 9px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("rounded box");
        let indices = document.style_for_node(node).expect("computed rounded style");
        let radii = *document.border_radii_style(indices).expect("radius style");

        // In horizontal RTL, start-start is the physical top-right corner.
        assert_eq!(radii.top_left.x, LengthPct::Px(10.0));
        assert_eq!(radii.top_left.y, LengthPct::Px(30.0));
        assert_eq!(radii.top_right.x, LengthPct::Px(7.0));
        assert_eq!(radii.top_right.y, LengthPct::Px(9.0));
        assert_eq!(radii.bottom_right.x, LengthPct::Px(10.0));
        assert_eq!(radii.bottom_right.y, LengthPct::Px(30.0));
        assert_eq!(radii.bottom_left.x, LengthPct::Pct(0.2));
        assert_eq!(radii.bottom_left.y, LengthPct::Pct(0.4));
    }

    #[test]
    fn logical_and_physical_declarations_share_one_cascade_order() {
        let html = "<html><body><div style='direction:rtl;margin-right:1px;margin-inline-start:2px;width:10px;inline-size:20px'></div><div style='direction:rtl;margin-inline-start:2px;margin-right:1px;inline-size:20px;width:10px'></div><div style='margin-inline-start:12px;direction:rtl'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let boxes = find_body(document.document())
            .children()
            .map(|node| {
                let indices = document.style_for_node(node).expect("computed style");
                document.box_model_style(indices).expect("box style").clone()
            })
            .collect::<Vec<_>>();

        assert_eq!(boxes[0].margin_right, LengthPct::Px(2.0));
        assert_eq!(boxes[0].width, PreferredSize::Px(20.0));
        assert_eq!(boxes[1].margin_right, LengthPct::Px(1.0));
        assert_eq!(boxes[1].width, PreferredSize::Px(10.0));
        assert_eq!(boxes[2].margin_right, LengthPct::Px(12.0));
        assert_eq!(boxes[2].margin_left, LengthPct::Px(0.0));
    }

    #[test]
    fn html_dir_and_css_direction_inherit_for_logical_resolution() {
        let html = "<html><body><section dir='RTL'><div style='margin-inline-start:9px'></div></section><section style='direction:rtl'><div style='padding-inline-end:7px'></div></section></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let values = find_body(document.document())
            .children()
            .map(|parent| {
                let child = document.document().element_ref(parent).expect("section").children().next().expect("child");
                let indices = document.style_for_node(child).expect("computed child style");
                let text = document.text_style(indices).expect("text style");
                let box_model = document.box_model_style(indices).expect("box style");
                (text.direction, box_model.margin_right, box_model.padding_left)
            })
            .collect::<Vec<_>>();

        assert_eq!(values[0], (TextDirection::Rtl, LengthPct::Px(9.0), LengthPct::Px(0.0)));
        assert_eq!(values[1], (TextDirection::Rtl, LengthPct::Px(0.0), LengthPct::Px(7.0)));
    }

    #[test]
    fn invalid_typed_logical_lengths_preserve_valid_fallbacks() {
        let html = "<html><body><div style='direction:rtl;inline-size:40px;inline-size:-10px;padding-inline-start:6px;padding-inline-start:-2px;border-inline-start:3px solid;border-inline-start-width:-4px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("logical box");
        let indices = document.style_for_node(node).expect("computed logical style");
        let box_model = document.box_model_style(indices).expect("box style");
        let border = document.border_style(indices).expect("border style");

        assert_eq!(box_model.width, PreferredSize::Px(40.0));
        assert_eq!(box_model.padding_right, LengthPct::Px(6.0));
        assert_eq!(border.border_right_width, FontRelativeLength::px(3.0).unwrap());
    }

    #[test]
    fn flex_properties_cross_the_style_boundary_as_renderer_owned_values() {
        use crate::document::{ContentAlignment, FlexDirection, FlexWrap, ItemAlignment, LengthPct, PreferredSize};
        let html = "<html><body><div style='display:flex;flex-flow:column wrap;flex:2 3 40px;order:-2;align-items:center;justify-content:space-between;gap:5px 10px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("flex element");
        let indices = document.style_for_node(node).expect("computed flex style");
        let box_model = document.box_model_style(indices).expect("box style");
        let layout = document.styles().layout_style(indices).expect("layout style");

        assert_eq!(box_model.display, Display::Flex);
        assert_eq!(layout.flex_direction, FlexDirection::Column);
        assert_eq!(layout.flex_wrap, FlexWrap::Wrap);
        assert_eq!(layout.flex_grow, 2.0);
        assert_eq!(layout.flex_shrink, 3.0);
        assert_eq!(layout.flex_basis, PreferredSize::Px(40.0));
        assert_eq!(layout.order, -2);
        assert_eq!(layout.align_items, ItemAlignment::Center);
        assert_eq!(layout.justify_content, ContentAlignment::SpaceBetween);
        assert_eq!(layout.row_gap, LengthPct::Px(5.0));
        assert_eq!(layout.column_gap, LengthPct::Px(10.0));
    }

    #[test]
    fn physical_justify_alignment_remains_distinct_from_logical_alignment() {
        use crate::document::ItemAlignment;
        let html = "<html><body><div id='left' style='justify-self:left;justify-items:right'></div><div id='right' style='justify-self:right;justify-items:left'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let body = find_body(document.document());
        let mut children = body.children();
        let left = children.next().expect("left alignment element");
        let right = children.next().expect("right alignment element");
        let left_layout = document.styles().layout_style(document.style_for_node(left).expect("left computed style")).expect("left layout style");
        let right_layout = document.styles().layout_style(document.style_for_node(right).expect("right computed style")).expect("right layout style");

        assert_eq!(left_layout.justify_self, ItemAlignment::Left);
        assert_eq!(left_layout.justify_items, ItemAlignment::Right);
        assert_eq!(right_layout.justify_self, ItemAlignment::Right);
        assert_eq!(right_layout.justify_items, ItemAlignment::Left);
    }

    #[test]
    fn negative_typed_gap_does_not_override_the_last_valid_gap() {
        use crate::document::LengthPct;
        let html = "<html><body><div style='display:grid;gap:12px 14px;gap:5px -10px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("grid element");
        let indices = document.style_for_node(node).expect("computed grid style");
        let layout = document.styles().layout_style(indices).expect("layout style");

        assert_eq!(layout.row_gap, LengthPct::Px(12.0));
        assert_eq!(layout.column_gap, LengthPct::Px(14.0));
    }

    #[test]
    fn legacy_grid_gap_aliases_share_the_canonical_computed_values() {
        use crate::document::LengthPct;
        let html = "<html><body><div id='parent' style='grid-row-gap:12px;grid-column-gap:14px'><div id='child' style='grid-row-gap:inherit;grid-column-gap:inherit'></div></div><div id='shorthand' style='grid-gap:6px 9px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        for id in ["parent", "child"] {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("gap alias element");
            let indices = document.style_for_node(node).expect("computed gap alias style");
            let layout = document.styles().layout_style(indices).expect("layout style");
            assert_eq!(layout.row_gap, LengthPct::Px(12.0));
            assert_eq!(layout.column_gap, LengthPct::Px(14.0));
        }

        let shorthand = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some("shorthand")).expect("grid-gap shorthand element");
        let indices = document.style_for_node(shorthand).expect("computed grid-gap shorthand style");
        let layout = document.styles().layout_style(indices).expect("layout style");
        assert_eq!(layout.row_gap, LengthPct::Px(6.0));
        assert_eq!(layout.column_gap, LengthPct::Px(9.0));
    }

    #[test]
    fn negative_typed_flex_basis_does_not_override_the_last_valid_basis() {
        use crate::document::PreferredSize;
        let html = "<html><body><div style='display:flex;flex-basis:40px;flex-basis:-10px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("flex element");
        let indices = document.style_for_node(node).expect("computed flex style");
        let layout = document.styles().layout_style(indices).expect("layout style");

        assert_eq!(layout.flex_basis, PreferredSize::Px(40.0));
    }

    #[test]
    fn negative_typed_border_spacing_does_not_override_valid_spacing() {
        let html = "<html><body><table style='border-spacing:5px 7px;border-spacing:-20px'><tr><td>A</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("table element");
        let indices = document.style_for_node(node).expect("computed table style");
        let style = document.box_model_style(indices).expect("box style");

        assert_eq!(style.border_spacing_horizontal, 5.0);
        assert_eq!(style.border_spacing_vertical, 7.0);
    }

    #[test]
    fn unsupported_or_malformed_list_marker_does_not_override_valid_marker() {
        use crate::document::ListStyleType;
        let html = "<html><body><ul><li style=\"list-style-type:square;list-style-type:symbols(cyclic)\">A</li><li style=\"list-style-type:circle;list-style-type:symbols(cyclic 'x')\">B</li></ul></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let list = find_body(document.document()).children().next().expect("list element");
        let markers = document
            .document()
            .element_ref(list)
            .expect("list node is an element")
            .children()
            .map(|node| {
                let indices = document.style_for_node(node).expect("computed list item style");
                document.text_style(indices).expect("text style").list_style_type
            })
            .collect::<Vec<_>>();

        assert_eq!(markers, vec![ListStyleType::Square, ListStyleType::Circle]);
    }

    #[test]
    fn unsupported_ideographic_text_transform_does_not_partially_override_fallback() {
        use crate::document::TextTransform;
        let html = "<html><body><p style='text-transform:uppercase;text-transform:lowercase full-width'>Abc</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("paragraph element");
        let indices = document.style_for_node(node).expect("computed paragraph style");

        assert_eq!(document.text_style(indices).expect("text style").text_transform, TextTransform::Uppercase);
    }

    #[test]
    fn invalid_typed_grid_tracks_do_not_override_the_last_valid_template() {
        use crate::document::{GridTemplateTrack, GridTrackBreadth, GridTrackSize, LengthPct};
        let html = "<html><body><div style='display:grid;grid-template-columns:100px;grid-template-columns:-10px'></div><div style='display:grid;grid-template-columns:80px;grid-template-columns:auto repeat(auto-fill,auto) auto'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let templates = find_body(document.document())
            .children()
            .map(|node| {
                let indices = document.style_for_node(node).expect("computed grid style");
                document.styles().layout_style(indices).expect("layout style").grid_template_columns.clone()
            })
            .collect::<Vec<_>>();

        assert_eq!(templates[0], vec![GridTemplateTrack::Single(GridTrackSize::Breadth(GridTrackBreadth::Length(LengthPct::Px(100.0))))]);
        assert_eq!(templates[1], vec![GridTemplateTrack::Single(GridTrackSize::Breadth(GridTrackBreadth::Length(LengthPct::Px(80.0))))]);
    }

    #[test]
    fn grid_tracks_names_flow_and_placement_cross_as_typed_values() {
        use crate::document::{GridAutoFlow, GridPlacement, GridTemplateTrack, GridTrackBreadth, GridTrackSize, LengthPct};
        let html = "<html><body><div style='display:grid;grid-template-columns:[start] 100px [middle] 1fr [end];grid-template-rows:20px;grid-auto-flow:column dense;gap:5px 10px;grid-column:2 / span 2'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("grid element");
        let indices = document.style_for_node(node).expect("computed grid style");
        let layout = document.styles().layout_style(indices).expect("layout style");

        assert_eq!(document.box_model_style(indices).expect("box style").display, Display::Grid);
        assert_eq!(layout.grid_auto_flow, GridAutoFlow::ColumnDense);
        assert_eq!(layout.grid_template_columns.len(), 2);
        assert_eq!(layout.grid_template_columns[0], GridTemplateTrack::Single(GridTrackSize::Breadth(GridTrackBreadth::Length(LengthPct::Px(100.0)))));
        assert_eq!(layout.grid_template_columns[1], GridTemplateTrack::Single(GridTrackSize::Breadth(GridTrackBreadth::Flex(1.0))));
        assert_eq!(layout.grid_template_rows, vec![GridTemplateTrack::Single(GridTrackSize::Breadth(GridTrackBreadth::Length(LengthPct::Px(20.0))))]);
        assert_eq!(layout.grid_column.start, GridPlacement::Line(std::num::NonZeroI16::new(2).unwrap()));
        assert_eq!(layout.grid_column.end, GridPlacement::Span(std::num::NonZeroU16::new(2).unwrap()));
        let line_names: Vec<Vec<&str>> = layout.grid_template_column_names.iter().map(|names| names.iter().map(|&name| document.styles().string(name).expect("interned line name")).collect()).collect();
        assert_eq!(line_names, vec![vec!["start"], vec!["middle"], vec!["end"]]);
    }

    #[test]
    fn display_contents_overrides_an_earlier_supported_fallback() {
        let html = "<html><body><div style='display:flex;display:contents'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("element");
        let indices = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(indices).expect("box style").display, Display::Contents);
    }

    #[test]
    fn authored_font_family_is_preserved_and_inherited() {
        let html = "<html><body style=\"font-family: 'DejaVu Serif'\"><p>Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let family_idx = document.font_style(style).expect("validated style handle").font_family.expect("paragraph should inherit the authored family");

        assert!(document.styles().string(family_idx).expect("family should belong to the computed-style table").contains("DejaVu Serif"));
    }

    #[test]
    fn later_font_shorthand_overrides_universal_inherit_shorthand() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline("<html><body><div id='target'>X</div></body></html>", Some("* { font: inherit; } div { font: 20px/1 Ahem; }"));
        let target = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target element");
        let style = document.style_for_node(target).expect("computed target style");

        assert_eq!(document.font_style(style).expect("font style").font_size, 20.0);
        assert_eq!(document.text_style(style).expect("text style").line_height, 20.0);
    }

    #[test]
    fn relative_font_shorthand_line_height_uses_its_resolved_font_size() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline("<html><body><div id='target'>X</div></body></html>", Some("div { font: 1.25em/1 Ahem; }"));
        let target = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target element");
        let style = document.style_for_node(target).expect("computed target style");

        assert_eq!(document.font_style(style).expect("font style").font_size, 20.0);
        assert_eq!(document.text_style(style).expect("text style").line_height, 20.0);
    }

    #[test]
    fn normal_font_weight_resolves_to_css_weight_400() {
        let html = "<html><body><p style=\"font-weight: normal\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.font_style(style).expect("validated style handle").font_weight, 400);
    }

    #[test]
    fn relative_font_weights_resolve_from_the_parent_computed_weight() {
        let html = "<html><body><div id='w100'><span id='b100'></span></div><div id='w400'><span id='b400'></span><span id='l400'></span></div><div id='w600'><span id='b600'></span><span id='l600'></span></div><div id='w800'><span id='l800'></span></div></body></html>";
        let css = "#w100{font-weight:100}#w400{font-weight:400}#w600{font-weight:600}#w800{font-weight:800}#b100,#b400,#b600{font-weight:bolder}#l400,#l600,#l800{font-weight:lighter}";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let weight = |id| {
            let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).expect("weight test element");
            let style = document.style_for_node(node).expect("weight test style");
            document.font_style(style).expect("weight test font").font_weight
        };

        assert_eq!(weight("b100"), 400);
        assert_eq!(weight("b400"), 700);
        assert_eq!(weight("b600"), 900);
        assert_eq!(weight("l400"), 100);
        assert_eq!(weight("l600"), 400);
        assert_eq!(weight("l800"), 700);
    }

    #[test]
    fn inline_custom_property_resolves_var_reference() {
        let html = "<html><body><p style=\"--accent: red; color: var(--accent);\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").color, 0xFF0000FF);
    }

    #[test]
    fn word_break_overflow_wrap_and_box_sizing_parse() {
        use crate::document::{BoxSizing, OverflowWrap, WordBreak};
        let html = "<html><body><p style=\"word-break: break-all; overflow-wrap: break-word; box-sizing: border-box\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").word_break, WordBreak::BreakAll);
        assert_eq!(document.text_style(style).expect("validated style handle").overflow_wrap, OverflowWrap::BreakWord);
        assert_eq!(document.box_model_style(style).expect("validated style handle").box_sizing, BoxSizing::BorderBox);
    }

    #[test]
    fn legacy_word_wrap_maps_to_overflow_wrap() {
        use crate::document::OverflowWrap;
        let html = "<html><body><p style=\"word-wrap: break-word\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").overflow_wrap, OverflowWrap::BreakWord);
    }

    #[test]
    fn float_and_clear_parse_into_box_model() {
        let html = "<html><body><p style=\"float:left; clear:both\">Hello</p><p style=\"float:right; clear:right\">World</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let box_model = document.box_model_style(style).expect("validated style handle");

        assert_eq!(box_model.float, Float::Left);
        assert_eq!(box_model.clear, Clear::Both);

        let right_idx = find_body(document.document()).children().nth(1).expect("second paragraph should be child of body");
        let right_style = document.style_for_node(right_idx).expect("second paragraph style should exist");
        let right_box_model = document.box_model_style(right_style).expect("validated second style handle");
        assert_eq!(right_box_model.float, Float::Right);
        assert_eq!(right_box_model.clear, Clear::Right);
    }

    #[test]
    fn line_height_stays_bound_to_final_font_size() {
        let html = "<html><body><p style=\"line-height:0.9em; font-size:450%;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let font_size = document.font_style(style).expect("validated style handle").font_size;
        let line_height = document.text_style(style).expect("validated style handle").line_height;

        assert!((line_height - font_size * 0.9).abs() < 0.01, "line-height should track the final font size");
    }

    #[test]
    fn invalid_author_css_does_not_drop_default_styles_or_boxes() {
        let html = "<html><body><p>Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p { color: red"]);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph should have computed style");

        assert_eq!(document.box_model_style(style).expect("validated style handle").display, Display::Block);
    }

    #[test]
    fn later_css_chunks_win_for_equal_specificity() {
        let html = "<html><body><p>Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p { color: red; }", "p { color: blue; }"]);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").color, 0x0000FFFF);
    }

    #[test]
    fn scope_limits_and_proximity_participate_in_matching_and_cascade() {
        let html = r#"<html><body>
            <div class="outer">
                <div class="inner"><p id="near" class="target">near</p></div>
                <div class="limit"><p id="limited" class="target">limited</p></div>
            </div>
        </body></html>"#;
        let css = r#"
            .target { color: black; }
            @scope (.outer) { .target { color: red; } }
            @scope (.inner) { .target { color: green; } }
            @scope (.outer) to (.limit) { .target { color: blue; } }
        "#;
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let color_of = |id| {
            let node = document.document().node_ids().find(|node| document.document().get_dom_id(*node) == Some(id)).expect("element by id");
            document.text_style(document.style_for_node(node).expect("computed style")).expect("text style").color
        };

        assert_eq!(color_of("near"), 0x008000FF, "the nearer scope wins after specificity");
        assert_eq!(color_of("limited"), 0xFF0000FF, "the scope limit blocks only the blue rule");
    }

    #[test]
    fn wpt_important_author_font_size_beats_inline_style_and_controls_em_line_height() {
        // Static adaptation of WPT css/css-cascade/important-vs-inline-002.html:
        // https://github.com/web-platform-tests/wpt/blob/master/css/css-cascade/important-vs-inline-002.html
        let html = "<html><body><p class=\"outer\" style=\"font-size: 24px\">Text</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(".outer { font-size: 18px !important; line-height: 2em; }"));

        let paragraph = find_body(document.document()).children().next().expect("paragraph");
        let style = document.style_for_node(paragraph).expect("paragraph style");

        assert_eq!(document.font_style(style).expect("font style").font_size, 18.0);
        assert_eq!(document.text_style(style).expect("text style").line_height, 36.0);
    }

    #[test]
    fn xhtml_author_max_width_uses_the_inherited_font_size() {
        use crate::document::{Float, PreferredSize};

        let parsed = html_parse::parse_xml_document(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><style><![CDATA[
                div#outer { font: 30px/4 Ahem; position: absolute; width: auto; }
                div#inner { float: left; max-width: 4em; }
            ]]></style></head><body><div id="outer"><div id="inner">12345678</div></div></body></html>"#,
        )
        .expect("valid XHTML");
        let css = match &parsed.head_metadata().stylesheets[0] {
            html_parse::StylesheetReference::Inline(css) => css.clone(),
            _ => panic!("expected inline stylesheet"),
        };
        let document = crate::style_document(parsed.build_dom(), &[&css]);
        let body = find_body(document.document());
        let outer = body.children().find(|node| document.document().is_element_node(*node)).expect("outer div");
        let inner = document.document().element_ref(outer).expect("outer element").children().find(|node| document.document().is_element_node(*node)).expect("inner div");
        let style = document.style_for_node(inner).expect("inner style");

        assert_eq!(document.font_style(style).expect("font style").font_size, 30.0);
        assert_eq!(document.box_model_style(style).expect("box style").float, Float::Left);
        assert_eq!(document.box_model_style(style).expect("box style").max_width, PreferredSize::Px(120.0));
    }

    #[test]
    fn wpt_unset_inherits_color_but_resets_non_inherited_margin() {
        // Semantic subset of WPT css/css-cascade/all-prop-unset-color.html.
        // `all: unset` is decomposed here into supported longhands so this test
        // stays local to CSS-wide keyword resolution:
        // https://github.com/web-platform-tests/wpt/blob/master/css/css-cascade/all-prop-unset-color.html
        let html = "<html><body><div style=\"color: red; margin-left: 20px\"><span style=\"color: unset; margin-left: unset\">Text</span></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let parent = find_body(document.document()).children().next().expect("parent div");
        let child = document.document().element_ref(parent).expect("parent element").children().find(|node| document.document().is_element_node(*node)).expect("child span");
        let style = document.style_for_node(child).expect("child style");

        assert_eq!(document.text_style(style).expect("text style").color, 0xFF0000FF);
        assert_eq!(document.box_model_style(style).expect("box style").margin_left, crate::document::LengthPct::Px(0.0));
    }

    #[test]
    fn current_color_border_uses_text_color() {
        let html = "<html><body><p style=\"color: red; border: 1px solid\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let border = document.border_style(style).expect("validated style handle");

        assert_eq!(border.border_top_color, 0xFF0000FF, "borders without an explicit color should use currentColor");
    }

    #[test]
    fn inherited_background_current_color_resolves_against_the_descendant_color() {
        let html = "<html><body><div style='color:red;background-color:currentcolor'><div><div id='target' style='color:green'>text</div></div></div></body></html>";
        let css = "div div { background-color: inherit }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));

        let target = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target element");
        let style = document.style_for_node(target).expect("target style");
        let background = document.background_style(style).expect("background style");

        assert!(background.background_color_current_color, "inherit must preserve the currentColor dependency");
        assert_eq!(document.style_view(style).expect("style view").background_color(), 0x008000FF);
    }

    #[test]
    fn em_length_uses_final_font_size_regardless_of_order() {
        let html = "<html><body><p style=\"padding-left: 1em; font-size: 32px;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let padding_left = document.box_model_style(style).expect("validated style handle").padding_left;

        assert_eq!(padding_left, crate::document::LengthPct::Px(32.0), "1em padding should resolve against the element's final font size");
    }

    #[test]
    fn em_length_sees_font_size_from_later_rule() {
        let html = "<html><body><p>Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p { padding-left: 1em; }", "p { font-size: 32px; }"]);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let padding_left = document.box_model_style(style).expect("validated style handle").padding_left;

        assert_eq!(padding_left, crate::document::LengthPct::Px(32.0), "em lengths should see font-size from any rule in the cascade");
    }

    #[test]
    fn descendant_rem_uses_the_computed_root_font_size() {
        let html = "<html><body><div id='target'></div></body></html>";
        let css = ":root { font-size: 25%; } #target { width: 25rem; height: 25rem; }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let target = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.box_model_style(document.style_for_node(target).expect("target style")).expect("box style");

        assert_eq!(style.width, PreferredSize::Px(100.0));
        assert_eq!(style.height, PreferredSize::Px(100.0));
    }

    #[test]
    fn root_font_units_remain_symbolic_until_root_metrics_are_available() {
        let html = "<html><body><div id='cap' style='font-size:1cap;width:2em'></div><div id='rch' style='font-size:1rch'></div><div id='rcap' style='font-size:1rcap'></div><div id='rlh' style='font-size:1rlh'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let by_id = |id| document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).expect("target");
        let cap = document.style_for_node(by_id("cap")).expect("cap style");
        let rch = document.style_for_node(by_id("rch")).expect("rch style");
        let rcap = document.style_for_node(by_id("rcap")).expect("rcap style");
        let rlh = document.style_for_node(by_id("rlh")).expect("rlh style");

        assert_eq!(document.font_style(cap).expect("cap font").font_size_cap_height_px, 16.0);
        assert!(matches!(document.box_model_style(cap).expect("cap box").width, PreferredSize::Calc { cap_height_px, .. } if cap_height_px == 32.0));
        assert_eq!(document.font_style(rch).expect("rch font").font_size_root_ch, 1.0);
        assert_eq!(document.font_style(rcap).expect("rcap font").font_size_root_cap_height, 1.0);
        assert_eq!(document.font_style(rlh).expect("rlh font").font_size_root_line_height, 1.0);
        assert_eq!(document.styles().used_view_with_root(cap, 0.5, 0.5, 0.8, 9.0, 13.0, 20.0).expect("used cap").font_size(), 12.8);
        assert_eq!(document.styles().used_view_with_root(rch, 0.5, 0.5, 0.8, 9.0, 13.0, 20.0).expect("used rch").font_size(), 9.0);
        assert_eq!(document.styles().used_view_with_root(rcap, 0.5, 0.5, 0.8, 9.0, 13.0, 20.0).expect("used rcap").font_size(), 13.0);
        assert_eq!(document.styles().used_view_with_root(rlh, 0.5, 0.5, 0.8, 9.0, 13.0, 20.0).expect("used rlh").font_size(), 20.0);
    }

    #[test]
    fn text_align_last_survives_later_text_align_rule() {
        use crate::document::TextAlign;
        let html = "<html><body><p>Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p { text-align-last: center; }", "p { text-align: justify; }"]);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let text = document.text_style(style).expect("validated style handle");

        assert_eq!(text.text_align, TextAlign::Justify);
        assert_eq!(text.text_align_last, TextAlign::Center, "explicit text-align-last should not be clobbered by a later text-align rule");
    }

    #[test]
    fn text_align_still_syncs_text_align_last_when_not_explicit() {
        use crate::document::TextAlign;
        let html = "<html><body><p style=\"text-align: center;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").text_align_last, TextAlign::Center);
    }

    #[test]
    fn absolute_length_units_convert_to_px() {
        let html = "<html><body><p style=\"width: 1in; height: 2.54cm;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let box_model = document.box_model_style(style).expect("validated style handle");

        assert_eq!(box_model.width, crate::document::PreferredSize::Px(96.0));
        assert_eq!(box_model.height, crate::document::PreferredSize::Px(96.0));
    }

    #[test]
    fn background_shorthand_color_comes_from_last_layer() {
        let html = "<html><body><p style=\"background: none, none blue;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.background_style(style).expect("validated style handle").background_color, 0x0000FFFF);
    }

    #[test]
    fn unsupported_gradient_does_not_erase_background_fallback() {
        let html = "<html><body><p style=\"background: red; background: linear-gradient(white, black);\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        let background = document.background_style(style).expect("validated style handle");
        assert_eq!(background.background_color, 0xFF0000FF);
        assert!(background.background_image_present, "unsupported painting must not erase the computed image layer's presence");
    }

    #[test]
    fn background_shorthand_color_paints_under_an_unsupported_image() {
        let html = "<html><body><p style=\"background: red; background: url('paper.png') blue;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        let background = document.background_style(style).expect("validated style handle");
        assert_eq!(background.background_color, 0x0000FFFF);
        assert!(background.background_image_present, "unsupported painting must not erase the computed image layer's presence");
    }

    #[test]
    fn variable_substituted_background_shorthand_keeps_its_color() {
        let html = "<html><body><p style=\"background-color:red;--a:url(nothing) green;background:var(--a)\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        let background = document.background_style(style).expect("validated style handle");
        assert_eq!(background.background_color, 0x008000FF);
        assert!(background.background_image_present);
    }

    #[test]
    fn text_decoration_and_outline_cross_as_typed_paint_values() {
        use crate::document::{DecorationColor, TextDecorationStyle};
        let html = "<html><body><span style='color:black;text-decoration:underline 2px dashed #123456;outline:4px dotted #abcdef'>A</span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("decorated span");
        let indices = document.style_for_node(node).expect("decorated style");
        let paint = document.background_style(indices).expect("paint style");

        assert!(paint.text_decoration.lines.underline());
        assert_eq!(paint.text_decoration.style, TextDecorationStyle::Dashed);
        assert_eq!(paint.text_decoration.color, DecorationColor::Rgba(0x123456FF));
        assert_eq!(paint.text_decoration.thickness, crate::document::TextDecorationThickness::Length(LengthPct::Px(2.0)));
        assert_eq!(paint.outline.width(), FontRelativeLength::px(4.0).unwrap());
        assert_eq!(paint.outline.style, BorderStyle::Dotted);
        assert_eq!(paint.outline.color, DecorationColor::Rgba(0xABCDEFFF));
    }

    #[test]
    fn ex_paint_and_grid_values_cross_only_through_validated_used_types() {
        use crate::document::{UsedGridTemplateTrack, UsedGridTrackBreadth, UsedGridTrackSize, UsedLengthPct};
        let html = "<html><body><div style='font-size:20px;outline:2ex solid;display:grid;grid-template-columns:3ex'>A</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("grid div");
        let indices = document.style_for_node(node).expect("grid style");
        let computed = document.styles().view(indices).expect("validated computed style");

        assert!(computed.requires_font_metrics());
        assert!(document.styles().used_view(indices, f32::NAN, 0.5, 0.8).is_none());
        assert!(document.styles().used_view(indices, 0.5, -0.1, 0.8).is_none());
        assert!(document.styles().used_view(indices, 0.5, 0.0, 0.8).is_some());

        let used = document.styles().used_view(indices, 0.6, 0.5, 0.8).expect("positive finite font-relative ratios");
        assert!((used.outline().width() - 24.0).abs() < 0.001);
        assert_eq!(used.grid_template_columns().collect::<Vec<_>>(), vec![UsedGridTemplateTrack::Single(UsedGridTrackSize::Breadth(UsedGridTrackBreadth::Length(UsedLengthPct::Px(36.0))))]);
    }

    #[test]
    fn metric_dependent_values_cross_only_when_the_model_has_a_closed_representation() {
        let html = "<html><body><p style='font-size:20px;font-size:2ex;line-height:24px;line-height:2ex;letter-spacing:3px;letter-spacing:2ex;width:10px;width:2ch'>A</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("paragraph");
        let indices = document.style_for_node(node).expect("paragraph style");
        let view = document.styles().view(indices).expect("validated style");
        let used = document.styles().used_view(indices, 0.6, 0.5, 0.8).expect("valid selected-face metrics");

        assert_eq!(view.font_size(), 20.0);
        assert_eq!(view.line_height(), 0.0);
        assert_eq!(used.line_height(), 24.0);
        assert_eq!(view.letter_spacing(), 3.0);
        assert_eq!(view.width(), PreferredSize::Ch(40.0));
    }

    #[test]
    fn unsupported_decoration_and_outline_styles_preserve_atomic_fallbacks() {
        use crate::document::{DecorationColor, TextDecorationStyle};
        let html = "<html><body><span style='text-decoration:underline solid red;text-decoration:line-through wavy blue;outline:2px solid red;outline:4px double blue'>A</span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("decorated span");
        let indices = document.style_for_node(node).expect("decorated style");
        let paint = document.background_style(indices).expect("paint style");

        assert!(paint.text_decoration.lines.underline());
        assert!(!paint.text_decoration.lines.line_through());
        assert_eq!(paint.text_decoration.style, TextDecorationStyle::Solid);
        assert_eq!(paint.text_decoration.color, DecorationColor::Rgba(0xFF0000FF));
        assert_eq!(paint.outline.width(), FontRelativeLength::px(2.0).unwrap());
        assert_eq!(paint.outline.style, BorderStyle::Solid);
        assert_eq!(paint.outline.color, DecorationColor::Rgba(0xFF0000FF));
    }

    #[test]
    fn decoration_current_color_resolves_from_the_final_text_color() {
        let html = "<html><body><span style='text-decoration:underline;outline:1px solid;color:#2468ac'>A</span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("decorated span");
        let indices = document.style_for_node(node).expect("decorated style");
        let view = document.styles().view(indices).expect("validated style view");

        assert_eq!(view.text_decoration_color(), 0x2468ACFF);
        assert_eq!(view.outline_color(), 0x2468ACFF);
    }

    #[test]
    fn float_keyword_is_case_insensitive() {
        let html = "<html><body><p style=\"FLOAT: LEFT\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.box_model_style(style).expect("validated style handle").float, Float::Left);
    }

    #[test]
    fn unitless_line_height_recomputes_for_child_font_size() {
        let html = "<html><body style=\"line-height: 1.5;\"><p style=\"font-size: 32px;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let line_height = document.text_style(style).expect("validated style handle").line_height;

        assert!((line_height - 48.0).abs() < 0.01, "unitless line-height should recompute against the child's font size, got {line_height}");
    }

    #[test]
    fn length_line_height_inherits_as_px() {
        let html = "<html><body style=\"line-height: 20px;\"><p style=\"font-size: 32px;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let line_height = document.text_style(style).expect("validated style handle").line_height;

        assert!((line_height - 20.0).abs() < 0.01, "length line-height should inherit as resolved px, got {line_height}");
    }

    #[test]
    fn authored_zero_line_height_remains_distinct_from_normal() {
        let html = "<html><body><div style='line-height:0'>zero</div><div style='line-height:normal'>normal</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let mut children = find_body(document.document()).children();
        let zero = document.styles().view(document.style_for_node(children.next().expect("zero div")).expect("zero style")).expect("valid zero style");
        let normal = document.styles().view(document.style_for_node(children.next().expect("normal div")).expect("normal style")).expect("valid normal style");

        assert_eq!(zero.line_height(), 0.0);
        assert!(!zero.line_height_is_normal());
        assert_eq!(normal.line_height(), 0.0);
        assert!(normal.line_height_is_normal());
    }

    #[test]
    fn font_shorthand_and_line_height_longhand_follow_cascade_order() {
        let html = "<html><body><div style='font:20px/1 Ahem;line-height:0'>zero</div><div style='line-height:0;font:20px/1 Ahem'>twenty</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let mut children = find_body(document.document()).children();
        let zero = document.styles().view(document.style_for_node(children.next().expect("zero div")).expect("zero style")).expect("valid zero style");
        let twenty = document.styles().view(document.style_for_node(children.next().expect("twenty div")).expect("twenty style")).expect("valid twenty style");

        assert_eq!(zero.font_size(), 20.0);
        assert_eq!(zero.line_height(), 0.0);
        assert!(!zero.line_height_is_normal());
        assert_eq!(twenty.font_size(), 20.0);
        assert_eq!(twenty.line_height(), 20.0);
        assert!(!twenty.line_height_is_normal());
    }

    #[test]
    fn ex_line_height_is_deferred_until_selected_font_metrics_are_available() {
        let html = "<html><body><div style='line-height:6ex;font-size:20px'>text</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let div = find_body(document.document()).children().next().expect("div");
        let indices = document.style_for_node(div).expect("div style");
        let computed = document.styles().view(indices).expect("valid computed style");
        let used = document.styles().used_view(indices, 0.8, 0.5, 0.8).expect("valid selected-face metrics");

        assert_eq!(computed.line_height(), 0.0);
        assert!(computed.requires_font_metrics());
        assert_eq!(used.line_height(), 96.0);
    }

    #[test]
    fn structural_pseudo_classes_match() {
        let html = "<html><body><p>One</p><p>Two</p><p>Three</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p:first-child { color: red; } p:nth-child(2) { color: blue; } p:last-child { color: green; }"]);

        let doc = document.document();
        let paragraphs: Vec<_> = find_body(doc).children().collect();
        let color_of = |idx| document.text_style(document.style_for_node(idx).expect("style")).expect("validated style handle").color;

        assert_eq!(color_of(paragraphs[0]), 0xFF0000FF, ":first-child should match the first paragraph");
        assert_eq!(color_of(paragraphs[1]), 0x0000FFFF, ":nth-child(2) should match the second paragraph");
        assert_eq!(color_of(paragraphs[2]), 0x008000FF, ":last-child should match the third paragraph");
    }

    #[test]
    fn not_first_child_excludes_the_first_child() {
        let html = "<html><body><p>One</p><p>Two</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p:not(:first-child) { color: red; }"]);

        let doc = document.document();
        let paragraphs: Vec<_> = find_body(doc).children().collect();
        let color_of = |idx| document.text_style(document.style_for_node(idx).expect("style")).expect("validated style handle").color;

        assert_eq!(color_of(paragraphs[0]), 0x000000FF, ":not(:first-child) must not match the first child");
        assert_eq!(color_of(paragraphs[1]), 0xFF0000FF);
    }

    #[test]
    fn rule_cascades_with_most_specific_matching_selector() {
        let html = "<html><body><p class=\"foo\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p, .foo { color: red; }", "p { color: blue; }"]);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").color, 0xFF0000FF, "the .foo selector's specificity should win over the later p rule");
    }

    #[test]
    fn adjacent_replaced_element_can_be_floated() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline("<html><body><img src='a'/> <img src='b'/></body></html>", Some("img + img { float: right; }"));
        let images = find_body(document.document()).children().filter(|&node| document.document().get_dom_tag(node) == Some("img")).collect::<Vec<_>>();

        let first = document.box_model_style(document.style_for_node(images[0]).expect("first image style")).expect("box model");
        let second = document.box_model_style(document.style_for_node(images[1]).expect("second image style")).expect("box model");
        assert_eq!(first.float, Float::None);
        assert_eq!(second.float, Float::Right);
    }

    #[test]
    fn undeclared_border_color_defaults_to_current_color() {
        let html = "<html><body><p style=\"color: red; border-width: 2px; border-style: solid;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let border = document.border_style(style).expect("validated style handle");

        assert_eq!(border.border_top_color, 0xFF0000FF, "border-color's initial value is currentColor");
        assert_eq!(border.border_left_color, 0xFF0000FF);
    }

    #[test]
    fn only_anchors_with_href_get_link_styling() {
        let html = "<html><body><a id=\"target\">Anchor</a><a href=\"x.html\">Link</a></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let doc = document.document();
        let anchors: Vec<_> = find_body(doc).children().collect();
        let plain = document.style_for_node(anchors[0]).expect("anchor style");
        let link = document.style_for_node(anchors[1]).expect("link style");

        assert_eq!(document.text_style(plain).expect("validated style handle").color, 0x000000FF, "href-less anchor must not be link-colored");
        assert!(!document.background_style(plain).expect("validated style handle").text_decoration.lines.underline(), "href-less anchor must not be underlined");
        assert_eq!(document.text_style(link).expect("validated style handle").color, 0x0000FFFF);
        assert!(document.background_style(link).expect("validated style handle").text_decoration.lines.underline(), "link should be underlined");
    }

    #[test]
    fn vertical_align_inherit_propagates_tbody_middle_to_cells() {
        use crate::document::VerticalAlignValue;
        let html = "<html><body><table><tbody><tr><td>X</td></tr></tbody></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let doc = document.document();
        let mut node = find_body(doc).node_id();
        for _ in 0..4 {
            // table > tbody > tr > td
            node = doc.element_ref(node).expect("element").children().find(|&child| doc.element_ref(child).is_some()).expect("child element");
        }
        let td = doc.element_ref(node).expect("td");
        assert_eq!(td.tag(), "td");
        let style = document.style_for_node(node).expect("td style should exist");

        assert_eq!(document.box_model_style(style).expect("validated style handle").vertical_align, VerticalAlignValue::Middle, "the UA sheet's vertical-align: inherit chain should reach the cell");
    }

    #[test]
    fn vertical_align_css_two_values_preserve_typed_offsets() {
        use crate::document::VerticalAlignValue;
        let html = "<html><body><span style='vertical-align: text-top'>A</span><span style='vertical-align: 25%'>B</span><span style='vertical-align: calc(20% - 2px)'>C</span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let spans: Vec<_> = find_body(document.document()).children().collect();
        let text_top = document.box_model_style(document.style_for_node(spans[0]).expect("first span style")).expect("validated first span style").vertical_align;
        let percentage = document.box_model_style(document.style_for_node(spans[1]).expect("second span style")).expect("validated second span style").vertical_align;
        let calc = document.box_model_style(document.style_for_node(spans[2]).expect("third span style")).expect("validated third span style").vertical_align;

        assert_eq!(text_top, VerticalAlignValue::TextTop);
        assert_eq!(percentage, VerticalAlignValue::Percent(0.25));
        assert_eq!(calc, VerticalAlignValue::Calc { absolute_px: -2.0, line_height_fraction: 0.2, x_height_px: 0.0 });
    }

    #[test]
    fn ex_vertical_align_is_deferred_until_selected_font_metrics_are_available() {
        use crate::document::VerticalAlignValue;
        let html = "<html><body><span style='font-size:20px;vertical-align:6ex'>A</span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let span = find_body(document.document()).children().next().expect("span");
        let indices = document.style_for_node(span).expect("span style");
        let computed = document.styles().view(indices).expect("valid computed style");
        let used = document.styles().used_view(indices, 0.8, 0.5, 0.8).expect("valid selected-face metrics");

        assert_eq!(computed.vertical_align(), VerticalAlignValue::Calc { absolute_px: 0.0, line_height_fraction: 0.0, x_height_px: 120.0 });
        assert!(computed.requires_font_metrics());
        assert_eq!(used.vertical_align(), VerticalAlignValue::Calc { absolute_px: 96.0, line_height_fraction: 0.0, x_height_px: 0.0 });
    }

    #[test]
    fn parser_supported_text_overflow_reaches_computed_box_style() {
        use crate::document::{OverflowMode, TextOverflow};
        let html = "<html><body><div style='overflow: hidden clip; text-overflow: ellipsis'>A</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let div = find_body(document.document()).children().next().expect("div should be a child of body");
        let style = document.style_for_node(div).expect("div style should exist");
        let box_style = document.box_model_style(style).expect("validated box style");

        assert_eq!(box_style.overflow_x, OverflowMode::Hidden);
        assert_eq!(box_style.overflow_y, OverflowMode::Clip);
        assert_eq!(box_style.text_overflow, TextOverflow::Ellipsis);
    }

    #[test]
    fn overflow_axes_are_normalized_after_the_cascade() {
        use crate::document::OverflowMode;
        let html = "<html><body><div style='overflow-x: visible; overflow-y: hidden'>A</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let div = find_body(document.document()).children().next().expect("div should be a child of body");
        let style = document.style_for_node(div).expect("div style should exist");
        let box_style = document.box_model_style(style).expect("validated box style");

        assert_eq!(box_style.overflow_x, OverflowMode::Auto);
        assert_eq!(box_style.overflow_y, OverflowMode::Hidden);
    }

    #[test]
    fn unsupported_custom_text_overflow_marker_does_not_override_ellipsis() {
        use crate::document::TextOverflow;
        let html = "<html><body><div style='text-overflow: ellipsis; text-overflow: \"marker\"'>A</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let div = find_body(document.document()).children().next().expect("div should be a child of body");
        let style = document.style_for_node(div).expect("div style should exist");

        assert_eq!(document.box_model_style(style).expect("validated box style").text_overflow, TextOverflow::Ellipsis);
    }

    #[test]
    fn text_align_initial_resets_inherited_alignment() {
        use crate::document::TextAlign;
        let html = "<html><body style=\"text-align: center;\"><p style=\"text-align: initial;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").text_align, TextAlign::Left);
    }

    #[test]
    fn list_item_keeps_inherited_text_align() {
        use crate::document::TextAlign;
        // The UA sheet's `li { text-align: match-parent }` must not reset
        // inherited justification.
        let html = "<html><body style=\"text-align: justify;\"><ul><li>Hello</li></ul></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let doc = document.document();
        let ul_idx = find_body(doc).children().next().expect("ul should be child of body");
        let li_idx = doc.element_ref(ul_idx).expect("ul element").children().next().expect("li should be child of ul");
        let style = document.style_for_node(li_idx).expect("li style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").text_align, TextAlign::Justify);
    }

    #[test]
    fn mark_system_colors_resolve_to_yellow_on_black() {
        let html = "<html><body><mark>Hello</mark></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let mark_idx = find_body(document.document()).children().next().expect("mark should be child of body");
        let style = document.style_for_node(mark_idx).expect("mark style should exist");

        assert_eq!(document.background_style(style).expect("validated style handle").background_color, 0xFFFF00FF, "Mark system color should be yellow");
        assert_eq!(document.text_style(style).expect("validated style handle").color, 0x000000FF, "MarkText system color should be black");
    }

    #[test]
    fn deprecated_system_colors_resolve_to_their_required_aliases() {
        use super::system_color_to_u32;
        use lightningcss::values::color::SystemColor;

        assert_eq!(system_color_to_u32(&SystemColor::ActiveCaption), system_color_to_u32(&SystemColor::Canvas));
        assert_eq!(system_color_to_u32(&SystemColor::Background), system_color_to_u32(&SystemColor::Canvas));
        assert_eq!(system_color_to_u32(&SystemColor::ButtonHighlight), system_color_to_u32(&SystemColor::ButtonFace));
        assert_eq!(system_color_to_u32(&SystemColor::ButtonShadow), system_color_to_u32(&SystemColor::ButtonFace));
        assert_eq!(system_color_to_u32(&SystemColor::InactiveCaption), system_color_to_u32(&SystemColor::Canvas));
        assert_eq!(system_color_to_u32(&SystemColor::InactiveCaptionText), system_color_to_u32(&SystemColor::GrayText));
        assert_eq!(system_color_to_u32(&SystemColor::ThreeDFace), system_color_to_u32(&SystemColor::ButtonFace));
    }

    #[test]
    fn modern_colors_convert_to_the_renderers_srgb_storage() {
        let html = "<html><body><p style=\"color: alpha(from red / 50%); background-color: lab(100% 0 0);\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").color, 0xFF000080);
        assert_eq!(document.background_style(style).expect("validated style handle").background_color, 0xFFFFFFFF);
    }

    #[test]
    fn current_color_on_color_computes_to_the_inherited_color() {
        let html = "<html><body><div style='color:green'><p style='color:red;color:currentColor'>Hello</p></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let div = find_body(document.document()).children().next().expect("div should be child of body");
        let paragraph = document.document().element_ref(div).expect("div element").children().next().expect("paragraph should be child of div");
        let style = document.style_for_node(paragraph).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").color, 0x008000FF);
    }

    #[test]
    fn hr_inset_border_renders_as_solid_gray() {
        let html = "<html><body><hr></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let hr_idx = find_body(document.document()).children().next().expect("hr should be child of body");
        let style = document.style_for_node(hr_idx).expect("hr style should exist");
        let border = document.border_style(style).expect("validated style handle");

        assert_eq!(border.border_top_style, crate::document::BorderStyle::Solid, "the UA sheet's `border-style: inset` should paint as solid");
        assert_eq!(border.border_top_width, FontRelativeLength::px(1.0).unwrap());
        assert_eq!(border.border_top_color, 0x808080FF, "undeclared border color should take hr's gray text color");
    }

    #[test]
    fn resolves_before_and_after_content_into_typed_pseudo_styles() {
        use html_style_model::{GeneratedContentItem, StyleStringId};

        let html = "<html><body><div data-x='B'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("div:before{content:'A' attr(data-x);color:green}div:after{content:'Z'}"));
        let div = find_body(document.document()).children().next().expect("div should be a child of body");
        let (before_style, before, _) = document.styles.before_style_for_node(div).expect("before pseudo should have computed content");
        let (_, after, _) = document.styles.after_style_for_node(div).expect("after pseudo should have computed content");

        let string = |id: StyleStringId| document.styles.string(id).expect("generated string should belong to style result");
        assert!(matches!(before.items.as_slice(), [GeneratedContentItem::Text(a), GeneratedContentItem::Attribute(x)] if string(*a) == "A" && string(*x) == "data-x"));
        assert!(matches!(after.items.as_slice(), [GeneratedContentItem::Text(z)] if string(*z) == "Z"));
        assert_eq!(document.text_style(before_style).expect("validated pseudo style").color, 0x008000FF);
        assert_eq!(document.box_model_style(before_style).expect("validated pseudo box style").display, Display::Inline);
    }

    #[test]
    fn pseudo_display_inherit_copies_the_originating_elements_display() {
        let html = "<html><body><div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("div:before{content:'A';display:inherit}"));
        let div = find_body(document.document()).children().next().expect("div should be a child of body");
        let (before_style, _, _) = document.styles.before_style_for_node(div).expect("before pseudo should exist");

        assert_eq!(document.box_model_style(before_style).expect("validated pseudo box style").display, Display::Block);
    }

    fn find_body(document: &Document) -> ElementRef<'_> {
        let html_idx = document.dom_root().expect("html root should exist");
        let html = document.element_ref(html_idx).expect("html should be element");
        let body_idx = html.children().find(|&child_idx| document.element_ref(child_idx).is_some_and(|element| element.tag() == "body")).expect("body should be child of html");
        document.element_ref(body_idx).expect("body should be element")
    }
}
