use super::*;

pub(super) fn apply_unparsed_property(
    styles: &mut ComputedStylesBuilder,
    style: &mut WorkingStyle,
    property: &Property<'_>,
) -> bool {
    let raw_property = match property {
        Property::Custom(custom) => Some((custom.name.as_ref(), &custom.value)),
        Property::Unparsed(unparsed) => Some((unparsed.property_id.name(), &unparsed.value)),
        _ => None,
    };
    if let Some((name, tokens)) = raw_property {
        let normalized_name = name.to_ascii_lowercase();
        match normalized_name.as_str() {
            "text-box" | "text-box-trim" | "text-box-edge" => {
                let _ = apply_text_box_property(style, &normalized_name, tokens);
                return true;
            }
            "font-feature-settings" => {
                if let Some(value) = token_list_to_css_string(tokens)
                    .as_deref()
                    .and_then(parse_font_feature_settings)
                {
                    style.font.font_feature_settings = value;
                }
                return true;
            }
            "font-kerning" => {
                if let Some(value) = token_list_to_css_string(tokens)
                    .as_deref()
                    .and_then(parse_font_kerning)
                {
                    style.font.font_kerning_features = value;
                }
                return true;
            }
            "font-variant-ligatures" => {
                if let Some(value) = token_list_to_css_string(tokens)
                    .as_deref()
                    .and_then(parse_font_variant_ligatures)
                {
                    style.font.font_variant_ligature_features = value;
                }
                return true;
            }
            "font-variant-numeric" => {
                if let Some(value) = token_list_to_css_string(tokens)
                    .as_deref()
                    .and_then(parse_font_variant_numeric)
                {
                    style.font.font_variant_numeric_features = value;
                }
                return true;
            }
            "break-before" | "page-break-before" => {
                if let Some(value) = parse_break_between(tokens) {
                    style.layout.break_before = value;
                }
                return true;
            }
            "break-after" | "page-break-after" => {
                if let Some(value) = parse_break_between(tokens) {
                    style.layout.break_after = value;
                }
                return true;
            }
            "break-inside" | "page-break-inside" => {
                if let Some(value) = parse_break_inside(tokens) {
                    style.layout.break_inside = value;
                }
                return true;
            }
            "widows" => {
                if let Some(value) = single_positive_integer(tokens) {
                    style.text.widows = value;
                }
                return true;
            }
            "orphans" => {
                if let Some(value) = single_positive_integer(tokens) {
                    style.text.orphans = value;
                }
                return true;
            }
            "contain" => {
                if let Some(value) = crate::style::syntax::contain::parse_tokens(tokens) {
                    style.box_model.size_containment = value.size;
                }
                return true;
            }
            _ => {}
        }
        if apply_custom_box_keyword(style, name, tokens) {
            return true;
        }
        if name.eq_ignore_ascii_case("content")
            && let Some(content) = parse_generated_content(styles, tokens)
        {
            style.generated_content = content;
            return true;
        }
        if name.eq_ignore_ascii_case("counter-reset")
            && let Some(resets) = parse_counter_directives(styles, tokens, 0)
        {
            style.counters.resets = resets;
            return true;
        }
        if name.eq_ignore_ascii_case("counter-increment")
            && let Some(increments) = parse_counter_directives(styles, tokens, 1)
        {
            style.counters.increments = increments;
            return true;
        }
        if name.eq_ignore_ascii_case("quotes")
            && let Some(quotes) = parse_quotes(styles, tokens)
        {
            style.text.quotes = quotes;
            return true;
        }
    }
    false
}
