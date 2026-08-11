use super::*;

/// Parse the CSS 2.1 generated-content forms that do not require counter or
/// replaced-image state. The outer `Option` distinguishes an invalid/unsupported
/// declaration from a valid suppressing value (`normal` or `none`).
pub(super) fn parse_generated_content(
    styles: &mut ComputedStylesBuilder,
    tokens: &TokenList<'_>,
) -> Option<Option<GeneratedContent>> {
    let significant: Vec<&TokenOrValue<'_>> = tokens
        .0
        .iter()
        .filter(|token| {
            !matches!(
                token,
                TokenOrValue::Token(Token::WhiteSpace(_) | Token::Comment(_))
            )
        })
        .collect();
    if significant.len() == 1
        && let TokenOrValue::Token(Token::Ident(keyword)) = significant[0]
    {
        return match keyword.as_ref().to_ascii_lowercase().as_str() {
            "normal" | "none" | "initial" | "unset" => Some(None),
            // `content` is not inherited; on a generated pseudo-element its
            // originating element normally computes to `normal`.
            "inherit" => Some(None),
            "revert" | "revert-layer" => None,
            _ => parse_generated_content_items(styles, &significant)
                .map(|items| Some(GeneratedContent { items })),
        };
    }
    parse_generated_content_items(styles, &significant)
        .map(|items| Some(GeneratedContent { items }))
}

pub(super) fn parse_generated_content_items(
    styles: &mut ComputedStylesBuilder,
    tokens: &[&TokenOrValue<'_>],
) -> Option<Vec<GeneratedContentItem>> {
    if tokens.is_empty() {
        return None;
    }
    let mut items = Vec::with_capacity(tokens.len());
    for token in tokens {
        let item = match token {
            TokenOrValue::Token(Token::String(value)) => {
                GeneratedContentItem::Text(styles.intern_string(value.as_ref()))
            }
            TokenOrValue::Token(Token::Ident(value)) => {
                match value.as_ref().to_ascii_lowercase().as_str() {
                    "open-quote" => GeneratedContentItem::OpenQuote,
                    "close-quote" => GeneratedContentItem::CloseQuote,
                    "no-open-quote" => GeneratedContentItem::NoOpenQuote,
                    "no-close-quote" => GeneratedContentItem::NoCloseQuote,
                    _ => return None,
                }
            }
            TokenOrValue::Function(function)
                if function.name.as_ref().eq_ignore_ascii_case("attr") =>
            {
                let name = function
                    .arguments
                    .0
                    .iter()
                    .find_map(|argument| match argument {
                        TokenOrValue::Token(Token::Ident(name)) => Some(name.as_ref()),
                        _ => None,
                    })?;
                GeneratedContentItem::Attribute(styles.intern_string(name))
            }
            TokenOrValue::Function(function)
                if function.name.as_ref().eq_ignore_ascii_case("counter") =>
            {
                let arguments = significant_tokens(&function.arguments);
                let name = token_ident(*arguments.first()?)?;
                let style = match arguments.as_slice() {
                    [_] => CounterStyle::Decimal,
                    [_, TokenOrValue::Token(Token::Comma), style] => {
                        parse_counter_style(token_ident(style)?)?
                    }
                    _ => return None,
                };
                GeneratedContentItem::Counter {
                    name: styles.intern_string(name),
                    style,
                }
            }
            TokenOrValue::Function(function)
                if function.name.as_ref().eq_ignore_ascii_case("counters") =>
            {
                let arguments = significant_tokens(&function.arguments);
                let (name, separator, counter_style) = match arguments.as_slice() {
                    [
                        name,
                        TokenOrValue::Token(Token::Comma),
                        TokenOrValue::Token(Token::String(separator)),
                    ] => (
                        token_ident(name)?,
                        separator.as_ref(),
                        CounterStyle::Decimal,
                    ),
                    [
                        name,
                        TokenOrValue::Token(Token::Comma),
                        TokenOrValue::Token(Token::String(separator)),
                        TokenOrValue::Token(Token::Comma),
                        style,
                    ] => (
                        token_ident(name)?,
                        separator.as_ref(),
                        parse_counter_style(token_ident(style)?)?,
                    ),
                    _ => return None,
                };
                GeneratedContentItem::Counters {
                    name: styles.intern_string(name),
                    separator: styles.intern_string(separator),
                    style: counter_style,
                }
            }
            // Generated replaced images require their own layout path and are
            // deliberately not misrepresented as text here.
            _ => return None,
        };
        items.push(item);
    }
    Some(items)
}

pub(super) fn significant_tokens<'a, 'i>(tokens: &'a TokenList<'i>) -> Vec<&'a TokenOrValue<'i>> {
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

pub(super) fn token_ident<'a, 'i>(token: &'a TokenOrValue<'i>) -> Option<&'a str> {
    match token {
        TokenOrValue::Token(Token::Ident(value)) => Some(value.as_ref()),
        _ => None,
    }
}

pub(super) fn parse_counter_style(value: &str) -> Option<CounterStyle> {
    match value.to_ascii_lowercase().as_str() {
        "decimal" => Some(CounterStyle::Decimal),
        "decimal-leading-zero" => Some(CounterStyle::DecimalLeadingZero),
        "lower-roman" => Some(CounterStyle::LowerRoman),
        "upper-roman" => Some(CounterStyle::UpperRoman),
        "lower-greek" => Some(CounterStyle::LowerGreek),
        "lower-alpha" | "lower-latin" => Some(CounterStyle::LowerAlpha),
        "upper-alpha" | "upper-latin" => Some(CounterStyle::UpperAlpha),
        "armenian" => Some(CounterStyle::Armenian),
        "georgian" => Some(CounterStyle::Georgian),
        "disc" => Some(CounterStyle::Disc),
        "circle" => Some(CounterStyle::Circle),
        "square" => Some(CounterStyle::Square),
        _ => None,
    }
}

pub(super) fn parse_counter_directives(
    styles: &mut ComputedStylesBuilder,
    tokens: &TokenList<'_>,
    default_value: i32,
) -> Option<Vec<CounterDirective>> {
    let tokens = significant_tokens(tokens);
    if tokens.len() == 1
        && token_ident(tokens[0]).is_some_and(|ident| ident.eq_ignore_ascii_case("none"))
    {
        return Some(Vec::new());
    }
    let mut directives = Vec::new();
    let mut cursor = 0;
    while cursor < tokens.len() {
        let name = token_ident(tokens[cursor])?;
        if name.eq_ignore_ascii_case("none")
            || name.eq_ignore_ascii_case("inherit")
            || name.eq_ignore_ascii_case("initial")
            || name.eq_ignore_ascii_case("unset")
        {
            return None;
        }
        cursor += 1;
        let value = match tokens.get(cursor) {
            Some(TokenOrValue::Token(Token::Number {
                int_value: Some(value),
                ..
            })) => {
                cursor += 1;
                value.to_owned()
            }
            _ => default_value,
        };
        directives.push(CounterDirective {
            name: styles.intern_string(name),
            value,
        });
    }
    (!directives.is_empty()).then_some(directives)
}

pub(super) fn parse_quotes(
    styles: &mut ComputedStylesBuilder,
    tokens: &TokenList<'_>,
) -> Option<QuoteStyle> {
    let tokens = significant_tokens(tokens);
    if tokens.len() == 1 {
        return match token_ident(tokens[0])?.to_ascii_lowercase().as_str() {
            "auto" => Some(QuoteStyle::Auto),
            "none" => Some(QuoteStyle::None),
            _ => None,
        };
    }
    if tokens.is_empty() || tokens.len() % 2 != 0 {
        return None;
    }
    let mut encoded = String::new();
    for (index, token) in tokens.iter().enumerate() {
        let TokenOrValue::Token(Token::String(value)) = token else {
            return None;
        };
        if index > 0 {
            encoded.push('\0');
        }
        encoded.push_str(value.as_ref());
    }
    Some(QuoteStyle::Pairs(styles.intern_string(&encoded)))
}
