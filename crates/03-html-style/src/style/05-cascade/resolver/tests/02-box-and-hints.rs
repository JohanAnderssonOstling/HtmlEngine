use super::*;

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

    assert_eq!((border.border_top_width, border.border_right_width, border.border_bottom_width, border.border_left_width), (FontRelativeLength::px(1.0).unwrap(), FontRelativeLength::px(2.0).unwrap(), FontRelativeLength::px(3.0).unwrap(), FontRelativeLength::px(4.0).unwrap()));
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
    let html = "<html><body><img id='percentage' src='x' width='50%' height='25%'><img id='pixels' src='x' width='80' height='3'><img id='overridden' src='x' width='50%' style='width:75%'><iframe id='frame' width='44' height='33'></iframe><svg id='svg' width='40' height='50%'></svg></body></html>";
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
    assert_eq!(style_for("frame").width, PreferredSize::Px(44.0));
    assert_eq!(style_for("frame").height, PreferredSize::Px(33.0));
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
