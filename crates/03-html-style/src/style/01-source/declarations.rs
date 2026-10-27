//! Compatibility preprocessing at the declaration boundary.
//!
//! CSS rule/declaration iterators own boundaries, escapes, comments, and nested
//! blocks. Property modules own their individual syntax and lowering rules.

use crate::style::syntax::{box_syntax, color_syntax, flex, motion, scope, selector_recovery};
use crate::style::syntax::value::{normalize_math, normalize_property_value, without_priority};
use crate::style::syntax::values::{tab_size, text_spacing, white_space};
use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserInput, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser, Token,
};
use lightningcss::properties::PropertyId;
use lightningcss::vendor_prefix::VendorPrefix;
use std::borrow::Cow;
use std::ops::Range;

pub(crate) fn normalize(input: &str) -> String {
    normalize_source(input, false)
}

pub(crate) fn normalize_inline(input: &str) -> String {
    normalize_source(input, true)
}

/// Preserve original rule-prelude errors before native recovery discards them.
/// Declaration values remain untouched in this syntax-only path.
pub(crate) fn normalize_rule_preludes(input: &str) -> Option<Cow<'_, str>> {
    if !input.contains(['\\', ':'])
        && !input
            .as_bytes()
            .windows(5)
            .any(|part| part.eq_ignore_ascii_case(b"scope"))
        && !input
            .as_bytes()
            .windows(9)
            .any(|part| part.eq_ignore_ascii_case(b"keyframes"))
    {
        return Some(Cow::Borrowed(input));
    }
    let mut parser_input = ParserInput::new(input);
    let mut parser = Parser::new(&mut parser_input);
    let mut scanner = Scanner::new(input, false, true);
    for _ in StyleSheetParser::new(&mut parser, &mut scanner) {}
    scanner
        .valid_preludes
        .then(|| apply_edits(input, scanner.edits))
}

fn normalize_source(input: &str, inline: bool) -> String {
    let normalized_imports = super::imports::normalize_layers(input);
    let input = normalized_imports.as_ref();
    let mut parser_input = ParserInput::new(input);
    let mut parser = Parser::new(&mut parser_input);
    let mut scanner = Scanner::new(input, inline, false);
    if inline {
        for _ in RuleBodyParser::new(&mut parser, &mut scanner) {}
    } else {
        for _ in StyleSheetParser::new(&mut parser, &mut scanner) {}
    }
    apply_edits(input, scanner.edits).into_owned()
}

fn apply_edits(input: &str, edits: Vec<(Range<usize>, String)>) -> Cow<'_, str> {
    if edits.is_empty() {
        return Cow::Borrowed(input);
    }
    let mut output = String::with_capacity(input.len());
    let mut copied = 0;
    for (range, replacement) in edits {
        output.push_str(&input[copied..range.start]);
        output.push_str(&replacement);
        copied = range.end;
    }
    output.push_str(&input[copied..]);
    Cow::Owned(output)
}

struct Scanner<'i> {
    source: &'i str,
    edits: Vec<(Range<usize>, String)>,
    depth: usize,
    inline: bool,
    font_face: bool,
    syntax_only: bool,
    valid_preludes: bool,
    scope_context: bool,
    keyframes: bool,
}

fn consume(input: &mut Parser<'_, '_>) {
    while input.next().is_ok() {}
}

impl<'i> Scanner<'i> {
    fn new(source: &'i str, inline: bool, syntax_only: bool) -> Self {
        Self {
            source,
            edits: Vec::new(),
            depth: 0,
            inline,
            font_face: false,
            syntax_only,
            valid_preludes: true,
            scope_context: true,
            keyframes: false,
        }
    }

    fn body(&mut self, input: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
        if self.depth >= 128 {
            consume(input);
            return Ok(());
        }
        self.depth += 1;
        for _ in RuleBodyParser::new(input, self) {}
        self.depth -= 1;
        Ok(())
    }
}

struct AtRuleContext {
    font_face: bool,
    scope_context: bool,
    valid_scope: bool,
    keyframes: bool,
}

impl<'i> AtRuleParser<'i> for Scanner<'i> {
    type Prelude = AtRuleContext;
    type AtRule = ();
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, ()>> {
        let start = input.position();
        consume(input);
        let keyframes = self.scope_context
            && [
                "keyframes",
                "-webkit-keyframes",
                "-moz-keyframes",
                "-o-keyframes",
                "-ms-keyframes",
            ]
            .iter()
            .any(|rule| name.eq_ignore_ascii_case(rule));
        if self.syntax_only && keyframes {
            self.valid_preludes &= motion::keyframes_name_is_valid(input.slice_from(start));
        }
        let valid_scope = if self.scope_context && name.eq_ignore_ascii_case("scope") {
            match selector_recovery::normalize(input.slice_from(start)) {
                Some(source) if scope::prelude_is_valid(&source) => {
                    if let Cow::Owned(source) = source {
                        self.edits
                            .push((start.byte_index()..input.position().byte_index(), source));
                    }
                    true
                }
                _ => false,
            }
        } else {
            true
        };
        Ok(AtRuleContext {
            keyframes,
            font_face: name.eq_ignore_ascii_case("font-face"),
            scope_context: self.scope_context
                && [
                    "scope",
                    "media",
                    "supports",
                    "layer",
                    "container",
                    "document",
                    "-moz-document",
                    "starting-style",
                    "nest",
                ]
                .iter()
                .any(|group| name.eq_ignore_ascii_case(group)),
            valid_scope,
        })
    }

    fn parse_block<'t>(
        &mut self,
        context: Self::Prelude,
        start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<(), ParseError<'i, ()>> {
        if !context.valid_scope {
            self.valid_preludes = false;
            consume(input);
            let end = input.position().byte_index();
            let end = end + usize::from(self.source.as_bytes().get(end) == Some(&b'}'));
            self.edits
                .push((start.position().byte_index()..end, String::new()));
            return Ok(());
        }
        let outer = self.font_face;
        let outer_scope_context = self.scope_context;
        let outer_keyframes = self.keyframes;
        self.font_face = context.font_face;
        self.scope_context = context.scope_context;
        self.keyframes = context.keyframes;
        let result = self.body(input);
        self.font_face = outer;
        self.scope_context = outer_scope_context;
        self.keyframes = outer_keyframes;
        result
    }
}

impl<'i> QualifiedRuleParser<'i> for Scanner<'i> {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = ();

    fn parse_prelude<'t>(&mut self, input: &mut Parser<'i, 't>) -> Result<(), ParseError<'i, ()>> {
        let start = input.position();
        consume(input);
        if self.syntax_only && self.keyframes {
            self.valid_preludes &= motion::keyframe_selectors_are_valid(input.slice_from(start));
        } else if self.scope_context && !self.keyframes {
            let source = input.slice_from(start);
            let replacement = match selector_recovery::normalize(source) {
                Some(Cow::Borrowed(_)) => None,
                Some(Cow::Owned(value)) => Some(value),
                None => {
                    self.valid_preludes = false;
                    Some(":not()".to_owned())
                }
            };
            if let Some(replacement) = replacement {
                self.edits.push((
                    start.byte_index()..input.position().byte_index(),
                    replacement,
                ));
            }
        }
        Ok(())
    }

    fn parse_block<'t>(
        &mut self,
        _: (),
        _: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<(), ParseError<'i, ()>> {
        let outer = self.keyframes;
        self.keyframes = false;
        let result = self.body(input);
        self.keyframes = outer;
        result
    }
}

impl<'i> DeclarationParser<'i> for Scanner<'i> {
    type Declaration = ();
    type Error = ();

    fn parse_value<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
        start: &ParserState,
    ) -> Result<(), ParseError<'i, ()>> {
        // Custom-property payloads are opaque until substituted into a known
        // property. Their blocks can contain declaration-looking tokens.
        if name.starts_with("--") {
            consume(input);
            return Ok(());
        }
        let value_start = input.position().byte_index();
        while let Ok(token) = input.next() {
            match token {
                // This may be a nested selector, e.g. `a:hover { ... }`.
                // Let RuleBodyParser retry it as a qualified rule.
                Token::CurlyBracketBlock => return Err(input.new_custom_error(())),
                Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock => {
                    input.parse_nested_block(|nested| {
                        consume(nested);
                        Ok(())
                    })?;
                }
                _ => {}
            }
        }
        let value_end = input.position().byte_index();
        if self.syntax_only {
            return Ok(());
        }
        if self.font_face {
            // Descriptors have a different grammar: font-weight may contain
            // a variable-font range. Share only the math token normalization.
            if let Cow::Owned(value) = normalize_math(&self.source[value_start..value_end]) {
                self.edits.push((value_start..value_end, value));
            }
            return Ok(());
        }
        let normalized_name = name.to_ascii_lowercase();
        let id = PropertyId::from(normalized_name.as_str());
        let value = normalize_property_value(&id, &self.source[value_start..value_end]);
        let replacement = match &id {
            PropertyId::Flex(_) => flex::normalized_declaration(&id, &value),
            PropertyId::WhiteSpace => white_space::normalized_declaration(&value),
            PropertyId::LetterSpacing | PropertyId::WordSpacing => {
                text_spacing::normalized_declaration(&id, &value)
            }
            PropertyId::TabSize(VendorPrefix::None) => tab_size::normalized_declaration(&value),
            _ => None,
        }
        .or_else(|| {
            (box_syntax::property_value_is_valid(&id, without_priority(&value)) == Some(false)
                || color_syntax::declaration_is_invalid(&id, without_priority(&value)))
                .then(String::new)
        })
        .or_else(|| {
            matches!(value, Cow::Owned(_)).then(|| {
                format!(
                    "{}{value}",
                    &self.source[start.position().byte_index()..value_start]
                )
            })
        });
        if let Some(replacement) = replacement {
            self.edits
                .push((start.position().byte_index()..value_end, replacement));
        }
        Ok(())
    }
}

impl<'i> RuleBodyItemParser<'i, (), ()> for Scanner<'i> {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        !self.inline
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize, normalize_inline};

    #[test]
    fn font_face_descriptors_keep_ranges_and_share_math_normalization() {
        assert_eq!(
            normalize("@media screen {@font-face {font-family:Reader;src:url(reader.woff2);font-weight:clamp(none,400,none) 900}} p {font-weight:0;font-weight:calc(-10)}"),
            "@media screen {@font-face {font-family:Reader;src:url(reader.woff2);font-weight:calc(400) 900}} p {;font-weight:1}"
        );
    }

    #[test]
    fn custom_property_payloads_remain_opaque_while_real_declarations_normalize() {
        let payload = "{border-width:hairline;white-space:preserve-breaks nowrap;--width:clamp(none,2px,none)}";
        let declarations = format!("--payload:{payload};border-width:hairline");
        let expected = format!("--payload:{payload};border-width:1px");
        assert_eq!(normalize_inline(&declarations), expected);
        assert_eq!(
            normalize(&format!("#target {{{declarations}}}")),
            format!("#target {{{expected}}}")
        );
        let value = "--width:clamp(none,2px,none);--text:'hairline'";
        assert_eq!(normalize_inline(value), value);
        for source in [
            "--x {border-width:hairline}",
            "@media screen {--x {border-width:hairline}}",
            ".card {a:hover {border-width:hairline}}",
        ] {
            assert_eq!(
                normalize(source),
                source.replace("border-width:hairline", "border-width:1px")
            );
        }
        // The CSS Syntax parser rejects rule preludes that look like custom
        // property declarations. Leave their ignored contents untouched.
        let invalid_rule = "--x:hover {border-width:hairline}";
        assert_eq!(normalize(invalid_rule), invalid_rule);
    }

    #[test]
    fn normalizes_declarations_but_not_selectors_or_supports_conditions() {
        let css = "white-space:hover { color: red } @supports (white-space: collapse) { p { white-space: preserve-breaks nowrap !important; } }";
        let normalized = normalize(css);
        assert!(normalized.contains("white-space:hover"));
        assert!(normalized.contains("@supports (white-space: collapse)"));
        assert!(normalized.contains("white-space: pre-line !important; --html-renderer-white-space: preserve-breaks-nowrap !important"));
    }
}
