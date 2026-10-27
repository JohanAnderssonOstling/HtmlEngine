//! Token-preserving replacement of contextual numeric functions.

use cssparser::{ParseError, Parser, ParserInput, Token};
use lightningcss::traits::Parse;
use std::borrow::Cow;
use std::ops::Range;

struct Edit {
    range: Range<usize>,
    replacement: String,
}

/// Bind contextual signs and admit non-finite arithmetic for grammar checks.
/// Only the validity result escapes; these stand-ins cannot compute a value.
pub(crate) fn syntax_value_is_valid(source: &str, validate: impl FnOnce(&str) -> bool) -> bool {
    let Ok(source) = nonzero_divisors(source) else {
        return false;
    };
    let Ok(source) = super::sign::lower(&source, |_| Some(1.0)) else {
        return false;
    };
    let Ok(source) = nonzero_divisors(&source) else {
        return false;
    };
    validate(&source)
}

/// Preserve math types while bypassing the native parser's obsolete rejection
/// of numeric zero divisors. These stand-ins are for syntax validation only.
pub(crate) fn nonzero_divisors(source: &str) -> Result<Cow<'_, str>, ()> {
    if !source.contains('/') {
        return Ok(Cow::Borrowed(source));
    }
    replace(source, |name, argument| {
        if !matches!(
            name.to_ascii_lowercase().as_str(),
            "calc"
                | "min"
                | "max"
                | "clamp"
                | "round"
                | "mod"
                | "rem"
                | "abs"
                | "sign"
                | "hypot"
                | "sin"
                | "cos"
                | "tan"
                | "asin"
                | "acos"
                | "atan"
                | "atan2"
                | "pow"
                | "sqrt"
                | "exp"
                | "log"
        ) {
            return Ok(None);
        }
        let mut input = ParserInput::new(argument);
        let mut edits = Vec::new();
        scan_zero_divisors(&mut Parser::new(&mut input), argument, &mut edits, 0)
            .map_err(|_| ())?;
        Ok((!edits.is_empty()).then(|| format!("{name}({})", apply_edits(argument, 0, &edits))))
    })
}

fn scan_zero_divisors<'i>(
    parser: &mut Parser<'i, '_>,
    source: &str,
    edits: &mut Vec<Edit>,
    depth: usize,
) -> Result<(), ParseError<'i, ()>> {
    if depth >= 128 {
        return Err(parser.new_custom_error(()));
    }
    while let Ok(token) = parser.next().cloned() {
        if matches!(token, Token::Delim('/')) {
            let start = parser.position().byte_index();
            let first_edit = edits.len();
            let token = parser.next()?.clone();
            if matches!(token, Token::Function(_) | Token::ParenthesisBlock) {
                parser.parse_nested_block(|nested| {
                    scan_zero_divisors(nested, source, edits, depth + 1)
                })?;
            }
            let end = parser.position().byte_index();
            let operand = apply_edits(&source[start..end], start, &edits[first_edit..]);
            if f32::parse_string(&format!("calc({operand})")) == Ok(0.0) {
                edits.truncate(first_edit);
                edits.push(Edit {
                    range: start..end,
                    // Parentheses retain operand boundaries: replacing (0)
                    // with a bare 1 must not turn invalid (0)2 into valid 12.
                    replacement: "(1)".to_owned(),
                });
            }
        } else if matches!(token, Token::Function(_) | Token::ParenthesisBlock) {
            parser.parse_nested_block(|nested| {
                scan_zero_divisors(nested, source, edits, depth + 1)
            })?;
        }
    }
    Ok(())
}

pub(crate) fn replace<'a>(
    source: &'a str,
    mut resolve: impl FnMut(&str, &str) -> Result<Option<String>, ()>,
) -> Result<Cow<'a, str>, ()> {
    if !source.contains('(') {
        return Ok(Cow::Borrowed(source));
    }
    replace_tokens(source, &mut resolve)
}

fn replace_tokens<'a>(
    source: &'a str,
    resolve: &mut impl FnMut(&str, &str) -> Result<Option<String>, ()>,
) -> Result<Cow<'a, str>, ()> {
    let mut input = ParserInput::new(source);
    let mut edits = Vec::new();
    walk(&mut Parser::new(&mut input), source, &mut edits, resolve, 0)?;
    if edits.is_empty() {
        Ok(Cow::Borrowed(source))
    } else {
        Ok(Cow::Owned(apply_edits(source, 0, &edits)))
    }
}

fn walk(
    parser: &mut Parser<'_, '_>,
    source: &str,
    edits: &mut Vec<Edit>,
    resolve: &mut impl FnMut(&str, &str) -> Result<Option<String>, ()>,
    depth: usize,
) -> Result<(), ()> {
    if depth >= 128 {
        return Err(());
    }
    loop {
        let start = parser.position().byte_index();
        let name = match parser.next_including_whitespace_and_comments() {
            Ok(Token::Function(name)) => Some(name.clone()),
            Ok(Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock) => {
                None
            }
            Ok(_) => continue,
            Err(_) => return Ok(()),
        };
        let first_edit = edits.len();
        let arguments = parser
            .parse_nested_block(|nested| -> Result<_, ParseError<'_, ()>> {
                let start = nested.position().byte_index();
                walk(nested, source, edits, resolve, depth + 1)
                    .map_err(|()| nested.new_custom_error(()))?;
                Ok(start..nested.position().byte_index())
            })
            .map_err(|_| ())?;
        let Some(name) = name else { continue };
        let argument = apply_edits(
            &source[arguments.clone()],
            arguments.start,
            &edits[first_edit..],
        );
        if let Some(replacement) = resolve(&name, &argument)? {
            edits.truncate(first_edit);
            edits.push(Edit {
                range: start..parser.position().byte_index(),
                replacement,
            });
        }
    }
}

fn apply_edits(source: &str, offset: usize, edits: &[Edit]) -> String {
    let mut result = String::with_capacity(source.len());
    let mut copied = 0;
    for edit in edits {
        let start = edit.range.start - offset;
        let end = edit.range.end - offset;
        result.push_str(&source[copied..start]);
        result.push_str(&edit.replacement);
        copied = end;
    }
    result.push_str(&source[copied..]);
    result
}
