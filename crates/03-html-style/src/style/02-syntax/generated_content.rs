//! Syntax and renderer support for generated-content declarations.

use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::ParserOptions;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ValueSupport {
    Supported,
    Unsupported,
    Invalid,
}

impl ValueSupport {
    pub(crate) fn is_valid(self) -> bool {
        self != Self::Invalid
    }
}

fn parsed_tokens<'a>(name: &'a str, value: &'a str) -> Option<TokenList<'a>> {
    match Property::parse_string(PropertyId::from(name), value, ParserOptions::default()).ok()? {
        Property::Unparsed(value) => Some(value.value),
        Property::Custom(value) => Some(value.value),
        _ => None,
    }
}

fn significant<'a, 'i>(tokens: &'a TokenList<'i>) -> Vec<&'a TokenOrValue<'i>> {
    tokens
        .0
        .iter()
        .filter(|token| {
            !matches!(
                token,
                TokenOrValue::Token(Token::WhiteSpace(_) | Token::Comment(_))
            )
        })
        .collect()
}

fn ident<'a, 'i>(token: &'a TokenOrValue<'i>) -> Option<&'a str> {
    match token {
        TokenOrValue::Token(Token::Ident(value)) => Some(value.as_ref()),
        _ => None,
    }
}

fn equals_any(value: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| value.eq_ignore_ascii_case(candidate))
}

fn css_wide(value: &str) -> bool {
    equals_any(
        value,
        &["inherit", "initial", "unset", "revert", "revert-layer"],
    )
}

fn valid_counter_name(value: &str) -> bool {
    !css_wide(value) && !equals_any(value, &["none", "default"])
}

fn supported_counter_style(value: &str) -> bool {
    equals_any(
        value,
        &[
            "decimal",
            "decimal-leading-zero",
            "lower-roman",
            "upper-roman",
            "lower-greek",
            "lower-alpha",
            "lower-latin",
            "upper-alpha",
            "upper-latin",
            "armenian",
            "georgian",
            "disc",
            "circle",
            "square",
        ],
    )
}

fn content_function(function: &lightningcss::properties::custom::Function<'_>) -> ValueSupport {
    let arguments = significant(&function.arguments);
    if function.name.as_ref().eq_ignore_ascii_case("attr") {
        return match arguments.as_slice() {
            [name] if ident(name).is_some() => ValueSupport::Supported,
            [name, ..] if ident(name).is_some() => ValueSupport::Unsupported,
            _ => ValueSupport::Invalid,
        };
    }
    if function.name.as_ref().eq_ignore_ascii_case("counter") {
        return match arguments.as_slice() {
            [name] if ident(name).is_some_and(valid_counter_name) => ValueSupport::Supported,
            [name, TokenOrValue::Token(Token::Comma), style]
                if ident(name).is_some_and(valid_counter_name) && ident(style).is_some() =>
            {
                if supported_counter_style(ident(style).unwrap()) {
                    ValueSupport::Supported
                } else {
                    ValueSupport::Unsupported
                }
            }
            _ => ValueSupport::Invalid,
        };
    }
    if function.name.as_ref().eq_ignore_ascii_case("counters") {
        return match arguments.as_slice() {
            [
                name,
                TokenOrValue::Token(Token::Comma),
                TokenOrValue::Token(Token::String(_)),
            ] if ident(name).is_some_and(valid_counter_name) => ValueSupport::Supported,
            [
                name,
                TokenOrValue::Token(Token::Comma),
                TokenOrValue::Token(Token::String(_)),
                TokenOrValue::Token(Token::Comma),
                style,
            ] if ident(name).is_some_and(valid_counter_name) && ident(style).is_some() => {
                if supported_counter_style(ident(style).unwrap()) {
                    ValueSupport::Supported
                } else {
                    ValueSupport::Unsupported
                }
            }
            _ => ValueSupport::Invalid,
        };
    }
    if equals_any(
        function.name.as_ref(),
        &[
            "url",
            "image",
            "image-set",
            "-webkit-image-set",
            "cross-fade",
            "linear-gradient",
            "radial-gradient",
            "conic-gradient",
            "target-counter",
            "target-counters",
            "leader",
            "string",
        ],
    ) {
        ValueSupport::Unsupported
    } else {
        ValueSupport::Invalid
    }
}

pub(crate) fn content(value: &str) -> ValueSupport {
    let Some(tokens) = parsed_tokens("content", value) else {
        return ValueSupport::Invalid;
    };
    let tokens = significant(&tokens);
    if tokens.len() == 1
        && let Some(keyword) = ident(tokens[0])
        && (css_wide(keyword) || equals_any(keyword, &["normal", "none"]))
    {
        return ValueSupport::Supported;
    }
    if tokens.is_empty() {
        return ValueSupport::Invalid;
    }

    let mut support = ValueSupport::Supported;
    let mut after_alt_separator = false;
    let mut main_items = 0;
    let mut alt_items = 0;
    for token in tokens {
        if matches!(token, TokenOrValue::Token(Token::Delim('/'))) {
            if after_alt_separator || main_items == 0 {
                return ValueSupport::Invalid;
            }
            after_alt_separator = true;
            support = ValueSupport::Unsupported;
            continue;
        }
        if after_alt_separator {
            let item = match token {
                TokenOrValue::Token(Token::String(_)) => ValueSupport::Supported,
                TokenOrValue::Function(function)
                    if equals_any(function.name.as_ref(), &["attr", "counter", "counters"]) =>
                {
                    content_function(function)
                }
                TokenOrValue::Var(_) | TokenOrValue::Env(_) => ValueSupport::Supported,
                _ => ValueSupport::Invalid,
            };
            if item == ValueSupport::Invalid {
                return item;
            }
            alt_items += 1;
            continue;
        }
        let item = match token {
            TokenOrValue::Token(Token::String(_)) => ValueSupport::Supported,
            TokenOrValue::Token(Token::Ident(value))
                if equals_any(
                    value.as_ref(),
                    &[
                        "open-quote",
                        "close-quote",
                        "no-open-quote",
                        "no-close-quote",
                    ],
                ) =>
            {
                ValueSupport::Supported
            }
            TokenOrValue::Url(_) | TokenOrValue::Token(Token::UnquotedUrl(_)) => {
                ValueSupport::Unsupported
            }
            TokenOrValue::Function(function) => content_function(function),
            TokenOrValue::Var(_) | TokenOrValue::Env(_) => ValueSupport::Supported,
            _ => ValueSupport::Invalid,
        };
        if item == ValueSupport::Invalid {
            return item;
        }
        if item == ValueSupport::Unsupported {
            support = item;
        }
        main_items += 1;
    }
    if after_alt_separator && alt_items == 0 {
        ValueSupport::Invalid
    } else {
        support
    }
}

pub(crate) fn counter_directive(name: &str, value: &str) -> ValueSupport {
    let Some(tokens) = parsed_tokens(name, value) else {
        return ValueSupport::Invalid;
    };
    let tokens = significant(&tokens);
    if tokens.len() == 1
        && ident(tokens[0])
            .is_some_and(|value| css_wide(value) || value.eq_ignore_ascii_case("none"))
    {
        return ValueSupport::Supported;
    }
    if tokens.is_empty() {
        return ValueSupport::Invalid;
    }
    let mut cursor = 0;
    let mut support = ValueSupport::Supported;
    while cursor < tokens.len() {
        let is_reset = name.eq_ignore_ascii_case("counter-reset");
        let (counter_name, unsupported_name_form) = match tokens[cursor] {
            TokenOrValue::Token(Token::Ident(counter_name)) => (counter_name.as_ref(), false),
            TokenOrValue::Function(function)
                if is_reset && function.name.as_ref().eq_ignore_ascii_case("reversed") =>
            {
                let arguments = significant(&function.arguments);
                match arguments.as_slice() {
                    [TokenOrValue::Token(Token::Ident(counter_name))] => {
                        (counter_name.as_ref(), true)
                    }
                    _ => return ValueSupport::Invalid,
                }
            }
            _ => return ValueSupport::Invalid,
        };
        if !valid_counter_name(counter_name) {
            return ValueSupport::Invalid;
        }
        cursor += 1;
        let mut item_support = if unsupported_name_form {
            ValueSupport::Unsupported
        } else {
            ValueSupport::Supported
        };
        if matches!(
            tokens.get(cursor),
            Some(TokenOrValue::Token(Token::Number {
                int_value: Some(_),
                ..
            }))
        ) {
            cursor += 1;
        } else if matches!(
            tokens.get(cursor),
            Some(TokenOrValue::Function(function))
                if function.name.as_ref().eq_ignore_ascii_case("calc")
        ) {
            cursor += 1;
            item_support = ValueSupport::Unsupported;
        }
        if item_support == ValueSupport::Unsupported {
            support = ValueSupport::Unsupported;
        }
    }
    support
}

pub(crate) fn quotes(value: &str) -> ValueSupport {
    let Some(tokens) = parsed_tokens("quotes", value) else {
        return ValueSupport::Invalid;
    };
    let tokens = significant(&tokens);
    if tokens.len() == 1
        && ident(tokens[0])
            .is_some_and(|value| css_wide(value) || equals_any(value, &["auto", "none"]))
    {
        return ValueSupport::Supported;
    }
    if !tokens.is_empty()
        && tokens.len().is_multiple_of(2)
        && tokens
            .iter()
            .all(|token| matches!(token, TokenOrValue::Token(Token::String(_))))
    {
        ValueSupport::Supported
    } else {
        ValueSupport::Invalid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_renderer_supported_content_forms() {
        for value in [
            "none",
            "'prefix' attr(title)",
            "counter(chapter, upper-roman)",
            "counters(section, '.')",
            "open-quote 'hello' close-quote",
        ] {
            assert_eq!(content(value), ValueSupport::Supported, "{value}");
        }
    }

    #[test]
    fn separates_unsupported_and_invalid_content_forms() {
        for value in [
            "url(icon.png)",
            "'label' / 'alternative'",
            "\"\" / counter(chapter)",
            "attr(title string)",
            "-webkit-image-set(url(icon.png) 1x)",
            "leader(dotted)",
        ] {
            assert_eq!(content(value), ValueSupport::Unsupported, "{value}");
        }
        for value in [
            "",
            "/ 'alternative'",
            "open-quote none",
            "counter()",
            "'label' 12px",
        ] {
            assert_eq!(content(value), ValueSupport::Invalid, "{value}");
        }
    }

    #[test]
    fn validates_related_generated_content_properties() {
        assert_eq!(
            counter_directive("counter-reset", "chapter 1 section"),
            ValueSupport::Supported
        );
        assert_eq!(
            counter_directive("counter-increment", "none extra"),
            ValueSupport::Invalid
        );
        assert_eq!(
            counter_directive("counter-reset", "reversed(chapter)"),
            ValueSupport::Unsupported
        );
        assert_eq!(
            counter_directive("counter-increment", "reversed(chapter)"),
            ValueSupport::Invalid
        );
        assert_eq!(
            counter_directive("counter-increment", "chapter calc(1)"),
            ValueSupport::Unsupported
        );
        assert_eq!(
            counter_directive("counter-reset", "default 1"),
            ValueSupport::Invalid
        );
        assert_eq!(quotes("'«' '»' '‹' '›'"), ValueSupport::Supported);
        assert_eq!(quotes("'open'"), ValueSupport::Invalid);
    }
}
