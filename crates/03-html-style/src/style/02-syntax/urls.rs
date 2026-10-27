//! URL token grammar. Request modifiers are validated without fetching.

use cssparser::{ParseError, Parser, Token};
use std::collections::HashSet;

pub(crate) fn parse<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    match parser.next()?.clone() {
        Token::UnquotedUrl(_) => Ok(()),
        Token::Function(name)
            if name.eq_ignore_ascii_case("url") || name.eq_ignore_ascii_case("src") =>
        {
            parser.parse_nested_block(|parser| {
                parser.expect_string()?;
                let mut modifiers = HashSet::new();
                while !parser.is_exhausted() {
                    let (function, name) = match parser.next()?.clone() {
                        Token::Ident(name) => (false, name),
                        Token::Function(name) => (true, name),
                        _ => return Err(parser.new_custom_error(())),
                    };
                    if !modifiers.insert((function, name.to_ascii_lowercase())) {
                        return Err(parser.new_custom_error(()));
                    }
                    if function {
                        parser.parse_nested_block(|parser| {
                            if name.eq_ignore_ascii_case("cross-origin") {
                                let value = parser.expect_ident()?;
                                if !value.eq_ignore_ascii_case("anonymous")
                                    && !value.eq_ignore_ascii_case("use-credentials")
                                {
                                    return Err(parser.new_custom_error(()));
                                }
                            } else if name.eq_ignore_ascii_case("integrity") {
                                parser.expect_string()?;
                            } else if name.eq_ignore_ascii_case("referrer-policy") {
                                let value = parser.expect_ident()?;
                                if ![
                                    "no-referrer",
                                    "no-referrer-when-downgrade",
                                    "same-origin",
                                    "origin",
                                    "strict-origin",
                                    "origin-when-cross-origin",
                                    "strict-origin-when-cross-origin",
                                    "unsafe-url",
                                ]
                                .iter()
                                .any(|name| value.eq_ignore_ascii_case(name))
                                {
                                    return Err(parser.new_custom_error(()));
                                }
                            } else {
                                // Unknown functions are opaque modifier payloads.
                                // They may contain declaration-looking tokens.
                                consume(parser, 0, false)?;
                            }
                            Ok(())
                        })?;
                    }
                }
                Ok(())
            })
        }
        _ => Err(parser.new_custom_error(())),
    }
}

/// Consume balanced component values, optionally enforcing declaration-value
/// restrictions at the top level. Return the number of significant tokens.
pub(crate) fn consume<'i>(
    parser: &mut Parser<'i, '_>,
    depth: usize,
    declaration: bool,
) -> Result<usize, ParseError<'i, ()>> {
    if depth >= 128 {
        return Err(parser.new_custom_error(()));
    }
    let mut count = 0;
    while !parser.is_exhausted() {
        match parser.next()? {
            Token::BadString(_)
            | Token::BadUrl(_)
            | Token::CloseParenthesis
            | Token::CloseSquareBracket
            | Token::CloseCurlyBracket => return Err(parser.new_custom_error(())),
            Token::Semicolon | Token::Delim('!') if declaration => {
                return Err(parser.new_custom_error(()));
            }
            Token::Function(_)
            | Token::ParenthesisBlock
            | Token::SquareBracketBlock
            | Token::CurlyBracketBlock => {
                parser
                    .parse_nested_block(|nested| consume(nested, depth + 1, false).map(|_| ()))?;
            }
            _ => {}
        }
        count += 1;
    }
    Ok(count)
}
