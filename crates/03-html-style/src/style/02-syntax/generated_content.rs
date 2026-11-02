//! Shared consumers of native generated-content tokens.
use super::value::{significant_tokens, token_ident, token_list_to_css_string};
use lightningcss::properties::Property;
use lightningcss::properties::custom::{Function, Token, TokenList, TokenOrValue};
use lightningcss::properties::list::CounterStyle;
use lightningcss::traits::Parse;

fn counter_ident_is_valid(name: &str) -> bool {
    ![
        "initial",
        "inherit",
        "unset",
        "default",
        "revert",
        "revert-layer",
        "none",
    ]
    .iter()
    .any(|keyword| name.eq_ignore_ascii_case(keyword))
}

pub(crate) struct CounterArguments<'a, 'css> {
    pub name: &'a str,
    pub separator: Option<&'a str>,
    pub style: Option<&'a TokenOrValue<'css>>,
}

/// Shared by syntax checks and renderer conversion, without interning strings.
pub(crate) fn counter_arguments<'a, 'css>(
    function: &'a Function<'css>,
) -> Option<CounterArguments<'a, 'css>> {
    let mut arguments = significant_tokens(&function.arguments);
    let name = token_ident(arguments.next()?)?;
    if !counter_ident_is_valid(name) {
        return None;
    }
    let separator = if function.name.as_ref().eq_ignore_ascii_case("counters") {
        let (
            Some(TokenOrValue::Token(Token::Comma)),
            Some(TokenOrValue::Token(Token::String(separator))),
        ) = (arguments.next(), arguments.next())
        else {
            return None;
        };
        Some(separator.as_ref())
    } else {
        None
    };
    let style = match arguments.next() {
        None => None,
        Some(TokenOrValue::Token(Token::Comma)) => Some(arguments.next()?),
        _ => return None,
    };
    if arguments.next().is_some() {
        return None;
    }
    if let Some(style) = style {
        let valid = if let Some(name) = token_ident(style) {
            counter_ident_is_valid(name)
        } else if matches!(style, TokenOrValue::Function(_)) {
            // Keep custom styles and symbols() distinct from renderer support.
            let tokens = TokenList(vec![style.clone()]);
            token_list_to_css_string(&tokens)
                .is_some_and(|css| CounterStyle::parse_string(&css).is_ok())
        } else {
            false
        };
        if !valid {
            return None;
        }
    }
    Some(CounterArguments {
        name,
        separator,
        style,
    })
}

/// Existing counter conversion, shared with syntax checking for literal lists.
pub(crate) fn for_each_counter_directive(
    tokens: &TokenList<'_>,
    default_value: i32,
    mut emit: impl FnMut(&str, i32),
) -> Option<()> {
    let mut tokens = significant_tokens(tokens);
    let mut found = false;
    while let Some(token) = tokens.next() {
        let name = token_ident(token)?;
        if name.eq_ignore_ascii_case("none") {
            return (!found && tokens.next().is_none()).then_some(());
        }
        if !counter_ident_is_valid(name) {
            return None;
        }
        let value = match tokens.clone().next() {
            Some(TokenOrValue::Token(Token::Number {
                int_value: Some(value),
                ..
            })) => {
                tokens.next();
                *value
            }
            _ => default_value,
        };
        emit(name, value);
        found = true;
    }
    found.then_some(())
}

/// Check retained constraints, without treating unsupported rendering as invalid
/// CSS. Deferred substitutions and counter expressions keep their native tokens.
pub(crate) fn native_value_is_valid(property: &Property<'_>) -> bool {
    let Property::Custom(raw) = property else {
        return true;
    };
    let content = raw.name.as_ref().eq_ignore_ascii_case("content");
    if !content
        && !raw.name.as_ref().eq_ignore_ascii_case("counter-reset")
        && !raw.name.as_ref().eq_ignore_ascii_case("counter-increment")
    {
        return true;
    }
    let tokens = &raw.value;
    if super::wide_keywords::token_list_css_wide_keyword(tokens).is_some()
        || super::substitution::needs_substitution(tokens)
    {
        return true;
    }
    if content {
        return significant_tokens(tokens).all(|token| match token {
            TokenOrValue::Function(function)
                if function.name.as_ref().eq_ignore_ascii_case("counter")
                    || function.name.as_ref().eq_ignore_ascii_case("counters") =>
            {
                counter_arguments(function).is_some()
            }
            TokenOrValue::Function(function)
                if function.name.as_ref().eq_ignore_ascii_case("attr") =>
            {
                significant_tokens(&function.arguments).next().is_some()
            }
            _ => true,
        });
    }
    // The renderer's literal-list conversion does not implement reversed() or
    // computed integer expressions. Do not misclassify those valid forms here.
    if tokens
        .0
        .iter()
        .any(|token| matches!(token, TokenOrValue::Function(_)))
    {
        return true;
    }
    for_each_counter_directive(tokens, 0, |_, _| {}).is_some()
}
