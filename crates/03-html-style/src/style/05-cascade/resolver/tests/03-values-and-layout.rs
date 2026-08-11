use super::*;

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
    let document = factory.parse_with_new_pipeline("<html><body style='line-height:20px'><div id='line'>Line</div><div id='letter'>Letter</div></body></html>", Some("#line::first-line { line-height:100px } #letter::first-letter { line-height:80px }"));
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
