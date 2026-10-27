//! Balanced component values shared by token-preserving preprocessing.

use cssparser::{ParseError, Parser, Token};

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
