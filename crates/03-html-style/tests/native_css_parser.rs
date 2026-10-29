use html_parse::parse_dom_document;
use html_style::{
    PropertySyntax, StyledDocument, declaration_syntax, selector_syntax_is_valid, style_document,
};
use html_style_model::{
    BorderCollapseMode, Float, GeneratedContentItem, LengthPct, PreferredSize, StyleView,
    TableLayoutMode, WhiteSpace,
};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::ParserOptions;

fn styled(css: &str, inline: &str) -> StyledDocument {
    let document = parse_dom_document(&format!(
        "<!doctype html><p id='target' style=\"{inline}\">Text</p>"
    ))
    .unwrap();
    style_document(document, &[css])
}

fn target(document: &StyledDocument) -> html_dom::DomNodeId {
    document
        .document()
        .node_ids()
        .find(|&node| document.document().get_dom_id(node) == Some("target"))
        .unwrap()
}

fn view(document: &StyledDocument) -> StyleView<'_> {
    document
        .style_view(document.style_for_node(target(document)).unwrap())
        .unwrap()
}

#[test]
fn line_relative_units_use_initial_parent_and_current_bases_without_cycles() {
    for (root_font, child_font) in [
        (
            "font-size:2lh;line-height:3lh",
            "font-size:1lh;line-height:2lh",
        ),
        ("font:2lh/3lh serif", "font:1lh/2lh serif"),
    ] {
        let html = format!(
            "<html style='{root_font};margin-top:1lh;padding-top:1rlh'>\
             <body style='font-size:1lh;line-height:2lh'>\
             <p id='target' style='{child_font};width:2lh;height:3rlh'>Text</p></body></html>"
        );
        let document = style_document(
            parse_dom_document(&html).unwrap(),
            &["#target::before {content:'Before';font:1lh/2lh serif;width:2lh;height:3rlh}"],
        );
        let initial_line_height = document.document().root_font_size() * 1.2;
        let root_line_height = initial_line_height * 3.0;
        let child_line_height = root_line_height * 4.0;
        let root = document.document().dom_root().unwrap();
        let root_style = document
            .style_view(document.style_for_node(root).unwrap())
            .unwrap();
        let close = |actual: f32, expected: f32| {
            assert!(
                (actual - expected).abs() < 0.001,
                "{actual} != {expected}: {root_font}"
            );
        };
        close(root_style.font.font_size, initial_line_height * 2.0);
        close(root_style.text.line_height, root_line_height);
        assert_eq!(
            root_style.box_model.margin_top,
            LengthPct::Px(root_line_height)
        );
        assert_eq!(
            root_style.box_model.padding_top,
            LengthPct::Px(root_line_height)
        );
        let child = view(&document);
        close(child.font.font_size, root_line_height * 2.0);
        close(child.text.line_height, child_line_height);
        assert_eq!(
            child.box_model.width,
            PreferredSize::Px(child_line_height * 2.0)
        );
        assert_eq!(
            child.box_model.height,
            PreferredSize::Px(root_line_height * 3.0)
        );
        let (pseudo, _, _) = document
            .styles()
            .before_style_for_node(target(&document))
            .unwrap();
        let pseudo = document.style_view(pseudo).unwrap();
        close(pseudo.font.font_size, child_line_height);
        close(pseudo.text.line_height, child_line_height * 2.0);
        assert_eq!(
            pseudo.box_model.width,
            PreferredSize::Px(child_line_height * 4.0)
        );
        assert_eq!(
            pseudo.box_model.height,
            PreferredSize::Px(root_line_height * 3.0)
        );
    }
}

#[test]
fn native_typed_values_are_converted_in_stylesheets_and_inline_styles() {
    for (css, inline) in [
        (
            "#target {width:calc(10px + 2em);font-size:20px;tab-size:3}",
            "",
        ),
        ("", "width:calc(10px + 2em);font-size:20px;tab-size:3"),
    ] {
        let document = styled(css, inline);
        let PreferredSize::Calc { absolute_px, .. } = view(&document).box_model.width else {
            panic!("native calculated size")
        };
        assert_eq!(absolute_px, 50.0);
        assert_eq!(view(&document).text.tab_size.value(), 3.0);
    }
}

#[test]
fn raw_token_conversion_preserves_floats_tables_font_features_and_generated_content() {
    let document = styled(
        "#target::before {content:'Hello';color:red}",
        "float:left;table-layout:fixed;border-collapse:collapse;font-feature-settings:'liga' 0",
    );
    let style = view(&document);
    assert_eq!(style.box_model.float, Float::Left);
    assert_eq!(style.box_model.table_layout, TableLayoutMode::Fixed);
    assert_eq!(
        style.box_model.border_collapse,
        BorderCollapseMode::Collapse
    );
    assert_eq!(style.font.font_feature_settings[0].tag(), *b"liga");
    assert_eq!(style.font.font_feature_settings[0].value(), 0);
    let (_, content, _) = document
        .styles()
        .before_style_for_node(target(&document))
        .unwrap();
    let [GeneratedContentItem::Text(text)] = content.items.as_slice() else {
        panic!("generated string")
    };
    assert_eq!(document.styles().string(*text), Some("Hello"));
}

#[test]
fn expressions_without_native_typed_values_keep_the_authored_fallback() {
    for expression in [
        "random(fixed 0,10px,20px)",
        "calc(1px * 1px / 1px)",
        "clamp(none,20px,30px)",
    ] {
        assert!(
            matches!(
                Property::parse_string(PropertyId::Width, expression, ParserOptions::default())
                    .unwrap(),
                Property::Unparsed(_)
            ),
            "{expression}"
        );
        let document = styled("", &format!("width:40px;width:{expression}"));
        assert_eq!(
            view(&document).box_model.width,
            PreferredSize::Px(40.0),
            "{expression}"
        );
    }
}

#[test]
fn native_acceptance_is_not_overridden_by_browser_grammar_checks() {
    // The dependency currently accepts a unitless nonzero width. The engine
    // consumes its typed value instead of maintaining a second grammar.
    assert!(matches!(
        Property::parse_string(PropertyId::Width, "60", ParserOptions::default()).unwrap(),
        Property::Width(_)
    ));
    assert_eq!(declaration_syntax("width", "60"), PropertySyntax::Valid);
    assert_eq!(
        view(&styled("", "width:40px;width:60")).box_model.width,
        PreferredSize::Px(60.0)
    );
    assert!(selector_syntax_is_valid(":heading(2n)"));
    assert!(!selector_syntax_is_valid("p >"));
}

#[test]
fn whitespace_and_spacing_extensions_are_not_rewritten_into_other_declarations() {
    let document = styled(
        "",
        "white-space:pre;white-space:preserve nowrap;letter-spacing:2px;letter-spacing:120%",
    );
    assert_eq!(view(&document).text.white_space, WhiteSpace::Pre);
    assert_eq!(view(&document).text.letter_spacing.absolute_px(), 2.0);
}

#[test]
fn native_variable_substitution_preserves_url_and_color_tokens() {
    let document = styled("", "--paint:url(cover.png) green;background:var(--paint)");
    assert_eq!(view(&document).background.background_color, 0x008000ff);
    assert!(view(&document).background.background_image_present);
}

#[test]
fn unsupported_typed_shorthand_components_preserve_every_authored_fallback() {
    let fallback = "margin:11px 12px 13px 14px;padding:21px 22px 23px 24px;\
        inset:31px 32px 33px 34px;border:4px solid red;border-radius:5px 6px 7px 8px";
    // `ic` is accepted by Lightning CSS but needs font metrics this engine
    // cannot resolve. Put it last so partial shorthand writes are observable.
    for (name, id, value) in [
        ("margin", PropertyId::Margin, "1px 2px 3px 1ic"),
        ("margin-block", PropertyId::MarginBlock, "1px 1ic"),
        ("margin-inline", PropertyId::MarginInline, "1px 1ic"),
        ("padding", PropertyId::Padding, "1px 2px 3px 1ic"),
        ("padding-block", PropertyId::PaddingBlock, "1px 1ic"),
        ("padding-inline", PropertyId::PaddingInline, "1px 1ic"),
        ("inset", PropertyId::Inset, "1px 2px 3px 1ic"),
        ("border-width", PropertyId::BorderWidth, "1px 2px 3px 1ic"),
        (
            "border-block-width",
            PropertyId::BorderBlockWidth,
            "1px 1ic",
        ),
        (
            "border-inline-width",
            PropertyId::BorderInlineWidth,
            "1px 1ic",
        ),
        ("border", PropertyId::Border, "1ic dashed blue"),
        ("border-block", PropertyId::BorderBlock, "1ic dashed blue"),
        ("border-inline", PropertyId::BorderInline, "1ic dashed blue"),
        (
            "border-radius",
            PropertyId::from("border-radius"),
            "1px 2px 3px 1ic",
        ),
    ] {
        let parsed = Property::parse_string(id, value, ParserOptions::default()).unwrap();
        assert!(!matches!(parsed, Property::Unparsed(_)), "{name}");
        for direction in ["ltr", "rtl"] {
            let fallback = format!("direction:{direction};{fallback}");
            let baseline = styled("", &fallback);
            let expected = view(&baseline);
            for inline in [true, false] {
                let declarations = format!("{fallback};{name}:{value}");
                let document = if inline {
                    styled("", &declarations)
                } else {
                    styled(&format!("#target {{{declarations}}}"), "")
                };
                let actual = view(&document);
                assert_eq!(actual.box_model, expected.box_model, "{name} {direction}");
                assert_eq!(actual.layout, expected.layout, "{name} {direction}");
                assert_eq!(actual.border, expected.border, "{name} {direction}");
                assert_eq!(actual.border_radii(), expected.border_radii(), "{name}");
            }
        }
    }
}

#[test]
fn css_wide_batches_preserve_partial_shorthands_all_exclusions_and_parent_defaults() {
    use html_style_model::{ItemAlignment, LengthPct, TextDirection, UnicodeBidi};

    for keyword in ["initial", "inherit", "unset"] {
        let document = styled(
            &format!(
                "body {{font-size:23px;color:purple;margin:10px 20px 30px 40px;padding:8px;width:123px;justify-items:legacy right}} \
                 #target {{direction:rtl;unicode-bidi:embed;all:{keyword};margin:inherit;margin-left:5px;padding:initial;justify-items:initial}}"
            ),
            "",
        );
        let style = view(&document);
        assert_eq!(style.text.direction, TextDirection::Rtl, "{keyword}");
        assert_eq!(
            style.box_model.unicode_bidi,
            UnicodeBidi::Embed,
            "{keyword}"
        );
        assert_eq!(
            style.layout.justify_items,
            ItemAlignment::LegacyRight,
            "{keyword}"
        );
        assert_eq!(style.box_model.margin_top, LengthPct::Px(10.0), "{keyword}");
        assert_eq!(
            style.box_model.margin_right,
            LengthPct::Px(20.0),
            "{keyword}"
        );
        assert_eq!(
            style.box_model.margin_bottom,
            LengthPct::Px(30.0),
            "{keyword}"
        );
        assert_eq!(style.box_model.margin_left, LengthPct::Px(5.0), "{keyword}");
        for padding in [
            style.box_model.padding_top,
            style.box_model.padding_right,
            style.box_model.padding_bottom,
            style.box_model.padding_left,
        ] {
            assert_eq!(padding, LengthPct::Px(0.0), "{keyword}");
        }
        assert_eq!(
            style.font.font_size,
            if keyword == "initial" {
                document.document().root_font_size()
            } else {
                23.0
            },
            "{keyword}"
        );
        assert_eq!(
            style.text.color,
            if keyword == "initial" {
                0x000000ff
            } else {
                0x800080ff
            },
            "{keyword}"
        );
        assert_eq!(
            style.box_model.width,
            if keyword == "inherit" {
                PreferredSize::Px(123.0)
            } else {
                PreferredSize::Auto
            },
            "{keyword}"
        );
    }
}

#[test]
fn element_and_pseudo_initialization_share_inheritance_and_keep_layout_defaults() {
    use html_style_model::{Display, ItemAlignment, LengthPct, PositionMode};

    let document =
        parse_dom_document("<!doctype html><body><span id='target'>Text</span></body>").unwrap();
    let document = style_document(
        document,
        &[
            "body {font-size:23px;color:red;border-collapse:collapse;justify-items:legacy right;\
         position:absolute;padding:10px;border:5px solid blue}\
         #target::before {content:'Before'}",
        ],
    );
    let element = view(&document);
    let (pseudo, _, _) = document
        .styles()
        .before_style_for_node(target(&document))
        .unwrap();
    let pseudo = document.style_view(pseudo).unwrap();
    for style in [&element, &pseudo] {
        assert_eq!(style.font.font_size, 23.0);
        assert_eq!(style.text.color, 0xff0000ff);
        assert_eq!(
            style.box_model.border_collapse,
            BorderCollapseMode::Collapse
        );
        assert_eq!(style.box_model.display, Display::Inline);
        assert_eq!(style.box_model.padding_top, LengthPct::Px(0.0));
        assert_eq!(style.layout.position, PositionMode::Static);
    }
    assert_eq!(element.layout.justify_items, ItemAlignment::LegacyRight);
    assert_eq!(pseudo.layout.justify_items, ItemAlignment::Normal);
}

#[test]
fn inherited_line_height_keeps_computed_lengths_and_rebinds_unitless_values() {
    for (line_height, child_height, pseudo_height) in [("2em", 40.0, 40.0), ("1.5", 60.0, 15.0)] {
        let document = styled(
            &format!(
                "body {{font-size:20px;line-height:{line_height}}}\
                 #target::before {{content:'Before';font-size:10px;line-height:inherit}}"
            ),
            "all:inherit;font-size:40px",
        );
        assert_eq!(
            view(&document).text.line_height,
            child_height,
            "{line_height}"
        );
        let (pseudo, _, _) = document
            .styles()
            .before_style_for_node(target(&document))
            .unwrap();
        assert_eq!(
            document.style_view(pseudo).unwrap().text.line_height,
            pseudo_height,
            "{line_height}"
        );
    }
}

#[test]
fn raw_token_conversions_preserve_values_surrounded_by_comments() {
    let gap = " /* comment */ \t";
    let document = styled(
        &format!(
            "#target::before {{content:{gap}counter({gap}chapter{gap},{gap}decimal{gap}){gap}}}"
        ),
        &format!(
            "float:{gap}left{gap};contain:{gap}size{gap};\
            font-feature-settings:{gap}'liga'{gap}0{gap};\
            font-variant-numeric:{gap}tabular-nums{gap}"
        ),
    );
    let style = view(&document);
    assert_eq!(style.box_model.float, Float::Left);
    assert!(style.box_model.size_containment);
    assert_eq!(style.font.font_feature_settings[0].tag(), *b"liga");
    assert_eq!(style.font.font_feature_settings[0].value(), 0);
    assert_eq!(style.font.font_variant_numeric_features[0].tag(), *b"tnum");
    let (_, content, _) = document
        .styles()
        .before_style_for_node(target(&document))
        .unwrap();
    let [GeneratedContentItem::Counter { name, style }] = content.items.as_slice() else {
        panic!("generated counter")
    };
    assert_eq!(document.styles().string(*name), Some("chapter"));
    assert_eq!(*style, html_style_model::CounterStyle::Decimal);
}

#[test]
fn font_longhands_keep_the_final_size_and_line_height_across_shorthands_and_keywords() {
    for font in ["italic bold 12px/2 serif", "initial", "inherit", "unset"] {
        let document = styled("", &format!("font:{font};font-size:30px;line-height:1.5"));
        let style = view(&document);
        assert_eq!(style.font.font_size, 30.0, "{font}");
        assert_eq!(style.text.line_height, 45.0, "{font}");
        assert_eq!(style.text.line_height_number, 1.5, "{font}");
    }
    let document = styled(
        "",
        "font-size:30px;line-height:1.5;font:italic bold 12px/2 serif",
    );
    assert_eq!(view(&document).font.font_size, 12.0);
    assert_eq!(view(&document).text.line_height, 24.0);
    assert_eq!(view(&document).font.font_weight, 700);
}

#[test]
fn hidden_uncomputable_shorthand_components_reject_the_whole_declaration() {
    let document = styled(
        "",
        "margin:11px 12px 13px 14px;margin:1px 2px 3px 1ic;margin-left:99px;\
        border:4px solid red;border-width:1px 2px 3px 1ic;border-left-width:9px",
    );
    let expected = styled(
        "",
        "margin:11px 12px 13px 99px;border:4px solid red;border-left-width:9px",
    );
    assert_eq!(view(&document).box_model, view(&expected).box_model);
    assert_eq!(view(&document).border, view(&expected).border);
}

#[test]
fn hidden_uncomputable_variable_components_unset_only_the_winning_longhands() {
    let document = styled(
        "",
        "--m:1px 2px 3px 1ic;margin:11px 12px 13px 14px;\
        margin:var(--m);margin-left:99px",
    );
    let expected = styled("", "margin:0 0 0 99px");
    assert_eq!(view(&document).box_model, view(&expected).box_model);
    assert_eq!(view(&document).layout, view(&expected).layout);
}

#[test]
fn resolved_declarations_survive_retries_and_reset_between_elements() {
    use html_style_model::{FontStyle, LengthPct};
    let mut html = String::from("<!doctype html><body>");
    for (id, font) in [
        ("typed", "italic bold 12px/2 serif"),
        ("wide", "inherit"),
        ("invalid", "not-a-font"),
        ("typed-again", "normal 18px/3 serif"),
    ] {
        html.push_str(&format!(
            "<p id='{id}' style='width:17px;width:1ic;--font:{font};font:var(--font);\
             font-size:30px;padding:1px;padding:1px 2px 3px 1ic;padding-left:9px;\
             --gap:7px;grid-gap:var(--gap)'>Text</p>"
        ));
    }
    let document = style_document(
        parse_dom_document(&html).unwrap(),
        &["body {font-weight:500;line-height:1.5}"],
    );
    for (id, font_style, weight, line_height) in [
        ("typed", FontStyle::Italic, 700, 60.0),
        ("wide", FontStyle::Normal, 500, 45.0),
        ("invalid", FontStyle::Normal, 500, 45.0),
        ("typed-again", FontStyle::Normal, 400, 90.0),
    ] {
        let node = document
            .document()
            .node_ids()
            .find(|&node| document.document().get_dom_id(node) == Some(id))
            .unwrap();
        let style = document
            .style_view(document.style_for_node(node).unwrap())
            .unwrap();
        assert_eq!(style.font.font_size, 30.0, "{id}");
        assert_eq!(style.font.font_style, font_style, "{id}");
        assert_eq!(style.font.font_weight, weight, "{id}");
        assert_eq!(style.text.line_height, line_height, "{id}");
        assert_eq!(style.box_model.width, PreferredSize::Px(17.0), "{id}");
        assert_eq!(style.box_model.padding_top, LengthPct::Px(1.0), "{id}");
        assert_eq!(style.box_model.padding_left, LengthPct::Px(9.0), "{id}");
        assert_eq!(style.layout.row_gap, LengthPct::Px(7.0), "{id}");
        assert_eq!(style.layout.column_gap, LengthPct::Px(7.0), "{id}");
    }
}

#[test]
fn native_grid_auto_repeat_constraints_preserve_fallback_and_substitution() {
    use html_style_model::{GridTemplateTrack, GridTrackBreadth, GridTrackSize, LengthPct};
    let fallback = vec![GridTemplateTrack::Single(GridTrackSize::Breadth(
        GridTrackBreadth::Length(LengthPct::Px(100.0)),
    ))];
    let columns = |document: &StyledDocument| view(document).layout.grid_template_columns.clone();
    for value in [
        "repeat(auto-fill,10px) repeat(auto-fit,20px)",
        "1fr repeat(auto-fill,10px)",
        "repeat(2,auto) repeat(auto-fill,10px)",
        "fit-content(5px) repeat(auto-fit,10px)",
        "minmax(auto,1fr) repeat(auto-fill,10px)",
        "repeat(auto-fill,auto)",
    ] {
        let declaration = format!("grid-template-columns:100px;grid-template-columns:{value}");
        assert_eq!(
            columns(&styled("", &declaration)),
            fallback,
            "inline {value}"
        );
        assert_eq!(
            columns(&styled(&format!("#target {{{declaration}}}"), "")),
            fallback,
            "stylesheet {value}"
        );
        let substituted = format!(
            "--tracks:{value};grid-template-columns:100px;grid-template-columns:var(--tracks)"
        );
        assert!(
            columns(&styled("", &substituted)).is_empty(),
            "substituted {value}"
        );
        let shorthand = format!(
            "grid-template:50px / 100px;grid-template:30px / {value};grid-template-columns:80px"
        );
        assert_eq!(
            view(&styled("", &shorthand)).layout.grid_template_rows,
            vec![GridTemplateTrack::Single(GridTrackSize::Breadth(
                GridTrackBreadth::Length(LengthPct::Px(50.0)),
            ))],
            "invalid hidden shorthand component {value}"
        );
    }
    for value in [
        "1fr",
        "repeat(2,auto)",
        "minmax(10px,1fr) repeat(auto-fit,20px)",
        "minmax(auto,20px) repeat(auto-fill,minmax(10px,1fr))",
        "repeat(2,10px) repeat(auto-fit,20px)",
    ] {
        let direct = columns(&styled("", &format!("grid-template-columns:{value}")));
        assert!(!direct.is_empty(), "valid {value}");
        assert_eq!(
            columns(&styled(
                "",
                &format!("--tracks:{value};grid-template-columns:var(--tracks)")
            )),
            direct,
            "valid substituted {value}"
        );
    }
}

#[test]
fn font_shorthand_resets_features_without_overriding_winning_longhands() {
    let parent_css = "body {font-size:22px;font-kerning:none;\
        font-feature-settings:'ss03' 2;font-variant-numeric:tabular-nums;\
        font-variant-ligatures:discretionary-ligatures}";
    let features = |document: &StyledDocument| {
        let font = view(document).font;
        [
            font.font_kerning_features.clone(),
            font.font_feature_settings.clone(),
            font.font_variant_numeric_features.clone(),
            font.font_variant_ligature_features.clone(),
        ]
    };
    let inherited = features(&styled(parent_css, ""));
    assert!(inherited.iter().all(|values| !values.is_empty()));
    let empty = Default::default();
    for (declarations, reset) in [
        ("font:italic bold 18px/1.5 serif", true),
        ("--f:italic bold 18px/1.5 serif;font:var(--f)", true),
        ("font:18px serif;width:10px;width:1ic", true),
        ("font:18px serif;font:1ic serif", true),
        (
            "font:18px serif;font-size:30px;line-height:1.2;font-family:monospace;\
          font-weight:500;font-style:normal;font-variant-caps:small-caps",
            true,
        ),
        ("font:initial", true),
        ("font:inherit", false),
        ("font:unset", false),
        ("font:1ic serif", false),
        ("--f:1ic serif;font:18px serif;font:var(--f)", false),
    ] {
        let expected = if reset { &empty } else { &inherited };
        assert_eq!(
            features(&styled(parent_css, declarations)),
            *expected,
            "inline {declarations}"
        );
        assert_eq!(
            features(&styled(
                &format!("{parent_css} #target {{{declarations}}}"),
                ""
            )),
            *expected,
            "stylesheet {declarations}"
        );
    }
    for declaration in [
        "font:18px serif;font-feature-settings:'ss03' 2",
        "font-feature-settings:'ss03' 2!important;font:18px serif",
    ] {
        let actual = features(&styled(parent_css, declaration));
        assert_eq!(actual[1], inherited[1], "{declaration}");
        assert!(actual[0].is_empty() && actual[2].is_empty() && actual[3].is_empty());
    }
    let document = styled(
        &format!("{parent_css} #target::before {{content:'Before';font:18px serif}}"),
        "",
    );
    let (pseudo, _, _) = document
        .styles()
        .before_style_for_node(target(&document))
        .unwrap();
    let font = document.style_view(pseudo).unwrap().font;
    assert!(font.font_kerning_features.is_empty());
    assert!(font.font_feature_settings.is_empty());
    assert!(font.font_variant_numeric_features.is_empty());
    assert!(font.font_variant_ligature_features.is_empty());
}
