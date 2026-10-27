use html_parse::parse_dom_document;
use html_style::{
    AuthorStylesheetInput, ImportLayer, MediaEnvironment, PropertyValueSyntax, StyleProgramCache,
    StyledDocument, StylesheetEntry, property_value_syntax, resolve_document_with_program,
    selector_syntax_is_valid, style_document, style_document_with_cached_program_and_timings,
    style_document_with_environment, stylesheet_entries, stylesheet_syntax_is_valid,
};
use html_style_model::{
    ItemAlignment, LengthPct, PreferredSize, ReaderStyleOverrides, StyleView, TabSize,
    resolve_used_preferred_size,
};

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
fn selector_recovery_keeps_has_lists_unforgiving_without_changing_is_and_where() {
    for selector in [
        ":has(.a, 123)",
        ":has(123,.a)",
        ":has(.a,)",
        ":has(,.a)",
        ":has(.a,:unknown)",
        ":has(.a,::before)",
        ":has(.a,:has(.b))",
        r":h\61 s(.a, 123)",
    ] {
        assert!(!selector_syntax_is_valid(selector), "{selector}");
        assert!(
            !stylesheet_syntax_is_valid(&format!("{selector} {{color:red}}")),
            "{selector}"
        );
    }
    for selector in [
        ":has(.a,.b)",
        ":has(> .a,+ .b,~ .c)",
        ":has([title=','], :is(.a,123))",
        ":has(:is(.a,:has(.b)))",
        ":is(.a,:has(.b,123))",
        ":where(:has(.b,123),.a)",
        ":not(:is(:has(.b,123),.a))",
        ":is(:has(.b,123))",
        r":h\61 s(> .a, .b)",
    ] {
        assert!(selector_syntax_is_valid(selector), "{selector}");
        assert!(
            stylesheet_syntax_is_valid(&format!("{selector} {{color:red}}")),
            "{selector}"
        );
    }
}

#[test]
fn selector_recovery_rejects_bad_component_tokens_even_inside_forgiving_lists() {
    for selector in [
        ":is(# C4єанйтж╕/┘ГЁжЮХєа▓┐ЁЭБМєаСеЁЭЖА%=[ямм0)",
        ":is(.a,[x))",
        ":where(.a,])",
        ":is(.a,[title=\"bad\nstring\"])",
        ":is(.a,url(bad url))",
    ] {
        assert!(!selector_syntax_is_valid(selector), "{selector}");
        assert_eq!(
            computed(
                &format!("#target {{color:blue}} #target,{selector} {{color:red}}"),
                ""
            ),
            computed("#target {color:blue}", ""),
            "{selector}"
        );
    }
    for selector in [
        ":is(.a,123)",
        ":where(.a,:unknown)",
        r#"[title=':has(.a,123)']"#,
        r#":is([title=')'],.a)"#,
        ":has(/* , */ .a, .b)",
    ] {
        assert!(selector_syntax_is_valid(selector), "{selector}");
    }
}

#[test]
fn selector_recovery_preserves_rule_recovery_forgiving_members_and_supports() {
    let blue = computed("#target {color:blue}", "");
    let red = computed("#target {color:red}", "");
    for invalid in [
        ":has(body,123)",
        ":has(body,:unknown)",
        ":has(body,::before)",
    ] {
        assert_eq!(
            computed(
                &format!("#target {{color:blue}} #target,{invalid} {{color:red}}"),
                ""
            ),
            blue,
            "{invalid}"
        );
        assert_eq!(
            computed(
                &format!(
                    "#target {{color:blue}} @media screen {{#target,{invalid} {{color:red}}}}"
                ),
                ""
            ),
            blue,
            "nested {invalid}"
        );
        assert_eq!(
            computed(
                &format!("#target,{invalid} {{color:red}} #target {{color:blue}}"),
                ""
            ),
            blue,
            "following rule {invalid}"
        );
    }
    for selector in [
        ":is(#target,:has(.a,123))",
        ":where(:has(.a,123),#target)",
        "body:has(> #target) #target",
        "body:has(#target,:is(.a,123)) #target",
    ] {
        assert_eq!(
            computed(&format!("{selector} {{color:red}}"), ""),
            red,
            "{selector}"
        );
    }
    for selector in [
        ":has(.a,123)",
        ":is(#target,:has(.a,123))",
        ":where(#target,:has(.a,123))",
    ] {
        assert_eq!(
            computed(
                &format!(
                    "#target {{color:blue}} @supports selector({selector}) {{#target {{color:red}}}}"
                ),
                ""
            ),
            blue,
            "supports {selector}"
        );
    }
    assert_eq!(
        computed(
            "#target {color:blue} @supports selector(:has(> .a,.b)) {#target {color:red}}",
            ""
        ),
        red
    );
}

#[test]
fn selector_recovery_validates_scope_boundaries_and_preserves_opaque_payloads() {
    let blue = computed("#target {color:blue}", "");
    for boundary in ["(:has(body,123))", "(body) to (:has(.a,123))"] {
        let scope = format!("@scope {boundary} {{#target {{color:red}}}}");
        assert!(!stylesheet_syntax_is_valid(&scope), "{scope}");
        assert_eq!(
            computed(&format!("#target {{color:blue}} {scope}"), ""),
            blue,
            "{scope}"
        );
    }
    let scope = "@scope (:is(body,:has(.a,123))) {#target {color:red}}";
    assert!(stylesheet_syntax_is_valid(scope));
    assert_eq!(computed(scope, ""), computed("#target {color:red}", ""));
    assert_eq!(
        computed(
            "#target {--payload:{:has(.a,123) {color:red}};color:blue}",
            ""
        ),
        blue
    );
}

#[test]
fn selector_recovery_bounds_nesting_before_invoking_the_native_parser() {
    let shallow = format!("{}#target{}", ":is(".repeat(16), ")".repeat(16));
    assert!(selector_syntax_is_valid(&shallow));
    let deep = format!("{}#target{}", ":is(".repeat(150), ")".repeat(150));
    assert!(!selector_syntax_is_valid(&deep));
    assert!(!stylesheet_syntax_is_valid(&format!(
        "{deep} {{color:red}}"
    )));
}

#[test]
fn selector_recovery_keeps_namespace_preludes_in_stylesheet_apis() {
    let selector = r"*|\\";
    let source = format!("@namespace 'blah'; {selector}");
    assert!(selector_syntax_is_valid(selector));
    assert!(!selector_syntax_is_valid(&source));
    assert!(stylesheet_syntax_is_valid(&format!(
        "{source} {{font-size:1em}}"
    )));
}
#[test]
fn unit_algebra_grammar_checks_final_dimensions_and_percentage_hints() {
    for property in ["width", "height"] {
        for value in [
            "calc(5px * 10lh / 1px)",
            "calc(20% * 0.5em / 1px)",
            "calc(4px * 4em / 1px)",
            "calc(400px / 4lh * 1px)",
            "calc(20% / 0.5em * 1px)",
            "calc(100px * 1px / 1px / 1)",
            "calc((50% + 2px) * 2em / 1px)",
            "calc(1px * 2s / 500ms)",
            "calc(1turn / 180deg * 1px)",
            "calc(192dpi / 1dppx * 1px)",
        ] {
            assert_eq!(
                property_value_syntax(property, value),
                PropertyValueSyntax::Valid,
                "{property}: {value}"
            );
        }
        for value in [
            "calc(1px * 1px)",
            "calc(1px / 1px)",
            "calc(1px * 1s)",
            "calc(1px / 1s)",
            "calc(1px * 1px + 1px)",
            "calc(1px * 1px + 1s * 1s)",
            "calc(1px * 1px / 1px+ 1px)",
            "calc(1px * 1px / 1px +1px)",
            "calc(1px * 1px / (0)2)",
        ] {
            assert_eq!(
                property_value_syntax(property, value),
                PropertyValueSyntax::Invalid,
                "{property}: {value}"
            );
        }
    }
}

#[test]
fn unit_algebra_linear_sizes_compute_real_fonts_line_heights_and_percentage_coefficients() {
    for property in ["width", "height"] {
        for (value, expected) in [
            ("calc(5px * 10lh / 1px)", "1000px"),
            ("calc(20% * 0.5em / 1px)", "160%"),
            ("calc(4px * 4em / 1px)", "256px"),
            ("calc(400px / 4lh * 1px)", "5px"),
            ("calc(20% / 0.5em * 1px)", "2.5%"),
            ("calc(100px * 1px / 1px / 1)", "100px"),
            ("calc(1px * 2s / 500ms)", "4px"),
            ("calc(1turn / 180deg * 1px)", "2px"),
            ("calc(1khz / 100hz * 1px)", "10px"),
            ("calc(192dpi / 1dppx * 1px)", "2px"),
            ("calc(sin(90deg * 1s / 1s) * 10px)", "10px"),
            ("calc(-1px * 1px / 1px)", "0px"),
        ] {
            let base = "font-size:16px;line-height:20px";
            let control = computed(&format!("#target {{{base};{property}:{expected}}}"), "");
            assert_eq!(
                computed(
                    &format!("#target {{{base};{property}:99px;{property}:{value}}}"),
                    ""
                ),
                control,
                "sheet {property}: {value}"
            );
            assert_eq!(
                computed(
                    &format!("#target {{{base}}}"),
                    &format!("{property}:{value}")
                ),
                control,
                "inline {property}: {value}"
            );
            assert_eq!(
                computed(
                    &format!("#target {{{base};--v:{value};{property}:var(--v)}}"),
                    ""
                ),
                control,
                "var {property}: {value}"
            );
        }
    }
}

#[test]
fn unit_algebra_invalid_dimensions_preserve_literals_and_unset_substituted_winners() {
    for property in ["width", "height"] {
        for value in [
            "calc(1px * 1px)",
            "calc(1px / 1px)",
            "calc(1px * 1s)",
            "calc(1px * 1px / (0)2)",
        ] {
            let fallback = computed(&format!("#target {{{property}:99px}}"), "");
            assert_eq!(
                computed(
                    &format!("#target {{{property}:99px;{property}:{value}}}"),
                    ""
                ),
                fallback,
                "sheet {property}: {value}"
            );
            assert_eq!(
                computed(
                    &format!("#target {{{property}:99px}}"),
                    &format!("{property}:{value}")
                ),
                fallback,
                "inline {property}: {value}"
            );
            assert_eq!(
                computed(
                    &format!("#target {{--v:{value};{property}:99px;{property}:var(--v)}}"),
                    ""
                ),
                computed(&format!("#target {{{property}:unset}}"), ""),
                "var {property}: {value}"
            );
        }
    }
}

#[test]
fn unit_algebra_sizes_requiring_nonlinear_layout_or_face_metrics_remain_unsupported() {
    use html_style::{PropertyCapability, UnsupportedStyleFeature, declaration_support};
    for property in ["width", "height"] {
        for value in [
            "calc(52px * 1px / 10%)",
            "calc(1ex * 1px / 1px)",
            "calc(1ch * 1px / 1px)",
            "calc(1% * 1% / 1px)",
        ] {
            assert_eq!(
                property_value_syntax(property, value),
                PropertyValueSyntax::UnsupportedValue,
                "{property}: {value}"
            );
            assert_eq!(
                declaration_support(property, value).capability,
                PropertyCapability::Unsupported(UnsupportedStyleFeature::Value),
                "{property}: {value}"
            );
            assert_eq!(
                computed(
                    &format!("#target {{{property}:99px;{property}:{value}}}"),
                    ""
                ),
                computed(&format!("#target {{{property}:99px}}"), ""),
                "sheet {property}: {value}"
            );
            assert_eq!(
                computed(
                    &format!("#target {{--v:{value};{property}:99px;{property}:var(--v)}}"),
                    ""
                ),
                computed(&format!("#target {{{property}:unset}}"), ""),
                "var {property}: {value}"
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
        assert_eq!(
            declaration_support(property, "calc(1em * 1px / 1px)").capability,
            PropertyCapability::Supported
        );
    }
}

#[test]
fn unit_algebra_preserves_color_fallbacks_when_the_native_backend_cannot_compute_a_channel() {
    use html_style::{PropertyCapability, UnsupportedStyleFeature, declaration_support};
    let value =
        "hsl(from hsl(50 50 50) h s calc((sin(l * (50rad / (50deg * (180 / pi)))) + 1) * 50))";
    assert_eq!(
        declaration_support("color", value).capability,
        PropertyCapability::Unsupported(UnsupportedStyleFeature::Value)
    );
    assert_eq!(
        computed(&format!("#target {{color:green;color:{value}}}"), ""),
        computed("#target {color:green}", "")
    );
    assert_eq!(
        computed(
            &format!("#target {{--v:{value};color:green;color:var(--v)}}"),
            ""
        ),
        computed("#target {color:unset}", "")
    );
}

#[test]
fn unit_algebra_rebinds_cached_sizes_to_each_font_and_viewport() {
    let css = "#font {width:calc(1em * 1px / 1px)} #viewport {width:calc(10px * 1cqw / 1px)}";
    let mut cache = StyleProgramCache::default();
    for (width, font_size) in [(400.0, 16.0), (600.0, 20.0), (400.0, 24.0)] {
        let document = parse_dom_document(&format!("<!doctype html><p id='font' style='font-size:{font_size}px'>a</p><p id='viewport'>b</p>")).unwrap();
        let inputs = [AuthorStylesheetInput {
            css,
            implicit_scope_root: document.dom_root(),
        }];
        let (styled, _, queries) = style_document_with_cached_program_and_timings(
            &mut cache,
            document,
            &inputs,
            MediaEnvironment::screen(width, None).unwrap(),
        );
        assert_eq!(
            style_by_id(&styled, "font").box_model.width,
            PreferredSize::Px(font_size)
        );
        assert_eq!(
            style_by_id(&styled, "viewport").box_model.width,
            PreferredSize::Px(width as f32 / 10.0)
        );
        assert!(queries.uses_viewport_units());
    }
    assert!(cache.stats().hits > 0);
}

#[test]
fn unit_algebra_preserves_arithmetic_boundaries_and_function_types() {
    for (value, expected) in [
        ("calc(max(2px * 1px / 1px, 3px))", "3px"),
        ("calc(abs(-2px * 1px / 1px))", "2px"),
        ("calc(sqrt(4px / 1px) * 1px)", "2px"),
        ("calc(pow(2px / 1px, 3) * 1px)", "8px"),
        ("calc(mod(5px * 1px / 1px, 2px))", "1px"),
        ("calc(hypot(3px * 1px / 1px, 4px))", "5px"),
    ] {
        assert_eq!(
            property_value_syntax("width", value),
            PropertyValueSyntax::Valid,
            "{value}"
        );
        assert_eq!(
            computed(&format!("#target {{width:{value}}}"), ""),
            computed(&format!("#target {{width:{expected}}}"), ""),
            "{value}"
        );
    }
    for value in [
        "calc(sqrt(4px * 1px))",
        "calc(pow(2px * 1px, 3))",
        "calc(sin(1px * 1px) * 1px)",
        "calc(max(1px * 1px, 1px))",
        "calc(1px * 1px / 1px +/**/1px)",
        "calc(1px * 1px / (0)2)",
    ] {
        assert_eq!(
            property_value_syntax("width", value),
            PropertyValueSyntax::Invalid,
            "{value}"
        );
    }
}

#[test]
fn paint_grammar_preserves_supported_background_colors_image_presence_and_invalid_fallbacks() {
    for (value, image) in [
        ("red", false),
        ("none red", false),
        ("url('picture.svg') red", true),
        ("none, url('picture.svg') red", true),
    ] {
        for inline in [false, true] {
            let document = parse_dom_document(&format!(
                "<!doctype html><p id='target' style=\"{}\">Text</p>",
                if inline {
                    format!("background:{value}")
                } else {
                    String::new()
                }
            ))
            .unwrap();
            let css = if inline {
                String::new()
            } else {
                format!("#target {{background:{value}}}")
            };
            let styled = style_document(document, &[&css]);
            let view = style_by_id(&styled, "target");
            assert_eq!(view.background.background_color, 0xff0000ff, "{value}");
            assert_eq!(view.background.background_image_present, image, "{value}");
        }
    }
    let fallback = computed("#target {background:blue}", "");
    for value in ["red, none", "red blue", "left / -1px red", "border red"] {
        assert_eq!(
            computed(
                &format!("#target {{background:blue;background:{value}}}"),
                ""
            ),
            fallback,
            "sheet {value}"
        );
        assert_eq!(
            computed("#target {background:blue}", &format!("background:{value}")),
            fallback,
            "inline {value}"
        );
        assert_eq!(
            computed(
                &format!("#target {{--v:{value};background:blue;background:var(--v)}}"),
                ""
            ),
            computed("#target {background:unset}", ""),
            "var {value}"
        );
    }
}

#[test]
fn paint_grammar_unimplemented_layers_and_images_keep_fallbacks_and_support_queries_false() {
    use html_style::{PropertyCapability, declaration_support};
    for (property, value) in [
        ("background-clip", "border-area text"),
        ("border-image", "1 / 2px"),
        ("border-image-slice", "calc(10 + sign(1em))"),
        ("border-image-width", "auto 1px"),
        ("border-image-outset", "calc(1 / 0)"),
        ("mask", "none stroke-box no-clip"),
        ("background", "border-area text red"),
        ("background", "padding-box border-area red"),
    ] {
        assert!(
            matches!(
                declaration_support(property, value).capability,
                PropertyCapability::Unsupported(_)
            ),
            "{property}: {value}"
        );
        let fallback = computed("#target {background:blue;color:green}", "");
        assert_eq!(
            computed(
                &format!("#target {{background:blue;color:green;{property}:{value}}}"),
                ""
            ),
            fallback,
            "sheet {property}: {value}"
        );
        assert_eq!(
            computed(
                "#target {background:blue;color:green}",
                &format!("{property}:{value}")
            ),
            fallback,
            "inline {property}: {value}"
        );
        assert_eq!(
            computed(
                &format!(
                    "#target {{background:blue;color:green}} @supports ({property}:{value}) {{#target {{color:red}}}}"
                ),
                ""
            ),
            fallback,
            "supports {property}: {value}"
        );
    }
    assert_eq!(
        computed(
            "#target {--v:border-area text red;background:blue;background:var(--v)}",
            ""
        ),
        computed("#target {background:unset}", "")
    );
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
fn inline_grammar_equivalent_vertical_align_combinations_compute_in_both_sources() {
    for (value, expected) in [
        ("baseline 0", "0"),
        ("0px baseline", "0px"),
        ("baseline super", "super"),
        ("sub baseline", "sub"),
        ("baseline top", "top"),
        ("middle 0", "middle"),
        ("text-top 0%", "text-top"),
        ("0em text-bottom", "text-bottom"),
        ("baseline -10px", "-10px"),
        ("calc(20% - 10px) baseline", "calc(20% - 10px)"),
    ] {
        let control = computed(&format!("#target {{vertical-align:{expected}}}"), "");
        assert_eq!(
            computed(
                &format!("#target {{vertical-align:bottom;vertical-align:{value}}}"),
                ""
            ),
            control,
            "sheet {value}"
        );
        assert_eq!(
            computed(
                "#target {vertical-align:bottom}",
                &format!("vertical-align:{value}")
            ),
            control,
            "inline {value}"
        );
        assert_eq!(
            computed(
                &format!("#target {{--v:{value};vertical-align:bottom;vertical-align:var(--v)}}"),
                ""
            ),
            control,
            "var {value}"
        );
        assert_eq!(
            computed(
                &format!("#target {{vertical-align:{value} !important;vertical-align:bottom}}"),
                "vertical-align:bottom"
            ),
            control,
            "priority {value}"
        );
    }
}

#[test]
fn inline_grammar_vertical_align_rejects_literals_and_unsets_invalid_substitutions() {
    let fallback = computed("#target {vertical-align:super}", "");
    let initial = computed("#target {vertical-align:initial}", "");
    for value in [
        "baseline 10",
        "first last",
        "baseline middle",
        "sub super",
        "10px 20px",
        "none",
    ] {
        assert_eq!(
            computed(
                &format!("#target {{vertical-align:super;vertical-align:{value}}}"),
                ""
            ),
            fallback,
            "sheet {value}"
        );
        assert_eq!(
            computed(
                "#target {vertical-align:super}",
                &format!("vertical-align:{value}")
            ),
            fallback,
            "inline {value}"
        );
        assert_eq!(
            computed(
                &format!("#target {{--v:{value};vertical-align:super;vertical-align:var(--v)}}"),
                ""
            ),
            initial,
            "var {value}"
        );
    }
}

#[test]
fn inline_grammar_unimplemented_baseline_modes_preserve_fallbacks_and_supports_queries() {
    use html_style::{PropertyCapability, UnsupportedStyleFeature, declaration_support};
    for value in [
        "first",
        "last sub",
        "central",
        "ideographic",
        "mathematical",
        "alphabetic",
        "hanging",
        "center",
        "text-top 10px",
        "super middle first",
    ] {
        assert_eq!(
            declaration_support("vertical-align", value).capability,
            PropertyCapability::Unsupported(UnsupportedStyleFeature::Value),
            "{value}"
        );
        let fallback = computed("#target {vertical-align:super}", "");
        assert_eq!(
            computed(
                &format!("#target {{vertical-align:super;vertical-align:{value}}}"),
                ""
            ),
            fallback,
            "sheet {value}"
        );
        assert_eq!(
            computed(
                "#target {vertical-align:super}",
                &format!("vertical-align:{value}")
            ),
            fallback,
            "inline {value}"
        );
        assert_eq!(
            computed(
                &format!(
                    "#target {{vertical-align:super}} @supports (vertical-align:{value}) {{#target {{vertical-align:bottom}}}}"
                ),
                ""
            ),
            fallback,
            "supports {value}"
        );
        assert_eq!(
            computed(
                &format!("#target {{--v:{value};vertical-align:super;vertical-align:var(--v)}}"),
                ""
            ),
            computed("#target {vertical-align:initial}", ""),
            "var {value}"
        );
    }
    for value in [
        "baseline super",
        "middle 0",
        "baseline 10px",
        "inherit",
        "var(--v)",
    ] {
        assert_eq!(
            declaration_support("vertical-align", value).capability,
            PropertyCapability::Supported,
            "{value}"
        );
    }
    assert_eq!(
        computed(
            "#target {vertical-align:super} @supports (vertical-align:baseline bottom) {#target {vertical-align:bottom}}",
            ""
        ),
        computed("#target {vertical-align:bottom}", "")
    );
}

#[test]
fn inline_grammar_vertical_align_rebinds_cached_contextual_shifts_to_the_viewport() {
    use html_style_model::VerticalAlignValue;
    let css = "#sheet {vertical-align:baseline calc(sign(2cqw - 10px) * 10px)} #variable {--v:baseline calc(sign(2cqw - 10px) * 10px);vertical-align:var(--v)}";
    let mut cache = StyleProgramCache::default();
    for (width, expected) in [(400.0, -10.0), (600.0, 10.0), (500.0, 0.0), (400.0, -10.0)] {
        let document = parse_dom_document("<!doctype html><p id='sheet'>a</p><p id='inline' style='vertical-align:baseline calc(sign(2cqw - 10px) * 10px)'>b</p><p id='variable'>c</p>").unwrap();
        let inputs = [AuthorStylesheetInput {
            css,
            implicit_scope_root: document.dom_root(),
        }];
        let (styled, _, queries) = style_document_with_cached_program_and_timings(
            &mut cache,
            document,
            &inputs,
            MediaEnvironment::screen(width, None).unwrap(),
        );
        for id in ["sheet", "inline", "variable"] {
            assert_eq!(
                style_by_id(&styled, id).box_model.vertical_align,
                VerticalAlignValue::Length(expected),
                "{id} at {width}"
            );
        }
        assert!(
            queries.uses_viewport_units(),
            "viewport dependency at {width}"
        );
    }
    assert!(cache.stats().hits > 0);
}

#[test]
fn inline_grammar_alignment_components_preserve_tree_and_random_function_contexts() {
    for value in [
        "baseline calc(sibling-index() * 2px)",
        "calc(sibling-index() * 2px)",
        "baseline random(2px, 2px)",
        "random(2px, 2px)",
    ] {
        assert_eq!(
            property_value_syntax("vertical-align", value),
            PropertyValueSyntax::Valid,
            "{value}"
        );
        let expected = computed("#target {vertical-align:2px}", "");
        assert_eq!(
            computed(&format!("#target {{vertical-align:{value}}}"), ""),
            expected,
            "sheet {value}"
        );
        assert_eq!(
            computed("", &format!("vertical-align:{value}")),
            expected,
            "inline {value}"
        );
        assert_eq!(
            computed(
                &format!("#target {{--v:{value};vertical-align:var(--v)}}"),
                ""
            ),
            expected,
            "var {value}"
        );
    }
}

#[test]
fn inline_grammar_vertical_align_contextual_shifts_resolve_without_syntax_standins() {
    for (value, expected) in [
        ("baseline calc(sign(1em - 100px) * 10px)", "-10px"),
        ("calc(sign(1em - 1px) * 20%) baseline", "20%"),
        ("baseline calc(sign(1em - 100px) * -10px)", "10px"),
    ] {
        let control = computed(&format!("#target {{vertical-align:{expected}}}"), "");
        assert_eq!(
            computed(&format!("#target {{vertical-align:{value}}}"), ""),
            control,
            "sheet {value}"
        );
        assert_eq!(
            computed("", &format!("vertical-align:{value}")),
            control,
            "inline {value}"
        );
        assert_eq!(
            computed(
                &format!("#target {{--v:{value};vertical-align:var(--v)}}"),
                ""
            ),
            control,
            "var {value}"
        );
    }
}

#[test]
fn inline_grammar_text_overflow_extensions_do_not_claim_marker_or_fade_painting() {
    use html_style::{PropertyCapability, UnsupportedStyleFeature, declaration_support};
    for property in ["text-overflow", "-o-text-overflow"] {
        for value in ["''", "'marker'", "clip ellipsis", "fade", "fade(10px)"] {
            assert_eq!(
                declaration_support(property, value).capability,
                PropertyCapability::Unsupported(UnsupportedStyleFeature::Value),
                "{property}: {value}"
            );
            let fallback = computed("#target {text-overflow:ellipsis}", "");
            assert_eq!(
                computed(
                    &format!("#target {{text-overflow:ellipsis;{property}:{value}}}"),
                    ""
                ),
                fallback,
                "sheet {property}: {value}"
            );
            assert_eq!(
                computed(
                    "#target {text-overflow:ellipsis}",
                    &format!("{property}:{value}")
                ),
                fallback,
                "inline {property}: {value}"
            );
            assert_eq!(
                computed(
                    &format!(
                        "#target {{text-overflow:ellipsis}} @supports ({property}:{value}) {{#target {{text-overflow:clip}}}}"
                    ),
                    ""
                ),
                fallback,
                "supports {property}: {value}"
            );
            assert_eq!(
                computed(
                    &format!("#target {{--v:{value};text-overflow:ellipsis;{property}:var(--v)}}"),
                    ""
                ),
                computed("#target {text-overflow:initial}", ""),
                "var {property}: {value}"
            );
        }
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
fn text_list_grammar_preserves_literal_fallbacks_and_unsets_invalid_substitutions() {
    for (property, good, bad, inherited) in [
        ("text-indent", "7px hanging", "10", "15px"),
        ("text-indent", "7px hanging", "10 hanging", "15px"),
        (
            "text-transform",
            "uppercase",
            "none lowercase",
            "capitalize",
        ),
        (
            "text-transform",
            "uppercase",
            "math-auto none",
            "capitalize",
        ),
        (
            "list-style-type",
            "decimal",
            "symbols(numeric 'a')",
            "square",
        ),
        (
            "list-style",
            "inside decimal",
            "inside symbols(alphabetic 'a')",
            "outside square",
        ),
    ] {
        let prior = format!("body {{{property}:{inherited}}} #target {{{property}:{good}}}");
        let invalid = format!(
            "body {{{property}:{inherited}}} #target {{{property}:{good};{property}:{bad}}}"
        );
        assert_eq!(
            computed(&invalid, ""),
            computed(&prior, ""),
            "literal {property}: {bad}"
        );
        let invalid = format!(
            "body {{{property}:{inherited}}} #target {{--bad:{bad};{property}:{good};{property}:var(--bad)}}"
        );
        assert_eq!(
            computed(&invalid, ""),
            computed(
                &format!("body {{{property}:{inherited}}} #target {{{property}:unset}}"),
                ""
            ),
            "substituted {property}: {bad}"
        );
    }
    for value in ["-10px hanging", "20% each-line", "calc(-10px)"] {
        assert_eq!(
            property_value_syntax("text-indent", value),
            PropertyValueSyntax::Valid,
            "{value}"
        );
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
        assert!(
            matches!(
                declaration_support(property, value).capability,
                PropertyCapability::Unsupported(_)
            ),
            "{property}: {value}"
        );
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
fn text_list_tab_math_preserves_number_and_length_types() {
    for value in [
        "3",
        "2.5",
        "4ch",
        "calc(-2)",
        "calc(-10px)",
        "calc(10 + sign(1em - 20px) * 5)",
        "calc(10px + sign(1em - 20px) * 5px)",
        "1px /* % */",
        "calc(NaN)",
        "calc(infinity)",
        "min(2, 4)",
    ] {
        assert_eq!(
            property_value_syntax("tab-size", value),
            PropertyValueSyntax::Valid,
            "{value}"
        );
    }
    for value in [
        "-1",
        "-10px",
        "20%",
        "calc(10px + 5%)",
        "calc(10px + 1)",
        "calc(sign(1em + 1) * 5px)",
        "calc(sign(1em) * 1s)",
        "1 2",
        "hanging 2px",
    ] {
        assert_eq!(
            property_value_syntax("tab-size", value),
            PropertyValueSyntax::Invalid,
            "{value}"
        );
    }
    for (size, amount) in [(10, 5), (30, 15)] {
        for (source, expected) in [
            (
                "calc(10 + sign(1em - 20px) * 5)".to_owned(),
                amount.to_string(),
            ),
            (
                "calc(10px + sign(1em - 20px) * 5px)".to_owned(),
                format!("{amount}px"),
            ),
        ] {
            assert_eq!(
                computed(
                    &format!("#target {{font-size:{size}px;tab-size:{source}}}"),
                    ""
                ),
                computed(
                    &format!("#target {{font-size:{size}px;tab-size:{expected}}}"),
                    ""
                ),
                "{source} at {size}px"
            );
            assert_eq!(
                computed("", &format!("font-size:{size}px;tab-size:{source}")),
                computed("", &format!("font-size:{size}px;tab-size:{expected}")),
                "inline {source} at {size}px"
            );
        }
    }
    for (source, expected) in [
        ("calc(-2)", "0"),
        ("calc(-10px)", "0px"),
        ("calc(NaN)", "0"),
    ] {
        assert_eq!(
            computed(&format!("#target {{tab-size:{source}}}"), ""),
            computed(&format!("#target {{tab-size:{expected}}}"), ""),
            "{source}"
        );
    }
    let document = parse_dom_document("<!doctype html><p id='target'>Text</p>").unwrap();
    let styled = style_document(document, &["#target {tab-size:calc(infinity)}"]);
    assert_eq!(
        style_by_id(&styled, "target").text.tab_size,
        TabSize::spaces(f32::MAX).unwrap()
    );
}

#[test]
fn text_list_tab_math_preserves_priority_substitution_and_inheritance() {
    let expression = "calc(10px + sign(1em - 20px) * 5px)";
    assert_eq!(
        computed(
            &format!("#target {{font-size:30px;tab-size:{expression}!important;tab-size:2}}"),
            ""
        ),
        computed("#target {font-size:30px;tab-size:15px}", "")
    );
    assert_eq!(
        computed(
            &format!("#target {{font-size:30px;--tab:{expression};tab-size:var(--tab)}}"),
            ""
        ),
        computed("#target {font-size:30px;tab-size:15px}", "")
    );
    assert_eq!(
        computed(
            "body {tab-size:4px} #target {--tab:calc(1px + 1);tab-size:2;tab-size:var(--tab)}",
            ""
        ),
        computed("body {tab-size:4px} #target {tab-size:inherit}", "")
    );
    assert_eq!(
        computed("#target {tab-size:4px;tab-size:calc(1px + 1)}", ""),
        computed("#target {tab-size:4px}", "")
    );
    let document =
        parse_dom_document("<!doctype html><div><p></p><p id='target'></p></div>").unwrap();
    let styled = style_document(
        document,
        &["#target {font-size:30px;tab-size:calc(sibling-index() * sign(1em - 20px) * 5px)}"],
    );
    assert_eq!(
        style_by_id(&styled, "target").text.tab_size,
        TabSize::length_px(10.0).unwrap()
    );
}

#[test]
fn rule_boundaries_scope_lists_are_unforgiving_and_element_only() {
    for prelude in [
        "",
        "(.a)",
        "(.a,.b:hover)",
        "(.a) to (.b)",
        "to (> .b)",
        "(.a) to (+ .b, ~ .c)",
        "(.a) to (& > &)",
        "(:is(.a, .bad <> .bad))",
        "(:where(.a))",
        r"([data-value='::before'])",
        "(.a) to (:has(> .b))",
    ] {
        assert!(
            stylesheet_syntax_is_valid(&format!("@scope {prelude} {{}}")),
            "{prelude}"
        );
    }
    for prelude in [
        "()",
        "to ()",
        "() to ()",
        "(.a, .b <> .c)",
        "(.b <> .c, .a)",
        "(.a,)",
        "(,.a)",
        "(div::before)",
        "(div::after)",
        "(div::part(foo))",
        "(div::slotted(span))",
        "(slotted(div))",
        "(.a) to (div::before)",
        "(> &) to (>>)",
        "(.a) to (.b, .c <> .d)",
        "(.a) unknown (.b)",
    ] {
        let rule = format!("@scope {prelude} {{}}");
        assert!(!stylesheet_syntax_is_valid(&rule), "{rule}");
        assert!(
            !stylesheet_syntax_is_valid(&format!("@media all {{@layer test {{{rule}}}}}")),
            "nested {rule}"
        );
    }
    assert!(stylesheet_syntax_is_valid(
        "a {--payload: {@scope () {}};content:'@scope () {}'}"
    ));
    assert!(stylesheet_syntax_is_valid("@unknown { @scope () {} }"));
}

#[test]
fn rule_boundaries_invalid_scopes_cannot_leak_recovered_members_into_cascade() {
    let baseline = "#target {color:blue;width:20px}";
    for prelude in [
        "()",
        "(.root, .bad <> .bad)",
        "(.root) to (.limit, .bad <> .bad)",
        "(.root) to ()",
        "(div::before)",
    ] {
        let css = format!(
            "{baseline} @scope {prelude} {{#target {{color:red;width:30;--payload:1}} @scope (.root) {{#target {{width:50px}}}}}} #target {{height:40px}}"
        );
        assert_eq!(
            computed(&css, ""),
            computed(&format!("{baseline} #target {{height:40px}}"), ""),
            "{prelude}"
        );
    }
    let document = parse_dom_document(
        "<!doctype html><section class='root'><p id='target'>Text</p></section>",
    )
    .unwrap();
    let styled = style_document(
        document,
        &["p {color:blue} @scope (.root, .bad <> .bad) {p {color:red}}"],
    );
    assert_eq!(style_by_id(&styled, "target").text.color, 0x0000ffff);
    let document = parse_dom_document(
        "<!doctype html><section class='root'><p id='target'>Text</p></section>",
    )
    .unwrap();
    let styled = style_document(document, &["p {color:blue} @scope (.root) {p {color:red}}"]);
    assert_eq!(style_by_id(&styled, "target").text.color, 0xff0000ff);
    let document = parse_dom_document(
        "<!doctype html><section class='root'><p id='target'>Text</p></section>",
    )
    .unwrap();
    let styled = style_document(
        document,
        &["p {color:blue} @scope (.root) to (.limit, .bad <> .bad) {p {color:red}}"],
    );
    assert_eq!(style_by_id(&styled, "target").text.color, 0x0000ffff);
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
fn flex_context_math_computes_with_fonts_and_preserves_longhand_winners() {
    for (size, factor, basis) in [(10, 0, 5), (30, 1, 15)] {
        for (source, expected) in [
            (
                "flex:sign(1em - 20px) 2".to_owned(),
                format!("flex:{factor} 2 0%"),
            ),
            (
                "flex:2 sign(1em - 20px)".to_owned(),
                format!("flex:2 {factor} 0%"),
            ),
            (
                "flex:10px sign(1em - 20px) 2".to_owned(),
                format!("flex:{factor} 2 10px"),
            ),
            (
                "--f:sign(1em - 20px) 2;flex:var(--f);flex-grow:9".to_owned(),
                "flex:9 2 0%".to_owned(),
            ),
            (
                "flex:sign(1em - 20px) 2 content !/**/IMPORTANT;flex:9 8 7px".to_owned(),
                format!("flex:{factor} 2 content"),
            ),
            (
                r"-webkit-flex:s\69 gn(1em - 20px) 2 content".to_owned(),
                format!("flex:{factor} 2 content"),
            ),
            (
                "flex:sign(1em - 20px) 2 10px".to_owned(),
                format!("flex:{factor} 2 10px"),
            ),
            (
                "flex:sign(1em - 20px) 2 content".to_owned(),
                format!("flex:{factor} 2 content"),
            ),
            (
                "flex-grow:sign(1em - 20px);flex-shrink:calc(2 + sign(1em - 20px))".to_owned(),
                format!(
                    "flex-grow:{factor};flex-shrink:{}",
                    if size == 10 { 1 } else { 3 }
                ),
            ),
            (
                "flex:2 3 calc(10px + sign(1em - 20px) * 5px)".to_owned(),
                format!("flex:2 3 {basis}px"),
            ),
            (
                "flex-grow:7!important;flex:sign(1em - 20px) 2 content".to_owned(),
                "flex:7 2 content".to_owned(),
            ),
            (
                "flex:sign(1em - 20px) 2 content;flex-shrink:8".to_owned(),
                format!("flex:{factor} 8 content"),
            ),
            (
                "flex-basis:40px!important;flex:sign(1em - 20px) 2 content".to_owned(),
                format!("flex:{factor} 2 40px"),
            ),
            (
                "--f:sign(1em - 20px) 2 content;flex:var(--f);flex-grow:9".to_owned(),
                "flex:9 2 content".to_owned(),
            ),
        ] {
            let css = format!("#target {{font-size:{size}px;{source}}}");
            let expected_css = format!("#target {{font-size:{size}px;{expected}}}");
            assert_eq!(computed(&css, ""), computed(&expected_css, ""), "{css}");
            assert_eq!(
                computed("", &format!("font-size:{size}px;{source}")),
                computed("", &format!("font-size:{size}px;{expected}")),
                "inline {css}"
            );
        }
    }
    let document = parse_dom_document("<div><p></p><p id='target'></p></div>").unwrap();
    let styled = style_document(
        document,
        &["#target {font-size:30px;flex:sign(1em - 20px) sibling-index();flex-grow:8}"],
    );
    let style = style_by_id(&styled, "target");
    assert_eq!(
        (
            style.layout.flex_grow,
            style.layout.flex_shrink,
            style.layout.flex_basis
        ),
        (8.0, 2.0, PreferredSize::Percent(0.0))
    );
}

#[test]
fn flex_context_invalid_math_preserves_fallbacks_and_unsets_bad_substitutions() {
    let invalid = "sign(1px + 1s) 2 10px";
    assert_eq!(
        computed(&format!("#target {{flex:3 4 20px;flex:{invalid}}}"), ""),
        computed("#target {flex:3 4 20px}", "")
    );
    assert_eq!(
        computed(
            &format!("#target {{--bad:{invalid};flex:3 4 20px;flex:var(--bad);flex-grow:7}}"),
            ""
        ),
        computed("#target {flex:initial;flex-grow:7}", "")
    );
}

#[test]
fn flex_context_balance_capabilities_preserve_whole_declaration_fallbacks() {
    use html_style::{PropertyCapability, declaration_support};
    for (property, value) in [
        ("flex-wrap", "balance"),
        ("flex-flow", "row balance"),
        ("flex-flow", "wrap-reverse balance column"),
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
                &format!("#target {{flex-flow:column wrap;{property}:{value}}}"),
                ""
            ),
            computed("#target {flex-flow:column wrap}", "")
        );
        assert_eq!(
            computed(
                &format!(
                    "#target {{flex-flow:column wrap}} @supports ({property}:{value}) {{#target {{flex-direction:row}}}}"
                ),
                ""
            ),
            computed("#target {flex-flow:column wrap}", "")
        );
    }
    for property in ["flex-wrap", "flex-flow"] {
        for value in ["initial", "inherit", "unset", "revert", "revert-layer"] {
            assert_eq!(
                declaration_support(property, value).capability,
                PropertyCapability::Supported
            );
        }
    }
    assert_eq!(
        declaration_support("flex-wrap", "wrap-reverse").capability,
        PropertyCapability::Supported
    );
}

#[test]
fn grid_grammar_invalid_literals_preserve_fallbacks_and_substitution_unsets() {
    for (property, value, fallback) in [
        ("grid-template-columns", "-10px", "20px"),
        (
            "grid-template-columns",
            "auto repeat(auto-fill,10px)",
            "20px",
        ),
        ("grid-auto-rows", "fit-content(-10px)", "20px"),
        ("grid-row-start", "span -1", "2"),
    ] {
        let previous = format!("#target {{{property}:{fallback}}}");
        let expected = computed(&previous, "");
        assert_eq!(
            computed(&format!("{previous} #target {{{property}:{value}}}"), ""),
            expected,
            "sheet {property}: {value}"
        );
        assert_eq!(
            computed(&previous, &format!("{property}:{value}")),
            expected,
            "inline {property}: {value}"
        );
        assert_eq!(
            computed(
                "",
                &format!("{property}:{fallback};--bad:{value};{property}:var(--bad)")
            ),
            computed("", &format!("{property}:unset")),
            "substituted {property}: {value}"
        );
    }
}

#[test]
fn grid_grammar_capabilities_preserve_native_layout_without_advertising_unrepresented_values() {
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
    for property in ["grid", "grid-template-columns", "grid-row-start"] {
        for keyword in ["initial", "inherit", "unset", "revert", "revert-layer"] {
            assert_eq!(
                declaration_support(property, keyword).capability,
                PropertyCapability::Supported
            );
            assert_eq!(
                computed(
                    &format!("@supports ({property}:{keyword}) {{#target {{color:red}}}}"),
                    ""
                ),
                computed("#target {color:red}", ""),
                "{property}: {keyword}"
            );
        }
    }
    for (property, value) in [
        ("grid-template-columns", "subgrid [a] repeat(auto-fill,[b])"),
        ("grid-template", "subgrid / 20px"),
        ("grid-row-start", "1 span"),
        ("grid-row-start", "span calc(-1)"),
        ("grid-template-columns", "repeat(auto-fill,auto)"),
        ("grid-template-rows", "repeat(auto-fit,1fr)"),
    ] {
        let support = declaration_support(property, value);
        assert_eq!(
            support.syntax,
            PropertySyntax::Unsupported,
            "{property}: {value}"
        );
        assert!(
            matches!(support.capability, PropertyCapability::Unsupported(_)),
            "{property}: {value}"
        );
        let previous = "#target {grid-template-columns:20px;grid-row-start:2}";
        let expected = computed(previous, "");
        assert_eq!(
            computed(&format!("{previous} #target {{{property}:{value}}}"), ""),
            expected,
            "sheet {property}"
        );
        assert_eq!(
            computed(previous, &format!("{property}:{value}")),
            expected,
            "inline {property}"
        );
        assert_eq!(
            computed(
                &format!("{previous} @supports ({property}:{value}) {{#target {{color:red}}}}"),
                ""
            ),
            expected,
            "supports {property}"
        );
    }
    for (property, value) in [
        ("grid-template-columns", "[subgrid] 20px"),
        ("grid-template-columns", "repeat(auto-fill,20px)"),
        ("grid-row-start", "span 2"),
    ] {
        assert_eq!(
            declaration_support(property, value).capability,
            PropertyCapability::Supported,
            "{property}: {value}"
        );
    }
}

#[test]
fn nonfinite_math_grammar_does_not_use_syntax_standins_for_color_rendering() {
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
    let fallback = "#target {color:blue}";
    for value in ["rgb(calc(0 / 0) 0 0)", "hsl(calc(1deg / 0) 50% 50%)"] {
        let support = declaration_support("color", value);
        assert_eq!(support.syntax, PropertySyntax::Unsupported, "{value}");
        assert!(
            matches!(support.capability, PropertyCapability::Unsupported(_)),
            "{value}"
        );
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
        assert_eq!(
            computed(
                &format!("{fallback} @supports (color:{value}) {{#target {{color:red}}}}"),
                ""
            ),
            computed(fallback, ""),
            "supports {value}"
        );
    }
}

#[test]
fn color_function_grammar_preserves_unsupported_fallbacks_and_feature_queries() {
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
    let fallback = "#target {color:blue;background-color:yellow}";
    for value in [
        "color-layers(red,blue)",
        "color-layers(red,rgb(random(fixed .5,0,255) 0 0))",
        "contrast-color(currentcolor)",
        "contrast-color(rgb(random(fixed .5,0,255) 0 0))",
        "alpha(from currentcolor / alpha)",
    ] {
        let support = declaration_support("color", value);
        assert_eq!(support.syntax, PropertySyntax::Unsupported, "{value}");
        assert!(
            matches!(support.capability, PropertyCapability::Unsupported(_)),
            "{value}"
        );
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
        assert_eq!(
            computed(
                &format!("{fallback} @supports (color:{value}) {{#target {{color:red}}}}"),
                ""
            ),
            computed(fallback, ""),
            "supports {value}"
        );
    }
}

#[test]
fn color_channel_grammar_keeps_uncomputed_values_out_of_feature_queries_and_cascade() {
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
    let fallback = "#target {color:blue;background-color:yellow}";
    for value in [
        "rgb(from currentcolor r g b)",
        "color-mix(in hsl,hsl(none none none),currentcolor)",
        "color(display-p3-linear .1 .2 .3)",
        "rgb(calc(sign(1em - 1px) * 20) 0 0)",
    ] {
        let support = declaration_support("color", value);
        assert_eq!(support.syntax, PropertySyntax::Unsupported, "{value}");
        assert!(
            matches!(support.capability, PropertyCapability::Unsupported(_)),
            "{value}"
        );
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
        assert_eq!(
            computed(
                &format!("{fallback} @supports (color:{value}) {{#target {{color:red}}}}"),
                ""
            ),
            computed(fallback, ""),
            "supports {value}"
        );
    }
}

#[test]
fn color_property_grammar_keeps_uncomputed_colors_out_of_cascade_and_supports() {
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
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
        assert_eq!(support.syntax, PropertySyntax::Unsupported);
        assert!(
            matches!(support.capability, PropertyCapability::Unsupported(_)),
            "{property}"
        );
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
fn color_property_grammar_rejects_invalid_literal_mixes_before_native_conversion() {
    for value in [
        "color-mix(in srgb,red -10%,blue)",
        "color-mix(in srgb,red 110%,blue)",
        "color-mix(in srgb longer hue,red,blue)",
    ] {
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
}

#[test]
fn color_property_grammar_nonfinite_hues_do_not_probe_native_conversion() {
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
    for value in [
        "hsl(calc(infinity) 100% 50%)",
        "hsl(calc(-infinity) 100% 50%)",
        "hwb(calc(infinity) 20% 10%)",
        "hwb(calc(-infinity) 20% 10%)",
    ] {
        let support = declaration_support("color", value);
        assert_eq!(support.syntax, PropertySyntax::Unsupported, "{value}");
        assert!(
            matches!(support.capability, PropertyCapability::Unsupported(_)),
            "{value}"
        );
    }
}

#[test]
fn decoration_auto_grammar_reports_unsupported_rendering_and_preserves_fallbacks() {
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
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
        assert_eq!(
            support.syntax,
            PropertySyntax::Unsupported,
            "{property}: {value}"
        );
        assert!(
            matches!(support.capability, PropertyCapability::Unsupported(_)),
            "{property}: {value}"
        );
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
fn scroll_button_selector_grammar_validates_directions_and_state_combinations() {
    for direction in [
        "up",
        "down",
        "left",
        "right",
        "block-start",
        "block-end",
        "inline-start",
        "inline-end",
        "prev",
        "next",
        "*",
    ] {
        for state in [
            "",
            ":focus",
            ":focus-visible",
            ":focus-within",
            ":hover",
            ":active",
            ":enabled",
            ":disabled",
            ":not(:disabled)",
            ":is(:focus,:hover)",
        ] {
            let selector = format!("#target::scroll-button({direction}){state}");
            assert!(selector_syntax_is_valid(&selector), "{selector}");
        }
    }
    for selector in [
        "::scroll-button( up )",
        "::scroll-button(/**/up/**/)",
        "::SCROLL-BUTTON(UP)",
        r"::s\63 roll-button(u\70 )",
        "#target:hover::scroll-button(up):focus",
        "section > #target::scroll-button(next):not(:disabled)",
        "::scroll-button(up):is(.discarded,:hover)",
    ] {
        assert!(selector_syntax_is_valid(selector), "{selector}");
    }
    for selector in [
        "::scroll-button",
        "::scroll-button()",
        "::scroll-button(north)",
        "::scroll-button('up')",
        "::scroll-button(42)",
        "::scroll-button(up down)",
        "::scroll-button(up,down)",
        "::scroll-button(up,)",
        "::scroll-button(calc(up))",
        "::scroll-button(**)",
        "::scroll-button(--direction)",
        "::scroll-button(var(--direction))",
    ] {
        assert!(!selector_syntax_is_valid(selector), "{selector}");
    }
}

#[test]
fn scroll_button_selector_grammar_preserves_pseudo_subject_and_forgiving_list_constraints() {
    for selector in [
        "::scroll-button(up).class",
        "::scroll-button(up)#id",
        "::scroll-button(up)[disabled]",
        "::scroll-button(up):empty",
        "::scroll-button(up):first-child",
        "::scroll-button(up):checked",
        "::scroll-button(up):not(:first-child)",
        "::scroll-button(up):not(.class)",
        "::scroll-button(up)::before",
        "::scroll-button(up)::scroll-button(down)",
        "::scroll-button(up) > div",
        "::scroll-button(up) div",
        "::scroll-button(up) + div",
        "div:has(::scroll-button(up))",
        ":not(::scroll-button(up))",
        "#target,::scroll-button(north)",
    ] {
        assert!(!selector_syntax_is_valid(selector), "{selector}");
    }
    assert!(selector_syntax_is_valid("#target,::scroll-button(up)"));
    assert!(selector_syntax_is_valid(":is(#target,::scroll-button(up))"));
    assert_eq!(
        computed(
            "#target {color:blue} #target::scroll-button(up),#target {color:red}",
            ""
        ),
        computed("#target {color:red}", "")
    );
    assert_eq!(
        computed(
            "#target {color:blue} #target::scroll-button(north),#target {color:red}",
            ""
        ),
        computed("#target {color:blue}", "")
    );
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
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
    for value in [
        "grid-lanes",
        "inline-grid-lanes",
        "inline grid-lanes",
        r"g\72 id-lanes",
    ] {
        let support = declaration_support("display", value);
        assert_eq!(support.syntax, PropertySyntax::Unsupported);
        assert!(
            matches!(support.capability, PropertyCapability::Unsupported(_)),
            "{value}"
        );
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
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
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
        assert_eq!(support.syntax, PropertySyntax::Unsupported);
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
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
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
            assert_eq!(support.syntax, PropertySyntax::Unsupported);
            assert!(
                matches!(support.capability, PropertyCapability::Unsupported(_)),
                "{property}: {value}"
            );
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
    for value in ["none underline", "underline none", "underline underline"] {
        assert_eq!(
            property_value_syntax("text-decoration-line", value),
            PropertyValueSyntax::Invalid
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
    use html_style::{PropertyCapability, PropertySyntax, declaration_support};
    for (property, value) in [
        ("background-image", "image(red)"),
        ("list-style-image", "image(red)"),
        ("cursor", "image-set('a.png' 1x),pointer"),
    ] {
        let support = declaration_support(property, value);
        assert_eq!(support.syntax, PropertySyntax::Unsupported);
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

#[test]
fn dashed_function_calls_validate_argument_lists_without_function_definitions() {
    for value in [
        "--func()",
        "--func(/**/)",
        "--func(auto)",
        "--func(100px)",
        "--func(#fff)",
        "--func(auto,100px,#fff)",
        "--func(\"thing\")",
        "--func(--bar(),--baz(--fez()))",
        "--func({1},2)",
        "--func({ 1 },2)",
        "--func({1,2,3},{4})",
        "--func({,},{4})",
        "--func({{}},{4})",
        "--func({[]},{4})",
        "--func(myident)",
        "--func(--myident)",
        "--func(--)",
        "--func(50px --myident:)",
        "--func({--myident:})",
        "--func({ --myident : })",
        "--func(10px,{ --myident : })",
        "--f\\75 nc(1px)",
        "--FUNC()",
        "calc(--func() + 1px)",
    ] {
        assert_eq!(
            property_value_syntax("top", value),
            PropertyValueSyntax::Valid,
            "{value}"
        );
    }
    for value in [
        "--func(!)",
        "--func(;)",
        "--func(})",
        "--func(])",
        "--func(asdf,)",
        "--func(,)",
        "--func(, )",
        "--func(a,,b)",
        "--func(a, ,b)",
        "--func(123 {})",
        "--func({}1)",
        "--func({} 1)",
        "--func(1,2{})",
        "--func(1,{}2)",
        "--func({})",
        "--func(1,{ })",
        "--func(--myident:)",
        "--func( --myident : )",
        "--func(--myident/**/:)",
        "--func(10px,--myident : )",
        "--func(--bad(!))",
    ] {
        assert_eq!(
            property_value_syntax("top", value),
            PropertyValueSyntax::Invalid,
            "{value}"
        );
        let declaration = format!("top:13px;top:{value}");
        assert_eq!(
            computed("", &declaration),
            computed("", "top:13px"),
            "inline {value}"
        );
        assert_eq!(
            computed(&format!("#target {{{declaration}}}"), ""),
            computed("", "top:13px"),
            "sheet {value}"
        );
    }
}

#[test]
fn dashed_function_missing_names_unset_winners_and_skip_unused_arguments() {
    let baseline = "color:green;background-color:red;top:13px;width:17px;margin:4px;font-family:serif;font-size:21px;line-height:26px;tab-size:3;letter-spacing:2px;font-feature-settings:'ss01' 2";
    for property in [
        "color",
        "background-color",
        "top",
        "width",
        "margin",
        "font",
        "font-family",
        "font-size",
        "line-height",
        "tab-size",
        "letter-spacing",
        "font-feature-settings",
    ] {
        let expected = computed("", &format!("{baseline};{property}:unset"));
        for declaration in [
            format!("{baseline};{property}:--missing(var(--argument))"),
            format!("--value:--missing();{baseline};{property}:var(--value)"),
        ] {
            assert_eq!(computed("", &declaration), expected, "inline {declaration}");
            assert_eq!(
                computed(&format!("#target {{{declaration}}}"), ""),
                expected,
                "sheet {declaration}"
            );
        }
    }
    assert_eq!(
        computed("", "margin:4px;margin:--missing();margin-left:9px"),
        computed("", "margin:unset;margin-left:9px")
    );
    for declaration in [
        "--found:green;color:var(--found,--missing())",
        "color:random-item(fixed 0,green,--missing(var(--cycle)))",
        "--a:--missing(var(--b));--b:var(--a,green);color:var(--b)",
        "--value:--missing();color:var(--value,green)",
    ] {
        assert_eq!(
            computed("", declaration),
            computed("", "color:green"),
            "{declaration}"
        );
    }
    assert_eq!(
        computed("", "--func:green;color:--func()"),
        computed("", "color:unset"),
        "custom properties are not function definitions"
    );
    let styled = style_document(
        parse_dom_document("<!doctype html><section style='--tone:green'><p id='target' style='color:inherit(--tone,--missing())'></p></section>").unwrap(),
        &[],
    );
    assert_eq!(style_by_id(&styled, "target").text.color, 0x008000ff);
}

#[test]
fn dashed_function_syntax_queries_keep_missing_names_and_quoted_text_distinct() {
    assert_eq!(
        computed("@supports (top:--missing()) {#target {color:green}}", ""),
        computed("", "color:green")
    );
    assert_eq!(
        computed(
            "@supports not (top:--missing(,)) {#target {color:green}}",
            ""
        ),
        computed("", "color:green")
    );
    assert_eq!(
        property_value_syntax("top", "'--missing()'"),
        PropertyValueSyntax::Invalid
    );
    assert_eq!(
        property_value_syntax("font-family", "'--missing()'"),
        PropertyValueSyntax::Valid
    );
    assert_eq!(
        computed("", "font-family:'--missing()'"),
        computed("", "font-family:\"--missing()\"")
    );
}
#[test]
fn random_math_shares_keys_with_items_and_rebinds_raw_custom_values() {
    let css = "section {--range:random(element-scoped,0px,4000000px)} p {position:relative;width:var(--range);height:random-item(element-scoped,0px,1000000px,2000000px,3000000px);left:random(--shared,0px,1000000px);right:random(--shared,10px,1000010px);margin:random-item(auto,0px,1000000px,2000000px,3000000px) random(auto,0px,4000000px);margin-left:17px;padding-top:random(element-scoped ua-margin-1,0px,4000000px);padding-right:random(element-scoped ua-margin-2,0px,4000000px)} p::before {content:'x';width:var(--range);height:random-item(element-scoped,0px,1000000px,2000000px,3000000px)}";
    let children = (0..64)
        .map(|n| format!("<p id='p{n}'></p>"))
        .collect::<String>();
    let document =
        parse_dom_document(&format!("<!doctype html><section>{children}</section>")).unwrap();
    let mut cache = StyleProgramCache::default();
    let mut previous = None;
    for pass in 0..3 {
        let inputs = [AuthorStylesheetInput {
            css,
            implicit_scope_root: document.dom_root(),
        }];
        let (program, timings) = cache.get_or_compile(
            &inputs,
            MediaEnvironment::screen(if pass == 2 { 500.0 } else { 400.0 }, Some(800.0)).unwrap(),
            document.root_font_size(),
        );
        let (styles, _, _) = resolve_document_with_program(
            &document,
            &program,
            &inputs,
            timings,
            &ReaderStyleOverrides::default(),
        );
        let mut values = Vec::new();
        let mut global = None;
        for n in 0..64 {
            let id = format!("p{n}");
            let node = document
                .node_ids()
                .find(|node| document.get_dom_id(*node) == Some(id.as_str()))
                .unwrap();
            let style = styles.view(styles.style_for_node(node).unwrap()).unwrap();
            let PreferredSize::Px(width) = style.box_model.width else {
                panic!("computed random width")
            };
            assert!((0.0..=4000000.0).contains(&width));
            values.push(width.to_bits());
            assert_eq!(
                style.box_model.height,
                PreferredSize::Px((width / 1000000.0).floor().min(3.0) * 1000000.0)
            );
            let Some(LengthPct::Px(left)) = style.layout.inset_left else {
                panic!("global key offset")
            };
            let Some(LengthPct::Px(right)) = style.layout.inset_right else {
                panic!("global key offset")
            };
            assert!((right - left - 10.0).abs() < 0.1);
            if let Some(global) = global {
                assert_eq!(left, global);
            }
            global = Some(left);
            assert_eq!(style.box_model.margin_left, LengthPct::Px(17.0));
            let LengthPct::Px(first) = style.box_model.padding_top else {
                panic!("first function index")
            };
            assert_eq!(
                style.box_model.margin_top,
                LengthPct::Px((first / 1000000.0).floor().min(3.0) * 1000000.0)
            );
            assert_eq!(style.box_model.margin_right, style.box_model.padding_right);
            let pseudo = styles
                .view(styles.before_style_for_node(node).unwrap().0)
                .unwrap();
            let PreferredSize::Px(width) = pseudo.box_model.width else {
                panic!("pseudo random width")
            };
            assert_eq!(
                pseudo.box_model.height,
                PreferredSize::Px((width / 1000000.0).floor().min(3.0) * 1000000.0)
            );
        }
        assert!(
            values.iter().any(|value| *value != values[0]),
            "raw custom-property functions evaluate on each consuming element"
        );
        if let Some(previous) = &previous {
            assert_eq!(&values, previous);
        }
        previous = Some(values);
    }
    assert_eq!(cache.stats().hits, 1);
    assert_eq!(cache.stats().misses, 2);
}

#[test]
fn random_math_fixed_ranges_steps_and_nested_values_compute() {
    for (property, value, expected) in [
        ("width", "random(fixed 0.5,10px,30px)", "20px"),
        ("width", "random(fixed 0.5,1in,200px,26px)", "148px"),
        (
            "width",
            "random(fixed 0.5,10px,30px,calc(infinity * 1px))",
            "10px",
        ),
        (
            "width",
            "random(fixed random(fixed 0.5,0,1),10px,30px)",
            "20px",
        ),
        (
            "color",
            "random-item(fixed random(fixed 1,0,1),red,green)",
            "green",
        ),
        ("width", "random(fixed 0.5,30px,10px)", "30px"),
        ("width", "random(fixed 0.5,-30px,-10px)", "0px"),
        ("width", "random(fixed 0.5,0px,100px,3px)", "51px"),
        ("width", "random(fixed 1,0px,100px,33.333px)", "100px"),
        ("width", "random(fixed 0.5,0px,100px,0px)", "50px"),
        ("width", "random(fixed 0.5,0px,100px,-5px)", "50px"),
        ("left", "calc(2 * random(fixed 0.25,0px,100px))", "50px"),
        ("z-index", "random(fixed 0.5,0,9)", "5"),
        ("font-weight", "random(fixed 0.5,300,500)", "400"),
        (
            "font-feature-settings",
            "'ss01' random(fixed 0.5,1,2)",
            "'ss01' 2",
        ),
        ("font-size", "random(fixed 0.5,16px,32px)", "24px"),
        (
            "font-style",
            "oblique random(fixed 0.5,10deg,20deg)",
            "oblique 15deg",
        ),
        ("font-size", "random(fixed 0.5,1em,2rem)", "24px"),
        ("line-height", "random(fixed 0.5,20px,40px)", "30px"),
        ("tab-size", "random(fixed 0.5,0,4)", "2"),
        ("letter-spacing", "random(fixed 0.5,0%,10%)", "5%"),
        (
            "color",
            "rgb(random(fixed 0.5,0,255) 0 0)",
            "rgb(127.5 0 0)",
        ),
        (
            "color",
            "rgb(from blue r g random(fixed 0.5,b,(150 + b)))",
            "rgb(0 0 255)",
        ),
        (
            "width",
            "random(fixed 0.5,random(fixed 0,10px,20px),30px)",
            "20px",
        ),
        (
            "width",
            "random-item(fixed 0,random(fixed 0.5,10px,30px),99px)",
            "20px",
        ),
    ] {
        assert_eq!(
            property_value_syntax(property, value),
            PropertyValueSyntax::Valid,
            "{property}:{value}"
        );
        let expected = computed("", &format!("{property}:{expected}"));
        for declaration in [
            format!("{property}:{value}"),
            format!("--value:{value};{property}:var(--value)"),
            format!("{property}:var(--missing,{value})"),
        ] {
            assert_eq!(computed("", &declaration), expected, "inline {declaration}");
            assert_eq!(
                computed(&format!("#target {{{declaration}}}"), ""),
                expected,
                "sheet {declaration}"
            );
        }
    }
}

#[test]
fn random_math_mixed_percentages_remain_deferred_until_size_resolution() {
    let styled = style_document(
        parse_dom_document(
            "<!doctype html><p id='target' style='width:random(fixed 0.5,10px,20%)'></p>",
        )
        .unwrap(),
        &[],
    );
    let node = styled
        .document()
        .node_ids()
        .find(|node| styled.document().get_dom_id(*node) == Some("target"))
        .unwrap();
    let used = styled
        .styles()
        .used_view(styled.style_for_node(node).unwrap(), 0.5, 0.5, 0.7)
        .unwrap();
    for (parent, expected) in [(20.0, 10.0), (100.0, 15.0), (400.0, 45.0)] {
        let actual = resolve_used_preferred_size(used.width(), 0.0, parent);
        assert!(
            (actual - expected).abs() < 0.0001,
            "expected {expected}, got {actual}"
        );
    }
}

#[test]
fn random_math_rejects_malformed_keys_and_inconsistent_argument_types() {
    for value in [
        "random()",
        "random(1px)",
        "random(,1px)",
        "random(1px,)",
        "random(1px,2px,)",
        "random(foo,1px,2px)",
        "random(fixed -1,1px,2px)",
        "random(fixed 0.5px,1px,2px)",
        "random(--a --b,1px,2px)",
        "random(element-scoped element-scoped,1px,2px)",
        "random(1px,2deg)",
        "random(1px,2px,3)",
        "random(1,2px)",
    ] {
        assert_eq!(
            property_value_syntax("width", value),
            PropertyValueSyntax::Invalid,
            "{value}"
        );
        assert_eq!(
            computed("", &format!("width:17px;width:{value}")),
            computed("", "width:17px"),
            "{value}"
        );
    }
}

#[test]
fn random_item_fixed_keys_resolve_values_variables_and_shorthands() {
    for (property, value, expected) in [
        ("color", "random-item(fixed 0,red,green,blue)", "red"),
        ("color", "random-item(fixed 0.5,red,green,blue)", "green"),
        ("color", "random-item(fixed 0.999,red,green,blue)", "blue"),
        ("color", "random-item(fixed 1,red,green,blue)", "blue"),
        (
            "color",
            "random-item(fixed calc(1 / 4),red,green,blue)",
            "red",
        ),
        ("color", "random-item(fixed calc(-1),red,green,blue)", "red"),
        ("color", "random-item(fixed calc(2),red,green,blue)", "blue"),
        ("color", "random-item(auto,red)", "red"),
        ("color", "random-item(--,green)", "green"),
        ("color", "random-item(--key element-scoped,green)", "green"),
        ("color", "random-item(element-scoped --key,green)", "green"),
        ("color", "random-item(property-scoped,green)", "green"),
        (
            "color",
            "random-item(--key property-index-scoped element-scoped,green)",
            "green",
        ),
        (
            "color",
            "random-item(fixed 0,random-item(fixed 1,blue,green),red)",
            "green",
        ),
        (
            "color",
            "random-item(fixed 1,var(--missing),green)",
            "green",
        ),
        (
            "color",
            "random-item(fixed 0,var(--missing,green),blue)",
            "green",
        ),
        (
            "color",
            "random-item(fixed 1,red,var(--missing,green))",
            "green",
        ),
        (
            "font-family",
            "random-item(fixed 0, {Times,serif}, {Arial,sans-serif})",
            "Times,serif",
        ),
        (
            "font-family",
            "random-item(fixed 1, { Times,serif }, { Arial,sans-serif })",
            "Arial,sans-serif",
        ),
        ("margin", "random-item(fixed 0,1px 2px,3px 4px)", "1px 2px"),
        ("left", "calc(1px + random-item(fixed 1,1px,2px))", "3px"),
        ("letter-spacing", "random-item(fixed 1,1px,2%)", "2%"),
        ("tab-size", "random-item(fixed 1,1,3)", "3"),
        (
            "font-feature-settings",
            "random-item(fixed 1,{'ss01' 2},{'ss02' 3})",
            "'ss02' 3",
        ),
        ("color", "RANDOM-ITEM(/**/fixed 0,red,green)", "red"),
        ("color", "random\\2d item(fixed 1,red,green)", "green"),
    ] {
        assert_eq!(
            property_value_syntax(property, value),
            PropertyValueSyntax::Valid,
            "{property}:{value}"
        );
        let wanted = computed("", &format!("{property}:{expected}"));
        for declaration in [
            format!("{property}:{value}"),
            format!("--value:{value};{property}:var(--value)"),
            format!("{property}:var(--missing,{value})"),
        ] {
            assert_eq!(computed("", &declaration), wanted, "inline {declaration}");
            assert_eq!(
                computed(&format!("#target {{{declaration}}}"), ""),
                wanted,
                "sheet {declaration}"
            );
        }
    }
    assert_eq!(
        computed(
            "",
            "--key:fixed 1;--tone:green;color:random-item(var(--key),red,var(--tone))"
        ),
        computed("", "color:green")
    );
    assert_eq!(
        computed(
            "",
            "margin:random-item(fixed 0,1px 2px,3px 4px);margin-left:17px"
        ),
        computed("", "margin:1px 2px;margin-left:17px")
    );
}

#[test]
fn random_item_grammar_invalid_selections_and_cycles_preserve_the_cascade() {
    for value in [
        "random-item()",
        "random-item( )",
        "random-item(auto)",
        "random-item(,serif)",
        "random-item(auto,!)",
        "random-item(auto,;)",
        "random-item(auto,{serif)",
        "random-item(auto,serif})",
        "random-item(auto,{Times,serif} extra)",
        "random-item(auto,extra {Times,serif})",
    ] {
        assert_eq!(
            property_value_syntax("font-family", value),
            PropertyValueSyntax::Invalid,
            "{value}"
        );
        assert_eq!(
            computed("", &format!("font-family:serif;font-family:{value}")),
            computed("", "font-family:serif"),
            "{value}"
        );
    }
    for value in [
        "random-item(serif,sans-serif)",
        "random-item(foo,red)",
        "random-item(fixed -0.5,red)",
        "random-item(fixed 1.5,red)",
        "random-item(fixed 0.5,red,,blue)",
        "random-item(auto,)",
        "random-item(fixed 0,var(--missing),blue)",
        "random-item(--x --y,red)",
        "random-item(element-scoped element-scoped,red)",
    ] {
        assert_eq!(
            property_value_syntax("color", value),
            PropertyValueSyntax::Valid,
            "{value}"
        );
        assert_eq!(
            computed("", &format!("color:green;color:{value}")),
            computed("", "color:unset"),
            "{value}"
        );
        assert_eq!(
            computed(&format!("#target {{color:green;color:{value}}}"), ""),
            computed("", "color:unset"),
            "sheet {value}"
        );
    }
    for declaration in [
        "--a:random-item(fixed 0,green,var(--a));color:var(--a)",
        "--a:random-item(fixed 0,var(--a),red);color:var(--a,green)",
        "--a:random-item(fixed 0,var(--b),blue);--b:var(--a,red);color:var(--a,green)",
        "--a:random-item(fixed 1,blue,green);color:random-item(fixed 0,var(--a),var(--missing))",
        "color:var(--missing,random-item(fixed 0,green,red))",
    ] {
        assert_eq!(
            computed("", declaration),
            computed("", "color:green"),
            "{declaration}"
        );
    }
    assert_eq!(
        computed("", "color:random-item(fixed 0,orange)red"),
        computed("", "color:unset")
    );
    assert_eq!(
        computed("", "tab-size:random-item(fixed 0,initial,3)"),
        computed("", "tab-size:initial")
    );
    assert_eq!(
        computed(
            "",
            "--value:random-item(fixed 0.5,red,,blue);color:var(--value,green)"
        ),
        computed("", "color:unset"),
        "an empty custom value substitutes nothing rather than its var fallback"
    );
    assert_eq!(
        computed("", "--value:random-item(foo,red);color:var(--value,green)"),
        computed("", "color:green"),
        "an invalid random key makes the custom value guaranteed-invalid"
    );
}

#[test]
fn random_item_scopes_stay_stable_through_style_and_program_caches() {
    let items = (1..=128)
        .map(|n| format!("{n}px"))
        .collect::<Vec<_>>()
        .join(",");
    let css = format!(
        "p {{position:relative;--choice:random-item(element-scoped,{items});width:var(--choice);height:random-item(element-scoped,{items});left:random-item(property-scoped,{items});right:random-item(ua-left,{items});padding:random-item(auto,{items}) random-item(auto,{items});margin-left:random-item(element-scoped ua-padding-1,{items});margin-right:random-item(element-scoped ua-padding-2,{items});padding-left:199px}} p::before {{content:'x';width:random-item(element-scoped,{items});height:random-item(element-scoped,{items})}}"
    );
    let children = (0..64)
        .map(|i| format!("<p id='p{i}'></p>"))
        .collect::<String>();
    let document =
        parse_dom_document(&format!("<!doctype html><section>{children}</section>")).unwrap();
    let mut cache = StyleProgramCache::default();
    let mut previous = None;
    let mut global = None;
    for pass in 0..3 {
        let inputs = [AuthorStylesheetInput {
            css: &css,
            implicit_scope_root: document.dom_root(),
        }];
        let environment =
            MediaEnvironment::screen(if pass == 2 { 500.0 } else { 400.0 }, Some(800.0)).unwrap();
        let (program, timings) =
            cache.get_or_compile(&inputs, environment, document.root_font_size());
        let (styles, _, _) = resolve_document_with_program(
            &document,
            &program,
            &inputs,
            timings,
            &ReaderStyleOverrides::default(),
        );
        let mut values = Vec::new();
        for i in 0..64 {
            let id = format!("p{i}");
            let node = document
                .node_ids()
                .find(|node| document.get_dom_id(*node) == Some(id.as_str()))
                .unwrap();
            let style = styles.view(styles.style_for_node(node).unwrap()).unwrap();
            assert_eq!(
                style.box_model.width, style.box_model.height,
                "element scope {id}"
            );
            assert_eq!(
                style.layout.inset_left, style.layout.inset_right,
                "property scope {id}"
            );
            if let Some(global) = global {
                assert_eq!(style.layout.inset_left, global, "shared across elements");
            }
            global = Some(style.layout.inset_left);
            assert_eq!(
                style.box_model.padding_top, style.box_model.margin_left,
                "first index {id}"
            );
            assert_eq!(
                style.box_model.padding_bottom, style.box_model.margin_left,
                "shorthand {id}"
            );
            assert_eq!(
                style.box_model.padding_right, style.box_model.margin_right,
                "second index {id}"
            );
            assert_eq!(style.box_model.padding_left, LengthPct::Px(199.0));
            let PreferredSize::Px(width) = style.box_model.width else {
                panic!("selected pixel size")
            };
            assert!((1.0..=128.0).contains(&width));
            values.push(width as u32);
            let pseudo = styles
                .view(styles.before_style_for_node(node).unwrap().0)
                .unwrap();
            assert_eq!(pseudo.box_model.width, pseudo.box_model.height);
        }
        assert!(
            values.iter().any(|value| *value != values[0]),
            "element-scoped custom values must not reuse another element's cached map"
        );
        if let Some(previous) = &previous {
            assert_eq!(&values, previous, "recomputed document keeps choices");
        }
        previous = Some(values);
    }
    assert_eq!(cache.stats().hits, 1);
    assert_eq!(cache.stats().misses, 2);
}

#[test]
fn random_item_custom_values_resolve_before_inheritance() {
    let items = (1..=128)
        .map(|n| format!("{n}px"))
        .collect::<Vec<_>>()
        .join(",");
    let styled = style_document(
        parse_dom_document(
            "<!doctype html><section id='parent'><p id='a'></p><p id='b'></p></section>",
        )
        .unwrap(),
        &[&format!(
            "section {{--value:random-item(auto,{items});width:var(--value)}} p {{position:relative;left:var(--value);top:inherit(--value)}}"
        )],
    );
    let PreferredSize::Px(value) = style_by_id(&styled, "parent").box_model.width else {
        panic!("chosen pixel size")
    };
    for id in ["a", "b"] {
        let child = style_by_id(&styled, id);
        assert_eq!(child.layout.inset_left, Some(LengthPct::Px(value)));
        assert_eq!(child.layout.inset_top, Some(LengthPct::Px(value)));
    }
}

#[test]
fn inherit_function_reads_parent_values_and_rebinds_cached_programs() {
    let css = "#target {--sides:99px;--x:99px;--tone:blue;--tabs:9;--space:7px;--name:--x;inset:inherit(--sides);left:inherit(--x);top:calc(inherit(var(--name)) + 1px);color:inherit(--tone);background-color:inherit(--missing,var(--tone));letter-spacing:inherit(--space);tab-size:inherit(--tabs);font-family:inherit(--family,{Times,serif})} #target::before {content:'x';--z:42;z-index:inherit(--z)}";
    let mut cache = StyleProgramCache::default();
    for (pass, length) in [12, 23].into_iter().enumerate() {
        let document = parse_dom_document(&format!("<!doctype html><section style='--x:{length}px;--tone:green;--tabs:3;--space:2px;--sides:1px 2px 3px 4px'><p id='target' style='--z:13'>x</p></section>")).unwrap();
        let inputs = [AuthorStylesheetInput {
            css,
            implicit_scope_root: document.dom_root(),
        }];
        let (styled, _, _) = style_document_with_cached_program_and_timings(
            &mut cache,
            document,
            &inputs,
            MediaEnvironment::default(),
        );
        let style = style_by_id(&styled, "target");
        assert_eq!(style.layout.inset_left, Some(LengthPct::Px(length as f32)));
        assert_eq!(style.layout.inset_right, Some(LengthPct::Px(2.0)));
        assert_eq!(style.layout.inset_bottom, Some(LengthPct::Px(3.0)));
        assert_eq!(
            style.layout.inset_top,
            Some(LengthPct::Px(length as f32 + 1.0))
        );
        assert_eq!(style.text.color, 0x008000ff);
        assert_eq!(style.background.background_color, 0x0000ffff);
        let control = style_document(
            parse_dom_document("<p id='target'></p>").unwrap(),
            &["#target {letter-spacing:2px;tab-size:3;font-family:Times,serif}"],
        );
        let reference = style_by_id(&control, "target");
        assert_eq!(style.text.letter_spacing, reference.text.letter_spacing);
        assert_eq!(style.text.tab_size, reference.text.tab_size);
        assert_eq!(
            styled.styles().string(style.font.font_family.unwrap()),
            control.styles().string(reference.font.font_family.unwrap())
        );
        let node = styled
            .document()
            .node_ids()
            .find(|node| styled.document().get_dom_id(*node) == Some("target"))
            .unwrap();
        let pseudo = styled
            .style_view(styled.styles().before_style_for_node(node).unwrap().0)
            .unwrap();
        assert_eq!(pseudo.layout.z_index, Some(13));
        assert_eq!(cache.stats().hits, pass as u64);
    }
    assert_eq!(cache.stats().misses, 1);
}

#[test]
fn inherit_function_accumulates_custom_values_and_handles_lazy_cycles() {
    let document = parse_dom_document(
        "<!doctype html><section id='e1'><section id='e2'><p id='e3'></p></section></section>",
    )
    .unwrap();
    let css = "#e1 {--v:1} #e2,#e3 {--v:calc(1 + inherit(--v))} section,p {z-index:var(--v)}";
    let styled = style_document(document, &[css]);
    for (id, expected) in [("e1", 1), ("e2", 2), ("e3", 3)] {
        assert_eq!(style_by_id(&styled, id).layout.z_index, Some(expected));
    }
    for inline in [
        "--a:inherit(--tone,var(--a));color:var(--a)",
        "--name:--tone;--a:inherit(var(--name),var(--a));color:var(--a)",
        "--a:inherit(--missing,var(--b));--b:var(--a,red);color:var(--a,green)",
        "--a:inherit(--missing,var(--b));--b:var(--a,red);--c:var(--a,green);color:var(--c)",
        "--a:inherit(--missing,var(--a,red));color:var(--a,green)",
    ] {
        let document = parse_dom_document(&format!("<!doctype html><section style='--tone:green'><p id='target' style='{inline}'></p></section>")).unwrap();
        let styled = style_document(document, &[]);
        assert_eq!(
            style_by_id(&styled, "target").text.color,
            0x008000ff,
            "{inline}"
        );
    }
    let root = style_document(
        parse_dom_document("<!doctype html><html id='target'></html>").unwrap(),
        &["html {--z:3;z-index:inherit(--z,4)}"],
    );
    assert_eq!(style_by_id(&root, "target").layout.z_index, Some(4));
}

#[test]
fn inherit_function_markers_apply_initial_and_invalid_computed_values() {
    for declaration in [
        "tab-size:inherit(--missing,initial)",
        "tab-size:var(--unknown,inherit(--missing,initial))",
    ] {
        let styled = style_document(parse_dom_document(&format!("<!doctype html><section style='tab-size:5'><p id='target' style='{declaration}'></p></section>")).unwrap(),&[]);
        assert_eq!(
            style_by_id(&styled, "target").text.tab_size,
            TabSize::spaces(8.0).unwrap(),
            "{declaration}"
        );
    }
    for declaration in [
        "tab-size:2;tab-size:inherit(--missing)",
        "--value:inherit(--missing);tab-size:2;tab-size:var(--value)",
        "--value:inherit(--missing,initial);tab-size:var(--value)",
    ] {
        let styled = style_document(parse_dom_document(&format!("<!doctype html><section style='tab-size:5'><p id='target' style='{declaration}'></p></section>")).unwrap(),&[]);
        assert_eq!(
            style_by_id(&styled, "target").text.tab_size,
            TabSize::spaces(5.0).unwrap(),
            "{declaration}"
        );
    }
}

#[test]
fn inherit_function_validates_arguments_and_preserves_substitution_boundaries() {
    for (property, fallback) in [
        ("color", "red"),
        ("font-family", "Arial"),
        ("tab-size", "2"),
    ] {
        let declaration =
            format!("color:green;font-family:serif;tab-size:5;{property}:inherit(!!,{fallback})");
        let baseline = computed("", "color:green;font-family:serif;tab-size:5");
        assert_eq!(computed("", &declaration), baseline, "inline {declaration}");
        assert_eq!(
            computed(&format!("#target {{{declaration}}}"), ""),
            baseline,
            "sheet {declaration}"
        );
    }
    for value in [
        "inherit(--x)",
        "calc(inherit(--x) + 1px)",
        "inherit(--x,)",
        "inherit(--x, )",
        "inherit(--x , )",
        "inherit(--x, foo)",
        "INHERIT(/**/--x,1px)",
        "i\\6e herit(--x)",
        "inherit(var(--name),2px)",
        "inherit({--x},{1px})",
    ] {
        assert_eq!(
            property_value_syntax("left", value),
            PropertyValueSyntax::Valid,
            "{value}"
        );
    }
    for value in [
        "inherit()",
        "inherit(!!, foo)",
        "inherit(, foo)",
        "inherit(--x --y)",
        "inherit(--x,1px,2px)",
        "inherit(--x,{} 2px)",
        "inherit(--x,!)",
        "inherit(--x,{!})",
        "inherit(--x,foo {})",
    ] {
        assert_eq!(
            property_value_syntax("left", value),
            PropertyValueSyntax::Invalid,
            "{value}"
        );
        assert_eq!(
            computed("", &format!("left:7px;left:{value}")),
            computed("", "left:7px"),
            "{value}"
        );
    }
    for value in [
        "inherit(--missing)",
        "inherit(--missing,)",
        "inherit(--missing,foo)",
    ] {
        for declaration in [
            format!("left:7px;left:{value}"),
            format!("--value:{value};left:7px;left:var(--value)"),
        ] {
            assert_eq!(
                computed("", &declaration),
                computed("", "left:unset"),
                "{declaration}"
            );
            assert_eq!(
                computed(&format!("#target {{{declaration}}}"), ""),
                computed("", "left:unset"),
                "sheet {declaration}"
            );
        }
    }
    for (declaration, expected) in [
        ("color:inherit(--missing,orange)red", 0x000000ff),
        ("color:var(--missing,inherit(--tone,blue))", 0x008000ff),
        ("color:inherit(--missing,var(--own))", 0x0000ffff),
        ("color:inherit(inherit(--name))", 0x008000ff),
        ("color:inherit(var(--unknown),green)", 0x008000ff),
        ("color:inherit(--wrong,green)", 0x000000ff),
    ] {
        let styled = style_document(parse_dom_document(&format!("<!doctype html><section style='--tone:green;--name:--tone;--wrong:3px'><p id='target' style='--own:blue;{declaration}'></p></section>")).unwrap(),&[]);
        assert_eq!(
            style_by_id(&styled, "target").text.color,
            expected,
            "{declaration}"
        );
    }
}

#[test]
fn tree_counting_values_use_dom_siblings_without_reusing_another_elements_style() {
    let source = "<!doctype html><section id='parent'><i class='item' id='a'>text</i>text<!--comment--><b hidden></b><i class='item' id='b'></i><i class='item' id='c'></i><b></b><i class='item' id='d'></i><i class='item' id='e'></i><i class='item' id='f'></i></section>";
    let css = "#parent {--index:sibling-index();--step:calc(1px * sibling-index())} .item {position:relative;left:calc(10px * var(--index));top:var(--step);z-index:sibling-index();font-size:calc(10px + sibling-count() * 1px);margin-top:calc(1vh * sibling-index());margin-left:calc(5% * sibling-count());color:color(srgb calc(0.1 * sibling-index()) 0.5 0.5);background-color:oklch(0.5 0.2 calc(30deg * sibling-index()));letter-spacing:calc(1% * sibling-count());word-spacing:calc(2% * sibling-index());tab-size:sibling-index()} .item::before {content:'x';z-index:calc(2 * sibling-index());font-size:calc(sibling-count() * 2px)}";
    let (styled, queries) = style_document_with_environment(
        parse_dom_document(source).unwrap(),
        &[css],
        MediaEnvironment::screen(600.0, Some(800.0)).unwrap(),
    );
    assert!(queries.uses_viewport_units());
    for (id, index) in [("a", 1), ("b", 3), ("c", 4), ("d", 6), ("e", 7), ("f", 8)] {
        let style = style_by_id(&styled, id);
        assert_eq!(
            style.layout.inset_left,
            Some(LengthPct::Px(index as f32 * 10.0)),
            "left {id}"
        );
        assert_eq!(
            style.layout.inset_top,
            Some(LengthPct::Px(index as f32)),
            "top {id}"
        );
        assert_eq!(style.layout.z_index, Some(index), "z-index {id}");
        assert_eq!(style.font.font_size, 18.0, "count {id}");
        assert_eq!(
            style.box_model.margin_top,
            LengthPct::Px(index as f32 * 8.0),
            "vh {id}"
        );
        assert_eq!(
            style.box_model.margin_left,
            LengthPct::Pct(0.4),
            "percent {id}"
        );
        assert_eq!(
            style.text.letter_spacing.font_size_fraction(),
            0.08,
            "letter {id}"
        );
        assert_eq!(
            style.text.word_spacing.font_size_fraction(),
            index as f32 * 0.02,
            "word {id}"
        );
        let control = style_document(
            parse_dom_document("<i id='target'></i>").unwrap(),
            &[&format!(
                "#target {{color:color(srgb {} 0.5 0.5);background-color:oklch(0.5 0.2 {}deg);tab-size:{index}}}",
                index as f32 * 0.1,
                index * 30
            )],
        );
        let expected = style_by_id(&control, "target");
        assert_eq!(style.text.color, expected.text.color, "color {id}");
        assert_eq!(
            style.background.background_color, expected.background.background_color,
            "hue {id}"
        );
        assert_eq!(style.text.tab_size, expected.text.tab_size, "tab {id}");
        let node = styled
            .document()
            .node_ids()
            .find(|node| styled.document().get_dom_id(*node) == Some(id))
            .unwrap();
        let pseudo = styled
            .style_view(styled.styles().before_style_for_node(node).unwrap().0)
            .unwrap();
        assert_eq!(pseudo.layout.z_index, Some(index * 2), "pseudo index {id}");
        assert_eq!(pseudo.font.font_size, 16.0, "pseudo count {id}");
    }
    let root = style_document(
        parse_dom_document("<html></html>").unwrap(),
        &["html {z-index:calc(sibling-index()*2);order:calc(sibling-count()*2)}"],
    );
    let html = root
        .document()
        .node_ids()
        .find(|node| root.document().get_dom_tag(*node) == Some("html"))
        .unwrap();
    let style = root.style_view(root.style_for_node(html).unwrap()).unwrap();
    assert_eq!(style.layout.z_index, Some(2));
    assert_eq!(style.layout.order, 2);
}

#[test]
fn tree_counting_rebinds_cached_programs_and_preserves_shorthand_winners() {
    let css = "section {--index:sibling-index()} p {position:relative;inset:calc(1px * var(--index));left:17px;height:calc(1px * sibling-count());font-size:calc(10px * sibling-count())} p span {z-index:var(--index)}";
    let mut cache = StyleProgramCache::default();
    for (pass, count) in [6, 2, 7].into_iter().enumerate() {
        let children = (1..=count)
            .map(|index| format!("<p id='p{index}'><span id='s{index}'></span></p>"))
            .collect::<String>();
        let document =
            parse_dom_document(&format!("<!doctype html><section>{children}</section>")).unwrap();
        let inputs = [AuthorStylesheetInput {
            css,
            implicit_scope_root: document.dom_root(),
        }];
        let (styled, _, _) = style_document_with_cached_program_and_timings(
            &mut cache,
            document,
            &inputs,
            MediaEnvironment::default(),
        );
        for index in 1..=count {
            let parent = style_by_id(&styled, &format!("p{index}"));
            assert_eq!(parent.layout.inset_left, Some(LengthPct::Px(17.0)));
            for offset in [
                parent.layout.inset_top,
                parent.layout.inset_right,
                parent.layout.inset_bottom,
            ] {
                assert_eq!(offset, Some(LengthPct::Px(index as f32)));
            }
            assert_eq!(parent.box_model.height, PreferredSize::Px(count as f32));
            assert_eq!(parent.font.font_size, count as f32 * 10.0);
            let child = style_by_id(&styled, &format!("s{index}"));
            // Computed font sizes inherit; custom-property tokens evaluate at
            // the consuming element, whose own sibling index is always one.
            assert_eq!(child.font.font_size, count as f32 * 10.0);
            assert_eq!(child.layout.z_index, Some(1));
        }
        assert_eq!(cache.stats().hits, pass as u64);
    }
    assert_eq!(cache.stats().misses, 1);
}

#[test]
fn tree_counting_grammar_and_substitution_keep_types_and_invalid_fallbacks() {
    for (property, value, expected) in [
        ("left", "calc(1px * sibling-index())", "1px"),
        ("left", "calc(1px * sibling-index( ))", "1px"),
        ("left", "calc(1em * sibling-index())", "1em"),
        ("left", "calc(1rem * sibling-count())", "1rem"),
        ("left", "calc(1vh * sibling-index())", "1vh"),
        ("left", "calc(1ch * sibling-count())", "1ch"),
        ("left", "calc(1% * sibling-index())", "1%"),
        ("left", "calc(5% * sibling-count())", "5%"),
        ("z-index", "sibling-index()", "1"),
        ("z-index", "sibling-count()", "1"),
        ("z-index", "calc(2 * sibling-index())", "2"),
        ("z-index", "calc(sibling-index())", "1"),
        ("z-index", "calc(sibling-count())", "1"),
        (
            "color",
            "color(srgb calc(0.1 * sibling-index()) 0.5 0.5)",
            "color(srgb 0.1 0.5 0.5)",
        ),
        (
            "color",
            "color(srgb 0 calc(0.1 * sibling-count()) 0.5)",
            "color(srgb 0 0.1 0.5)",
        ),
        (
            "color",
            "color(srgb 0 0 calc(0.1 * sibling-index()))",
            "color(srgb 0 0 0.1)",
        ),
        (
            "color",
            "color(srgb 0.5 0.5 0.5 / calc(0.1 * sibling-count()))",
            "color(srgb 0.5 0.5 0.5 / 0.1)",
        ),
        (
            "color",
            "oklch(calc(0.1 * sibling-index()) 0.2 180)",
            "oklch(0.1 0.2 180)",
        ),
        (
            "color",
            "oklch(0.5 calc(0.05 * sibling-count()) 180)",
            "oklch(0.5 0.05 180)",
        ),
        (
            "color",
            "oklch(0.5 0.2 calc(30deg * sibling-index()))",
            "oklch(0.5 0.2 30deg)",
        ),
        (
            "color",
            "oklch(0.5 0.2 180 / calc(0.1 * sibling-index()))",
            "oklch(0.5 0.2 180 / 0.1)",
        ),
        ("color", "alpha(from green / sibling-index())", "green"),
        ("color", "alpha(from green / sibling-count())", "green"),
        (
            "color",
            "alpha(from green / calc(sibling-index() * 0.2))",
            "alpha(from green / 0.2)",
        ),
        ("left", "calc(1px * SIBLING-INDEX(/**/))", "1px"),
        ("left", "calc(1px * s\\69 bling-index())", "1px"),
        ("letter-spacing", "calc(2% * sibling-index())", "2%"),
        ("word-spacing", "calc(1px * sibling-count())", "1px"),
        ("tab-size", "sibling-count()", "1"),
        (
            "font-feature-settings",
            "'ss03' sibling-count()",
            "'ss03' 1",
        ),
    ] {
        assert_eq!(
            property_value_syntax(property, value),
            PropertyValueSyntax::Valid,
            "{property}:{value}"
        );
        let wanted = computed("", &format!("{property}:{expected}"));
        for declaration in [
            format!("{property}:{value}"),
            format!("--value:{value};{property}:var(--value)"),
            format!("{property}:var(--missing,{value})"),
        ] {
            assert_eq!(computed("", &declaration), wanted, "inline {declaration}");
            assert_eq!(
                computed(&format!("#target {{{declaration}}}"), ""),
                wanted,
                "sheet {declaration}"
            );
        }
    }
    for (property, value) in [
        ("left", "sibling-index()"),
        ("top", "calc(sibling-index() - 1)"),
        ("inset", "sibling-count()"),
        ("left", "calc(1px * sibling-index(1))"),
        ("z-index", "sibling-count(foo)"),
        ("z-index", "calc(1px * sibling-index())"),
        ("color", "color(srgb calc(1px * sibling-index()) 0 0)"),
        ("color", "oklch(calc(1px * sibling-count()) 0 0)"),
    ] {
        assert_eq!(
            property_value_syntax(property, value),
            PropertyValueSyntax::Invalid,
            "{property}:{value}"
        );
        let baseline = "left:13px;z-index:7;color:red";
        assert_eq!(
            computed("", &format!("{baseline};{property}:{value}")),
            computed("", baseline),
            "fallback {property}:{value}"
        );
        assert_eq!(
            computed(
                "",
                &format!("--value:{value};{baseline};{property}:var(--value)")
            ),
            computed("", &format!("{baseline};{property}:unset")),
            "unset {property}:{value}"
        );
    }
    assert_eq!(
        computed("", "--empty:;left:calc(1px*sibling-index(var(--empty)))"),
        computed("", "left:1px")
    );
}

#[test]
fn flex_keywords_compute_and_keep_independent_longhand_winners() {
    for (keyword, expected) in [
        ("content", PreferredSize::Content),
        ("min-content", PreferredSize::MinContent),
        ("max-content", PreferredSize::MaxContent),
        ("fit-content", PreferredSize::FitContent),
        ("stretch", PreferredSize::Stretch),
    ] {
        assert_eq!(
            property_value_syntax("flex-basis", keyword),
            PropertyValueSyntax::Valid
        );
        for value in [format!("1 0 {keyword}"), format!("{keyword} 1 0")] {
            assert_eq!(
                property_value_syntax("flex", &value),
                PropertyValueSyntax::Valid,
                "{value}"
            );
            let document = parse_dom_document("<p id='target'>Text</p>").unwrap();
            let styled = style_document(document, &[&format!("#target {{flex:{value}}}")]);
            let node = styled
                .document()
                .node_ids()
                .find(|node| styled.document().get_dom_id(*node) == Some("target"))
                .unwrap();
            let style = styled
                .style_view(styled.style_for_node(node).unwrap())
                .unwrap();
            assert_eq!(
                (
                    style.layout.flex_grow,
                    style.layout.flex_shrink,
                    style.layout.flex_basis
                ),
                (1.0, 0.0, expected),
                "{value}"
            );
        }
        for declarations in [
            format!("flex:1 0 {keyword};flex-grow:3"),
            format!("flex-grow:3!important;flex:1 0 {keyword}"),
            format!("--flex:1 0 {keyword};flex:var(--flex);flex-grow:3"),
            format!("--basis:{keyword};flex:1 0 var(--basis);flex-grow:3"),
        ] {
            let expected = format!("flex-grow:3;flex-shrink:0;flex-basis:{keyword}");
            assert_eq!(
                computed(&format!("#target {{{declarations}}}"), ""),
                computed(&format!("#target {{{expected}}}"), ""),
                "{declarations}"
            );
            assert_eq!(
                computed("", &declarations),
                computed("", &expected),
                "inline {declarations}"
            );
        }
    }
    for (source, expected) in [
        ("flex:content", "flex:1 1 content"),
        ("flex:0 fit-content", "flex:0 1 fit-content"),
        ("flex:2 0 m\\61 x-content", "flex:2 0 max-content"),
        ("flex:2/**/0/**/MAX-CONTENT", "flex:2 0 max-content"),
        ("flex:calc(-2) 3 content", "flex:0 3 content"),
        ("flex-grow:calc(-2)", "flex-grow:0"),
        ("flex-shrink:calc(NaN)", "flex-shrink:0"),
        ("flex-basis:calc(-10px)", "flex-basis:0px"),
        ("flex-basis:calc(-10%)", "flex-basis:0%"),
        ("flex:1 1 calc(-10em)", "flex:1 1 0px"),
        ("width:calc(-10px)", "width:0px"),
    ] {
        assert_eq!(
            computed(&format!("#target {{{source}}}"), ""),
            computed(&format!("#target {{{expected}}}"), ""),
            "{source}"
        );
    }
    for source in [
        "flex:-1 0 content",
        "flex:1 content 2",
        "flex:content content",
        "flex:1 0 stretch auto",
        "flex-basis:max-content auto",
    ] {
        assert_eq!(
            computed(&format!("#target {{flex:2 3 10px;{source}}}"), ""),
            computed("#target {flex:2 3 10px}", ""),
            "{source}"
        );
    }
    assert_eq!(
        computed(
            "#target {--bad:1 bogus;flex:2 3 10px;flex:var(--bad);flex-grow:5}",
            ""
        ),
        computed("#target {flex:initial;flex-grow:5}", "")
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
fn system_fonts_compute_fallbacks_and_reset_only_their_winning_longhands() {
    const DIRTY: &str = "font:italic small-caps bold 30px/2 fantasy;\
        font-kerning:none;font-variant-numeric:oldstyle-nums;\
        font-variant-ligatures:none;font-feature-settings:'ss03' 2";
    const RESETS: &str = "font-kerning:auto;font-variant-numeric:normal;\
        font-variant-ligatures:normal;font-feature-settings:normal";
    for keyword in [
        "caption",
        "icon",
        "menu",
        "message-box",
        "small-caption",
        "status-bar",
        "MENU/**/",
        "m\\65 nu",
        "/**/caption/**/",
    ] {
        assert_eq!(
            property_value_syntax("font", keyword),
            PropertyValueSyntax::Valid,
            "{keyword}"
        );
        for declaration in [
            format!("font:{keyword}"),
            format!("--font:{keyword};font:var(--font)"),
            format!("font:var(--missing,{keyword})"),
            format!("font:{keyword} !/**/IMPORTANT"),
        ] {
            let declarations = format!("{DIRTY};{declaration}");
            let expected = computed("", &format!("font:medium system-ui;{RESETS}"));
            assert_eq!(
                computed("", &declarations),
                expected,
                "inline {declarations}"
            );
            assert_eq!(
                computed(&format!("#target {{{declarations}}}"), ""),
                expected,
                "sheet {declarations}"
            );
        }
        let winners = "font-weight:700!important;font-size:23px!important;line-height:35px!important;\
            font-kerning:normal!important;font-feature-settings:'ss03' 2!important";
        let expected = computed("", &format!("font:medium system-ui;{RESETS};{winners}"));
        for declarations in [
            format!("{winners};font:{keyword}"),
            format!("{winners};--font:{keyword};font:var(--font)"),
            format!("font:{keyword};{winners}"),
        ] {
            assert_eq!(computed("", &declarations), expected, "{declarations}");
            assert_eq!(
                computed(&format!("#target {{{declarations}}}"), ""),
                expected,
                "sheet {declarations}"
            );
        }
    }
    for shorthand in ["20px serif", "italic small-caps bold 24px/2 fantasy"] {
        let expected = computed("", &format!("font:{shorthand};{RESETS}"));
        for declaration in [
            format!("font:{shorthand}"),
            format!("--font:{shorthand};font:var(--font)"),
        ] {
            assert_eq!(
                computed("", &format!("{DIRTY};{declaration}")),
                expected,
                "{declaration}"
            );
        }
    }
    for invalid in [
        "menu serif",
        "menu 20px",
        "menu / 2",
        "menu, caption",
        "'menu'",
    ] {
        assert_eq!(
            property_value_syntax("font", invalid),
            PropertyValueSyntax::Invalid,
            "{invalid}"
        );
        assert_eq!(
            computed("", &format!("{DIRTY};font:{invalid}")),
            computed("", DIRTY)
        );
    }
    for keyword in ["initial", "inherit", "unset"] {
        let parent = format!("body {{{DIRTY}}}");
        assert_eq!(
            computed(&parent, &format!("{DIRTY};font:{keyword}")),
            computed(
                &parent,
                &format!(
                    "font:{keyword};font-kerning:{keyword};font-variant-numeric:{keyword};font-variant-ligatures:{keyword};font-feature-settings:{keyword}"
                )
            ),
            "font:{keyword} resets"
        );
    }
    assert_eq!(
        computed(
            &format!("body {{{DIRTY}}}"),
            "--font:menu serif;font:var(--font);font-weight:600"
        ),
        computed(&format!("body {{{DIRTY}}}"), "font:inherit;font-weight:600")
    );
    let document = parse_dom_document("<p id='system'>Text</p><p id='named'>Text</p>").unwrap();
    let styled = style_document(
        document,
        &["#system {font:menu} #named {--font:menu;font:20px var(--font)}"],
    );
    for (id, family, size) in [("system", "system-ui", 16.0), ("named", "menu", 20.0)] {
        let node = styled
            .document()
            .node_ids()
            .find(|node| styled.document().get_dom_id(*node) == Some(id))
            .unwrap();
        let style = styled
            .style_view(styled.style_for_node(node).unwrap())
            .unwrap();
        assert_eq!(
            styled.styles().string(style.font.font_family.unwrap()),
            Some(family)
        );
        assert_eq!(style.font.font_size, size);
    }
}

#[test]
fn contextual_font_weight_math_resolves_before_clamping_and_preserves_cascade() {
    for (value, expected) in [
        ("calc(sign(1rem - 1px) * 10)", 10),
        ("calc(sign(1rem - 1px) * -10000)", 1),
        ("calc(sign(1rem - 1px) * 10000)", 1000),
        ("calc(100 + (sign(20cqw - 10px) * 5))", 105),
        ("calc(300 + sign(1em - 18px) * 50)", 350),
        ("calc(400 + sign(1lh - 25px) * 50)", 450),
        ("calc(NaN + sign(1em - 18px))", 1),
        ("calc(sign(1em - 18px) * infinity)", 1000),
        ("calc(-10000)", 1),
        ("calc(10000)", 1000),
        ("calc(NaN)", 1),
        ("calc(infinity)", 1000),
        ("calc(-infinity)", 1),
        ("min(-2,-1)", 1),
        ("clamp(none,1200,none)", 1000),
    ] {
        assert_eq!(
            property_value_syntax("font-weight", value),
            PropertyValueSyntax::Valid,
            "{value}"
        );
        for declaration in [
            format!("font-weight:{value}"),
            format!("--weight:{value};font-weight:var(--weight)"),
            format!("font-weight:var(--missing,{value})"),
            format!("font-weight:{value} !/**/IMPORTANT"),
        ] {
            let declarations =
                format!("font-size:20px;line-height:30px;font-weight:500;{declaration}");
            let expected = computed(
                "",
                &format!("font-size:20px;line-height:30px;font-weight:{expected}"),
            );
            assert_eq!(
                computed("", &declarations),
                expected,
                "inline {declarations}"
            );
            assert_eq!(
                computed(&format!("#target {{{declarations}}}"), ""),
                expected,
                "sheet {declarations}"
            );
        }
    }
    for value in [
        "0",
        "-1",
        "1001",
        "0.5",
        "calc(1px)",
        "calc(10%)",
        "calc(400 + 1px)",
        "calc(sign(1px + 1) * 400)",
        "calc(sign(1px - 1s) * 400)",
    ] {
        assert_eq!(
            property_value_syntax("font-weight", value),
            PropertyValueSyntax::Invalid,
            "{value}"
        );
        assert_eq!(
            computed("", &format!("font-weight:500;font-weight:{value}")),
            computed("", "font-weight:500"),
            "fallback {value}"
        );
        assert_eq!(
            computed(
                "body {font-weight:600}",
                &format!("--weight:{value};font-weight:500;font-weight:var(--weight)")
            ),
            computed("body {font-weight:600}", "font-weight:inherit"),
            "unset {value}"
        );
    }
    let math = "font-weight:calc(400 + sign(1em - 18px) * 100)";
    for declaration in [
        format!("{math};font-size:20px"),
        format!("font-size:20px;{math}"),
    ] {
        assert_eq!(
            computed("", &declaration),
            computed("", "font-size:20px;font-weight:500")
        );
    }
    assert_eq!(
        computed(
            "",
            &format!("font-weight:700!important;{math};font-size:20px")
        ),
        computed("", "font-weight:700;font-size:20px")
    );
}

fn numeric_style_at(css: &str, width: f64, height: Option<f64>) -> (u16, PreferredSize, bool) {
    let document = parse_dom_document("<p id='target'>Text</p>").unwrap();
    let (styled, queries) = style_document_with_environment(
        document,
        &[css],
        MediaEnvironment::screen(width, height).unwrap(),
    );
    let node = styled
        .document()
        .node_ids()
        .find(|node| styled.document().get_dom_id(*node) == Some("target"))
        .unwrap();
    let style = styled
        .style_view(styled.style_for_node(node).unwrap())
        .unwrap();
    (
        style.font.font_weight,
        style.box_model.width,
        queries.uses_viewport_units(),
    )
}

#[test]
fn container_units_use_viewport_fallback_and_track_viewport_dependencies() {
    let css = "#target {font-weight:calc(400 + sign(2cqw - 10px) * 100)}";
    for (width, expected) in [(400.0, 300), (500.0, 400), (600.0, 500)] {
        assert_eq!(
            numeric_style_at(css, width, None),
            (expected, PreferredSize::Auto, true)
        );
    }
    for (unit, expected) in [
        ("cqw", 60.0),
        ("cqi", 60.0),
        ("cqh", 80.0),
        ("cqb", 80.0),
        ("cqmin", 60.0),
        ("cqmax", 80.0),
        ("svw", 60.0),
        ("lvw", 60.0),
        ("dvw", 60.0),
        ("vi", 60.0),
        ("svi", 60.0),
        ("lvi", 60.0),
        ("dvi", 60.0),
        ("svh", 80.0),
        ("lvh", 80.0),
        ("dvh", 80.0),
        ("vb", 80.0),
        ("svb", 80.0),
        ("lvb", 80.0),
        ("dvb", 80.0),
        ("svmin", 60.0),
        ("lvmin", 60.0),
        ("dvmin", 60.0),
        ("svmax", 80.0),
        ("lvmax", 80.0),
        ("dvmax", 80.0),
    ] {
        let css = format!("#target {{width:10{unit}}}");
        assert_eq!(
            numeric_style_at(&css, 600.0, Some(800.0)),
            (400, PreferredSize::Px(expected), true),
            "{unit}"
        );
    }
    assert_eq!(
        numeric_style_at("#target {width:17px;width:10cqh}", 600.0, None),
        (400, PreferredSize::Px(17.0), true)
    );
    assert_eq!(
        numeric_style_at("#target {width:17px}", 600.0, None),
        (400, PreferredSize::Px(17.0), false)
    );
    for (width, expected) in [(400.0, 300), (600.0, 500)] {
        assert_eq!(
            feature_settings_at(
                "#target {font-feature-settings:'ss03' calc(400 + sign(2cqw - 10px) * 100)}",
                "",
                width
            ),
            (vec![(*b"ss03", expected)], true)
        );
    }
}

#[test]
fn sign_products_compute_typed_values_in_both_source_paths_and_substitutions() {
    assert_eq!(
        property_value_syntax("line-height", "sign(20%)"),
        PropertyValueSyntax::Valid
    );
    for (property, value, expected) in [
        ("font-size", "calc(sign(1rem - 1px) * 100%)", "100%"),
        ("font-size", "calc(sign(1rem - 30px) * 100%)", "0px"),
        ("font-size", "calc(40px + sign(1rem - 30px) * 10px)", "30px"),
        (
            "font-size",
            "calc(sign(sign(1rem - 1px) * 10px) * 100%)",
            "100%",
        ),
        ("font-size", "calc(SIGN(1rem - 1px)/**/ * 10px)", "10px"),
        (
            "font-style",
            "oblique calc(sign(1rem - 1px) * 10deg)",
            "oblique 10deg",
        ),
        (
            "font-style",
            "oblique calc(sign(10deg) * 10deg)",
            "oblique 10deg",
        ),
        (
            "font-style",
            "oblique calc(30deg + (sign(20cqw - 10px) * 5deg))",
            "oblique 35deg",
        ),
        (
            "font-style",
            "oblique calc(30deg + (sign(2cqw - 10px) * 5deg))",
            "oblique 35deg",
        ),
        ("line-height", "calc(10% + (sign(2cqw - 10px) * 5%))", "15%"),
        (
            "line-height",
            "calc(10px + (sign(2cqw - 10px) * 5px))",
            "15px",
        ),
        ("line-height", "calc(sign(1rem - 30px) * 10px)", "0px"),
        ("line-height", "calc(sign(1rem - 30px) * 2)", "0"),
        (
            "aspect-ratio",
            "sign(1rem - 1px) / sign(1rem - 1px)",
            "1 / 1",
        ),
        (
            "aspect-ratio",
            "sign(1rem - 30px) / sign(1rem - 1px)",
            "0 / 1",
        ),
        (
            "aspect-ratio",
            "calc(2 + sign(1rem - 30px)) / calc(2 + sign(1rem - 1px))",
            "1 / 3",
        ),
    ] {
        assert_eq!(
            property_value_syntax(property, value),
            PropertyValueSyntax::Valid,
            "{property}:{value}"
        );
        let parent = "body {font-size:20px;line-height:40px}";
        let expected = computed(parent, &format!("{property}:{expected}"));
        for declaration in [
            format!("{property}:{value}"),
            format!("--value:{value};{property}:var(--value)"),
            format!("{property}:var(--missing,{value})"),
            format!("{property}:{value} !/**/IMPORTANT"),
        ] {
            assert_eq!(
                computed(parent, &declaration),
                expected,
                "inline {declaration}"
            );
            assert_eq!(
                computed(&format!("{parent} #target {{{declaration}}}"), ""),
                expected,
                "sheet {declaration}"
            );
        }
    }
    let parent = "body {font-size:20px;line-height:40px}";
    for (property, value) in [("font-size", "100%"), ("line-height", "20px")] {
        assert_eq!(
            computed(
                parent,
                &format!("{property}:calc(sign(1lh - 30px) * {value})")
            ),
            computed(parent, &format!("{property}:{value}")),
            "parent lh basis for {property}"
        );
    }
    assert_eq!(
        computed(
            "body {font-size:20px;line-height:calc(2 + sign(1em - 15px))}",
            "font-size:10px"
        ),
        computed("body {font-size:20px;line-height:3}", "font-size:10px")
    );
    for (property, invalid, baseline) in [
        ("font-size", "calc(sign(1rem + 1) * 10px)", "20px"),
        ("font-size", "calc(sign(1px - 1s) * 10px)", "20px"),
        ("font-size", "calc(sign(1rem,1px) * 10px)", "20px"),
        ("font-size", "calc(sign(1rem - 1px) + 10px)", "20px"),
        ("font-size", "sign(1rem - 1px)", "20px"),
        (
            "font-style",
            "oblique calc(sign(1rem - 1px) * 10px)",
            "italic",
        ),
        ("line-height", "calc(10px + sign(1rem - 1px) * 5)", "30px"),
        ("aspect-ratio", "calc(sign(1rem - 1px) * 10px) / 1", "2 / 1"),
    ] {
        assert_eq!(
            property_value_syntax(property, invalid),
            PropertyValueSyntax::Invalid,
            "{property}:{invalid}"
        );
        let previous = format!("{property}:{baseline}");
        let declaration = format!("{previous};{property}:{invalid}");
        assert_eq!(
            computed("", &declaration),
            computed("", &previous),
            "{declaration}"
        );
        assert_eq!(
            computed(&format!("#target {{{declaration}}}"), ""),
            computed("", &previous)
        );
        assert_eq!(
            computed(
                "",
                &format!("--value:{invalid};{previous};{property}:var(--value)")
            ),
            computed("", &format!("{property}:unset")),
            "unset {property}"
        );
    }
    assert_eq!(
        computed("", "font:italic 20px/30px serif;font:4em/-2em monospace"),
        computed("", "font:italic 20px/30px serif")
    );
    assert_eq!(
        computed("", "font:oblique -10deg 20px/30px serif"),
        computed("", "font:oblique 10deg 20px/30px serif")
    );
}

#[test]
fn sign_products_recompute_with_their_viewport_and_preserve_longhand_winners() {
    for (width, amount) in [(400.0, -1.0), (500.0, 0.0), (600.0, 1.0)] {
        for (property, value, expected) in [
            (
                "font-size",
                "calc(20px + sign(2cqw - 10px) * 5px)",
                format!("{}px", 20.0 + amount * 5.0),
            ),
            (
                "line-height",
                "calc(10px + sign(2cqw - 10px) * 5px)",
                format!("{}px", 10.0 + amount * 5.0),
            ),
            (
                "aspect-ratio",
                "calc(2 + sign(2cqw - 10px)) / 1",
                format!("{} / 1", 2.0 + amount),
            ),
        ] {
            let source = format!("#target {{{property}:{value}}}");
            let control = format!("#target {{{property}:{expected}}}");
            let read = |css: &str| {
                let document = parse_dom_document("<p id='target'>Text</p>").unwrap();
                let (styled, queries) = style_document_with_environment(
                    document,
                    &[css],
                    MediaEnvironment::screen(width, None).unwrap(),
                );
                let node = styled
                    .document()
                    .node_ids()
                    .find(|node| styled.document().get_dom_id(*node) == Some("target"))
                    .unwrap();
                let style = styled
                    .style_view(styled.style_for_node(node).unwrap())
                    .unwrap();
                (
                    (
                        style.font.font_size,
                        style.text.line_height,
                        style.box_model.aspect_ratio,
                    ),
                    queries.uses_viewport_units(),
                )
            };
            let actual = read(&source);
            assert_eq!(actual.0, read(&control).0, "{property} at {width}");
            assert!(actual.1, "viewport dependency for {property}");
        }
    }
    let shorthand = "font:italic 20px/30px serif";
    let line_height = "line-height:calc(10px + sign(2cqw - 10px) * 5px)";
    assert_eq!(
        computed("", &format!("{shorthand};{line_height}")),
        computed("", "font:italic 20px/15px serif")
    );
    assert_eq!(
        computed("", &format!("{line_height}!important;{shorthand}")),
        computed("", "font:italic 20px/15px serif")
    );
}

#[test]
fn hairline_widths_compute_and_preserve_declaration_grammar_and_substitution() {
    for property in [
        "border",
        "border-width",
        "border-top",
        "border-right",
        "border-bottom",
        "border-left",
        "border-top-width",
        "border-right-width",
        "border-bottom-width",
        "border-left-width",
        "border-block",
        "border-inline",
        "border-block-start",
        "border-block-end",
        "border-inline-start",
        "border-inline-end",
        "border-block-width",
        "border-inline-width",
        "border-block-start-width",
        "border-block-end-width",
        "border-inline-start-width",
        "border-inline-end-width",
        "outline",
        "outline-width",
    ] {
        assert_eq!(
            property_value_syntax(property, "hairline"),
            PropertyValueSyntax::Valid,
            "{property}"
        );
        for value in ["hairline", "HAIRLINE", "h\\61 irline", "var(--width)"] {
            let declaration = format!("--width:hairline;{property}:{value}");
            let expected = computed(&format!("#target {{{BASE};{property}:1px}}"), "");
            assert_eq!(
                computed(&format!("#target {{{BASE};{declaration}}}"), ""),
                expected,
                "{declaration}"
            );
            assert_eq!(
                computed("", &format!("{BASE};{declaration}")),
                computed("", &format!("{BASE};{property}:1px")),
                "inline {declaration}"
            );
        }
    }
    for (source, expected) in [
        (
            "border:rgb(1,2,3) hairline solid",
            "border:rgb(1,2,3) 1px solid",
        ),
        ("border-width:5px hairline", "border-width:5px 1px"),
        (
            "border-width:var(--width) hairline;--width:3px",
            "border-width:3px 1px",
        ),
        (
            "border\\2d left\\2d width:hairline",
            "border-left-width:1px",
        ),
        ("\\62 order-left-width:hairline", "border-left-width:1px"),
        (
            "--é:hairline;border-left-width:var(--é)",
            "border-left-width:1px",
        ),
        (
            "border-left-width:var(--missing,hairline)",
            "border-left-width:1px",
        ),
    ] {
        assert_eq!(
            computed(&format!("#target {{{BASE};{source}}}"), ""),
            computed(&format!("#target {{{BASE};{expected}}}"), ""),
            "{source}"
        );
        assert_eq!(
            computed("", &format!("{BASE};{source}")),
            computed("", &format!("{BASE};{expected}")),
            "inline {source}"
        );
    }
    for invalid in [
        "border:hairline hairline solid",
        "border-width:calc(hairline)",
        "border-left-width:hairline 3px",
        "border-color:hairline",
    ] {
        assert_eq!(
            computed(&format!("#target {{{BASE};{invalid}}}"), ""),
            computed(&format!("#target {{{BASE}}}"), ""),
            "{invalid}"
        );
    }
    assert_eq!(
        computed(
            "#target {--bad:hairline hairline;border-left-width:7px;border-left-width:var(--bad)}",
            ""
        ),
        computed("#target {border-left-width:initial}", "")
    );
}

#[test]
fn invalid_substituted_values_unset_instead_of_exposing_losing_declarations() {
    for (declarations, expected) in [
        ("--bad:60;left:13px;left:var(--bad)", "left:initial"),
        (
            "--bad:list-item list-item;display:block;display:var(--bad)",
            "display:initial",
        ),
        (
            "--a:list-item;--b:list-item;display:block;display:var(--a) var(--b)",
            "display:initial",
        ),
        (
            "--bad:20;letter-spacing:9px;letter-spacing:var(--bad)",
            "letter-spacing:initial",
        ),
        (
            "--bad:min(1px,0);margin-left:9px;margin-left:var(--bad)",
            "margin-left:initial",
        ),
    ] {
        assert_eq!(
            computed(&format!("#target {{{declarations}}}"), ""),
            computed(&format!("#target {{{expected}}}"), ""),
            "{declarations}"
        );
        assert_eq!(
            computed("", declarations),
            computed("", expected),
            "inline {declarations}"
        );
    }
}

#[test]
fn invalid_declarations_preserve_prior_values_in_stylesheets_and_inline_styles() {
    for invalid in [
        "background:red,green",
        "background:transparent,green",
        "display:list-item list-item",
        "display:list-item block list-item",
        "display:list-item/**/list-item",
        "top:60",
        "right:60",
        "bottom:60",
        "left:60",
        "left:60/**/",
        "text-decoration-line:none underline",
        "text-decoration-line:underline underline",
        "text-decoration-line:underline/**/underline",
        "text-decoration-thickness:60002020",
        "text-decoration-thickness:10e2",
        "outline-style:hidden",
        "all:unset inherit",
    ] {
        let css = format!("#target {{{BASE}}}");
        assert_eq!(
            computed(&format!("{css} #target {{{invalid}}}"), ""),
            computed(&css, ""),
            "stylesheet {invalid}"
        );
        assert_eq!(
            computed("", &format!("{BASE};{invalid}")),
            computed("", BASE),
            "inline {invalid}"
        );
    }
}

#[test]
fn clamp_whitespace_comments_and_priorities_preserve_computed_values() {
    let expected = computed("#target {margin-top:30px}", "");
    for value in [
        "clamp(10px,35px ,30px)",
        "calc(clamp(10px,35px ,30px))",
        "clamp(10px,35px /*foo*/,30px)",
        "calc(clamp(10px,35px /*foo*/,30px))",
        "clamp(10px /*foo*/,35px,30px)",
        "calc(clamp(10px /*foo*/,35px,30px))",
        "clamp(10px ,35px,30px)",
        "calc(clamp(10px ,35px,30px))",
    ] {
        assert_eq!(
            computed(
                &format!("#target {{margin-top:7px;margin-top:{value}}}"),
                ""
            ),
            expected,
            "stylesheet {value}"
        );
        assert_eq!(
            computed("", &format!("margin-top:7px;margin-top:{value}")),
            expected,
            "inline {value}"
        );
        assert_eq!(
            property_value_syntax("margin-top", value),
            PropertyValueSyntax::Valid
        );
    }
    assert_eq!(
        computed(
            "#target {margin-top:7px!important;margin-top:clamp(10px,35px ,30px)!important}",
            ""
        ),
        expected
    );
    assert_eq!(
        computed("#target {left:13px!important;left:60!important}", ""),
        computed("#target {left:13px!important}", "")
    );
    assert_eq!(
        computed("#target {left:13px !/**/IMPORTANT}", ""),
        computed("#target {left:13px}", "")
    );
}

#[test]
fn syntax_validation_recognizes_css_wide_keywords_and_real_substitutions() {
    for name in ["font-size", "text-indent", "text-decoration-color", "width"] {
        for keyword in [
            "inherit",
            "INITIAL",
            "unset",
            "revert",
            "revert-layer",
            "inherit/**/",
        ] {
            assert_eq!(
                property_value_syntax(name, keyword),
                PropertyValueSyntax::Valid,
                "{name}:{keyword}"
            );
        }
    }
    for value in [
        "var(--x)",
        "var(--x,)",
        "calc(1px + var(--x))",
        "env(safe-area-inset-top, 1px)",
    ] {
        assert_eq!(
            property_value_syntax("width", value),
            PropertyValueSyntax::Valid,
            "{value}"
        );
    }
    for value in [
        "var(--x ())",
        "var(--x () )",
        "var(--x() )",
        "var(--x (),)",
        "var(--x(),)",
        "env(safe-area-inset-top ())",
        "env(safe-area-inset-top () )",
        "env(safe-area-inset-top() )",
        "env(safe-area-inset-top (),)",
        "env(safe-area-inset-top(),)",
        "'var(--x)'",
        "10px /* var(--x) */ invalid",
    ] {
        assert_eq!(
            property_value_syntax("width", value),
            PropertyValueSyntax::Invalid,
            "{value}"
        );
    }
    for value in ["unset inherit", "inherit initial", "initial unset"] {
        assert_eq!(
            property_value_syntax("all", value),
            PropertyValueSyntax::Invalid,
            "{value}"
        );
    }
}

#[test]
fn nonfinite_math_and_outline_offset_validate_dimensions() {
    for value in [
        "calc(10.135262721212548pc - 199pt / NaN)",
        "min(-15px, NaN * 1px)",
        "min(NaN * 1px, -15px)",
    ] {
        assert_eq!(
            property_value_syntax("width", value),
            PropertyValueSyntax::Valid,
            "{value}"
        );
    }
    for value in ["hypot(2px,3)", "hypot(1,2)", "pow(1,2)", "60"] {
        assert_eq!(
            property_value_syntax("outline-offset", value),
            PropertyValueSyntax::Invalid,
            "{value}"
        );
    }
    for value in [
        "-2px",
        "0",
        "hypot(2px,3px)",
        "calc(2 * 1px)",
        "calc(1px + 2em)",
    ] {
        assert_eq!(
            property_value_syntax("outline-offset", value),
            PropertyValueSyntax::Valid,
            "{value}"
        );
    }
}

#[test]
fn numeric_validation_rejects_negative_literals_and_incompatible_math_types() {
    for (property, value) in [
        ("border", "-2em"),
        ("border-width", "-1px"),
        ("font-size", "-10px"),
        ("font-size", "-20%"),
        ("line-height", "-2"),
        ("line-height", "-10px"),
        ("line-height", "-200%"),
        ("line-height", "min(1px,2)"),
        ("border-left-width", "min(0)"),
        ("border-left-width", "min(1px, 0)"),
        ("border-left-width", "max(0)"),
        ("border-left-width", "max(1px, 0)"),
        ("margin-left", "min(1px, 0)"),
        ("margin-left", "min(1%, 0)"),
        ("margin-left", "max(1px, 0)"),
        ("margin-left", "max(1%, 0)"),
        ("border-width", "calc(2px) -1px"),
        ("font-size", "-10px/**/"),
        ("outline-offset", "60/**/"),
        ("margin", "1px 2"),
        ("margin-inline", "auto min(1%, 0)"),
        ("border-right", "solid min(1em, 0) red"),
        ("letter-spacing", "clamp(0,10rem,100%)"),
        ("letter-spacing", "20/**/"),
        ("word-spacing", "max(2px,0)"),
    ] {
        assert_eq!(
            property_value_syntax(property, value),
            PropertyValueSyntax::Invalid,
            "{property}:{value}"
        );
        let css = format!("#target {{{BASE}}}");
        assert_eq!(
            computed(&format!("{css} #target {{{property}:{value}}}"), ""),
            computed(&css, ""),
            "stylesheet {property}:{value}"
        );
        assert_eq!(
            computed("", &format!("{BASE};{property}:{value}")),
            computed("", BASE),
            "inline {property}:{value}"
        );
    }
}

#[test]
fn numeric_validation_preserves_ranges_units_and_calculated_values() {
    for (property, value) in [
        ("font-size", "-0px"),
        ("font-size", "calc(-10px)"),
        ("font-size", "min(-10px,-20px)"),
        ("font-size", "calc(10px + 20%)"),
        ("line-height", "2"),
        ("line-height", "2/**/"),
        ("line-height", "0"),
        ("line-height", "calc(-2)"),
        ("line-height", "calc(-2px)"),
        ("line-height", "min(1.5,2)"),
        ("line-height", "calc(10 + (sign(2cqw - 10px) * 5))"),
        ("border", "calc(-1px) solid red"),
        ("border", "3px solid rgb(-10,0,0)"),
        ("border-width", "calc(-1px) 2px"),
        ("border-left-width", "min(0px,1px)"),
        ("margin-left", "min(-1px,0px)"),
        ("margin-left", "min(1%,2px)"),
        ("margin-inline", "auto -2px"),
        ("margin", "-1px 0 -2% auto"),
        ("letter-spacing", "120%"),
        ("letter-spacing", "calc(2ch - 30%)"),
        ("word-spacing", "min(2%,3px)"),
        ("letter-spacing", "normal/**/"),
    ] {
        assert_eq!(
            property_value_syntax(property, value),
            PropertyValueSyntax::Valid,
            "{property}:{value}"
        );
    }
    assert_eq!(
        computed("#target {margin-left:7px;margin-left:min(-1px,0px)}", ""),
        computed("#target {margin-left:-1px}", "")
    );
    assert_eq!(
        computed(
            "#target {letter-spacing:120%;letter-spacing:2px !/**/IMPORTANT}",
            ""
        ),
        computed("#target {letter-spacing:2px}", "")
    );
    assert_eq!(
        computed(
            "#target {letter-spacing:120%;letter-spacing:normal/**/}",
            ""
        ),
        computed("#target {letter-spacing:normal}", "")
    );
    assert_eq!(
        computed("#target {tab-size:4 !/**/IMPORTANT}", ""),
        computed("#target {tab-size:4}", "")
    );
    assert_eq!(
        computed(
            "#target {white-space:preserve-breaks nowrap !/**/IMPORTANT}",
            ""
        ),
        computed("#target {white-space:preserve-breaks nowrap}", "")
    );
}

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
        assert_eq!(
            property_value_syntax(property, invalid),
            PropertyValueSyntax::Invalid,
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
        assert_eq!(
            property_value_syntax("font-feature-settings", invalid),
            PropertyValueSyntax::Invalid,
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
        (
            "'liga' calc(sign(2ch - 1px)), 'liga' 0",
            vec![(*b"liga", 0)],
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
fn calculated_feature_settings_resolve_units_round_and_clamp() {
    for (value, expected) in [
        ("calc(1 + 1)", 2),
        ("calc(2.5)", 3),
        ("calc(-2.5)", 0),
        ("calc(NaN)", 0),
        ("calc(NaN + sign(1em - 10px))", 0),
        ("calc(log(-1) + sign(1em - 10px))", 0),
        ("calc(infinity)", u32::MAX),
        ("clamp(10, 5 ,3)", 10),
        ("hypot(3,4)", 5),
        ("calc(10 + sign(1em - 10px) * 5)", 15),
        ("calc(10 + sign(1em - 16px) * 5)", 10),
        ("calc(10 + sign(1em - 20px) * 5)", 5),
        ("calc(10 + sign(min(1em,20px) - 18px) * 5)", 5),
    ] {
        let declaration = format!("font-feature-settings:'ss03' {value}");
        assert_eq!(
            property_value_syntax("font-feature-settings", &format!("'ss03' {value}")),
            PropertyValueSyntax::Valid,
            "{value}"
        );
        assert_eq!(
            feature_settings_at("", &declaration, 600.0).0,
            vec![(*b"ss03", expected)],
            "inline {value}"
        );
        assert_eq!(
            feature_settings_at(&format!("#target {{{declaration}}}"), "", 600.0).0,
            vec![(*b"ss03", expected)],
            "stylesheet {value}"
        );
    }
    for (size, expected) in [(10, 5), (20, 15)] {
        let css = format!(
            "#target {{font-feature-settings:'ss03' calc(10 + sign(1em - 18px) * 5);font-size:{size}px}}"
        );
        assert_eq!(
            feature_settings_at(&css, "", 600.0).0,
            vec![(*b"ss03", expected)]
        );
    }
    assert_eq!(
        feature_settings_at("body {font-size:20px;font-feature-settings:'ss03' calc(10 + sign(1em - 18px) * 5)} #target {font-size:10px}", "", 600.0).0,
        vec![(*b"ss03", 15)]
    );
    let css = "#target {font-feature-settings:'ss03' calc(10 + sign(2vw - 10px) * 5)}";
    assert_eq!(
        feature_settings_at(css, "", 400.0),
        (vec![(*b"ss03", 5)], true)
    );
    assert_eq!(
        feature_settings_at(css, "", 800.0),
        (vec![(*b"ss03", 15)], true)
    );
    assert_eq!(
        feature_settings_at(
            "",
            "--settings:'ss03' calc(1em - 10px);font-feature-settings:var(--settings)",
            600.0
        )
        .0,
        vec![]
    );
    // Keep valid context-dependent syntax even for units not resolved by this renderer.
    assert_eq!(
        property_value_syntax(
            "font-feature-settings",
            "'liga' calc(10 + sign(2cqw - 10px) * 5)"
        ),
        PropertyValueSyntax::Valid
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
        assert_eq!(
            property_value_syntax(property, invalid),
            PropertyValueSyntax::Invalid,
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

#[test]
fn pagination_tokens_preserve_aliases_comments_and_positive_integer_math() {
    for (property, value, expected) in [
        ("break-before", "p\\61 ge/**/", "page"),
        ("break-after", "ALWAYS/**/", "page"),
        ("break-before", "all", "page"),
        ("break-after", "column/**/", "column"),
        ("break-inside", "avoid-page/**/", "avoid"),
        ("page-break-before", "always/**/", "always"),
        ("page-break-after", "recto/**/", "always"),
        ("page-break-before", "verso", "always"),
        ("page-break-inside", "avoid/**/", "avoid"),
        ("widows", "234/**/", "234"),
        ("orphans", "calc(1 + 234)", "235"),
        ("widows", "calc(2.5)", "3"),
        ("orphans", "calc(-1)", "1"),
        ("widows", "calc(0)", "1"),
        ("orphans", "calc(NaN)", "1"),
        ("widows", "calc(infinity)", "255"),
        ("orphans", "999", "255"),
        ("widows", "clamp(4, 2 ,3)", "4"),
        ("orphans", "calc(10 + sign(1em - 10px) * 5)", "15"),
    ] {
        assert_eq!(
            property_value_syntax(property, value),
            PropertyValueSyntax::Valid,
            "{property}:{value}"
        );
        let baseline = format!("{property}:{expected}");
        let declaration = format!("{property}:{value}");
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
        let substituted = format!("--pagination:{value};{property}:var(--pagination)");
        assert_eq!(
            computed("", &substituted),
            computed("", &baseline),
            "substitution {substituted}"
        );
    }
    assert_eq!(
        computed("#target {break-before:page;break-before:region}", ""),
        computed("#target {break-before:auto}", "")
    );
    assert_eq!(
        computed("#target {break-inside:avoid;break-inside:avoid-region}", ""),
        computed("#target {break-inside:auto}", "")
    );
    assert_eq!(
        computed(
            "body {widows:7;orphans:8} #target {widows:2;widows:unset;orphans:2;orphans:inherit}",
            ""
        ),
        computed("#target {widows:7;orphans:8}", "")
    );
    assert_eq!(
        computed(
            "#target {widows:5!important;widows:calc(2 + 1) !/**/IMPORTANT}",
            ""
        ),
        computed("#target {widows:3}", "")
    );
    assert_eq!(
        computed(
            "@layer base, overrides;@layer base {#target {break-before:page;widows:7}} @layer overrides {#target {page-break-before:avoid;page-break-before:revert-layer;widows:2;widows:revert-layer}}",
            ""
        ),
        computed("#target {break-before:page;widows:7}", "")
    );
}

#[test]
fn numeric_box_constraints_apply_before_cascade_and_after_substitution() {
    let mut cases = Vec::new();
    for name in [
        "gap",
        "row-gap",
        "column-gap",
        "grid-gap",
        "grid-row-gap",
        "grid-column-gap",
        "border-spacing",
        "block-size",
        "inline-size",
        "min-block-size",
        "min-inline-size",
        "max-block-size",
        "max-inline-size",
        "border-block-width",
        "border-inline-width",
        "border-block-start-width",
        "border-block-end-width",
        "border-inline-start-width",
        "border-inline-end-width",
    ] {
        cases.push((name, "-10px"));
        cases.push((name, "60"));
        cases.push((name, "min(10px, 0)"));
        if name.ends_with("size") {
            cases.push((name, "fit-content(-10px)"));
            cases.push((name, "fit-content(60)"));
        }
    }
    for name in [
        "gap",
        "grid-gap",
        "border-spacing",
        "border-block-width",
        "border-inline-width",
    ] {
        cases.push((name, "10px -20px"));
        cases.push((name, "10px 60"));
    }
    for name in [
        "border-block",
        "border-inline",
        "border-block-start",
        "border-block-end",
        "border-inline-start",
        "border-inline-end",
    ] {
        cases.push((name, "-10px solid green"));
        cases.push((name, "60 solid green"));
    }
    for (name, invalid) in cases {
        assert_eq!(
            property_value_syntax(name, invalid),
            PropertyValueSyntax::Invalid,
            "{name}:{invalid}"
        );
        let valid = format!("{name}:7px");
        let invalid_declaration = format!("{name}:{invalid}");
        assert_eq!(
            computed("", &format!("{valid};{invalid_declaration}")),
            computed("", &valid),
            "inline {invalid_declaration}"
        );
        assert_eq!(
            computed(&format!("#target {{{valid};{invalid_declaration}}}"), ""),
            computed(&format!("#target {{{valid}}}"), ""),
            "stylesheet {invalid_declaration}"
        );
        let substituted = format!("--bad:{invalid};{valid};{name}:var(--bad)");
        let initial = format!("{name}:initial");
        assert_eq!(
            computed("", &substituted),
            computed("", &initial),
            "inline substitution {invalid_declaration}"
        );
        assert_eq!(
            computed(&format!("#target {{{substituted}}}"), ""),
            computed(&format!("#target {{{initial}}}"), ""),
            "stylesheet substitution {invalid_declaration}"
        );
    }
}

#[test]
fn gap_aliases_and_logical_box_values_preserve_valid_syntax() {
    for name in [
        "gap",
        "row-gap",
        "column-gap",
        "grid-gap",
        "grid-row-gap",
        "grid-column-gap",
    ] {
        for value in [
            "normal",
            "-0px",
            "0",
            "20%",
            "calc(20px + 3%)",
            "calc(-10px)",
            "inherit/**/",
            "var(--gap)",
        ] {
            assert_eq!(
                property_value_syntax(name, value),
                PropertyValueSyntax::Valid,
                "{name}:{value}"
            );
        }
    }
    for (name, value) in [
        ("border-spacing", "10px 2em"),
        ("border-spacing", "-0px"),
        ("border-spacing", "calc(-10px)"),
        ("inline-size", "fit-content(20%)"),
        ("block-size", "min-content"),
        ("min-block-size", "auto"),
        ("max-inline-size", "none"),
        ("border-block-width", "thin 3px"),
        ("border-inline", "3px solid green"),
    ] {
        assert_eq!(
            property_value_syntax(name, value),
            PropertyValueSyntax::Valid,
            "{name}:{value}"
        );
    }
    for name in [
        "width",
        "height",
        "block-size",
        "inline-size",
        "min-block-size",
        "min-inline-size",
        "max-block-size",
        "max-inline-size",
    ] {
        for value in [
            "calc(-10px)",
            "fit-content(calc(-10px))",
            "fit-content(-0px)",
        ] {
            assert_eq!(
                property_value_syntax(name, value),
                PropertyValueSyntax::Valid,
                "{name}:{value}"
            );
        }
    }
    for (alias, canonical) in [
        ("grid-gap", "gap"),
        ("grid-row-gap", "row-gap"),
        ("grid-column-gap", "column-gap"),
    ] {
        assert_eq!(
            computed("", &format!("{alias}:13px")),
            computed("", &format!("{canonical}:13px"))
        );
        assert_eq!(
            computed("", &format!("{canonical}:7px;{alias}:13px")),
            computed("", &format!("{canonical}:13px"))
        );
        assert_eq!(
            computed("", &format!("{alias}:13px;{canonical}:7px")),
            computed("", &format!("{canonical}:7px"))
        );
    }
}

#[test]
fn radius_and_flex_constraints_preserve_cascade_and_unset_invalid_substitutions() {
    let mut cases = Vec::new();
    for name in [
        "border-radius",
        "border-top-left-radius",
        "border-top-right-radius",
        "border-bottom-right-radius",
        "border-bottom-left-radius",
        "border-start-start-radius",
        "border-start-end-radius",
        "border-end-start-radius",
        "border-end-end-radius",
        "-webkit-border-radius",
        "-moz-border-radius",
    ] {
        for invalid in ["-10px", "10px -20px", "10px 60", "min(1px, 0)"] {
            cases.push((name, "7px", invalid));
        }
    }
    for invalid in [
        "2 3 4",
        "1 2 calc(0)",
        "1 2 calc(3 - 3)",
        "-1 2 10px",
        "1 -2 10px",
        "1 2 -10px",
        "1 2 10px 20px",
        "1 10px 2",
        "none 1",
        "",
    ] {
        cases.push(("flex", "3 4 7px", invalid));
    }
    for name in ["flex-grow", "flex-shrink"] {
        for invalid in ["-1", "2px", "1 2", "min(1px, 2px)"] {
            cases.push((name, "3", invalid));
        }
    }
    for (name, valid, invalid) in cases {
        assert_eq!(
            property_value_syntax(name, invalid),
            PropertyValueSyntax::Invalid,
            "{name}:{invalid}"
        );
        let prior = format!("{name}:{valid}");
        let declarations = format!("{prior};{name}:{invalid}");
        assert_eq!(
            computed("", &declarations),
            computed("", &prior),
            "inline {declarations}"
        );
        assert_eq!(
            computed(&format!("#target {{{declarations}}}"), ""),
            computed(&format!("#target {{{prior}}}"), ""),
            "stylesheet {declarations}"
        );
        let substituted = format!("--bad:{invalid};{prior};{name}:var(--bad)");
        let initial = format!("{name}:initial");
        assert_eq!(
            computed("", &substituted),
            computed("", &initial),
            "inline {substituted}"
        );
        assert_eq!(
            computed(&format!("#target {{{substituted}}}"), ""),
            computed(&format!("#target {{{initial}}}"), ""),
            "stylesheet {substituted}"
        );
    }
}

#[test]
fn radius_and_flex_grammar_preserve_valid_ranges_and_component_order() {
    for (name, value) in [
        ("border-radius", "10% 2em / 3px 4%"),
        ("border-radius", "-0px / 0"),
        ("border-radius", "calc(-10px)"),
        ("border-start-start-radius", "calc(10px + 2%) 20%"),
        ("border-radius", "inherit/**/"),
        ("flex", "1"),
        ("flex", "2 3"),
        ("flex", "2 3 0"),
        ("flex", "6px 4 5"),
        ("flex", "6px 4"),
        ("flex", "7% 8"),
        ("flex", "auto 1 2"),
        ("flex", "none"),
        ("flex", "1/**/2/**/10px"),
        ("flex", "calc(-1) calc(-1) 0"),
        ("flex", "1 1 calc(-10em)"),
        ("flex", "calc(1) calc(2 + 1) calc(3px)"),
        ("flex-grow", "calc(-10)"),
        ("flex-shrink", "calc(1 + 2)"),
        ("flex-grow", "var(--grow)"),
    ] {
        assert_eq!(
            property_value_syntax(name, value),
            PropertyValueSyntax::Valid,
            "{name}:{value}"
        );
    }
    for (input, expected) in [
        ("6px 4 5", "4 5 6px"),
        ("6px 4", "4 1 6px"),
        ("calc(1) calc(2 + 1) calc(3px)", "1 3 3px"),
        ("2 3 0", "2 3 0px"),
    ] {
        assert_eq!(
            computed("", &format!("flex:{input}")),
            computed("", &format!("flex:{expected}")),
            "{input}"
        );
    }
}

fn item_alignments(css: &str, inline: &str) -> Vec<(ItemAlignment, ItemAlignment)> {
    let document = parse_dom_document(&format!(
        "<!doctype html><html><body><section id='parent'><div id='target' style='{inline}'><p id='child'>Text</p></div></section></body></html>"
    )).unwrap();
    let styled = style_document(document, &[css]);
    ["parent", "target", "child"]
        .into_iter()
        .map(|name| {
            let node = styled
                .document()
                .node_ids()
                .find(|id| styled.document().get_dom_id(*id) == Some(name))
                .unwrap();
            let view = styled
                .style_view(styled.style_for_node(node).unwrap())
                .unwrap();
            (view.layout.align_items, view.layout.justify_items)
        })
        .collect()
}

#[test]
fn legacy_items_compute_from_parent_markers_and_propagate_through_defaults() {
    for (keyword, expected) in [
        ("left", ItemAlignment::LegacyLeft),
        ("right", ItemAlignment::LegacyRight),
        ("center", ItemAlignment::LegacyCenter),
    ] {
        let parent = format!("#parent {{justify-items:legacy {keyword}}}");
        for declarations in [
            "",
            "justify-items:legacy",
            "justify-items:initial",
            "justify-items:unset",
            "justify-items:inherit",
            "all:initial",
            "all:unset",
            "justify-items:var(--missing)",
            "--value:legacy;justify-items:var(--value)",
            "justify-items:leg\\61 cy/**/",
        ] {
            let values = item_alignments(&parent, declarations);
            assert_eq!(
                values.iter().map(|value| value.1).collect::<Vec<_>>(),
                vec![expected; 3],
                "{keyword}, inline {declarations}"
            );
            let values = item_alignments(&format!("{parent} #target {{{declarations}}}"), "");
            assert_eq!(
                values.iter().map(|value| value.1).collect::<Vec<_>>(),
                vec![expected; 3],
                "{keyword}, stylesheet {declarations}"
            );
        }
        let substituted =
            item_alignments(&parent, "--value:flex-end legacy;place-items:var(--value)");
        assert_eq!(substituted[1], (ItemAlignment::FlexEnd, expected));
        let values = item_alignments(&parent, "place-items:flex-end legacy");
        assert_eq!(values[1], (ItemAlignment::FlexEnd, expected));
        assert_eq!(values[2].1, expected);
        let values = item_alignments(&format!("{parent} #target {{justify-items:normal}}"), "");
        assert_eq!(values[1].1, ItemAlignment::Normal);
        assert_eq!(values[2].1, ItemAlignment::Normal);
    }
    for keyword in ["left", "right", "center", "stretch"] {
        let css = format!("#parent {{justify-items:{keyword}}}");
        for declarations in [
            "",
            "justify-items:legacy",
            "justify-items:initial",
            "justify-items:unset",
        ] {
            let values = item_alignments(&css, declarations);
            assert_eq!(
                values[1].1,
                ItemAlignment::Normal,
                "{keyword}, {declarations}"
            );
            assert_eq!(values[2].1, ItemAlignment::Normal);
        }
    }
}

#[test]
fn legacy_items_respect_component_grammar_cascade_priority_and_layers() {
    for (name, value) in [
        ("justify-items", "legacy"),
        ("justify-items", "LEGACY/**/"),
        ("place-items", "flex-end legacy"),
        ("place-items", "first baseline legacy"),
    ] {
        assert_eq!(
            property_value_syntax(name, value),
            PropertyValueSyntax::Valid,
            "{name}:{value}"
        );
    }
    for (name, value) in [
        ("justify-items", "legacy legacy"),
        ("justify-items", "safe legacy"),
        ("justify-items", "legacy start"),
        ("place-items", "legacy"),
        ("place-items", "legacy flex-end"),
        ("place-items", "flex-end legacy legacy"),
    ] {
        assert_eq!(
            property_value_syntax(name, value),
            PropertyValueSyntax::Invalid,
            "{name}:{value}"
        );
    }
    let parent = "#parent {justify-items:legacy right}";
    let values = item_alignments(
        &format!("{parent} #target {{justify-items:center!important;justify-items:legacy}}"),
        "",
    );
    assert_eq!(values[1].1, ItemAlignment::Center);
    assert_eq!(values[2].1, ItemAlignment::Normal);
    let values = item_alignments(
        &format!(
            "{parent} @layer a,b; @layer a {{#target {{justify-items:legacy left}}}} @layer b {{#target {{justify-items:revert-layer}}}}"
        ),
        "",
    );
    assert_eq!(values[1].1, ItemAlignment::LegacyLeft);
    assert_eq!(values[2].1, ItemAlignment::LegacyLeft);
    let values = item_alignments(
        parent,
        "place-items:flex-end legacy;place-items:var(--missing)",
    );
    assert_eq!(
        values[1],
        (ItemAlignment::Normal, ItemAlignment::LegacyRight)
    );
}

#[test]
fn unbounded_clamps_compute_lengths_colors_and_integers_in_both_source_paths() {
    for (name, value, expected) in [
        ("font-size", "clamp(none,2em,30px)", "30px"),
        ("outline-width", "clamp(none,2em,30px)", "30px"),
        ("outline-width", "calc(-10px)", "0px"),
        ("outline", "calc(-10px) solid red", "0px solid red"),
        ("letter-spacing", "clamp(2em,1px,1em)", "40px"),
        ("border-top-width", "calc(2em + 3px)", "43px"),
        ("border-top-width", "calc(-10px)", "0px"),
        ("line-height", "clamp(none,2em,30px)", "30px"),
        ("letter-spacing", "clamp(none,2em,30px)", "30px"),
        ("border-top-width", "clamp(none,2em,30px)", "30px"),
        ("padding-left", "clamp(none,2em,30px)", "30px"),
        ("row-gap", "clamp(none,2em,30px)", "30px"),
        ("margin-top", "clamp(2em,1px,1em)", "40px"),
        ("margin-top", "clamp(none,33px,30px)", "30px"),
        ("margin-top", "clamp(33px,30px,none)", "33px"),
        ("margin-top", "clamp(none,30px,none)", "30px"),
        ("margin-top", "clamp(none,2em,30px)", "30px"),
        ("margin-top", "clamp(3em,30px,none)", "60px"),
        (
            "margin-top",
            "clamp(clamp(none,2em,3px),4px,clamp(none,6px,7em))",
            "4px",
        ),
        ("margin-top", "clamp(none,calc(3px + 5px),none)", "8px"),
        ("margin-top", "clamp(none,-30px,none)", "-30px"),
        ("color", "rgb(clamp(none,33,30) 0 0)", "rgb(30 0 0)"),
        ("color", "rgb(clamp(33,30,none) 0 0)", "rgb(33 0 0)"),
        ("color", "rgb(clamp(none,30,none) 0 0)", "rgb(30 0 0)"),
        ("color", "rgb(from red clamp(none,r,r + 30) g b)", "red"),
        ("z-index", "clamp(none,33,30)", "30"),
        ("z-index", "clamp(33,30,none)", "33"),
        ("z-index", "clamp(none,30,none)", "30"),
        ("z-index", "calc(3 - 2)", "1"),
        ("z-index", "calc(-3 + 2)", "-1"),
        ("z-index", "calc(2.5)", "3"),
        ("z-index", "calc(-2.5)", "-2"),
        ("z-index", "calc(-2.6)", "-3"),
        ("z-index", "calc(sign(2em - 1px) + 2)", "3"),
        ("z-index", "calc(NaN)", "0"),
        ("z-index", "calc(infinity)", "2147483647"),
        ("z-index", "calc(-infinity)", "-2147483648"),
    ] {
        assert_eq!(
            property_value_syntax(name, value),
            PropertyValueSyntax::Valid,
            "{name}:{value}"
        );
        let actual = format!("font-size:20px;{name}:{value}");
        let expected = format!("font-size:20px;{name}:{expected}");
        assert_eq!(
            computed("", &actual),
            computed("", &expected),
            "inline {actual}"
        );
        assert_eq!(
            computed(&format!("#target {{{actual}}}"), ""),
            computed(&format!("#target {{{expected}}}"), ""),
            "stylesheet {actual}"
        );
    }
}

#[test]
fn unbounded_clamp_substitution_preserves_argument_arity_and_invalid_cascade_rules() {
    for invalid in [
        "clamp(none,none,30px)",
        "clamp(none,30px,none,40px)",
        "clamp(none,20%,30px)",
        "clamp(none,1px + ,none)",
        "clamp(none,30px,none extra)",
    ] {
        let name = if invalid.contains('%') {
            "border-top-width"
        } else {
            "margin-top"
        };
        assert_eq!(
            property_value_syntax(name, invalid),
            PropertyValueSyntax::Invalid,
            "{name}:{invalid}"
        );
        let prior = format!("{name}:7px");
        let declarations = format!("{prior};{name}:{invalid}");
        assert_eq!(
            computed("", &declarations),
            computed("", &prior),
            "{declarations}"
        );
        let declarations = format!("--bad:{invalid};{prior};{name}:var(--bad)");
        assert_eq!(
            computed("", &declarations),
            computed("", &format!("{name}:initial")),
            "{declarations}"
        );
    }
    for (declarations, expected) in [
        (
            "--args:1px,2px;margin-top:7px;margin-top:clamp(none,var(--args),30px)",
            "margin-top:initial",
        ),
        (
            "--args:20px,30px;margin-top:clamp(none,var(--args))",
            "margin-top:20px",
        ),
        (
            "--lower:none;--middle:20px;margin-top:clamp(var(--lower),var(--middle),30px)",
            "margin-top:20px",
        ),
        (
            "--value:clamp(none,20px,none);margin-top:var(--value)",
            "margin-top:20px",
        ),
        (
            "--value:20px;margin-top:clamp(none,var(--value),none)",
            "margin-top:20px",
        ),
        (
            "--args:1px,2px;margin-top:clamp(none,30px,var(--args))",
            "margin-top:initial",
        ),
    ] {
        assert_eq!(
            computed("", declarations),
            computed("", expected),
            "inline {declarations}"
        );
        assert_eq!(
            computed(&format!("#target {{{declarations}}}"), ""),
            computed(&format!("#target {{{expected}}}"), ""),
            "stylesheet {declarations}"
        );
    }
}

#[test]
fn signed_integer_validation_preserves_fallbacks_and_rejects_dimensions() {
    for invalid in [
        "1.5",
        "calc(1px)",
        "min(1px,2px)",
        "clamp(none,20%,30%)",
        "auto 1",
        "1 2",
    ] {
        assert_eq!(
            property_value_syntax("z-index", invalid),
            PropertyValueSyntax::Invalid,
            "{invalid}"
        );
        let declarations = format!("z-index:7;z-index:{invalid}");
        assert_eq!(
            computed("", &declarations),
            computed("", "z-index:7"),
            "{declarations}"
        );
        let declarations = format!("--bad:{invalid};z-index:7;z-index:var(--bad)");
        assert_eq!(
            computed("", &declarations),
            computed("", "z-index:initial"),
            "{declarations}"
        );
    }
    for (declarations, expected) in [
        ("z-index:calc(-2.5)!/**/IMPORTANT;z-index:8", "z-index:-2"),
        (
            "--value:clamp(none,-2.5,none);z-index:var(--value)",
            "z-index:-2",
        ),
        ("z-index:clamp(none,-7,none)", "z-index:-7"),
        ("z-index:auto/**/", "z-index:initial"),
    ] {
        assert_eq!(
            computed("", declarations),
            computed("", expected),
            "{declarations}"
        );
    }
}

#[test]
fn outline_width_ranges_distinguish_negative_literals_from_negative_math() {
    for invalid in [
        "outline-width:-10px",
        "outline-width:60",
        "outline:-10px solid red",
        "outline:60 solid red",
        "outline:hidden",
        "outline-width:min(1px,0)",
    ] {
        let prior = "outline:7px solid green";
        assert_eq!(
            computed("", &format!("{prior};{invalid}")),
            computed("", prior),
            "{invalid}"
        );
        assert_eq!(
            computed(&format!("#target {{{prior};{invalid}}}"), ""),
            computed(&format!("#target {{{prior}}}"), ""),
            "{invalid}"
        );
    }
}

#[test]
fn invalid_import_layers_remain_false_media_conditions_without_creating_layers() {
    for layer in [
        "layer()",
        "layer(A B)",
        "layer(A . B)",
        "layer(A, B, C)",
        "LaYeR( )",
    ] {
        let rule = format!("@import 'base.css' {layer};");
        assert!(stylesheet_syntax_is_valid(&rule), "{rule}");
        let entries = stylesheet_entries(&rule);
        let [StylesheetEntry::Import(import)] = entries.as_slice() else {
            panic!("{rule}: {entries:?}");
        };
        assert_eq!(import.url, "base.css");
        assert_eq!(import.layer, None);
        assert!(import.media.is_some());
        let wrapped = import.wrap_resolved_css("#target {color:red}");
        assert_eq!(
            computed(&format!("#target {{color:blue}} {wrapped}"), ""),
            computed("#target {color:blue}", ""),
            "{rule}"
        );
        for (operator, expected) in [("and", "blue"), ("or", "red")] {
            let source = format!("@import 'base.css' {layer} {operator} (min-width:1px);");
            assert!(stylesheet_syntax_is_valid(&source), "{source}");
            let entries = stylesheet_entries(&source);
            let [StylesheetEntry::Import(import)] = entries.as_slice() else {
                panic!("boolean condition: {entries:?}");
            };
            let wrapped = import.wrap_resolved_css("#target {color:red}");
            assert_eq!(
                computed(&format!("#target {{color:blue}} {wrapped}"), ""),
                computed(&format!("#target {{color:{expected}}}"), ""),
                "{source}"
            );
            assert!(stylesheet_syntax_is_valid(&wrapped), "wrapped {source}");
        }
        // The false condition must retain valid media-query alternatives.
        let entries = stylesheet_entries(&format!("@import 'base.css' {layer}, screen;"));
        let [StylesheetEntry::Import(import)] = entries.as_slice() else {
            panic!("media alternative: {entries:?}");
        };
        let wrapped = import.wrap_resolved_css("#target {color:red}");
        assert_eq!(
            computed(&format!("#target {{color:blue}} {wrapped}"), ""),
            computed("#target {color:red}", ""),
            "{rule}, screen"
        );
    }
}

#[test]
fn import_normalization_preserves_valid_layers_comments_strings_and_escapes() {
    for (source, layer) in [
        ("@import 'base.css' layer;", ImportLayer::Anonymous),
        (
            "@import 'base.css' layer(A.B);",
            ImportLayer::Named("A.B".into()),
        ),
        (
            "@\\69mport 'base.css' l\\61yer(A.B);",
            ImportLayer::Named("A.B".into()),
        ),
    ] {
        assert!(stylesheet_syntax_is_valid(source), "{source}");
        let entries = stylesheet_entries(source);
        let [StylesheetEntry::Import(import)] = entries.as_slice() else {
            panic!("{source}: {entries:?}");
        };
        assert_eq!(import.layer, Some(layer));
        assert_eq!(import.media, None);
    }
    let source = "/* @import 'ignored.css' layer(); */ #target {content:\"@import 'ignored.css' layer()\";color:red}";
    assert!(
        stylesheet_entries(source)
            .iter()
            .all(|entry| matches!(entry, StylesheetEntry::Css(_)))
    );
    assert_eq!(
        computed(source, ""),
        computed(
            "#target {content:\"@import 'ignored.css' layer()\";color:red}",
            ""
        )
    );
    let source =
        "@import 'first.css' layer(); @import 'second.css' layer(theme); #target {color:green}";
    let entries = stylesheet_entries(source);
    assert!(
        matches!(&entries[0], StylesheetEntry::Import(import) if import.url == "first.css" && import.layer.is_none() && import.media.is_some())
    );
    assert!(
        matches!(&entries[1], StylesheetEntry::Import(import) if import.url == "second.css" && import.layer == Some(ImportLayer::Named("theme".into())))
    );
    assert!(matches!(&entries[2], StylesheetEntry::Css(_)));
}

#[test]
fn noninteractive_pseudo_elements_reject_own_states_without_rejecting_origin_states() {
    for pseudo in [
        "::view-transition",
        "::view-transition-group(root)",
        "::view-transition-image-pair(root)",
        "::view-transition-old(root)",
        "::view-transition-new(root)",
        "::selection",
        "::target-text",
        "::highlight(example)",
        "::spelling-error",
        "::grammar-error",
        "::search-text",
    ] {
        assert!(selector_syntax_is_valid(pseudo), "{pseudo}");
        for state in ["hover", "active", "focus", "focus-visible", "focus-within"] {
            assert!(
                !selector_syntax_is_valid(&format!("{pseudo}:{state}")),
                "{pseudo}:{state}"
            );
            assert!(
                selector_syntax_is_valid(&format!("a:{state}{pseudo}")),
                "a:{state}{pseudo}"
            );
        }
    }
    for selector in [
        "::before:hover",
        "::after:active",
        "::marker:hover",
        "::view-transition-old(root):only-child",
        "::view-transition-new(root):only-child",
    ] {
        assert!(selector_syntax_is_valid(selector), "{selector}");
    }
}

#[test]
fn invalid_pseudo_element_states_invalidate_unforgiving_rule_lists_and_supports_queries() {
    for pseudo in [
        "::view-transition",
        "::view-transition-old(root)",
        "::selection",
        "::highlight(example)",
        "::search-text",
    ] {
        let invalid = format!("{pseudo}:hover");
        assert_eq!(
            computed(
                &format!("#target {{color:blue}} #target,{invalid} {{color:red}}"),
                ""
            ),
            computed("#target {color:blue}", ""),
            "{invalid}"
        );
        assert_eq!(
            computed(
                &format!(
                    "#target {{color:blue}} @supports selector({invalid}) {{#target {{color:red}}}}"
                ),
                ""
            ),
            computed("#target {color:blue}", ""),
            "supports {invalid}"
        );
    }
    assert_eq!(
        computed(
            "#target {color:blue} #target,a:hover::view-transition-old(root) {color:red}",
            ""
        ),
        computed("#target {color:red}", "")
    );
}

#[test]
fn picker_names_and_part_relational_selectors_do_not_admit_invalid_rule_lists() {
    for selector in [
        "::picker(select)",
        "::picker(SELECT)",
        "::picker(s\\65 lect)",
        "a:has(:hover)::part(label)",
        "::part(label):not(:hover)",
        "::part(label):is(:has(:hover),:hover)",
        "::part(label):where(:has(:hover),:hover)",
    ] {
        assert!(selector_syntax_is_valid(selector), "{selector}");
    }
    for selector in [
        "::picker(foo)",
        "::picker(s\\65 lector)",
        "::part(label):has(:hover)",
        "::part(label):has(p)",
        "::part(label):not(:has(:hover))",
        "::part(label):not(:not(:has(:hover)))",
    ] {
        assert!(!selector_syntax_is_valid(selector), "{selector}");
        assert_eq!(
            computed(
                &format!("#target {{color:blue}} #target,{selector} {{color:red}}"),
                ""
            ),
            computed("#target {color:blue}", ""),
            "invalid selector group {selector}"
        );
    }
    assert_eq!(
        computed(
            "#target {color:blue} @supports selector(::part(label):is(:has(:hover),:hover)) {#target {color:red}}",
            ""
        ),
        computed("#target {color:blue}", "")
    );
}

#[test]
fn host_context_accepts_single_compounds_and_keeps_valid_selector_groups() {
    for arguments in [
        ".theme",
        "div.theme",
        ".a/**/.b",
        "[data-theme='dark']",
        ".\\74 heme",
        ":not(.a,.b)",
        ":is(.a,.b)",
        ":has(>p)",
        ":host-context(.outer)",
    ] {
        let selector = format!(":host-context({arguments})");
        assert!(selector_syntax_is_valid(&selector), "{selector}");
        assert_eq!(
            computed(
                &format!("#target {{color:blue}} #target,{selector} {{color:red}}"),
                ""
            ),
            computed("#target {color:red}", ""),
            "valid selector group {selector}"
        );
    }
    for arguments in [
        "",
        ".a,.b",
        ".a .b",
        ".a+.b",
        "div/**/span",
        ">.a",
        "::before",
        ":bogus",
        "!",
    ] {
        let selector = format!(":host-context({arguments})");
        assert!(!selector_syntax_is_valid(&selector), "{selector}");
        assert_eq!(
            computed(
                &format!("#target {{color:blue}} #target,{selector} {{color:red}}"),
                ""
            ),
            computed("#target {color:blue}", ""),
            "invalid selector group {selector}"
        );
    }
    assert!(!selector_syntax_is_valid(
        "::part(label):host-context(:hover)"
    ));
    assert!(!selector_syntax_is_valid(":has(:host-context(:has(p)))"));
    assert_eq!(
        computed(
            "#target {color:blue} @supports selector(:host-context(:is(.a,:unknown))) {#target {color:red}}",
            ""
        ),
        computed("#target {color:blue}", "")
    );
}
