use super::*;

#[allow(unused_variables)]
pub(super) fn apply(context: &mut PropertyContext<'_, '_>, property: &Property<'_>) -> ApplyResult {
    let doc = context.doc;
    let styles = &mut *context.styles;
    let style = &mut *context.style;
    let resolved_root_font_size = context.resolved_root_font_size;
    let parent_font_size = context.parent_font_size;
    let parent_font_weight = context.parent_font_weight;
    let parent_color = context.parent_color;
    match property {
        Property::Custom(custom) => match custom.name.as_ref().to_ascii_lowercase().as_str() {
            "float" => {
                if let Some(value) =
                    single_ident_keyword(&custom.value).map(str::to_ascii_lowercase)
                {
                    style.box_model.float =
                        logical_float(value.as_str(), style.text.direction, style.box_model.float);
                }
            }
            "clear" => {
                if let Some(value) =
                    single_ident_keyword(&custom.value).map(str::to_ascii_lowercase)
                {
                    style.box_model.clear =
                        logical_clear(value.as_str(), style.text.direction, style.box_model.clear);
                }
            }
            _ => {}
        },
        Property::Hyphens(value, _) => {
            use lightningcss::properties::text::Hyphens as CssHyphens;
            style.text.hyphens = match value {
                CssHyphens::None => Hyphens::None,
                CssHyphens::Manual => Hyphens::Manual,
                CssHyphens::Auto => Hyphens::Auto,
            };
        }
        Property::WordBreak(wb) => {
            use lightningcss::properties::text::WordBreak as LcWordBreak;
            style.text.word_break = match wb {
                LcWordBreak::Normal => WordBreak::Normal,
                LcWordBreak::BreakAll => WordBreak::BreakAll,
                LcWordBreak::KeepAll => WordBreak::KeepAll,
                // Legacy value: behaves as `overflow-wrap: break-word`.
                LcWordBreak::BreakWord => {
                    if style.text.overflow_wrap == OverflowWrap::Normal {
                        style.text.overflow_wrap = OverflowWrap::BreakWord;
                    }
                    WordBreak::Normal
                }
            };
        }
        Property::OverflowWrap(ow) | Property::WordWrap(ow) => {
            use lightningcss::properties::text::OverflowWrap as LcOverflowWrap;
            style.text.overflow_wrap = match ow {
                LcOverflowWrap::Normal => OverflowWrap::Normal,
                LcOverflowWrap::BreakWord => OverflowWrap::BreakWord,
                LcOverflowWrap::Anywhere => OverflowWrap::Anywhere,
            };
        }
        Property::BoxSizing(bs, _) => {
            use lightningcss::properties::size::BoxSizing as LcBoxSizing;
            style.box_model.box_sizing = match bs {
                LcBoxSizing::ContentBox => BoxSizing::ContentBox,
                LcBoxSizing::BorderBox => BoxSizing::BorderBox,
            };
        }

        _ => {
            if let Property::Unparsed(unparsed) = property {
                if apply_custom_box_keyword(style, unparsed.property_id.name(), &unparsed.value) {
                    return ApplyResult::Applied;
                }
                match unparsed.property_id.name() {
                    "page-break-before" | "page-break-after" | "page-break-inside"
                    | "break-before" | "break-after" | "break-inside" => {
                        // Not supported yet.
                        return ApplyResult::Applied;
                    }
                    "content" => {
                        if let Some(content) = parse_generated_content(styles, &unparsed.value) {
                            style.generated_content = content;
                            return ApplyResult::Applied;
                        }
                    }
                    "float" => {
                        if let Some(value) =
                            single_ident_keyword(&unparsed.value).map(str::to_ascii_lowercase)
                        {
                            style.box_model.float = logical_float(
                                value.as_str(),
                                style.text.direction,
                                style.box_model.float,
                            );
                            return ApplyResult::Applied;
                        }
                    }
                    "clear" => {
                        if let Some(value) =
                            single_ident_keyword(&unparsed.value).map(str::to_ascii_lowercase)
                        {
                            style.box_model.clear = logical_clear(
                                value.as_str(),
                                style.text.direction,
                                style.box_model.clear,
                            );
                            return ApplyResult::Applied;
                        }
                    }
                    "flex-basis" => {
                        if let Some(value) =
                            single_ident_keyword(&unparsed.value).map(str::to_ascii_lowercase)
                        {
                            style.layout.flex_basis = match value.as_str() {
                                "min-content" => PreferredSize::MinContent,
                                "max-content" => PreferredSize::MaxContent,
                                "fit-content" => PreferredSize::FitContent,
                                _ => style.layout.flex_basis,
                            };
                            return ApplyResult::Applied;
                        }
                    }
                    _ => {}
                }
            }
            let prop_name = property.property_id().name().to_string();
            if matches!(
                prop_name.as_str(),
                "float" | "clear" | "css-float" | "css-clear"
            ) && let Ok(css) = property.to_css_string(false, PrinterOptions::default())
                && let Some(value) = css.split(':').nth(1).map(|v| v.trim().to_ascii_lowercase())
            {
                let value = value.as_str();
                if prop_name == "float" {
                    style.box_model.float =
                        logical_float(value, style.text.direction, style.box_model.float);
                    return ApplyResult::Applied;
                }
                style.box_model.clear =
                    logical_clear(value, style.text.direction, style.box_model.clear);
                return ApplyResult::Applied;
            }
        }
    }
    ApplyResult::Applied
}
