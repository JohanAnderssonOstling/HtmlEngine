use crate::style::syntax::value::{
    NumberValue, length_is_valid, literal_numeric_tokens_are_valid, normalize_math, parse_number,
    split_important, substitutions,
};
use crate::style::syntax::{sign, tree_counting};
use lightningcss::traits::Parse;
use lightningcss::values::length::{Length, LengthPercentage};
use std::borrow::Cow;

pub const CASCADE_MARKER: &str = "--html-renderer-tab-size";

pub(crate) enum ParsedTabSize {
    Spaces(NumberValue),
    Length(LengthPercentage),
    ContextualLength(String),
}

pub(crate) fn parse(value: &str) -> Option<ParsedTabSize> {
    let value = normalize_math(value.trim());
    if let Some(number) = parse_number(&value) {
        return literal_numeric_tokens_are_valid(&value, true, true)
            .then_some(ParsedTabSize::Spaces(number));
    }
    if !literal_numeric_tokens_are_valid(&value, true, false) {
        return None;
    }
    let lowered = sign::lower(&value, |_| Some(1.0)).ok()?;
    let length = Length::parse_string(&lowered).ok()?;
    if !length_is_valid(&length) {
        return None;
    }
    Some(if matches!(lowered, Cow::Owned(_)) {
        // Retain the expression; the syntax stand-in never computes a tab.
        ParsedTabSize::ContextualLength(value.into_owned())
    } else {
        ParsedTabSize::Length(LengthPercentage::parse_string(&value).ok()?)
    })
}

pub fn normalized_declaration(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let (value, important) = split_important(trimmed);
    let value = value.trim();
    let suffix = if important { " !important" } else { "" };
    let lower = value.to_ascii_lowercase();
    let marker_value = match lower.as_str() {
        "inherit" | "unset" => "inherit".to_owned(),
        "initial" => "8".to_owned(),
        // The resolver has no earlier-origin snapshot; preserve its existing
        // revert approximation by retaining the inherited value.
        "revert" | "revert-layer" => "inherit".to_owned(),
        _ => {
            if substitutions(value) != Ok(true) {
                let lowered = tree_counting::lower(value, |_| 1).ok()?;
                let random = crate::style::syntax::random_math::for_syntax(&lowered).ok()?;
                parse(&random)?;
            }
            value.to_owned()
        }
    };
    Some(format!("{CASCADE_MARKER}: {marker_value}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::{ParsedTabSize, normalized_declaration, parse};
    use crate::style::syntax::value::NumberValue;

    #[test]
    fn accepts_non_negative_numbers_and_lengths_only() {
        assert!(matches!(parse("2.5"), Some(ParsedTabSize::Spaces(_))));
        assert!(matches!(
            parse("/**/3/**/"),
            Some(ParsedTabSize::Spaces(NumberValue::Number(3.0)))
        ));
        assert!(matches!(parse("4ch"), Some(ParsedTabSize::Length(_))));
        assert!(parse("-2").is_none());
        assert!(parse("-10px").is_none());
        assert!(parse("20%").is_none());
    }

    #[test]
    fn emits_an_internal_cascade_marker() {
        assert_eq!(
            normalized_declaration("4 !important").as_deref(),
            Some("--html-renderer-tab-size: 4 !important")
        );
        assert_eq!(
            normalized_declaration("initial").as_deref(),
            Some("--html-renderer-tab-size: 8")
        );
    }
}
