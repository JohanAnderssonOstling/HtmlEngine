use super::*;

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
