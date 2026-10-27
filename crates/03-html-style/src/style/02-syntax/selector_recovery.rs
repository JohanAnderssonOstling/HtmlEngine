//! Preserve selector errors before the native forgiving parser discards them.

use super::scope;
use cssparser::{ParseError, Parser, ParserInput, Token};
use std::borrow::Cow;
use std::ops::Range;

struct Edit {
    range: Range<usize>,
}

/// Invalid `:has()` lists become an invalid selector, so enclosing `:is()` and
/// `:where()` can discard that member through the native parser's own recovery.
/// Bad component-value tokens invalidate the complete selector instead.
pub(crate) fn normalize(source: &str) -> Option<Cow<'_, str>> {
    if !source.contains('(') {
        return Some(Cow::Borrowed(source));
    }
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    let mut edits = Vec::new();
    scan(&mut parser, source, &mut edits, 0, false).ok()?;
    Some(project(source, 0..source.len(), &edits))
}

fn project<'a>(source: &'a str, range: Range<usize>, edits: &[Edit]) -> Cow<'a, str> {
    if edits.is_empty() {
        return Cow::Borrowed(&source[range]);
    }
    let mut output = String::new();
    let mut copied = range.start;
    for edit in edits {
        output.push_str(&source[copied..edit.range.start]);
        output.push_str("not()");
        copied = edit.range.end;
    }
    output.push_str(&source[copied..range.end]);
    Cow::Owned(output)
}

fn scan<'i>(
    parser: &mut Parser<'i, '_>,
    source: &str,
    edits: &mut Vec<Edit>,
    depth: usize,
    inside_has: bool,
) -> Result<(), ParseError<'i, ()>> {
    if depth >= 128 {
        return Err(parser.new_custom_error(()));
    }
    let mut colon = false;
    while !parser.is_exhausted() {
        let start = parser.position().byte_index();
        let token = parser.next_including_whitespace_and_comments()?.clone();
        match token {
            Token::BadString(_)
            | Token::BadUrl(_)
            | Token::CloseParenthesis
            | Token::CloseSquareBracket
            | Token::CloseCurlyBracket => return Err(parser.new_custom_error(())),
            Token::Comment(_) => continue,
            Token::Function(ref name) => {
                let has = colon && name.eq_ignore_ascii_case("has");
                let first_edit = edits.len();
                let valid = parser.parse_nested_block(|nested| {
                    let content_start = nested.position().byte_index();
                    scan(nested, source, edits, depth + 1, inside_has || has)?;
                    let content_end = nested.position().byte_index();
                    Ok(!has
                        || (!inside_has
                            && scope::relative_list_is_valid(&project(
                                source,
                                content_start..content_end,
                                &edits[first_edit..],
                            ))))
                })?;
                if !valid {
                    edits.truncate(first_edit);
                    edits.push(Edit {
                        range: start..parser.position().byte_index(),
                    });
                }
            }
            Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
                parser.parse_nested_block(|nested| {
                    scan(nested, source, edits, depth + 1, inside_has)
                })?;
            }
            _ => {}
        }
        colon = matches!(token, Token::Colon);
    }
    Ok(())
}
