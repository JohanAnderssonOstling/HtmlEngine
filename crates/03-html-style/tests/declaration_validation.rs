use html_parse::parse_dom_document;
use html_style::{
    MediaEnvironment, PropertyValueSyntax, StyledDocument, property_value_syntax,
    selector_syntax_is_valid, style_document, style_document_with_environment,
};
use html_style_model::StyleView;

fn style_by_id<'a>(styled: &'a StyledDocument, id: &str) -> StyleView<'a> {
    let node = styled
        .document()
        .node_ids()
        .find(|node| styled.document().get_dom_id(*node) == Some(id))
        .unwrap();
    styled
        .style_view(styled.style_for_node(node).unwrap())
        .unwrap()
}

#[test]
fn geometry_grammar_does_not_advertise_or_apply_unimplemented_painting() {
    use html_style::{PropertyCapability, declaration_support};
    for (property, value) in [
        ("background-position-x", "x-end 10px"),
        ("background-size", "calc(sign(1em) * 10px)"),
        ("mask-position", "right 1px bottom 2px"),
        ("mask-size", "20% auto"),
        ("transform-origin", "left top 10px"),
        ("perspective-origin", "left 1px top 2px"),
        ("perspective", "10px"),
        ("transform", "perspective(none)"),
        ("scale", "calc(2 * sign(1em))"),
        ("clip-path", "polygon(round 1px, 0 0, 1px 2px)"),
    ] {
        assert!(
            matches!(
                declaration_support(property, value).capability,
                PropertyCapability::Unsupported(_)
            ),
            "{property}: {value}"
        );
        assert_eq!(
            computed(
                &format!("#target {{color:blue;width:20px;{property}:{value};height:30px}}"),
                ""
            ),
            computed("#target {color:blue;width:20px;height:30px}", ""),
            "{property}: {value}"
        );
        assert_eq!(
            computed(
                &format!(
                    "#target {{color:blue}} @supports ({property}:{value}) {{#target {{color:red}}}}"
                ),
                ""
            ),
            computed("#target {color:blue}", ""),
            "supports {property}: {value}"
        );
    }
}

#[test]
fn motion_grammar_never_advertises_or_applies_unimplemented_playback() {
    use html_style::{PropertyCapability, UnsupportedStyleFeature, declaration_support};
    for (property, value, feature) in [
        (
            "transition",
            "allow-discrete display 1s",
            UnsupportedStyleFeature::Transitions,
        ),
        (
            "transition-behavior",
            "allow-discrete",
            UnsupportedStyleFeature::Transitions,
        ),
        (
            "animation",
            "auto ease-in anim",
            UnsupportedStyleFeature::Animations,
        ),
        (
            "animation-duration",
            "auto",
            UnsupportedStyleFeature::Animations,
        ),
        (
            "animation-range-start",
            "cover calc(sign(1em) * 10%)",
            UnsupportedStyleFeature::Animations,
        ),
    ] {
        assert_eq!(
            declaration_support(property, value).capability,
            PropertyCapability::Unsupported(feature)
        );
        assert_eq!(
            computed(&format!("#target {{color:blue;{property}:{value}}}"), ""),
            computed("#target {color:blue}", "")
        );
        assert_eq!(
            computed(
                &format!(
                    "#target {{color:blue}} @supports ({property}:{value}) {{#target {{color:red}}}}"
                ),
                ""
            ),
            computed("#target {color:blue}", "")
        );
    }
}

#[test]
fn filter_grammar_never_advertises_or_applies_unimplemented_paint() {
    use html_style::{PropertyCapability, UnsupportedStyleFeature, declaration_support};
    for property in [
        "filter",
        "backdrop-filter",
        "-webkit-filter",
        "-webkit-backdrop-filter",
    ] {
        for value in [
            "none",
            "brightness(calc(1 / 0))",
            "blur(calc(10px + sign(1em - 1px) * 5px))",
            "drop-shadow(red 1px 2px)",
            "var(--filter)",
        ] {
            assert_eq!(
                declaration_support(property, value).capability,
                PropertyCapability::Unsupported(UnsupportedStyleFeature::FilterEffects),
                "{property}: {value}"
            );
            let css = format!("#target {{color:blue;width:20px;{property}:{value};height:30px}}");
            assert_eq!(
                computed(&css, ""),
                computed("#target {color:blue;width:20px;height:30px}", ""),
                "{css}"
            );
            assert_eq!(
                computed(
                    &format!(
                        "#target {{color:blue}} @supports ({property}:{value}) {{#target {{color:red}}}}"
                    ),
                    ""
                ),
                computed("#target {color:blue}", ""),
                "supports {property}: {value}"
            );
        }
        for value in ["inherit", "initial", "unset", "revert", "revert-layer"] {
            assert_eq!(
                property_value_syntax(property, value),
                PropertyValueSyntax::UnsupportedProperty,
                "{property}: {value}"
            );
        }
    }
}

#[test]
fn text_list_grammar_does_not_advertise_unimplemented_text_or_markers() {
    use html_style::{PropertyCapability, declaration_support};
    for (property, value, good) in [
        ("text-transform", "math-auto", "uppercase"),
        ("word-break", "manual", "keep-all"),
        ("word-break", "auto-phrase", "keep-all"),
        ("list-style-type", "symbols(numeric '0' '1')", "decimal"),
        (
            "list-style",
            "inside symbols(cyclic 'x')",
            "outside decimal",
        ),
    ] {
        assert_not_supported(html_style::declaration_support(property, value));
        assert_eq!(
            computed(
                &format!("#target {{{property}:{good};{property}:{value}}}"),
                ""
            ),
            computed(&format!("#target {{{property}:{good}}}"), ""),
            "{property}: {value}"
        );
        assert_eq!(
            computed(
                &format!(
                    "#target {{{property}:{good}}} @supports ({property}:{value}) {{#target {{color:red}}}}"
                ),
                ""
            ),
            computed(&format!("#target {{{property}:{good}}}"), ""),
            "supports {property}: {value}"
        );
    }
    for property in [
        "text-transform",
        "word-break",
        "list-style-type",
        "text-indent",
    ] {
        for value in ["initial", "inherit", "unset", "revert", "revert-layer"] {
            assert_eq!(
                declaration_support(property, value).capability,
                PropertyCapability::Supported,
                "{property}: {value}"
            );
        }
    }
}

#[test]
fn unimplemented_custom_media_does_not_register_aliases() {
    // Syntax support does not register aliases in the renderer.
    assert_eq!(
        computed(
            "#target {color:blue} @custom-media --small true; @media (--small) {#target {color:red}}",
            ""
        ),
        computed("#target {color:blue}", "")
    );
}

#[test]
fn color_property_grammar_keeps_uncomputed_colors_out_of_cascade_and_supports() {
    use html_style::{PropertyCapability, declaration_support};
    let fallback = "#target {color:blue;background-color:yellow;border:2px solid red;text-decoration:underline green}";
    let expected = computed(fallback, "");
    for property in [
        "color",
        "background-color",
        "border-top-color",
        "border-color",
        "border-inline-color",
        "text-decoration-color",
    ] {
        let value = "color-mix(in oklab,currentcolor,white)";
        let support = declaration_support(property, value);

        assert_not_supported(support);
        assert_eq!(
            computed(&format!("{fallback} #target {{{property}:{value}}}"), ""),
            expected,
            "stylesheet {property}"
        );
        assert_eq!(
            computed(fallback, &format!("{property}:{value}")),
            expected,
            "inline {property}"
        );
        assert_eq!(
            computed(
                &format!("{fallback} @supports ({property}:{value}) {{#target {{color:red}}}}"),
                ""
            ),
            expected,
            "supports {property}"
        );
    }
    assert_eq!(
        declaration_support("color", "rgb(12 34 56)").capability,
        PropertyCapability::Supported
    );
    assert_ne!(
        computed("#target {color:rgb(12 34 56)}", ""),
        computed("", "")
    );
}

#[test]
fn color_property_grammar_rejects_invalid_hue_interpolation() {
    let value = "color-mix(in srgb longer hue,red,blue)";
    let fallback = "#target {color:blue}";
    assert_eq!(
        computed(&format!("{fallback} #target {{color:{value}}}"), ""),
        computed(fallback, ""),
        "sheet {value}"
    );
    assert_eq!(
        computed(fallback, &format!("color:{value}")),
        computed(fallback, ""),
        "inline {value}"
    );
}

#[test]
fn decoration_auto_grammar_reports_unsupported_rendering_and_preserves_fallbacks() {
    use html_style::declaration_support;
    let fallback = "#target {outline:4px dotted blue}";
    let expected = computed(fallback, "");
    for (property, value) in [
        ("outline-color", "auto"),
        ("outline", "auto solid 2px"),
        ("outline", "auto auto 2px"),
        ("outline", "2px solid color-mix(in oklab,currentcolor,blue)"),
        ("text-emphasis-position", "auto"),
    ] {
        let support = declaration_support(property, value);
        assert_not_supported(support);
        assert_eq!(
            computed(&format!("{fallback} #target {{{property}:{value}}}"), ""),
            expected,
            "stylesheet {property}: {value}"
        );
        assert_eq!(
            computed(fallback, &format!("{property}:{value}")),
            expected,
            "inline {property}: {value}"
        );
        assert_eq!(
            computed(
                &format!("{fallback} @supports ({property}:{value}) {{#target {{color:red}}}}"),
                ""
            ),
            expected,
            "supports {property}: {value}"
        );
    }
}

#[test]
fn unsupported_part_states_do_not_match_or_enable_supports() {
    for selector in [
        "#target::part(label):dir(ltr)",
        "#target::part(label):lang(en)",
        "#target::part(label):state(active)",
        "#target::part(label):active-view-transition-type(type)",
        "#target::part(label):xr-overlay",
        "#target:xr-overlay",
    ] {
        assert_eq!(
            computed(
                &format!("#target {{color:blue}} {selector} {{color:red}}"),
                ""
            ),
            computed("#target {color:blue}", "")
        );
        assert_eq!(
            computed(
                &format!("@supports selector({selector}) {{#target {{color:red}}}}"),
                ""
            ),
            computed("", "")
        );
    }
    let styled = style_document(
        parse_dom_document("<div id='target'></div>").unwrap(),
        &[
            "#target::before {content:'before';color:blue} #target::part(label):dir(ltr)::before {color:red}",
        ],
    );
    let node = styled
        .document()
        .node_ids()
        .find(|node| styled.document().get_dom_id(*node) == Some("target"))
        .unwrap();
    let before = styled
        .style_view(styled.styles().before_style_for_node(node).unwrap().0)
        .unwrap();
    assert_eq!(before.text.color, 0x0000ffff);
}

#[test]
fn unsupported_pseudo_chains_do_not_match_or_enable_supports() {
    for selector in [
        "#target::before::marker",
        "#target::picker(select)::before",
        "#target::slotted(*)::picker-icon",
        "#target::column",
        "#target::before::column::scroll-marker",
        "#target::search-text:current",
    ] {
        assert_eq!(
            computed(
                &format!("#target {{color:blue}} {selector} {{color:red;content:'unsupported'}}"),
                ""
            ),
            computed("#target {color:blue}", "")
        );
        assert_eq!(
            computed(
                &format!("@supports selector({selector}) {{#target {{color:red}}}}"),
                ""
            ),
            computed("", "")
        );
    }
    let styled = style_document(
        parse_dom_document("<div id='target'></div>").unwrap(),
        &[
            "#target::before {content:'before';color:blue} #target::picker(select)::before {color:red} #target::before::marker {color:green}",
        ],
    );
    let node = styled
        .document()
        .node_ids()
        .find(|node| styled.document().get_dom_id(*node) == Some("target"))
        .unwrap();
    let before = styled
        .style_view(styled.styles().before_style_for_node(node).unwrap().0)
        .unwrap();
    assert_eq!(before.text.color, 0x0000ffff);
}

#[test]
fn scroll_button_selector_grammar_keeps_controls_unmatched_and_feature_queries_false() {
    for selector in [
        "#target::scroll-button(up)",
        "#target::scroll-button(*):enabled",
        "#target::scroll-button(prev):not(:disabled)",
    ] {
        assert!(selector_syntax_is_valid(selector));
        assert_eq!(
            computed(
                &format!("#target {{color:blue}} {selector} {{color:red;content:'button'}}"),
                ""
            ),
            computed("#target {color:blue}", "")
        );
        assert_eq!(
            computed(
                &format!("@supports selector({selector}) {{#target {{color:red}}}}"),
                ""
            ),
            computed("", "")
        );
    }
    assert_eq!(
        computed("@supports selector(#target) {#target {color:blue}}", ""),
        computed("#target {color:blue}", "")
    );
    let styled = style_document(
        parse_dom_document("<div id='target'></div>").unwrap(),
        &[
            "#target::before {content:'before';color:blue} #target::scroll-button(up),#target::before {color:green} #target::scroll-button(up):enabled {color:red;content:'button'}",
        ],
    );
    let node = styled
        .document()
        .node_ids()
        .find(|node| styled.document().get_dom_id(*node) == Some("target"))
        .unwrap();
    let before = styled
        .style_view(styled.styles().before_style_for_node(node).unwrap().0)
        .unwrap();
    assert_eq!(before.text.color, 0x008000ff);
}

#[test]
fn grid_lanes_grammar_preserves_supported_display_fallbacks_and_feature_queries() {
    use html_style::{PropertyCapability, declaration_support};
    for value in [
        "grid-lanes",
        "inline-grid-lanes",
        "inline grid-lanes",
        r"g\72 id-lanes",
    ] {
        let support = declaration_support("display", value);

        assert_not_supported(support);
        assert_eq!(
            computed(
                &format!("@supports (display:{value}) {{#target {{color:red}}}}"),
                ""
            ),
            computed("", "")
        );
        assert_eq!(
            computed("", &format!("display:inline-block;display:{value}")),
            computed("", "display:inline-block")
        );
        assert_eq!(
            computed(
                &format!("#target {{display:inline-block;display:{value}}}"),
                ""
            ),
            computed("", "display:inline-block")
        );
    }
    assert_eq!(
        computed(
            "",
            "display:inline-block;--layout:grid-lanes;display:var(--layout)"
        ),
        computed("", "display:unset")
    );
    assert_eq!(
        computed("", "--layout:flex;display:var(--layout)"),
        computed("", "display:flex")
    );
    assert_eq!(
        declaration_support("display", "grid").capability,
        PropertyCapability::Supported
    );
}

#[test]
fn anchor_grammar_keeps_anchor_layout_unsupported_and_native_cascade_fallbacks() {
    use html_style::declaration_support;
    for (property, baseline, value) in [
        ("top", "20px", "anchor(--target top)"),
        ("width", "40px", "anchor-size(--target width)"),
        (
            "margin-left",
            "10px",
            "calc(anchor-size(--target width) * 2)",
        ),
        ("align-self", "stretch", "anchor-center"),
        ("place-self", "stretch", "anchor-center right"),
        ("width", "40px", r"a\6e chor-size(--target)"),
    ] {
        let support = declaration_support(property, value);

        assert_not_supported(support);
        assert_eq!(
            computed(
                &format!("@supports ({property}:{value}) {{#target {{color:red}}}}"),
                ""
            ),
            computed("", "")
        );
        let fallback = format!("{property}:{baseline}");
        let declaration = format!("{fallback};{property}:{value}");
        assert_eq!(
            computed("", &declaration),
            computed("", &fallback),
            "inline {property}"
        );
        assert_eq!(
            computed(&format!("#target {{{declaration}}}"), ""),
            computed("", &fallback),
            "sheet {property}"
        );
    }
    assert_eq!(
        computed("", "width:40px;--w:anchor-size(--target);width:var(--w)"),
        computed("", "width:unset")
    );
    assert_eq!(
        computed(
            "",
            "align-self:stretch;--alignment:anchor-center;align-self:var(--alignment)"
        ),
        computed("", "align-self:unset")
    );
}

#[test]
fn intrinsic_size_grammar_preserves_native_fallbacks_and_reports_unsupported_values() {
    use html_style::declaration_support;
    for (property, baseline) in [
        ("width", "40px"),
        ("min-width", "20px"),
        ("max-width", "100px"),
        ("flex-basis", "30px"),
        ("inline-size", "50px"),
    ] {
        for value in [
            "calc-size(min-content, size * 2)",
            r"c\61 lc-size(any, 100px)",
        ] {
            let support = declaration_support(property, value);

            assert_not_supported(support);
            assert_eq!(
                computed(
                    &format!("@supports ({property}:{value}) {{#target {{color:red}}}}"),
                    ""
                ),
                computed("", "")
            );
            let fallback = format!("{property}:{baseline}");
            let declaration = format!("{fallback};{property}:{value}");
            assert_eq!(
                computed("", &declaration),
                computed("", &fallback),
                "inline {property}: {value}"
            );
            assert_eq!(
                computed(&format!("#target {{{declaration}}}"), ""),
                computed("", &fallback),
                "sheet {property}: {value}"
            );
        }
    }
    // Pending variables stay eligible. An unsupported substituted value is
    // invalid at computed-value time and therefore becomes unset.
    assert_eq!(
        computed(
            "",
            "width:40px;--w:calc-size(min-content,size);width:var(--w)"
        ),
        computed("", "width:unset")
    );
    assert_eq!(
        computed("", "--w:40px;width:var(--w)"),
        computed("", "width:40px")
    );
}

#[test]
fn unsupported_error_decorations_preserve_the_rendered_underline() {
    for value in ["none", "underline", "underline overline line-through"] {
        assert_eq!(
            property_value_syntax("text-decoration-line", value),
            PropertyValueSyntax::Valid
        );
    }
    for value in [
        "spelling-error",
        "grammar-error",
        "spelling-error overline",
        "underline blink",
    ] {
        assert_eq!(
            property_value_syntax("text-decoration-line", value),
            PropertyValueSyntax::UnsupportedValue
        );
        assert_eq!(
            computed(
                "",
                &format!("text-decoration-line:underline;text-decoration-line:{value}")
            ),
            computed("", "text-decoration-line:underline")
        );
    }
}

#[test]
fn typography_grammar_shadow_and_adjustment_capabilities_remain_explicit() {
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
    for (property, value) in [
        ("text-shadow", "inset 1px 2px 3px 4px currentcolor"),
        ("box-shadow", "none"),
        ("text-size-adjust", "calc(100% + sign(1em - 1px) * 1%)"),
        ("font-stretch", "calc(100% + sign(1em - 1px) * 1%)"),
    ] {
        let support = declaration_support(property, value);
        assert_eq!(support.syntax, PropertySyntax::Unsupported, "{property}");
        assert!(
            matches!(support.capability, PropertyCapability::Unsupported(_)),
            "{property}"
        );
        assert_eq!(
            computed(
                &format!("@supports ({property}:{value}) {{#target {{color:red}}}}"),
                ""
            ),
            computed("", "")
        );
        assert_eq!(
            computed("", &format!("{property}:{value}")),
            computed("", "")
        );
    }
}

#[test]
fn shape_grammar_keeps_clipping_capabilities_and_cascade_unchanged() {
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
    for value in [
        "path('M0 0L10 10Z')",
        "rect(0 100% 100% 0)",
        "xywh(0 0 10px 20px)",
        "shape(from center, line to top right)",
    ] {
        for property in ["clip-path", "-webkit-clip-path"] {
            let support = declaration_support(property, value);
            assert_eq!(support.syntax, PropertySyntax::Unsupported);
            assert!(matches!(
                support.capability,
                PropertyCapability::Unsupported(_)
            ));
            assert_eq!(
                computed(
                    &format!("@supports ({property}:{value}) {{#target {{color:red}}}}"),
                    ""
                ),
                computed("", "")
            );
            assert_eq!(
                computed("", &format!("{property}:{value}")),
                computed("", "")
            );
        }
    }
}

#[test]
fn image_values_capabilities_preserve_existing_list_marker_fallbacks() {
    use html_style::declaration_support;
    for (property, value) in [
        ("background-image", "image(red)"),
        ("list-style-image", "image(red)"),
        ("cursor", "image-set('a.png' 1x),pointer"),
    ] {
        let support = declaration_support(property, value);

        assert_not_supported(support);
        assert_eq!(
            computed(
                &format!("@supports ({property}:{value}) {{#target {{color:red}}}}"),
                ""
            ),
            computed("", "")
        );
    }
    let baseline = "list-style-image:url(marker.png)";
    for value in ["image(red)", "cross-fade(red,green)", "paint(reader)"] {
        let declaration = format!("{baseline};list-style-image:{value}");
        assert_eq!(
            computed("", &declaration),
            computed("", baseline),
            "inline {value}"
        );
        assert_eq!(
            computed(&format!("#target {{{declaration}}}"), ""),
            computed("", baseline),
            "sheet {value}"
        );
    }
}

#[test]
fn auxiliary_grammar_queries_keep_scrolling_containers_and_resampling_unsupported() {
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
    for (property, value) in [
        ("scroll-margin", "-2px 3em"),
        ("scroll-padding", "auto 10%"),
        ("container-type", "size anchored scroll-state"),
        ("container", "reader / inline-size scroll-state"),
        ("image-rendering", "pixelated"),
    ] {
        let support = declaration_support(property, value);
        assert_eq!(support.syntax, PropertySyntax::Unsupported);
        assert!(matches!(
            support.capability,
            PropertyCapability::Unsupported(_)
        ));
        assert_eq!(
            computed(
                &format!("@supports ({property}:{value}) {{#target {{color:red}}}}"),
                ""
            ),
            computed("", "")
        );
        assert_eq!(
            computed("", &format!("color:green;{property}:{value}")),
            computed("", "color:green")
        );
    }
}

#[test]
fn color_grammar_syntax_queries_do_not_advertise_unimplemented_paint_features() {
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
    for (property, value) in [
        ("font-palette", "palette-mix(in oklab,light,dark)"),
        ("color-scheme", "only light purple"),
        ("caret-color", "auto red"),
    ] {
        let support = declaration_support(property, value);
        assert_eq!(support.syntax, PropertySyntax::Unsupported);
        assert!(matches!(
            support.capability,
            PropertyCapability::Unsupported(_)
        ));
        assert_eq!(
            computed(
                &format!("@supports ({property}:{value}) {{#target {{color:red}}}}"),
                ""
            ),
            computed("", "")
        );
        assert_eq!(
            computed("", &format!("color:green;{property}:{value}")),
            computed("", "color:green")
        );
    }
}

#[test]
fn easing_syntax_queries_keep_playback_capabilities_explicit() {
    use html_style::{PropertyCapability, declaration_support};

    for property in ["animation-timing-function", "transition-timing-function"] {
        let support = declaration_support(property, "linear(0,1)");
        assert!(matches!(
            support.capability,
            PropertyCapability::Unsupported(_)
        ));
        assert_eq!(
            computed(
                &format!("@supports ({property}:linear(0,1)) {{#target {{color:green}}}}"),
                ""
            ),
            computed("", "")
        );
        assert_eq!(
            computed(
                &format!(
                    "@supports not ({property}:steps(1,jump-none)) {{#target {{color:green}}}}"
                ),
                ""
            ),
            computed("", "color:green")
        );
        assert_eq!(
            computed("", &format!("color:green;{property}:linear(0,1)")),
            computed("", "color:green")
        );
    }
}

#[test]
fn css_wide_keywords_normalize_case_before_computed_value_application() {
    let parent = "body {font:29px/3 fantasy;color:purple;direction:rtl;text-align:right}";
    for property in [
        "color",
        "font",
        "font-size",
        "line-height",
        "direction",
        "text-align",
        "margin",
    ] {
        for keyword in ["inherit", "initial", "unset", "revert", "revert-layer"] {
            let expected = computed(parent, &format!("{BASE};{property}:{keyword}"));
            for value in [
                keyword.to_ascii_uppercase(),
                format!("var(--missing,{})", keyword.to_ascii_uppercase()),
            ] {
                assert_eq!(
                    computed(parent, &format!("{BASE};{property}:{value}")),
                    expected,
                    "inline {property}:{value}"
                );
                assert_eq!(
                    computed(
                        &format!("{parent} #target {{{BASE};{property}:{value}}}"),
                        ""
                    ),
                    expected,
                    "sheet {property}:{value}"
                );
            }
        }
    }
}

#[test]
fn font_family_wide_keywords_keep_quoted_names_and_resolve_native_variants() {
    let family = |css: &str, inline: &str| {
        let styled = style_document(
            parse_dom_document(&format!(
                "<!doctype html><section style=\"font-family:'Parent Face'\"><p id='target' style=\"font-family:serif;{inline}\"></p></section>"
            ))
            .unwrap(),
            &[css],
        );
        style_by_id(&styled, "target")
            .font
            .font_family
            .map(|id| styled.styles().string(id).unwrap().to_owned())
    };
    for keyword in ["inherit", "unset", "revert", "revert-layer"] {
        for value in [
            keyword.to_owned(),
            format!("/**/{} /**/", keyword.to_ascii_uppercase()),
            format!("var(--missing,{keyword})"),
        ] {
            assert_eq!(
                family("", &format!("font-family:{value}")),
                Some("Parent Face".to_owned()),
                "{value}"
            );
        }
    }
    for value in ["initial", "INITIAL/**/", "var(--missing,initial)"] {
        assert_eq!(family("", &format!("font-family:{value}")), None, "{value}");
    }
    for keyword in ["inherit", "initial", "unset", "revert", "revert-layer"] {
        assert_eq!(
            family("", &format!("font-family:'{keyword}'")),
            Some(format!("\"{keyword}\"")),
            "quoted {keyword}"
        );
        assert_eq!(
            family(
                "",
                &format!("--family:'{keyword}';font-family:var(--family)")
            ),
            Some(format!("\"{keyword}\"")),
            "substituted quoted {keyword}"
        );
    }
    assert_eq!(
        family("", r"font-family:\75 nset"),
        Some("Parent Face".to_owned())
    );
    assert_eq!(
        computed("", "font-family:serif;font-family:unset"),
        computed("", "font-family:var(--missing)")
    );
}

#[test]
fn font_family_rollbacks_restore_layer_values_and_preserve_other_font_winners() {
    let family = |css: &str| {
        let styled = style_document(
            parse_dom_document("<!doctype html><section style='font-family:monospace'><p id='target'></p></section>").unwrap(),
            &[css],
        );
        let style = style_by_id(&styled, "target");
        assert_eq!(style.font.font_size, 23.0);
        style
            .font
            .font_family
            .map(|id| styled.styles().string(id).unwrap().to_owned())
    };
    for css in [
        "@layer base,top;@layer base {#target {font:23px serif}} @layer top {#target {font-family:revert-layer}}",
        "@layer base,top;@layer base {#target {font:23px serif!important;font-family:revert-layer!important}} @layer top {#target {font-family:fantasy!important}}",
    ] {
        // Important rollback removes the interval down to this layer's
        // normal declarations, including later layers' important values.
        let expected = if css.contains("important") {
            "monospace"
        } else {
            "serif"
        };
        assert_eq!(family(css), Some(expected.to_owned()), "{css}");
    }
    assert_eq!(
        family("#target {font:23px serif;font-family:revert}"),
        Some("monospace".to_owned())
    );
    assert_eq!(
        family("#target {font:23px serif;font-family:inherit}"),
        Some("monospace".to_owned())
    );
}

fn computed(css: &str, inline: &str) -> String {
    let inline = inline.replace('&', "&amp;").replace('"', "&quot;");
    let document = parse_dom_document(&format!(
        "<!doctype html><html><body><p id='target' style=\"{inline}\">Text</p></body></html>"
    ))
    .unwrap();
    let styled = style_document(document, &[css]);
    let target = styled
        .document()
        .node_ids()
        .find(|id| styled.document().get_dom_id(*id) == Some("target"))
        .unwrap();
    let view = styled
        .style_view(styled.style_for_node(target).unwrap())
        .unwrap();
    // String handles belong to one style store. Compare their contents when
    // checking independently computed documents.
    let mut font = view.font.clone();
    let family = font
        .font_family
        .take()
        .map(|id| styled.styles().string(id).unwrap());
    let cjk_family = font
        .cjk_font_family
        .take()
        .map(|id| styled.styles().string(id).unwrap());
    let mut text = view.text.clone();
    let list_image = text
        .list_style_image
        .take()
        .map(|id| styled.styles().string(id).unwrap());
    format!(
        "{font:?}{family:?}{cjk_family:?}{text:?}{list_image:?}{:?}{:?}{:?}{:?}{:?}",
        view.box_model,
        view.border,
        view.background,
        view.layout,
        view.border_radii()
    )
}

const BASE: &str = "color:#123456;background-color:#abcdef;font-size:21px;line-height:31px;\
    border:3px solid green;position:absolute;left:13px;right:17px;top:19px;bottom:23px;\
    text-decoration-line:overline;text-decoration-thickness:4px;outline:3px solid red;margin-left:9px";

#[test]
fn renderer_property_ids_preserve_css_case_rules_and_variable_names() {
    let declarations = "--Fit:cover;--fit:contain;OBJECT-FIT:var(--Fit);\
        DIRECTION:rtl;FLOAT:inline-start;CLEAR:both;OUTLINE-OFFSET:3px;\
        TABLE-LAYOUT:fixed;BORDER-COLLAPSE:collapse;CAPTION-SIDE:bottom;EMPTY-CELLS:hide;\
        FONT-KERNING:none;FONT-FEATURE-SETTINGS:'liga' 0;\
        FONT-VARIANT-NUMERIC:oldstyle-nums;FONT-VARIANT-LIGATURES:no-common-ligatures;\
        TEXT-BOX-TRIM:trim-both;TEXT-BOX-EDGE:cap alphabetic;CONTAIN:size;WIDOWS:4;ORPHANS:3";
    let expected = "object-fit:cover;direction:rtl;float:inline-start;clear:both;outline-offset:3px;\
        table-layout:fixed;border-collapse:collapse;caption-side:bottom;empty-cells:hide;\
        font-kerning:none;font-feature-settings:'liga' 0;\
        font-variant-numeric:oldstyle-nums;font-variant-ligatures:no-common-ligatures;\
        text-box-trim:trim-both;text-box-edge:cap alphabetic;contain:size;widows:4;orphans:3";
    assert_eq!(computed("", declarations), computed("", expected));
    assert_eq!(
        computed(&format!("#target {{{declarations}}}"), ""),
        computed("", expected)
    );
    assert_ne!(computed("", declarations), computed("", ""));
    assert_eq!(
        property_value_syntax("--Object-Fit", "anything"),
        PropertyValueSyntax::Valid
    );
    assert_eq!(
        property_value_syntax("unrecognized-property", "anything"),
        PropertyValueSyntax::UnsupportedProperty
    );
}

#[test]
fn renderer_property_aliases_share_cascade_slots_and_keep_priority() {
    for (declarations, expected) in [
        (
            "row-gap:9px;GRID-ROW-GAP:3px;column-gap:7px;GRID-COLUMN-GAP:5px",
            "row-gap:3px;column-gap:5px",
        ),
        ("GRID-ROW-GAP:3px;row-gap:9px", "row-gap:9px"),
        ("GRID-ROW-GAP:3px!important;row-gap:9px", "row-gap:3px"),
        ("--Gap:3px;GRID-GAP:var(--Gap)", "gap:3px"),
        (
            "PAGE-BREAK-BEFORE:always;break-before:avoid",
            "break-before:avoid",
        ),
        (
            "break-before:avoid;PAGE-BREAK-BEFORE:always",
            "break-before:page",
        ),
        (
            "break-after:avoid;PAGE-BREAK-AFTER:always",
            "break-after:page",
        ),
        (
            "break-inside:avoid;PAGE-BREAK-INSIDE:auto",
            "break-inside:auto",
        ),
    ] {
        assert_eq!(
            computed("", declarations),
            computed("", expected),
            "{declarations}"
        );
        assert_eq!(
            computed(&format!("#target {{{declarations}}}"), ""),
            computed("", expected),
            "stylesheet {declarations}"
        );
    }
}

#[test]
fn renderer_property_ids_preserve_inheritance_initial_and_layer_rollback() {
    let css = "body {object-fit:cover;font-kerning:none;widows:4}\
        #target {object-fit:contain;OBJECT-FIT:inherit;table-layout:fixed;TABLE-LAYOUT:initial;\
        font-kerning:normal;FONT-KERNING:unset;widows:8;WIDOWS:unset}";
    assert_eq!(
        computed(css, ""),
        computed(
            "body {font-kerning:none;widows:4} #target {object-fit:cover;table-layout:auto}",
            ""
        )
    );
    assert_eq!(
        computed(
            "@layer base, overrides;@layer base {#target {object-fit:cover;grid-row-gap:3px}}\
            @layer overrides {#target {OBJECT-FIT:contain;OBJECT-FIT:revert-layer;\
            GRID-ROW-GAP:8px;GRID-ROW-GAP:revert-layer}}",
            ""
        ),
        computed("#target {object-fit:cover;row-gap:3px}", "")
    );
}

#[test]
fn invalid_font_switches_preserve_fallbacks_and_unset_invalid_substitutions() {
    for (property, invalid, previous) in [
        (
            "font-variant-ligatures",
            "none normal",
            "discretionary-ligatures",
        ),
        (
            "font-variant-ligatures",
            "normal common-ligatures",
            "discretionary-ligatures",
        ),
        (
            "font-variant-ligatures",
            "common-ligatures no-common-ligatures",
            "discretionary-ligatures",
        ),
        (
            "font-variant-ligatures",
            "discretionary-ligatures no-discretionary-ligatures",
            "common-ligatures",
        ),
        (
            "font-variant-ligatures",
            "historical-ligatures no-historical-ligatures",
            "common-ligatures",
        ),
        (
            "font-variant-ligatures",
            "contextual no-contextual",
            "discretionary-ligatures",
        ),
        (
            "font-variant-ligatures",
            "contextual/**/contextual",
            "discretionary-ligatures",
        ),
        ("font-variant-numeric", "auto", "oldstyle-nums"),
        (
            "font-variant-numeric",
            "normal lining-nums",
            "oldstyle-nums",
        ),
        (
            "font-variant-numeric",
            "lining-nums oldstyle-nums",
            "tabular-nums",
        ),
        (
            "font-variant-numeric",
            "proportional-nums normal",
            "oldstyle-nums",
        ),
        (
            "font-variant-numeric",
            "tabular-nums proportional-nums",
            "oldstyle-nums",
        ),
        (
            "font-variant-numeric",
            "normal diagonal-fractions",
            "oldstyle-nums",
        ),
        (
            "font-variant-numeric",
            "diagonal-fractions stacked-fractions",
            "oldstyle-nums",
        ),
        ("font-variant-numeric", "ordinal normal", "oldstyle-nums"),
        (
            "font-variant-numeric",
            "normal slashed-zero",
            "oldstyle-nums",
        ),
        (
            "font-variant-numeric",
            "ordinal/**/ordinal",
            "oldstyle-nums",
        ),
        ("font-kerning", "normal auto", "none"),
        ("font-kerning", "none, auto", "normal"),
    ] {
        assert_ne!(
            property_value_syntax(property, invalid),
            PropertyValueSyntax::Valid,
            "{property}:{invalid}"
        );
        let baseline = format!("{property}:{previous}");
        let declarations = format!("{baseline};{property}:{invalid}");
        assert_eq!(
            computed("", &declarations),
            computed("", &baseline),
            "inline {declarations}"
        );
        assert_eq!(
            computed(
                &format!("#target {{{baseline}}} #target {{{property}:{invalid}}}"),
                ""
            ),
            computed("", &baseline),
            "stylesheet {property}:{invalid}"
        );
        let substituted = format!("--switches:{invalid};{baseline};{property}:var(--switches)");
        let unset = format!("{property}:unset");
        assert_eq!(
            computed("", &substituted),
            computed("", &unset),
            "inline {substituted}"
        );
        assert_eq!(
            computed(&format!("#target {{{substituted}}}"), ""),
            computed("", &unset),
            "stylesheet {substituted}"
        );
    }
}

#[test]
fn font_switch_tokens_accept_comments_escapes_and_combined_categories() {
    for (property, value, expected) in [
        ("font-kerning", "n\\6f ne/**/", "none"),
        ("font-kerning", "normal/**/", "normal"),
        (
            "font-variant-numeric",
            "oldstyle-nums/**/TABULAR-NUMS/**/diagonal-fractions ordinal slashed-zero",
            "oldstyle-nums tabular-nums diagonal-fractions ordinal slashed-zero",
        ),
        ("font-variant-numeric", "n\\6f rmal/**/", "normal"),
        (
            "font-variant-ligatures",
            "no-common-ligatures/**/discretionary-ligatures/**/historical-ligatures contextual",
            "no-common-ligatures discretionary-ligatures historical-ligatures contextual",
        ),
        ("font-variant-ligatures", "n\\6f ne/**/", "none"),
    ] {
        assert_eq!(
            property_value_syntax(property, value),
            PropertyValueSyntax::Valid,
            "{property}:{value}"
        );
        let wanted = computed("", &format!("{property}:{expected}"));
        let declaration = format!("{property}:{value}");
        assert_eq!(computed("", &declaration), wanted, "inline {declaration}");
        assert_eq!(
            computed(&format!("#target {{{declaration}}}"), ""),
            wanted,
            "stylesheet {declaration}"
        );
        let declaration = format!("--switches:{value};{property}:var(--switches)");
        assert_eq!(
            computed("", &declaration),
            wanted,
            "substituted {declaration}"
        );
    }
    for property in [
        "font-kerning",
        "font-variant-numeric",
        "font-variant-ligatures",
    ] {
        let reset = format!("{property}:normal");
        assert_eq!(
            computed(
                "",
                &format!("{property}:none;{property}:normal/**/ !/**/IMPORTANT")
            ),
            computed("", &reset),
            "priority/reset {property}"
        );
    }
}

fn feature_settings_at(css: &str, inline: &str, width: f64) -> (Vec<([u8; 4], u32)>, bool) {
    let inline = inline.replace('&', "&amp;").replace('"', "&quot;");
    let document = parse_dom_document(&format!(
        "<!doctype html><html><body><p id='target' style=\"{inline}\">Text</p></body></html>"
    ))
    .unwrap();
    let (styled, queries) = style_document_with_environment(
        document,
        &[css],
        MediaEnvironment::screen(width, Some(600.0)).unwrap(),
    );
    let target = styled
        .document()
        .node_ids()
        .find(|id| styled.document().get_dom_id(*id) == Some("target"))
        .unwrap();
    let font = styled
        .font_style(styled.style_for_node(target).unwrap())
        .unwrap();
    (
        font.font_feature_settings
            .iter()
            .map(|feature| (feature.tag(), feature.value()))
            .collect(),
        queries.uses_viewport_units(),
    )
}

#[test]
fn invalid_feature_settings_preserve_fallbacks_and_unset_invalid_substitutions() {
    for invalid in [
        "normal 'dlig'",
        "'c2sc', normal",
        "'tnum' 'hist'",
        "'silly' off",
        "dlig",
        "'liga' -1",
        "'liga' 1.5",
        "'liga' 1e0",
        "'liga' 1 on",
        "'liga',",
        ", 'liga'",
        "'aébc'",
        "'\\1 abc'",
        "'a\\7f bc'",
        "'liga' calc(1px)",
        "'liga' min(1,2px)",
        "'liga' calc(1px + 2)",
    ] {
        assert_ne!(
            property_value_syntax("font-feature-settings", invalid),
            PropertyValueSyntax::Valid,
            "{invalid}"
        );
        let baseline = "font-feature-settings:'ss03' 2";
        let declaration = format!("{baseline};font-feature-settings:{invalid}");
        assert_eq!(
            computed("", &declaration),
            computed("", baseline),
            "inline {invalid}"
        );
        assert_eq!(
            computed(&format!("#target {{{declaration}}}"), ""),
            computed("", baseline),
            "stylesheet {invalid}"
        );
        let substituted =
            format!("--settings:{invalid};{baseline};font-feature-settings:var(--settings)");
        assert_eq!(
            feature_settings_at("", &substituted, 600.0).0,
            Vec::new(),
            "substituted {invalid}"
        );
    }
}

#[test]
fn feature_settings_preserve_quoted_tags_case_and_last_duplicate_values() {
    for (value, expected) in [
        (
            "'liga' /**/OFF, 'PKRN', 'a,b!' 2",
            vec![(*b"PKRN", 1), (*b"a,b!", 2), (*b"liga", 0)],
        ),
        ("'\\22\\7d\\2f\\2a'", vec![([34, 125, 47, 42], 1)]),
        ("'\\22\\22\\22\\22'", vec![([34, 34, 34, 34], 1)]),
        ("'a\\27\\5c b'", vec![([97, 39, 92, 98], 1)]),
        ("' a, '", vec![(*b" a, ", 1)]),
        (
            "'liga' 1, 'liga' 0, 'LIGA' ON",
            vec![(*b"LIGA", 1), (*b"liga", 0)],
        ),
        ("normal/**/", vec![]),
    ] {
        assert_eq!(
            property_value_syntax("font-feature-settings", value),
            PropertyValueSyntax::Valid,
            "{value}"
        );
        let declaration = format!("font-feature-settings:{value}");
        assert_eq!(
            feature_settings_at("", &declaration, 600.0).0,
            expected,
            "inline {value}"
        );
        assert_eq!(
            feature_settings_at(&format!("#target {{{declaration}}}"), "", 600.0).0,
            expected,
            "stylesheet {value}"
        );
        let declaration = format!("--settings:{value};font-feature-settings:var(--settings)");
        assert_eq!(
            feature_settings_at("", &declaration, 600.0).0,
            expected,
            "substituted {value}"
        );
    }
    assert_eq!(
        feature_settings_at(
            "body {font-feature-settings:'ss03' 2}",
            "font-feature-settings:unset",
            600.0
        )
        .0,
        vec![(*b"ss03", 2)]
    );
    assert_eq!(
        feature_settings_at(
            "body {font-feature-settings:'ss03' 2}",
            "font-feature-settings:initial",
            600.0
        )
        .0,
        vec![]
    );
    assert_eq!(
        feature_settings_at(
            "",
            "font-feature-settings:'ss03' 2; font-feature-settings:normal/**/ !/**/IMPORTANT",
            600.0
        )
        .0,
        vec![]
    );
}

#[test]
fn invalid_pagination_values_preserve_fallbacks_and_unset_substitutions() {
    for (property, invalid, previous) in [
        ("break-before", "none", "page"),
        ("break-before", "avoid region", "page"),
        ("break-after", "none", "page"),
        ("break-after", "avoid region", "page"),
        ("break-inside", "region", "avoid"),
        ("break-inside", "auto avoid", "avoid"),
        ("page-break-before", "page", "always"),
        ("page-break-after", "column", "always"),
        ("page-break-inside", "avoid-page", "avoid"),
        ("widows", "auto", "5"),
        ("widows", "1 234", "5"),
        ("widows", "-234", "5"),
        ("widows", "-1", "5"),
        ("widows", "0", "5"),
        ("orphans", "auto", "5"),
        ("orphans", "1 234", "5"),
        ("orphans", "-234", "5"),
        ("orphans", "-1", "5"),
        ("orphans", "0", "5"),
        ("widows", "1.5", "5"),
        ("orphans", "calc(1px)", "5"),
        ("widows", "min(1px,2)", "5"),
        ("orphans", "2/**/3", "5"),
    ] {
        assert_ne!(
            property_value_syntax(property, invalid),
            PropertyValueSyntax::Valid,
            "{property}:{invalid}"
        );
        let baseline = format!("{property}:{previous}");
        let declaration = format!("{baseline};{property}:{invalid}");
        assert_eq!(
            computed("", &declaration),
            computed("", &baseline),
            "inline {declaration}"
        );
        assert_eq!(
            computed(&format!("#target {{{declaration}}}"), ""),
            computed("", &baseline),
            "stylesheet {declaration}"
        );
        let substituted = format!("--pagination:{invalid};{baseline};{property}:var(--pagination)");
        let unset = format!("{property}:unset");
        assert_eq!(
            computed("", &substituted),
            computed("", &unset),
            "inline substitution {substituted}"
        );
        assert_eq!(
            computed(&format!("#target {{{substituted}}}"), ""),
            computed("", &unset),
            "stylesheet substitution {substituted}"
        );
    }
}

fn assert_not_supported(support: html_style::DeclarationSupport) {
    assert!(
        support.syntax != html_style::PropertySyntax::Valid
            || support.capability != html_style::PropertyCapability::Supported,
        "{support:?}"
    );
}
