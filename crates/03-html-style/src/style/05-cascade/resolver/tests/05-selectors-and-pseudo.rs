use super::*;

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
