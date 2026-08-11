use super::*;

#[test]
fn matched_rule_hot_path_record_stays_compact() {
    assert!(std::mem::size_of::<MatchedRule>() <= 12);
}

#[test]
fn style_sharing_keeps_parent_inheritance_in_the_cache_key() {
    let html = "<html><body><div class='red'><span id='red' class='child'>x</span></div><div class='green'><span id='green' class='child'>x</span></div></body></html>";
    let mut factory = DocumentFactory::new();
    let document = factory.parse_with_new_pipeline(html, Some(".red { color: red } .green { color: green } .child { color: inherit }"));
    let color = |id| {
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).unwrap();
        document.text_style(document.style_for_node(node).unwrap()).unwrap().color
    };

    assert_eq!(color("red"), 0xff0000ff);
    assert_eq!(color("green"), 0x008000ff);
}

#[test]
fn style_sharing_keeps_structural_selector_results_distinct() {
    let html = "<html><body><p id='first' class='item'>x</p><p id='second' class='item'>x</p></body></html>";
    let mut factory = DocumentFactory::new();
    let document = factory.parse_with_new_pipeline(html, Some(".item { color: green } .item:first-child { color: red }"));
    let color = |id| {
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).unwrap();
        document.text_style(document.style_for_node(node).unwrap()).unwrap().color
    };

    assert_eq!(color("first"), 0xff0000ff);
    assert_eq!(color("second"), 0x008000ff);
}

#[test]
fn style_sharing_reuses_custom_maps_and_copies_counter_directives() {
    let html = "<html><body><div class='item'><span id='first'>x</span></div><div class='item'><span id='second'>x</span></div></body></html>";
    let mut factory = DocumentFactory::new();
    let document = factory.parse_with_new_pipeline(html, Some(".item { --tone: green; counter-reset: chapter 3 } .item span { color: var(--tone) }"));
    let nodes = ["first", "second"].map(|id| document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).unwrap());
    for node in nodes {
        let style = document.style_for_node(node).unwrap();
        assert_eq!(document.text_style(style).unwrap().color, 0x008000ff);
        let parent = document.document().get_dom_parent(node).unwrap();
        let counters = document.styles().counter_directives_for_node(parent).expect("shared parent keeps counter directives");
        assert_eq!(counters.resets.len(), 1);
        assert_eq!(counters.resets[0].value, 3);
    }
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
