use html_style_model::WhiteSpace;
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};

pub const CASCADE_MARKER: &str = "--html-renderer-white-space";

pub fn parse_shorthand(value: &str) -> Option<WhiteSpace> {
    let words = value.split_ascii_whitespace().map(str::to_ascii_lowercase).collect::<Vec<_>>();
    if words.is_empty() {
        return None;
    }
    if words.len() == 1 {
        match words[0].as_str() {
            "normal" => return Some(WhiteSpace::Normal),
            "pre" => return Some(WhiteSpace::Pre),
            "nowrap" => return Some(WhiteSpace::NoWrap),
            "pre-wrap" => return Some(WhiteSpace::PreWrap),
            "pre-line" => return Some(WhiteSpace::PreLine),
            "break-spaces" => return Some(WhiteSpace::BreakSpaces),
            _ => {}
        }
    }

    #[derive(Clone, Copy)]
    enum Collapse {
        Collapse,
        Preserve,
        PreserveBreaks,
        BreakSpaces,
    }

    let mut collapse = None;
    let mut nowrap = None;
    for word in words {
        let next_collapse = match word.as_str() {
            "collapse" => Some(Collapse::Collapse),
            "preserve" => Some(Collapse::Preserve),
            "preserve-breaks" => Some(Collapse::PreserveBreaks),
            "break-spaces" => Some(Collapse::BreakSpaces),
            _ => None,
        };
        if let Some(next) = next_collapse {
            if collapse.replace(next).is_some() {
                return None;
            }
            continue;
        }
        let next_nowrap = match word.as_str() {
            "wrap" => Some(false),
            "nowrap" => Some(true),
            _ => None,
        };
        if let Some(next) = next_nowrap {
            if nowrap.replace(next).is_some() {
                return None;
            }
            continue;
        }
        return None;
    }

    let collapse = collapse.unwrap_or(Collapse::Collapse);
    let nowrap = nowrap.unwrap_or(false);
    Some(match (collapse, nowrap) {
        (Collapse::Collapse, false) => WhiteSpace::Normal,
        (Collapse::Collapse, true) => WhiteSpace::NoWrap,
        (Collapse::Preserve, false) => WhiteSpace::PreWrap,
        (Collapse::Preserve, true) => WhiteSpace::Pre,
        (Collapse::PreserveBreaks, false) => WhiteSpace::PreLine,
        (Collapse::PreserveBreaks, true) => WhiteSpace::PreserveBreaksNoWrap,
        (Collapse::BreakSpaces, false) => WhiteSpace::BreakSpaces,
        (Collapse::BreakSpaces, true) => WhiteSpace::BreakSpacesNoWrap,
    })
}

pub fn from_marker_tokens(tokens: &TokenList<'_>) -> Option<WhiteSpace> {
    let mut significant = tokens.0.iter().filter(|token| !token.is_whitespace() && !matches!(token, TokenOrValue::Token(Token::Comment(_))));
    let TokenOrValue::Token(Token::Ident(value)) = significant.next()? else { return None };
    if significant.next().is_some() {
        return None;
    }
    match value.as_ref().to_ascii_lowercase().as_str() {
        "normal" => Some(WhiteSpace::Normal),
        "pre" => Some(WhiteSpace::Pre),
        "nowrap" => Some(WhiteSpace::NoWrap),
        "pre-wrap" => Some(WhiteSpace::PreWrap),
        "pre-line" => Some(WhiteSpace::PreLine),
        "break-spaces" => Some(WhiteSpace::BreakSpaces),
        "preserve-breaks-nowrap" => Some(WhiteSpace::PreserveBreaksNoWrap),
        "break-spaces-nowrap" => Some(WhiteSpace::BreakSpacesNoWrap),
        _ => None,
    }
}

pub(crate) fn normalized_declaration(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let (value, important) = split_important(trimmed);
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    let (fallback, marker) = match lower.as_str() {
        "inherit" | "initial" | "unset" | "revert" | "revert-layer" => (lower.as_str(), lower.as_str()),
        _ => {
            let white_space = parse_shorthand(value)?;
            (fallback_value(white_space), marker_value(white_space))
        }
    };
    let important = if important { " !important" } else { "" };
    Some(format!("white-space: {fallback}{important}; {CASCADE_MARKER}: {marker}{important}"))
}

fn fallback_value(value: WhiteSpace) -> &'static str {
    match value {
        WhiteSpace::Normal => "normal",
        WhiteSpace::Pre => "pre",
        WhiteSpace::NoWrap => "nowrap",
        WhiteSpace::PreWrap => "pre-wrap",
        WhiteSpace::PreLine | WhiteSpace::PreserveBreaksNoWrap => "pre-line",
        WhiteSpace::BreakSpaces | WhiteSpace::BreakSpacesNoWrap => "break-spaces",
    }
}

fn marker_value(value: WhiteSpace) -> &'static str {
    match value {
        WhiteSpace::Normal => "normal",
        WhiteSpace::Pre => "pre",
        WhiteSpace::NoWrap => "nowrap",
        WhiteSpace::PreWrap => "pre-wrap",
        WhiteSpace::PreLine => "pre-line",
        WhiteSpace::BreakSpaces => "break-spaces",
        WhiteSpace::PreserveBreaksNoWrap => "preserve-breaks-nowrap",
        WhiteSpace::BreakSpacesNoWrap => "break-spaces-nowrap",
    }
}

fn split_important(value: &str) -> (&str, bool) {
    let Some(bang) = value.rfind('!') else { return (value, false) };
    if value[bang + 1..].trim().eq_ignore_ascii_case("important") { (&value[..bang], true) } else { (value, false) }
}

#[cfg(test)]
mod tests {
    use super::{from_marker_tokens, parse_shorthand};
    use html_style_model::WhiteSpace;
    use lightningcss::properties::custom::TokenList;
    use lightningcss::stylesheet::ParserOptions;
    use lightningcss::traits::ParseWithOptions;

    #[test]
    fn parses_level_four_white_space_axes_without_losing_nowrap() {
        assert_eq!(parse_shorthand("wrap collapse"), Some(WhiteSpace::Normal));
        assert_eq!(parse_shorthand("preserve nowrap"), Some(WhiteSpace::Pre));
        assert_eq!(parse_shorthand("preserve-breaks nowrap"), Some(WhiteSpace::PreserveBreaksNoWrap));
        assert_eq!(parse_shorthand("break-spaces nowrap"), Some(WhiteSpace::BreakSpacesNoWrap));
        assert_eq!(parse_shorthand("collapse balance"), None);
    }

    #[test]
    fn parses_internal_cascade_marker_tokens() {
        let tokens = TokenList::parse_string_with_options("preserve-breaks-nowrap", ParserOptions::default()).expect("valid custom-property value");
        assert_eq!(from_marker_tokens(&tokens), Some(WhiteSpace::PreserveBreaksNoWrap));
    }
}
