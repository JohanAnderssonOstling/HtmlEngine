use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::ParseWithOptions;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ParsedContain {
    pub(crate) size: bool,
    pub(crate) fully_supported: bool,
}

pub(crate) fn parse(value: &str) -> Option<ParsedContain> {
    let tokens = TokenList::parse_string_with_options(value, ParserOptions::default()).ok()?;
    parse_tokens(&tokens)
}

pub(crate) fn parse_tokens(tokens: &TokenList<'_>) -> Option<ParsedContain> {
    let keywords = tokens
        .0
        .iter()
        .filter(|token| !matches!(token, TokenOrValue::Token(Token::WhiteSpace(_) | Token::Comment(_))))
        .map(|token| match token {
            TokenOrValue::Token(Token::Ident(value)) => Some(value.as_ref().to_ascii_lowercase()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;

    match keywords.as_slice() {
        [value] if value == "none" => Some(ParsedContain { size: false, fully_supported: true }),
        [value] if value == "strict" => Some(ParsedContain { size: true, fully_supported: false }),
        [value] if value == "content" => Some(ParsedContain { size: false, fully_supported: false }),
        [] => None,
        values => {
            let mut size = false;
            let mut inline_size = false;
            let mut layout = false;
            let mut style = false;
            let mut paint = false;
            for value in values {
                let seen = match value.as_str() {
                    "size" => &mut size,
                    "inline-size" => &mut inline_size,
                    "layout" => &mut layout,
                    "style" => &mut style,
                    "paint" => &mut paint,
                    _ => return None,
                };
                if *seen {
                    return None;
                }
                *seen = true;
            }
            if size && inline_size {
                return None;
            }
            Some(ParsedContain { size, fully_supported: size && !inline_size && !layout && !style && !paint })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ParsedContain, parse};

    #[test]
    fn validates_the_contain_grammar_and_reports_the_supported_size_subset() {
        assert_eq!(parse("none"), Some(ParsedContain { size: false, fully_supported: true }));
        assert_eq!(parse("size"), Some(ParsedContain { size: true, fully_supported: true }));
        assert_eq!(parse("layout size"), Some(ParsedContain { size: true, fully_supported: false }));
        assert_eq!(parse("strict"), Some(ParsedContain { size: true, fully_supported: false }));
        assert!(parse("size inline-size").is_none());
        assert!(parse("layout layout").is_none());
        assert!(parse("strict layout").is_none());
    }
}
