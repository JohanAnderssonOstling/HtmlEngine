use super::value::{significant_tokens, token_ident};
use cssparser::match_ignore_ascii_case;
use lightningcss::properties::custom::TokenList;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ParsedContain {
    pub(crate) size: bool,
    pub(crate) fully_supported: bool,
}

pub(crate) fn parse_tokens(tokens: &TokenList<'_>) -> Option<ParsedContain> {
    let mut keywords = significant_tokens(tokens);
    let mut seen = 0u8;
    while let Some(token) = keywords.next() {
        let keyword = token_ident(token)?;
        let bit = match_ignore_ascii_case! { keyword,
            "none" | "strict" | "content" => return (seen == 0 && keywords.next().is_none())
                .then_some(ParsedContain {
                    size: keyword.eq_ignore_ascii_case("strict"),
                    fully_supported: keyword.eq_ignore_ascii_case("none"),
                }),
            "size" => 1,
            "inline-size" => 2,
            "layout" => 4,
            "style" => 8,
            "paint" => 16,
            _ => return None,
        };
        if seen & bit != 0 {
            return None;
        }
        seen |= bit;
    }
    let size = seen & 1 != 0;
    let inline_size = seen & 2 != 0;
    (seen != 0 && !(size && inline_size)).then_some(ParsedContain {
        size,
        fully_supported: seen == 1,
    })
}

#[cfg(test)]
mod tests {
    use super::{ParsedContain, parse_tokens};
    use crate::allocation_test_support::count_allocations;
    use lightningcss::properties::custom::TokenList;
    use lightningcss::stylesheet::ParserOptions;
    use lightningcss::traits::ParseWithOptions;
    fn parse(value: &str) -> Option<ParsedContain> {
        let tokens = TokenList::parse_string_with_options(value, ParserOptions::default()).ok()?;
        let (parsed, allocations) = count_allocations(|| parse_tokens(&tokens));
        assert_eq!(allocations, 0, "{value}");
        parsed
    }

    #[test]
    fn converts_containment_keywords_and_support_metadata_without_allocating() {
        for (value, size, fully_supported) in [
            ("none", false, true),
            ("NoNe", false, true),
            ("size", true, true),
            ("\\73ize", true, true),
            ("layout size", true, false),
            ("SiZe /**/ LaYoUt StYlE PaInT", true, false),
            ("inline-size layout style paint", false, false),
            ("strict", true, false),
            ("StRiCt", true, false),
            ("content", false, false),
        ] {
            assert_eq!(
                parse(value),
                Some(ParsedContain {
                    size,
                    fully_supported
                }),
                "{value}"
            );
        }
        for value in [
            "",
            "size inline-size",
            "inline-size size",
            "size size",
            "inline-size inline-size",
            "layout layout",
            "style style",
            "paint paint",
            "strict layout",
            "layout strict",
            "none size",
            "size none",
            "content paint",
            "paint content",
            "size 1",
            "sıze",
        ] {
            assert!(parse(value).is_none(), "{value}");
        }
    }
}
