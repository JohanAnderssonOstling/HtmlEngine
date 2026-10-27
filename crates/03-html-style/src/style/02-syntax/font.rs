//! Font grammar, system-font fallback and reset-only shorthand longhands.

use super::property_id::RendererPropertyId;
use super::value::{NumberValue, parse_number, parsed_property, without_priority};
use cssparser::{Parser, ParserInput, Token};
use lightningcss::properties::font::{AbsoluteFontWeight, FontWeight};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::traits::Parse;
use static_self::IntoOwned;
use std::borrow::Cow;

pub(crate) const RESET_LONGHANDS: [RendererPropertyId; 4] = [
    RendererPropertyId::FontKerning,
    RendererPropertyId::FontFeatureSettings,
    RendererPropertyId::FontVariantNumeric,
    RendererPropertyId::FontVariantLigatures,
];

pub(crate) fn shorthand_literals_are_valid(source: &str) -> bool {
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    while let Ok(token) = parser.next() {
        match token {
            Token::Number { value, .. } if *value < 0.0 => return false,
            Token::Percentage { unit_value, .. } if *unit_value < 0.0 => return false,
            Token::Dimension { value, unit, .. } if *value < 0.0 => {
                // An oblique angle may be negative; sizes and line heights
                // may only become negative inside a calculation.
                if !["deg", "grad", "rad", "turn"]
                    .iter()
                    .any(|angle| unit.eq_ignore_ascii_case(angle))
                {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

pub(crate) fn weight_is_valid(source: &str) -> bool {
    match parse_number(source) {
        Some(NumberValue::Number(value)) => value.is_finite() && (1.0..=1000.0).contains(&value),
        Some(NumberValue::Calculation(_)) => true,
        None => matches!(
            FontWeight::parse_string(source),
            Ok(FontWeight::Bolder
                | FontWeight::Lighter
                | FontWeight::Absolute(AbsoluteFontWeight::Normal | AbsoluteFontWeight::Bold))
        ),
    }
}

pub(crate) fn normalize_weight<'a>(id: &PropertyId<'_>, source: &'a str) -> Cow<'a, str> {
    if !matches!(id, PropertyId::FontWeight) {
        return Cow::Borrowed(source);
    }
    let value = without_priority(source);
    if !matches!(parse_number(value), Some(NumberValue::Calculation(_))) {
        return Cow::Borrowed(source);
    }
    // Clamp constant calculations before native parsing erases the distinction
    // between a range-checked literal and an out-of-range calculated result.
    let Ok(number) = f32::parse_string(value) else {
        return Cow::Borrowed(source);
    };
    Cow::Owned(format!(
        "{}{}",
        number.max(1.0).min(1000.0),
        &source[value.len()..]
    ))
}

pub(crate) fn normalize_system_font<'a>(id: &PropertyId<'_>, source: &'a str) -> Cow<'a, str> {
    if !matches!(id, PropertyId::Font) {
        return Cow::Borrowed(source);
    }
    let value = without_priority(source);
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let system_font = parser.expect_ident().is_ok_and(|name| {
        [
            "caption",
            "icon",
            "menu",
            "message-box",
            "small-caption",
            "status-bar",
        ]
        .iter()
        .any(|keyword| name.eq_ignore_ascii_case(keyword))
    }) && parser.is_exhausted();
    if !system_font {
        return Cow::Borrowed(source);
    }
    // No platform font-role provider is available. CSS Fonts permits a UA
    // default font when the requested system font cannot be obtained. Use
    // the ordinary native shorthand so all supported longhands participate
    // in winner selection, font scaling and reader font overrides.
    Cow::Owned(format!("medium system-ui{}", &source[value.len()..]))
}

pub(crate) fn longhand<'a>(
    property: &Property<'_>,
    target: &PropertyId<'_>,
) -> Option<Property<'a>> {
    if !matches!(property, Property::Font(_)) {
        return None;
    }
    let id = RendererPropertyId::from_lightning(target)?;
    if !RESET_LONGHANDS.contains(&id) {
        return None;
    }
    let initial = if id == RendererPropertyId::FontKerning {
        "auto"
    } else {
        "normal"
    };
    parsed_property(id.lightning_id(), initial).map(IntoOwned::into_owned)
}
