//! Token-preserving compatibility checks shared by source and syntax validation.

use cssparser::{ParseError, Parser, ParserInput, Token, parse_important};
use lightningcss::properties::custom::{TokenList, UnparsedProperty};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::{Parse, ParseWithOptions};
use lightningcss::values::calc::{Calc, MathFunction};
use lightningcss::values::length::{Length, LengthPercentage};
use static_self::IntoOwned;
use std::borrow::Cow;
use std::ops::Range;

pub(crate) fn parsed_property<'i>(id: PropertyId<'i>, value: &'i str) -> Option<Property<'i>> {
    match normalize_property_value(&id, value) {
        Cow::Borrowed(value) => parse_property(id, value),
        Cow::Owned(value) => parse_property(id, &value).map(IntoOwned::into_owned),
    }
}

fn parse_property<'i>(id: PropertyId<'i>, value: &'i str) -> Option<Property<'i>> {
    // The native flex parser may interpret sign(length) as the basis before
    // its number type can be resolved. Retain component order until then.
    if matches!(id, PropertyId::Flex(_))
        && matches!(super::sign::lower(value, |_| Some(1.0)), Ok(Cow::Owned(_)))
    {
        return Some(Property::Unparsed(UnparsedProperty {
            property_id: id,
            value: TokenList::parse_string_with_options(value, ParserOptions::default()).ok()?,
        }));
    }
    let mut input = ParserInput::new(value);
    Parser::new(&mut input)
        .parse_entirely(|parser| Property::parse(id, parser, &ParserOptions::default()))
        .ok()
}

pub(crate) fn normalize_property_value<'a>(id: &PropertyId<'_>, value: &'a str) -> Cow<'a, str> {
    if matches!(id, PropertyId::TextJustify) {
        let component = without_priority(value);
        let mut input = ParserInput::new(component);
        let mut parser = Parser::new(&mut input);
        if parser.expect_ident_matching("distribute").is_ok() && parser.is_exhausted() {
            return Cow::Owned(format!("inter-character{}", &value[component.len()..]));
        }
    }
    if let Cow::Owned(value) = super::font::normalize_system_font(id, value) {
        return Cow::Owned(value);
    }
    let normalized = normalize_math(value);
    if matches!(id, PropertyId::VerticalAlign)
        && let Cow::Owned(value) = super::inline_values::normalize_vertical_align(&normalized)
    {
        return Cow::Owned(value);
    }
    if let Cow::Owned(value) = super::font::normalize_weight(id, &normalized) {
        return Cow::Owned(value);
    }
    match normalize_hairline(id, &normalized) {
        Cow::Owned(value) => Cow::Owned(value),
        Cow::Borrowed(_) => normalized,
    }
}

fn normalize_hairline<'a>(id: &PropertyId<'_>, value: &'a str) -> Cow<'a, str> {
    if !matches!(
        id,
        PropertyId::Border
            | PropertyId::BorderWidth
            | PropertyId::BorderTop
            | PropertyId::BorderRight
            | PropertyId::BorderBottom
            | PropertyId::BorderLeft
            | PropertyId::BorderTopWidth
            | PropertyId::BorderRightWidth
            | PropertyId::BorderBottomWidth
            | PropertyId::BorderLeftWidth
            | PropertyId::BorderBlock
            | PropertyId::BorderInline
            | PropertyId::BorderBlockStart
            | PropertyId::BorderBlockEnd
            | PropertyId::BorderInlineStart
            | PropertyId::BorderInlineEnd
            | PropertyId::BorderBlockWidth
            | PropertyId::BorderInlineWidth
            | PropertyId::BorderBlockStartWidth
            | PropertyId::BorderBlockEndWidth
            | PropertyId::BorderInlineStartWidth
            | PropertyId::BorderInlineEndWidth
            | PropertyId::Outline
            | PropertyId::OutlineWidth
    ) || (!value.contains('\\')
        && !value
            .as_bytes()
            .windows(8)
            .any(|part| part.eq_ignore_ascii_case(b"hairline")))
    {
        return Cow::Borrowed(value);
    }
    // The renderer's default device grid is one CSS pixel. Use its thinnest
    // visible line, within the UA-defined (0, 1px] hairline range. Retain
    // strings, comments, and nested values such as var() until substitution.
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let mut output = String::new();
    let mut copied = 0;
    loop {
        let start = parser.position().byte_index();
        let Ok(token) = parser.next_including_whitespace_and_comments() else {
            break;
        };
        match token {
            Token::Ident(name) if name.eq_ignore_ascii_case("hairline") => {
                output.push_str(&value[copied..start]);
                output.push_str("1px");
                copied = parser.position().byte_index();
            }
            Token::Function(_)
            | Token::ParenthesisBlock
            | Token::SquareBracketBlock
            | Token::CurlyBracketBlock => {
                let result: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|nested| {
                    while nested.next().is_ok() {}
                    Ok(())
                });
                if result.is_err() {
                    return Cow::Borrowed(value);
                }
            }
            _ => {}
        }
    }
    if copied == 0 {
        return Cow::Borrowed(value);
    }
    output.push_str(&value[copied..]);
    Cow::Owned(output)
}

pub(crate) enum IntegerValue {
    Integer(i64),
    Calculation(TokenList<'static>),
}

pub(crate) enum NumberValue {
    Number(f32),
    Calculation(TokenList<'static>),
}

pub(crate) fn parse_number(source: &str) -> Option<NumberValue> {
    let source = normalize_math(source);
    let mut input = ParserInput::new(&source);
    Parser::new(&mut input)
        .parse_entirely(parse_number_value)
        .ok()
}

pub(crate) fn parse_number_value<'i>(
    parser: &mut Parser<'i, '_>,
) -> Result<NumberValue, ParseError<'i, ()>> {
    if let Ok(value) = parser.try_parse(|parser| parser.expect_number()) {
        return Ok(NumberValue::Number(value));
    }
    number_calculation(parser).map(NumberValue::Calculation)
}

pub(crate) fn parse_integer(css: &str, minimum: u32) -> Option<IntegerValue> {
    let normalized = normalize_math(css);
    let mut input = ParserInput::new(&normalized);
    Parser::new(&mut input)
        .parse_entirely(|parser| parse_integer_value(parser, minimum))
        .ok()
}

pub(crate) fn parse_integer_value<'i>(
    parser: &mut Parser<'i, '_>,
    minimum: u32,
) -> Result<IntegerValue, ParseError<'i, ()>> {
    parse_integer_with_minimum(parser, i64::from(minimum))
}

pub(crate) fn parse_signed_integer(css: &str) -> Option<IntegerValue> {
    let normalized = normalize_math(css);
    let mut input = ParserInput::new(&normalized);
    Parser::new(&mut input)
        .parse_entirely(|parser| parse_integer_with_minimum(parser, i64::from(i32::MIN)))
        .ok()
}

fn parse_integer_with_minimum<'i>(
    parser: &mut Parser<'i, '_>,
    minimum: i64,
) -> Result<IntegerValue, ParseError<'i, ()>> {
    if let Ok(value) = parser.try_parse(|parser| parser.expect_integer()) {
        if i64::from(value) < minimum {
            return Err(parser.new_custom_error(()));
        }
        return Ok(IntegerValue::Integer(i64::from(value)));
    }
    number_calculation(parser).map(IntegerValue::Calculation)
}

fn number_calculation<'i>(
    parser: &mut Parser<'i, '_>,
) -> Result<TokenList<'static>, ParseError<'i, ()>> {
    let start = parser.position();
    let value = Calc::<Length>::parse(parser).map_err(|_| parser.new_custom_error(()))?;
    if !number_is_valid(&value) {
        return Err(parser.new_custom_error(()));
    }
    // Preserve math tokens until units resolve: serializing the dependency's
    // simplified calculation tree loses NaN constants.
    let tokens =
        TokenList::parse_string_with_options(parser.slice_from(start), ParserOptions::default())
            .map_err(|_| parser.new_custom_error(()))?;
    Ok(tokens.into_owned())
}

/// The priority belongs to a declaration, not to its value grammar.
pub(crate) fn without_priority(value: &str) -> &str {
    if !value.contains('!') {
        return value;
    }
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    loop {
        let start = parser.position();
        let state = parser.state();
        match parser.next() {
            Ok(
                Token::Function(_)
                | Token::ParenthesisBlock
                | Token::SquareBracketBlock
                | Token::CurlyBracketBlock,
            ) => {
                // next() skips nested blocks lazily. Consume the whole block
                // before recording the position of a possible priority.
                let result: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|nested| {
                    while nested.next().is_ok() {}
                    Ok(())
                });
                if result.is_err() {
                    return value;
                }
            }
            Ok(Token::Delim('!')) => {
                parser.reset(&state);
                if parser
                    .try_parse(|parser| {
                        parse_important(parser)?;
                        parser.expect_exhausted()
                    })
                    .is_ok()
                {
                    return value[..start.byte_index()].trim_end();
                }
                let _ = parser.next();
            }
            Err(_) => return value,
            _ => {}
        }
    }
}

pub(crate) fn split_important(value: &str) -> (&str, bool) {
    let without = without_priority(value);
    (without, without.len() != value.len())
}

pub(crate) fn css_wide_keyword(value: &str) -> bool {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    parser.expect_ident().is_ok_and(|ident| {
        ["inherit", "initial", "unset", "revert", "revert-layer"]
            .iter()
            .any(|word| ident.eq_ignore_ascii_case(word))
    }) && parser.is_exhausted()
}

/// A recovery token list must contain a parsed substitution, rather than a
/// substring in a string/comment. Validate inherited references alongside the
/// dependency's var/env argument grammar.
pub(crate) fn substitutions(value: &str) -> Result<bool, ()> {
    TokenList::parse_string_with_options(value, ParserOptions::default())
        .map_err(|_| ())
        .and_then(|tokens| super::substitution::substitutions(&tokens.0))
}

pub(crate) fn may_have_substitutions(value: &str) -> bool {
    let candidate = value.contains('\\')
        || value.contains("--")
        || value
            .as_bytes()
            .windows(4)
            .any(|part| part.eq_ignore_ascii_case(b"var(") || part.eq_ignore_ascii_case(b"env("))
        || value
            .as_bytes()
            .windows(8)
            .any(|part| part.eq_ignore_ascii_case(b"inherit("))
        || value
            .as_bytes()
            .windows(12)
            .any(|part| part.eq_ignore_ascii_case(b"random-item("));
    if !candidate {
        return false;
    }
    // Resolve escapes and token boundaries before asking the native token
    // parser to validate substitutions. Strings and URL text are opaque.
    let mut found = false;
    let result = super::numeric_functions::replace(value, |name, _| {
        found |= name.eq_ignore_ascii_case("var")
            || name.eq_ignore_ascii_case("env")
            || super::substitution::is_lazy_function(name);
        Ok(None)
    });
    found || result.is_err()
}

/// Lower unbounded clamp limits to equivalent math and work around alpha.72's
/// handling of whitespace before clamp commas. Preserve other tokens verbatim.
pub(crate) fn normalize_math(value: &str) -> Cow<'_, str> {
    if !value
        .as_bytes()
        .windows(5)
        .any(|part| part.eq_ignore_ascii_case(b"clamp"))
        && !value.contains('\\')
    {
        return Cow::Borrowed(value);
    }
    struct Edit {
        range: Range<usize>,
        replacement: &'static str,
    }
    fn is_none(value: &str) -> bool {
        let mut input = ParserInput::new(value);
        let mut parser = Parser::new(&mut input);
        parser.expect_ident_matching("none").is_ok() && parser.is_exhausted()
    }
    fn walk<'i>(
        parser: &mut Parser<'i, '_>,
        source: &str,
        clamp: bool,
        edits: &mut Vec<Edit>,
        depth: usize,
    ) -> Result<Vec<Range<usize>>, ParseError<'i, ()>> {
        if depth > 128 {
            return Err(parser.new_custom_error(()));
        }
        let mut whitespace = Vec::new();
        let mut arguments = Vec::new();
        let mut argument_start = parser.position().byte_index();
        loop {
            let start = parser.position().byte_index();
            let Ok(token) = parser.next_including_whitespace_and_comments().cloned() else {
                break;
            };
            let end = parser.position().byte_index();
            match token {
                Token::WhiteSpace(_) => whitespace.push(start..end),
                Token::Comment(_) => {}
                Token::Comma if clamp => {
                    edits.extend(whitespace.drain(..).map(|range| Edit {
                        range,
                        replacement: "",
                    }));
                    arguments.push(argument_start..start);
                    argument_start = end;
                }
                Token::Function(name) => {
                    whitespace.clear();
                    let nested_clamp = name.eq_ignore_ascii_case("clamp");
                    let arguments = parser.parse_nested_block(|nested| {
                        walk(nested, source, nested_clamp, edits, depth + 1)
                    })?;
                    if nested_clamp && arguments.len() == 3 {
                        // Substitutions can insert commas or whole limits.
                        // Lower only after their argument structure is known.
                        let function = &source[start..parser.position().byte_index()];
                        if may_have_substitutions(function) && substitutions(function) != Ok(false)
                        {
                            continue;
                        }
                        let no_minimum = is_none(&source[arguments[0].clone()]);
                        let no_maximum = is_none(&source[arguments[2].clone()]);
                        let replacement = match (no_minimum, no_maximum) {
                            (true, true) => "calc(",
                            (true, false) => "min(",
                            (false, true) => "max(",
                            (false, false) => continue,
                        };
                        edits.push(Edit {
                            range: start..end,
                            replacement,
                        });
                        if no_minimum {
                            edits.push(Edit {
                                range: arguments[0].start..arguments[1].start,
                                replacement: "",
                            });
                        }
                        if no_maximum {
                            edits.push(Edit {
                                range: arguments[1].end..arguments[2].end,
                                replacement: "",
                            });
                        }
                    }
                }
                Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
                    whitespace.clear();
                    parser.parse_nested_block(|nested| {
                        walk(nested, source, false, edits, depth + 1)
                    })?;
                }
                _ => whitespace.clear(),
            }
        }
        if clamp {
            arguments.push(argument_start..parser.position().byte_index());
        }
        Ok(arguments)
    }
    let mut input = ParserInput::new(value);
    let mut edits = Vec::new();
    if walk(&mut Parser::new(&mut input), value, false, &mut edits, 0).is_err() || edits.is_empty()
    {
        return Cow::Borrowed(value);
    }
    edits.sort_unstable_by_key(|edit| (edit.range.start, std::cmp::Reverse(edit.range.end)));
    let mut output = String::with_capacity(value.len());
    let mut copied = 0;
    for edit in edits {
        // Removing a none limit also removes whitespace edits inside it.
        if edit.range.start < copied {
            continue;
        }
        output.push_str(&value[copied..edit.range.start]);
        output.push_str(edit.replacement);
        copied = edit.range.end;
    }
    output.push_str(&value[copied..]);
    Cow::Owned(output)
}

#[derive(Clone, Copy, PartialEq)]
enum Dimension {
    Length,
    Number,
}

fn length_dimension(value: &Length) -> Option<Dimension> {
    match value {
        Length::Value(_) => Some(Dimension::Length),
        Length::Calc(calc) => calc_dimension(calc, length_dimension),
    }
}

fn percentage_dimension(value: &LengthPercentage) -> Option<Dimension> {
    match value {
        LengthPercentage::Dimension(_) | LengthPercentage::Percentage(_) => Some(Dimension::Length),
        LengthPercentage::Calc(calc) => calc_dimension(calc, percentage_dimension),
    }
}

fn same_dimension<V>(
    a: &Calc<V>,
    b: &Calc<V>,
    dimension: fn(&V) -> Option<Dimension>,
) -> Option<Dimension> {
    let a = calc_dimension(a, dimension)?;
    (a == calc_dimension(b, dimension)?).then_some(a)
}

fn calc_dimension<V>(value: &Calc<V>, dimension: fn(&V) -> Option<Dimension>) -> Option<Dimension> {
    match value {
        Calc::Value(value) => dimension(value),
        Calc::Number(_) => Some(Dimension::Number),
        Calc::Sum(a, b) => same_dimension(a, b, dimension),
        Calc::Product(_, value) => calc_dimension(value, dimension),
        Calc::Function(function) => match &**function {
            MathFunction::Calc(value) | MathFunction::Abs(value) => {
                calc_dimension(value, dimension)
            }
            MathFunction::Sign(value) => {
                calc_dimension(value, dimension).map(|_| Dimension::Number)
            }
            MathFunction::Min(values) | MathFunction::Max(values) | MathFunction::Hypot(values) => {
                let first = values.first()?;
                let first_dimension = calc_dimension(first, dimension)?;
                values
                    .iter()
                    .all(|value| calc_dimension(value, dimension) == Some(first_dimension))
                    .then_some(first_dimension)
            }
            MathFunction::Clamp(a, b, c) => {
                let first_dimension = same_dimension(a, b, dimension)?;
                (calc_dimension(c, dimension)? == first_dimension).then_some(first_dimension)
            }
            MathFunction::Round(_, a, b) | MathFunction::Rem(a, b) | MathFunction::Mod(a, b) => {
                same_dimension(a, b, dimension)
            }
        },
    }
}

/// Check literal tokens before the dependency can erase their provenance by
/// simplifying math. Range restrictions apply to literals, not to operands
/// within a function. Nested colors, strings and comments are not lengths.
pub(crate) fn literal_numeric_tokens_are_valid(
    value: &str,
    non_negative: bool,
    numbers_allowed: bool,
) -> bool {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    while let Ok(token) = parser.next() {
        let number = match token {
            Token::Number { value, .. } => {
                if !numbers_allowed && *value != 0.0 {
                    return false;
                }
                *value
            }
            Token::Dimension { value, .. } => *value,
            Token::Percentage { unit_value, .. } => *unit_value,
            _ => continue,
        };
        if non_negative && number < 0.0 {
            return false;
        }
    }
    true
}

pub(crate) fn number_is_valid(value: &Calc<Length>) -> bool {
    calc_dimension(value, length_dimension) == Some(Dimension::Number)
}

pub(crate) fn length_is_valid(value: &Length) -> bool {
    length_dimension(value) == Some(Dimension::Length)
}

pub(crate) fn length_percentage_is_valid(value: &LengthPercentage) -> bool {
    percentage_dimension(value) == Some(Dimension::Length)
}

// Line height also accepts a number. Unresolved number-valued math can be
// retained by the dependency in its Length variant.
pub(crate) fn length_percentage_or_number_is_valid(value: &LengthPercentage) -> bool {
    percentage_dimension(value).is_some()
}

pub(crate) fn numeric_component<'i>(
    parser: &mut Parser<'i, '_>,
) -> Result<&'i str, ParseError<'i, ()>> {
    let start = parser.position();
    match parser.next()? {
        Token::Number { .. } | Token::Percentage { .. } | Token::Dimension { .. } => {}
        Token::Function(_) => {
            parser
                .parse_nested_block(|parser| super::urls::consume(parser, 0, false).map(|_| ()))?;
        }
        _ => return Err(parser.new_custom_error(())),
    }
    Ok(parser.slice_from(start))
}

#[cfg(test)]
mod tests {
    use super::{normalize_math, without_priority};

    #[test]
    fn clamp_fix_preserves_significant_tokens_strings_and_comments() {
        assert_eq!(
            normalize_math("clamp(10px,35px /*foo*/,30px)"),
            "clamp(10px,35px/*foo*/,30px)"
        );
        assert_eq!(
            normalize_math("clamp(10p/**/x ,35px,30px)"),
            "clamp(10p/**/x,35px,30px)"
        );
        assert_eq!(
            normalize_math("'clamp(10px ,20px,30px)'"),
            "'clamp(10px ,20px,30px)'"
        );
        assert_eq!(
            normalize_math("clamp(1px + 2px ,3px,4px)"),
            "clamp(1px + 2px,3px,4px)"
        );
        assert_eq!(normalize_math("CLAMP(1px ,2px,3px)"), "CLAMP(1px,2px,3px)");
    }

    #[test]
    fn unbounded_clamp_lowering_preserves_tokens_and_defers_substitution() {
        for (source, expected) in [
            ("clamp(none,30px,40px)", "min(30px,40px)"),
            ("clamp(20px,30px,none)", "max(20px,30px)"),
            ("clamp(none,30px,none)", "calc(30px)"),
            ("clamp(/*a*/NONE,30px,/*b*/none)", "calc(30px)"),
            ("cl\\61 mp(n\\6f ne,30px,none)", "calc(30px)"),
            (
                "clamp(none,clamp(20px,30px,none),40px)",
                "min(max(20px,30px),40px)",
            ),
            ("'clamp(none,30px,none)'", "'clamp(none,30px,none)'"),
            (
                "/*clamp(none,30px,none)*/ 30px",
                "/*clamp(none,30px,none)*/ 30px",
            ),
            (
                "clamp(none,var(--arguments),30px)",
                "clamp(none,var(--arguments),30px)",
            ),
            (
                "clamp(none,env(safe-area-inset-top),none)",
                "clamp(none,env(safe-area-inset-top),none)",
            ),
            ("clamp(none,1px,2px,none)", "clamp(none,1px,2px,none)"),
        ] {
            assert_eq!(normalize_math(source), expected, "{source}");
        }
    }

    #[test]
    fn strips_only_a_real_trailing_priority() {
        assert_eq!(without_priority("10px !/**/IMPORTANT"), "10px");
        assert_eq!(
            without_priority("var(--x, '!important')"),
            "var(--x, '!important')"
        );
        assert_eq!(without_priority("10px !bogus"), "10px !bogus");
        assert_eq!(
            without_priority("clamp(1px,2px,3px) !important"),
            "clamp(1px,2px,3px)"
        );
        assert_eq!(
            without_priority("calc(1px + min(2px,3px))!important"),
            "calc(1px + min(2px,3px))"
        );
    }
}
