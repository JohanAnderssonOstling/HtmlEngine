use lightningcss::properties::size::{MaxSize, Size};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::{Parse, TrySign};
use lightningcss::values::length::{Length, LengthPercentage, LengthPercentageOrAuto};

/// Strict web-syntax validation for box properties where Lightning CSS either
/// treats an unknown property as a custom declaration (`float`/`clear`) or
/// deliberately parses values more permissively than the property grammar.
/// `None` means this module does not own the property's extra validation.
pub fn property_value_is_valid(property_name: &str, value: &str) -> Option<bool> {
    let name = property_name.to_ascii_lowercase();
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    if matches!(lower.as_str(), "inherit" | "initial" | "unset" | "revert" | "revert-layer") || value.contains("var(") || value.contains("env(") {
        return targeted_property(&name).then_some(true);
    }

    match name.as_str() {
        "float" => return Some(matches!(lower.as_str(), "left" | "right" | "none" | "inline-start" | "inline-end")),
        "clear" => return Some(matches!(lower.as_str(), "left" | "right" | "both" | "none" | "inline-start" | "inline-end")),
        "object-fit" => return Some(matches!(lower.as_str(), "fill" | "contain" | "cover" | "none" | "scale-down" | "contain scale-down" | "scale-down contain" | "cover scale-down" | "scale-down cover")),
        "object-position" => {
            let top_level_components = top_level_component_count(value);
            return Some(top_level_components != 3 && lightningcss::values::position::Position::parse_string(value).is_ok());
        }
        "outline-offset" => return Some(lower == "inset" || Length::parse_string(value).is_ok()),
        "flex-basis" if matches!(lower.as_str(), "content" | "fit-content" | "min-content" | "max-content") => return Some(true),
        _ if !targeted_property(&name) => return None,
        _ => {}
    }

    // Lightning CSS currently accepts a non-zero unitless number as a length.
    if name != "aspect-ratio" && value.parse::<f32>().is_ok_and(|number| number != 0.0) {
        return Some(false);
    }

    let property = match Property::parse_string(PropertyId::from(name.as_str()), value, ParserOptions::default()) {
        Ok(property) if !matches!(property, Property::Unparsed(_) | Property::Custom(_)) => property,
        _ => return Some(false),
    };
    Some(property_has_valid_range(&property))
}

fn top_level_component_count(value: &str) -> usize {
    let mut depth = 0usize;
    let mut in_component = false;
    let mut count = 0usize;
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '/' && characters.peek() == Some(&'*') {
            let _ = characters.next();
            let mut previous = '\0';
            for comment in characters.by_ref() {
                if previous == '*' && comment == '/' {
                    break;
                }
                previous = comment;
            }
            continue;
        }
        match character {
            '(' => {
                if depth == 0 && !in_component {
                    count += 1;
                    in_component = true;
                }
                depth += 1;
            }
            ')' => depth = depth.saturating_sub(1),
            c if c.is_ascii_whitespace() && depth == 0 => in_component = false,
            _ if !in_component => {
                count += 1;
                in_component = true;
            }
            _ => {}
        }
    }
    count
}

fn targeted_property(name: &str) -> bool {
    matches!(
        name,
        "float"
            | "clear"
            | "flex-basis"
            | "width"
            | "height"
            | "aspect-ratio"
            | "object-fit"
            | "object-position"
            | "outline-offset"
            | "min-width"
            | "min-height"
            | "max-width"
            | "max-height"
            | "padding"
            | "padding-top"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "padding-block"
            | "padding-block-start"
            | "padding-block-end"
            | "padding-inline"
            | "padding-inline-start"
            | "padding-inline-end"
    )
}

fn property_has_valid_range(property: &Property<'_>) -> bool {
    match property {
        Property::Width(value) | Property::Height(value) | Property::MinWidth(value) | Property::MinHeight(value) => size_is_non_negative(value),
        Property::MaxWidth(value) | Property::MaxHeight(value) => max_size_is_non_negative(value),
        Property::AspectRatio(value) => value.ratio.as_ref().is_none_or(|ratio| ratio.0.is_finite() && ratio.1.is_finite() && ratio.0 >= 0.0 && ratio.1 >= 0.0),
        Property::FlexBasis(value, _) => flex_basis_is_non_negative(value),
        Property::PaddingTop(value)
        | Property::PaddingRight(value)
        | Property::PaddingBottom(value)
        | Property::PaddingLeft(value)
        | Property::PaddingBlockStart(value)
        | Property::PaddingBlockEnd(value)
        | Property::PaddingInlineStart(value)
        | Property::PaddingInlineEnd(value) => padding_value_is_non_negative(value),
        Property::Padding(value) => [&value.top, &value.right, &value.bottom, &value.left].into_iter().all(padding_value_is_non_negative),
        Property::PaddingBlock(value) => [&value.block_start, &value.block_end].into_iter().all(padding_value_is_non_negative),
        Property::PaddingInline(value) => [&value.inline_start, &value.inline_end].into_iter().all(padding_value_is_non_negative),
        _ => true,
    }
}

fn flex_basis_is_non_negative(value: &LengthPercentageOrAuto) -> bool {
    match value {
        LengthPercentageOrAuto::Auto => true,
        LengthPercentageOrAuto::LengthPercentage(length) => !length_is_negative(length),
    }
}

fn size_is_non_negative(value: &Size) -> bool {
    !matches!(value, Size::LengthPercentage(length) | Size::FitContentFunction(length) if length_is_negative(length))
}

fn max_size_is_non_negative(value: &MaxSize) -> bool {
    !matches!(value, MaxSize::LengthPercentage(length) | MaxSize::FitContentFunction(length) if length_is_negative(length))
}

fn padding_value_is_non_negative(value: &LengthPercentageOrAuto) -> bool {
    match value {
        LengthPercentageOrAuto::Auto => false,
        LengthPercentageOrAuto::LengthPercentage(length) => !length_is_negative(length),
    }
}

fn length_is_negative(value: &LengthPercentage) -> bool {
    // CSS range restrictions compare the numeric value with zero. IEEE -0.0
    // therefore remains a valid zero rather than a negative length.
    value.try_sign().is_some_and(|sign| sign < 0.0)
}

#[cfg(test)]
mod tests {
    use super::property_value_is_valid;

    #[test]
    fn rejects_recovered_box_values_without_rejecting_valid_ranges() {
        assert_eq!(property_value_is_valid("float", "left right"), Some(false));
        assert_eq!(property_value_is_valid("float", "inline-start"), Some(true));
        assert_eq!(property_value_is_valid("width", "-10px"), Some(false));
        assert_eq!(property_value_is_valid("max-height", "-1px"), Some(false));
        assert_eq!(property_value_is_valid("max-height", "-0px"), Some(true));
        assert_eq!(property_value_is_valid("width", "60"), Some(false));
        assert_eq!(property_value_is_valid("width", "20%"), Some(true));
        assert_eq!(property_value_is_valid("padding", "auto"), Some(false));
        assert_eq!(property_value_is_valid("padding", "10px 20%"), Some(true));
        assert_eq!(property_value_is_valid("object-fit", "scale-down"), Some(true));
        assert_eq!(property_value_is_valid("object-fit", "contain cover"), Some(false));
        assert_eq!(property_value_is_valid("object-position", "right 10px bottom 20%"), Some(true));
        assert_eq!(property_value_is_valid("object-position", "left right"), Some(false));
        assert_eq!(property_value_is_valid("outline-offset", "-2px"), Some(true));
        assert_eq!(property_value_is_valid("outline-offset", "inset"), Some(true));
        assert_eq!(property_value_is_valid("outline-offset", "20%"), Some(false));
        assert_eq!(property_value_is_valid("color", "red"), None);
    }
}
