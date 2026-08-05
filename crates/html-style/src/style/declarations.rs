//! Compatibility preprocessing at the declaration boundary.
//!
//! This module owns the lexical walk over complete stylesheets and inline
//! declaration lists. Property modules own only their individual syntax and
//! lowering rules.

use super::{box_syntax, tab_size, text_spacing, white_space};

pub(crate) fn normalize(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut copied_until = 0usize;
    let mut offset = 0usize;
    let mut declaration_boundary = true;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;

    while offset < input.len() {
        if input[offset..].starts_with("/*") {
            offset = comment_end(input, offset);
            continue;
        }
        let ch = input[offset..].chars().next().expect("offset is in bounds");
        if matches!(ch, '\'' | '"') {
            offset = quoted_end(input, offset, ch);
            declaration_boundary = false;
            continue;
        }
        match ch {
            '(' => {
                paren_depth += 1;
                declaration_boundary = false;
                offset += 1;
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                offset += 1;
            }
            '[' => {
                bracket_depth += 1;
                declaration_boundary = false;
                offset += 1;
            }
            ']' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                offset += 1;
            }
            '{' | ';' | '}' if paren_depth == 0 && bracket_depth == 0 => {
                declaration_boundary = true;
                offset += 1;
            }
            _ if declaration_boundary && paren_depth == 0 && bracket_depth == 0 && is_ident_start(ch) => {
                let name_start = offset;
                offset = ident_end(input, offset);
                let normalized_name = input[name_start..offset].to_ascii_lowercase();
                let colon = skip_space_and_comments(input, offset);
                if input.as_bytes().get(colon) != Some(&b':') {
                    declaration_boundary = false;
                    continue;
                }
                let value_start = colon + 1;
                let Some((value_end, terminator)) = declaration_value_end(input, value_start) else {
                    declaration_boundary = false;
                    continue;
                };
                if terminator == Some('{') {
                    declaration_boundary = false;
                    continue;
                }
                let raw_value = &input[value_start..value_end];
                let replacement = match normalized_name.as_str() {
                    "white-space" => white_space::normalized_declaration(raw_value),
                    "letter-spacing" | "word-spacing" => text_spacing::normalized_declaration(&normalized_name, raw_value),
                    "tab-size" => tab_size::normalized_declaration(raw_value),
                    _ => None,
                }
                .or_else(|| matches!(box_syntax::property_value_is_valid(&normalized_name, raw_value), Some(false)).then(String::new));
                let Some(replacement) = replacement else {
                    declaration_boundary = false;
                    continue;
                };
                output.push_str(&input[copied_until..name_start]);
                output.push_str(&replacement);
                copied_until = value_end;
                offset = value_end;
                declaration_boundary = false;
            }
            _ => {
                if !ch.is_whitespace() {
                    declaration_boundary = false;
                }
                offset += ch.len_utf8();
            }
        }
    }

    if copied_until == 0 {
        return input.to_owned();
    }
    output.push_str(&input[copied_until..]);
    output
}

fn declaration_value_end(input: &str, mut offset: usize) -> Option<(usize, Option<char>)> {
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    while offset < input.len() {
        if input[offset..].starts_with("/*") {
            offset = comment_end(input, offset);
            continue;
        }
        let ch = input[offset..].chars().next()?;
        if matches!(ch, '\'' | '"') {
            offset = quoted_end(input, offset, ch);
            continue;
        }
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => return Some((offset, Some('{'))),
            '{' => brace_depth += 1,
            '}' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => return Some((offset, Some('}'))),
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ';' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => return Some((offset, Some(';'))),
            _ => {}
        }
        offset += ch.len_utf8();
    }
    Some((offset, None))
}

fn skip_space_and_comments(input: &str, mut offset: usize) -> usize {
    while offset < input.len() {
        if input[offset..].starts_with("/*") {
            offset = comment_end(input, offset);
            continue;
        }
        let ch = input[offset..].chars().next().expect("offset is in bounds");
        if !ch.is_whitespace() {
            break;
        }
        offset += ch.len_utf8();
    }
    offset
}

fn comment_end(input: &str, start: usize) -> usize {
    input[start + 2..].find("*/").map_or(input.len(), |relative| start + 2 + relative + 2)
}

fn quoted_end(input: &str, start: usize, quote: char) -> usize {
    let mut offset = start + quote.len_utf8();
    let mut escaped = false;
    while offset < input.len() {
        let ch = input[offset..].chars().next().expect("offset is in bounds");
        offset += ch.len_utf8();
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            break;
        }
    }
    offset
}

fn ident_end(input: &str, mut offset: usize) -> usize {
    while offset < input.len() {
        let ch = input[offset..].chars().next().expect("offset is in bounds");
        if !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')) {
            break;
        }
        offset += ch.len_utf8();
    }
    offset
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || matches!(ch, '-' | '_')
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn normalizes_declarations_but_not_selectors_or_supports_conditions() {
        let css = "white-space:hover { color: red } @supports (white-space: collapse) { p { white-space: preserve-breaks nowrap !important; } }";
        let normalized = normalize(css);
        assert!(normalized.contains("white-space:hover"));
        assert!(normalized.contains("@supports (white-space: collapse)"));
        assert!(normalized.contains("white-space: pre-line !important; --html-renderer-white-space: preserve-breaks-nowrap !important"));
    }
}
