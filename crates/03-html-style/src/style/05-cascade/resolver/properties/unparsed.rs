use super::*;

pub(super) fn apply_unparsed_property(
    styles: &mut ComputedStylesBuilder,
    style: &mut WorkingStyle,
    property: &Property<'_>,
    root_font_size: f32,
) -> bool {
    let raw_property = match property {
        Property::Custom(custom) => Some((custom.name.as_ref(), &custom.value)),
        Property::Unparsed(unparsed) => Some((unparsed.property_id.name(), &unparsed.value)),
        _ => None,
    };
    if let Some((name, tokens)) = raw_property {
        let normalized_name = name.to_ascii_lowercase();
        match normalized_name.as_str() {
            "object-fit" => {
                if let Some(value) = parse_object_fit(tokens) {
                    style.box_model.object_fit = value;
                }
                return true;
            }
            "object-position" => {
                if let Some(value) = parse_object_position(tokens, style.font.font_size, root_font_size) {
                    style.box_model.object_position = value;
                }
                return true;
            }
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

fn parse_object_fit(tokens: &TokenList<'_>) -> Option<ObjectFit> {
    let css = token_list_to_css_string(tokens)?.to_ascii_lowercase();
    let words = css.split_ascii_whitespace().collect::<Vec<_>>();
    match words.as_slice() {
        ["fill"] => Some(ObjectFit::Fill),
        ["contain"] => Some(ObjectFit::Contain),
        ["cover"] => Some(ObjectFit::Cover),
        ["none"] => Some(ObjectFit::None),
        ["scale-down"] | ["contain", "scale-down"] | ["scale-down", "contain"] => Some(ObjectFit::ScaleDown),
        ["cover", "scale-down"] | ["scale-down", "cover"] => Some(ObjectFit::CoverScaleDown),
        _ => None,
    }
}

fn parse_object_position(tokens: &TokenList<'_>, font_size: f32, root_font_size: f32) -> Option<ObjectPosition> {
    use lightningcss::values::position::{HorizontalPositionKeyword, Position, PositionComponent, VerticalPositionKeyword};

    let css = token_list_to_css_string(tokens)?;
    let position = Position::parse_string(&css).ok()?;
    fn axis<S>(component: PositionComponent<S>, end: S, font_size: f32, root_font_size: f32) -> Option<ObjectPositionAxis>
    where
        S: Copy + PartialEq,
    {
        match component {
            PositionComponent::Center => Some(ObjectPositionAxis { origin: ObjectPositionOrigin::Start, offset: LengthPct::Pct(0.5) }),
            PositionComponent::Length(value) => Some(ObjectPositionAxis { origin: ObjectPositionOrigin::Start, offset: computed_length_pct(&value, font_size, root_font_size)? }),
            PositionComponent::Side { side, offset } => Some(ObjectPositionAxis {
                origin: if side == end { ObjectPositionOrigin::End } else { ObjectPositionOrigin::Start },
                offset: offset.as_ref().map_or(Some(LengthPct::Px(0.0)), |value| computed_length_pct(value, font_size, root_font_size))?,
            }),
        }
    }
    Some(ObjectPosition {
        x: axis(position.x, HorizontalPositionKeyword::Right, font_size, root_font_size)?,
        y: axis(position.y, VerticalPositionKeyword::Bottom, font_size, root_font_size)?,
    })
}

pub(super) fn unparsed_object_property_is_computable(property: &Property<'_>, font_size: f32, root_font_size: f32) -> Option<bool> {
    let (name, tokens) = match property {
        Property::Custom(custom) => (custom.name.as_ref(), &custom.value),
        Property::Unparsed(unparsed) => (unparsed.property_id.name(), &unparsed.value),
        _ => return None,
    };
    if name.eq_ignore_ascii_case("object-fit") {
        Some(parse_object_fit(tokens).is_some())
    } else if name.eq_ignore_ascii_case("object-position") {
        Some(parse_object_position(tokens, font_size, root_font_size).is_some())
    } else {
        None
    }
}
