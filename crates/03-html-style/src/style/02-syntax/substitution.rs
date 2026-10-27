//! Argument grammar shared by arbitrary-value validation and substitution.

use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};

pub(crate) fn is_inherit(name: &str) -> bool {
    name.eq_ignore_ascii_case("inherit")
}

pub(crate) fn is_random_item(name: &str) -> bool {
    name.eq_ignore_ascii_case("random-item")
}

pub(crate) fn is_random_math(name: &str) -> bool {
    name.eq_ignore_ascii_case("random")
}

pub(crate) fn is_custom_function(name: &str) -> bool {
    name.starts_with("--")
}

pub(crate) fn is_lazy_function(name: &str) -> bool {
    is_inherit(name) || is_random_item(name) || is_custom_function(name)
}

fn ignorable(token: &TokenOrValue<'_>) -> bool {
    matches!(
        token,
        TokenOrValue::Token(Token::WhiteSpace(_) | Token::Comment(_))
    )
}

fn trim<'a, 'css>(mut tokens: &'a [TokenOrValue<'css>]) -> &'a [TokenOrValue<'css>] {
    while tokens.first().is_some_and(ignorable) {
        tokens = &tokens[1..];
    }
    while tokens.last().is_some_and(ignorable) {
        tokens = &tokens[..tokens.len() - 1];
    }
    tokens
}

fn unwrapped<'a, 'css>(tokens: &'a [TokenOrValue<'css>]) -> Result<&'a [TokenOrValue<'css>], ()> {
    let tokens = trim(tokens);
    if !matches!(
        tokens.first(),
        Some(TokenOrValue::Token(Token::CurlyBracketBlock))
    ) {
        declaration_value(tokens, false)?;
        return Ok(tokens);
    }
    let mut depth = 0;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            TokenOrValue::Token(Token::CurlyBracketBlock) => depth += 1,
            TokenOrValue::Token(Token::CloseCurlyBracket) => {
                depth -= 1;
                if depth == 0 {
                    return if index + 1 == tokens.len() {
                        let contents = &tokens[1..index];
                        declaration_value(contents, true)?;
                        Ok(contents)
                    } else {
                        Err(())
                    };
                }
            }
            _ => {}
        }
    }
    Err(())
}

fn declaration_value(tokens: &[TokenOrValue<'_>], wrapped: bool) -> Result<(), ()> {
    let mut depth = 0;
    for token in tokens {
        match token {
            TokenOrValue::Token(Token::Semicolon | Token::Delim('!')) if depth == 0 => {
                return Err(());
            }
            TokenOrValue::Token(Token::CurlyBracketBlock) if depth == 0 && !wrapped => {
                return Err(());
            }
            TokenOrValue::Token(
                Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock,
            ) => depth += 1,
            TokenOrValue::Token(
                Token::CloseParenthesis | Token::CloseSquareBracket | Token::CloseCurlyBracket,
            ) => depth -= 1,
            _ => {}
        }
    }
    Ok(())
}

pub(crate) type Arguments<'a, 'css> = (&'a [TokenOrValue<'css>], Option<&'a [TokenOrValue<'css>]>);

fn raw_arguments<'a, 'css>(
    tokens: &'a TokenList<'css>,
) -> Result<Vec<&'a [TokenOrValue<'css>]>, ()> {
    let mut depth = 0;
    let mut start = 0;
    let mut arguments = Vec::new();
    for (index, token) in tokens.0.iter().enumerate() {
        match token {
            TokenOrValue::Token(
                Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock,
            ) => depth += 1,
            TokenOrValue::Token(
                Token::CloseParenthesis | Token::CloseSquareBracket | Token::CloseCurlyBracket,
            ) => depth -= 1,
            TokenOrValue::Token(Token::Comma) if depth == 0 => {
                arguments.push(&tokens.0[start..index]);
                start = index + 1;
            }
            TokenOrValue::Token(Token::Semicolon | Token::Delim('!')) if depth == 0 => {
                return Err(());
            }
            _ => {}
        }
    }
    arguments.push(&tokens.0[start..]);
    Ok(arguments)
}

pub(crate) fn function_arguments<'a, 'css>(
    tokens: &'a TokenList<'css>,
) -> Result<Vec<&'a [TokenOrValue<'css>]>, ()> {
    raw_arguments(tokens)?.into_iter().map(unwrapped).collect()
}

pub(crate) fn custom_function_arguments<'a, 'css>(
    tokens: &'a TokenList<'css>,
) -> Result<Vec<&'a [TokenOrValue<'css>]>, ()> {
    if trim(&tokens.0).is_empty() {
        return Ok(Vec::new());
    }
    raw_arguments(tokens)?
        .into_iter()
        .map(|raw| {
            let raw = trim(raw);
            let argument = unwrapped(raw)?;
            if trim(argument).is_empty() {
                return Err(());
            }
            // Leading --name: is reserved for named arguments. A brace wrapper
            // explicitly makes it an ordinary declaration-value argument.
            if !matches!(
                raw.first(),
                Some(TokenOrValue::Token(Token::CurlyBracketBlock))
            ) {
                let mut significant = raw.iter().filter(|token| !ignorable(token));
                let dashed = match significant.next() {
                    Some(TokenOrValue::DashedIdent(_)) => true,
                    Some(TokenOrValue::Token(Token::Ident(name))) => name.starts_with("--"),
                    _ => false,
                };
                if dashed && matches!(significant.next(), Some(TokenOrValue::Token(Token::Colon))) {
                    return Err(());
                }
            }
            Ok(argument)
        })
        .collect()
}

pub(crate) fn arguments<'a, 'css>(tokens: &'a TokenList<'css>) -> Result<Arguments<'a, 'css>, ()> {
    let arguments = function_arguments(tokens)?;
    match arguments.as_slice() {
        [name] => Ok((name, None)),
        [name, fallback] => Ok((name, Some(fallback))),
        _ => Err(()),
    }
}

pub(crate) fn random_item_arguments<'a, 'css>(
    tokens: &'a TokenList<'css>,
) -> Result<Vec<&'a [TokenOrValue<'css>]>, ()> {
    let arguments = function_arguments(tokens)?;
    if arguments.len() < 2 || trim(arguments[0]).is_empty() {
        return Err(());
    }
    Ok(arguments)
}

pub(crate) fn custom_name<'a>(tokens: &'a [TokenOrValue<'_>]) -> Option<&'a str> {
    match trim(tokens) {
        [TokenOrValue::DashedIdent(name)] => Some(name.as_ref()),
        [TokenOrValue::Token(Token::Ident(name))] if name.starts_with("--") => Some(name.as_ref()),
        _ => None,
    }
}

pub(crate) fn needs_substitution(tokens: &TokenList<'_>) -> bool {
    tokens.0.iter().any(|token| match token {
        TokenOrValue::Var(_) => true,
        TokenOrValue::Function(function) => {
            is_lazy_function(function.name.as_ref()) || needs_substitution(&function.arguments)
        }
        _ => false,
    })
}

pub(crate) fn contains_lazy_function(tokens: &TokenList<'_>) -> bool {
    contains_function(tokens, is_lazy_function)
}

pub(crate) fn contains_random_item(tokens: &TokenList<'_>) -> bool {
    contains_function(tokens, is_random_item)
}

pub(crate) fn contains_random_math(tokens: &TokenList<'_>) -> bool {
    contains_function(tokens, is_random_math)
}

pub(crate) fn contains_function(tokens: &TokenList<'_>, predicate: fn(&str) -> bool) -> bool {
    tokens.0.iter().any(|token| match token {
        TokenOrValue::Function(function) => {
            predicate(function.name.as_ref()) || contains_function(&function.arguments, predicate)
        }
        TokenOrValue::Var(variable) => variable
            .fallback
            .as_ref()
            .is_some_and(|tokens| contains_function(tokens, predicate)),
        TokenOrValue::Env(environment) => environment
            .fallback
            .as_ref()
            .is_some_and(|tokens| contains_function(tokens, predicate)),
        _ => false,
    })
}

pub(crate) fn substitutions(tokens: &[TokenOrValue<'_>]) -> Result<bool, ()> {
    let mut found = false;
    for token in tokens {
        match token {
            TokenOrValue::Var(variable) => {
                if let Some(fallback) = &variable.fallback {
                    substitutions(&fallback.0)?;
                }
                found = true;
            }
            TokenOrValue::Env(environment) => {
                if let Some(fallback) = &environment.fallback {
                    substitutions(&fallback.0)?;
                }
                found = true;
            }
            TokenOrValue::Function(function) => {
                if is_custom_function(function.name.as_ref()) {
                    for argument in custom_function_arguments(&function.arguments)? {
                        substitutions(argument)?;
                    }
                    found = true;
                } else if is_random_item(function.name.as_ref()) {
                    for argument in random_item_arguments(&function.arguments)? {
                        substitutions(argument)?;
                    }
                    found = true;
                } else if is_inherit(function.name.as_ref()) {
                    let (name, fallback) = arguments(&function.arguments)?;
                    let name_substitutions = substitutions(name)?;
                    if custom_name(name).is_none() && !name_substitutions {
                        return Err(());
                    }
                    if let Some(fallback) = fallback {
                        substitutions(fallback)?;
                    }
                    found = true;
                } else {
                    found |= substitutions(&function.arguments.0)?;
                }
            }
            _ => {}
        }
    }
    Ok(found)
}
