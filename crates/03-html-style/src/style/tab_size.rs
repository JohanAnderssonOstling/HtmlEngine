use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::TrySign;
use lightningcss::values::length::LengthPercentage;
use lightningcss::values::percentage::DimensionPercentage;

pub const CASCADE_MARKER: &str = "--html-renderer-tab-size";

#[derive(Clone, Debug)]
pub enum ParsedTabSize {
    Spaces(f32),
    Length(LengthPercentage),
}

pub fn parse(value: &str) -> Option<ParsedTabSize> {
    let value = value.trim();
    if value.contains('%') {
        return None;
    }
    if let Ok(number) = value.parse::<f32>() {
        return (number.is_finite() && number >= 0.0).then_some(ParsedTabSize::Spaces(number));
    }
    match Property::parse_string(PropertyId::TextIndent, value, ParserOptions::default()).ok()? {
        Property::TextIndent(indent) if !indent.hanging && !indent.each_line && !matches!(indent.value, DimensionPercentage::Percentage(_)) && !indent.value.try_sign().is_some_and(f32::is_sign_negative) => {
            Some(ParsedTabSize::Length(indent.value))
        }
        _ => None,
    }
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
            parse(value)?;
            value.to_owned()
        }
    };
    Some(format!("{CASCADE_MARKER}: {marker_value}{suffix}"))
}

fn split_important(value: &str) -> (&str, bool) {
    let Some(bang) = value.rfind('!') else { return (value, false) };
    if value[bang + 1..].trim().eq_ignore_ascii_case("important") { (&value[..bang], true) } else { (value, false) }
}

#[cfg(test)]
mod tests {
    use super::{ParsedTabSize, normalized_declaration, parse};

    #[test]
    fn accepts_non_negative_numbers_and_lengths_only() {
        assert!(matches!(parse("2.5"), Some(ParsedTabSize::Spaces(_))));
        assert!(matches!(parse("4ch"), Some(ParsedTabSize::Length(_))));
        assert!(parse("-2").is_none());
        assert!(parse("-10px").is_none());
        assert!(parse("20%").is_none());
    }

    #[test]
    fn emits_an_internal_cascade_marker() {
        assert_eq!(normalized_declaration("4 !important").as_deref(), Some("--html-renderer-tab-size: 4 !important"));
        assert_eq!(normalized_declaration("initial").as_deref(), Some("--html-renderer-tab-size: 8"));
    }
}
