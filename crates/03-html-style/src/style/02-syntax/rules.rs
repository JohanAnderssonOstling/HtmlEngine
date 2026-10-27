//! Rule grammar checks beyond the dependency's recovery representation.

use crate::style::source::imports;
use cssparser::{ParseError, Parser, ParserInput, Token};
use lightningcss::rules::CssRule;
use lightningcss::rules::page::PageMarginBox;
use lightningcss::stylesheet::{ParserFlags, ParserOptions, PrinterOptions, StyleSheet};
use lightningcss::traits::{Parse, ToCss};
use lightningcss::visitor::{Visit, VisitTypes, Visitor};
use std::ops::Range;

pub(crate) fn stylesheet_is_valid(css: &str) -> bool {
    let Some(css) = crate::style::source::declarations::normalize_rule_preludes(css) else {
        return false;
    };
    let normalized = imports::normalize_layers(&css);
    match StyleSheet::parse(&normalized, ParserOptions::default()) {
        Ok(mut stylesheet) => stylesheet.visit(&mut RuleValidator::default()).is_ok(),
        Err(_) => position_try_bodies_are_recoverable(&normalized),
    }
}

#[derive(Default)]
struct RuleValidator {
    position_try_locations: Vec<(u32, u32)>,
}

fn position_try_bodies_are_recoverable(css: &str) -> bool {
    let Ok(mut stylesheet) = StyleSheet::parse(
        css,
        ParserOptions {
            error_recovery: true,
            ..ParserOptions::default()
        },
    ) else {
        return false;
    };
    let mut validator = RuleValidator::default();
    if stylesheet.visit(&mut validator).is_err() || validator.position_try_locations.is_empty() {
        return false;
    }

    // Invalid descriptors and nested rules do not invalidate @position-try.
    // Its typed AST identifies real rules, excluding lookalikes in custom
    // properties. Recheck their original headers and everything outside their
    // bodies strictly. This source is only used for syntax validation; the
    // cascade's recovery parser retains valid descriptors in the real input.
    let mut input = ParserInput::new(css);
    let mut bodies = Vec::new();
    if position_try_bodies(
        &mut Parser::new(&mut input),
        &validator.position_try_locations,
        &mut bodies,
        0,
    )
    .is_err()
        || bodies.is_empty()
    {
        return false;
    }
    let mut headers = String::with_capacity(css.len());
    let mut copied = 0;
    for body in bodies {
        headers.push_str(&css[copied..body.start]);
        copied = body.end;
    }
    headers.push_str(&css[copied..]);
    StyleSheet::parse(&headers, ParserOptions::default())
        .is_ok_and(|mut sheet| sheet.visit(&mut RuleValidator::default()).is_ok())
}

fn position_try_bodies<'i>(
    parser: &mut Parser<'i, '_>,
    locations: &[(u32, u32)],
    bodies: &mut Vec<Range<usize>>,
    depth: usize,
) -> Result<(), ParseError<'i, ()>> {
    if depth > 128 {
        return Err(parser.new_custom_error(()));
    }
    let mut position_try = false;
    loop {
        parser.skip_whitespace();
        let location = parser.current_source_location();
        let Ok(token) = parser.next().cloned() else {
            return Ok(());
        };
        match token {
            Token::AtKeyword(_) => {
                position_try = locations.contains(&(location.line, location.column));
            }
            Token::CurlyBracketBlock => {
                parser.parse_nested_block(|nested| {
                    if position_try {
                        let start = nested.position().byte_index();
                        while nested.next().is_ok() {}
                        bodies.push(start..nested.position().byte_index());
                        Ok(())
                    } else {
                        position_try_bodies(nested, locations, bodies, depth + 1)
                    }
                })?;
                position_try = false;
            }
            Token::Semicolon => position_try = false,
            _ => {}
        }
    }
}

impl<'i> Visitor<'i> for RuleValidator {
    type Error = ();

    fn visit_types(&self) -> VisitTypes {
        lightningcss::visit_types!(RULES)
    }

    fn visit_rule(&mut self, rule: &mut CssRule<'i>) -> Result<(), ()> {
        if let CssRule::PositionTry(rule) = rule {
            self.position_try_locations
                .push((rule.loc.line, rule.loc.column));
        }
        // The page parser owns valid margin boxes in its PageMarginRule list.
        // Elsewhere the dependency retains these names as unknown at-rules.
        if let CssRule::Unknown(rule) = rule {
            if rule.name.eq_ignore_ascii_case("custom-media") {
                let css = rule
                    .to_css_string(PrinterOptions::default())
                    .map_err(|_| ())?;
                let parsed = StyleSheet::parse(
                    &css,
                    ParserOptions {
                        flags: ParserFlags::CUSTOM_MEDIA,
                        ..ParserOptions::default()
                    },
                )
                .map_err(|_| ())?;
                if !matches!(parsed.rules.0.as_slice(), [CssRule::CustomMedia(_)]) {
                    return Err(());
                }
            }
            if PageMarginBox::parse_string(rule.name.as_ref()).is_ok() {
                return Err(());
            }
        }
        rule.visit_children(self)
    }
}
