//! Reuse native value parsers for gaps in the native property dispatch table.
use super::value::{significant_tokens, token_list_to_css_string};
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};
use lightningcss::properties::size::Size;
use lightningcss::properties::{Property, PropertyId};
use lightningcss::traits::Parse;
use lightningcss::values::length::LengthPercentage;
use lightningcss::values::percentage::Percentage;

pub(crate) enum NativeFallback {
    FlexBasis(Size),
    ZIndex(i32),
    TextSpacing(LengthPercentage),
}

pub(crate) fn applies_to(id: &PropertyId<'_>) -> bool {
    matches!(
        id,
        PropertyId::FlexBasis(_)
            | PropertyId::ZIndex
            | PropertyId::LetterSpacing
            | PropertyId::WordSpacing
    )
}

pub(crate) fn parse(property: &Property<'_>) -> Option<NativeFallback> {
    let Property::Unparsed(raw) = property else {
        return None;
    };
    if !applies_to(&raw.property_id) {
        return None;
    }
    match raw.property_id {
        PropertyId::FlexBasis(_) => {
            let css = token_list_to_css_string(&raw.value)?;
            let value = Size::parse_string(&css).ok()?;
            // These have the same intrinsic sizing meaning in both properties.
            // Other Size variants need their own flex-basis range/conversion work.
            matches!(
                value,
                Size::MinContent(_) | Size::MaxContent(_) | Size::FitContent(_) | Size::Stretch(_)
            )
            .then_some(NativeFallback::FlexBasis(value))
        }
        PropertyId::ZIndex => integer_math(&raw.value).map(NativeFallback::ZIndex),
        PropertyId::LetterSpacing | PropertyId::WordSpacing => {
            text_spacing(&raw.value).map(NativeFallback::TextSpacing)
        }
        _ => None,
    }
}

pub(crate) fn single_percentage(tokens: &TokenList<'_>) -> Option<Percentage> {
    let mut values = significant_tokens(tokens);
    match (values.next(), values.next()) {
        (Some(TokenOrValue::Token(Token::Percentage { unit_value, .. })), None) => {
            Some(Percentage(*unit_value))
        }
        _ => None,
    }
}

pub(crate) fn text_spacing(tokens: &TokenList<'_>) -> Option<LengthPercentage> {
    // A standalone percentage is already a native parsed value.
    if let Some(value) = single_percentage(tokens) {
        return Some(LengthPercentage::Percentage(value));
    }
    let css = token_list_to_css_string(tokens)?;
    LengthPercentage::parse_string(&css)
        .ok()
        .filter(super::ast_validation::length_percentage_has_valid_math)
}

fn integer_math(tokens: &TokenList<'_>) -> Option<i32> {
    let mut values = significant_tokens(tokens);
    // Literal fractions remain invalid for <integer>. Only math functions
    // compute as <number> and round, with half-way ties toward positive infinity.
    if !matches!(values.next(), Some(TokenOrValue::Function(_))) || values.next().is_some() {
        return None;
    }
    let css = token_list_to_css_string(tokens)?;
    let value = f32::parse_string(&css).ok()?;
    value
        .is_finite()
        .then(|| (f64::from(value) + 0.5).floor() as i32)
}
