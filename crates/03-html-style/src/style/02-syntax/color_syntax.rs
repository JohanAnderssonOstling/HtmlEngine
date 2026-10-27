//! Color-related property grammars beyond the native parser's coverage.
//! Syntax conformance is separate from rendering capability.

mod components;

use super::substitution::{contains_function, is_random_math};
use super::value::{PercentageValue, numeric_component, parse_percentage_value};
use super::{numeric_functions, sign, urls};
use cssparser::{ParseError, Parser, ParserInput};
use lightningcss::properties::PropertyId;
use lightningcss::properties::custom::TokenList;
use lightningcss::properties::effects::BlendMode;
use lightningcss::traits::Parse;
use lightningcss::values::color::CssColor;
use lightningcss::values::ident::CustomIdent;
use std::borrow::Cow;

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    let color_count = color_component_count(id);
    if !matches!(
        id,
        PropertyId::FontPalette | PropertyId::ColorScheme | PropertyId::CaretColor
    ) && color_count.is_none()
    {
        return None;
    }
    let value = if matches!(id, PropertyId::FontPalette) {
        match sign::lower(value, |_| Some(1.0)) {
            Ok(value) => value,
            Err(()) => return Some(false),
        }
    } else {
        Cow::Borrowed(value)
    };
    let mut input = ParserInput::new(&value);
    Some(
        Parser::new(&mut input)
            .parse_entirely(|parser| match id {
                _ if color_count.is_some() => {
                    color(parser, 0)?;
                    for _ in 1..color_count.unwrap() {
                        if parser.is_exhausted() {
                            break;
                        }
                        color(parser, 0)?;
                    }
                    Ok(())
                }
                PropertyId::FontPalette => palette(parser, 0),
                PropertyId::ColorScheme => color_scheme(parser),
                PropertyId::CaretColor => {
                    caret_color(parser)?;
                    if !parser.is_exhausted() {
                        caret_color(parser)?;
                    }
                    Ok(())
                }
                _ => unreachable!("guarded color property ID"),
            })
            .is_ok(),
    )
}

/// Longhands and color-only shorthands that consume the shared color grammar.
/// Properties such as outline-color and caret-color add their own keywords.
pub(crate) fn color_component_count(id: &PropertyId<'_>) -> Option<usize> {
    match id {
        PropertyId::Color
        | PropertyId::BackgroundColor
        | PropertyId::BorderTopColor
        | PropertyId::BorderBottomColor
        | PropertyId::BorderLeftColor
        | PropertyId::BorderRightColor
        | PropertyId::BorderBlockStartColor
        | PropertyId::BorderBlockEndColor
        | PropertyId::BorderInlineStartColor
        | PropertyId::BorderInlineEndColor
        | PropertyId::TextDecorationColor(_)
        | PropertyId::TextEmphasisColor(_) => Some(1),
        PropertyId::BorderColor => Some(4),
        PropertyId::BorderBlockColor | PropertyId::BorderInlineColor => Some(2),
        _ => None,
    }
}

/// Check source tokens before native color conversion can erase invalid
/// percentage ranges or interpolation modifiers. Reuse the declaration entry
/// point for CSS-wide keywords, substitutions and context-dependent math.
pub(crate) fn declaration_is_invalid(id: &PropertyId<'_>, value: &str) -> bool {
    color_component_count(id).is_some()
        && crate::declaration_syntax_impl(id.name(), id.clone(), value)
            == crate::PropertySyntax::Invalid
}

pub(crate) fn raw_color_is_unsupported(id: &PropertyId<'_>, tokens: &TokenList<'_>) -> bool {
    color_component_count(id).is_some()
        && (contains_function(tokens, |name| {
            name.eq_ignore_ascii_case("color-mix")
                || name.eq_ignore_ascii_case("color-layers")
                || name.eq_ignore_ascii_case("contrast-color")
        }) || !contains_function(tokens, |name| {
            is_random_math(name)
                || name.eq_ignore_ascii_case("sibling-index")
                || name.eq_ignore_ascii_case("sibling-count")
        }))
}

pub(crate) fn source_uses_unrepresented_function(value: &str) -> bool {
    if !value.contains('\\')
        && !value
            .as_bytes()
            .windows(12)
            .any(|part| part.eq_ignore_ascii_case(b"color-layers"))
        && !value
            .as_bytes()
            .windows(14)
            .any(|part| part.eq_ignore_ascii_case(b"contrast-color"))
    {
        return false;
    }
    let mut found = false;
    let _ = numeric_functions::replace(value, |name, _| {
        found |= name.eq_ignore_ascii_case("color-layers")
            || name.eq_ignore_ascii_case("contrast-color");
        Ok(None)
    });
    found
}

fn caret_color<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("auto"))
        .is_ok()
    {
        return Ok(());
    }
    CssColor::parse(parser)
        .map(|_| ())
        .map_err(|_| parser.new_custom_error(()))
}

fn color_scheme<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("normal"))
        .is_ok()
    {
        return Ok(());
    }
    let only_first = parser
        .try_parse(|parser| parser.expect_ident_matching("only"))
        .is_ok();
    let mut schemes = 0;
    while !parser.is_exhausted() {
        let name = CustomIdent::parse(parser).map_err(|_| parser.new_custom_error(()))?;
        if name.0.eq_ignore_ascii_case("normal") {
            return Err(parser.new_custom_error(()));
        }
        if name.0.eq_ignore_ascii_case("only") {
            if only_first || schemes == 0 {
                return Err(parser.new_custom_error(()));
            }
            // The && grammar permits only before or after the entire list,
            // rather than between two scheme names.
            parser.expect_exhausted()?;
            return Ok(());
        }
        schemes += 1;
    }
    if schemes == 0 {
        Err(parser.new_custom_error(()))
    } else {
        Ok(())
    }
}

fn palette<'i>(parser: &mut Parser<'i, '_>, depth: usize) -> Result<(), ParseError<'i, ()>> {
    if depth >= 128 {
        return Err(parser.new_custom_error(()));
    }
    if let Ok(name) = parser.try_parse(|parser| parser.expect_ident_cloned()) {
        return if name.starts_with("--")
            || ["normal", "light", "dark"]
                .iter()
                .any(|keyword| name.eq_ignore_ascii_case(keyword))
        {
            Ok(())
        } else {
            Err(parser.new_custom_error(()))
        };
    }
    parser.expect_function_matching("palette-mix")?;
    parser.parse_nested_block(|parser| {
        if parser
            .try_parse(|parser| parser.expect_ident_matching("in"))
            .is_ok()
        {
            interpolation_method(parser)?;
            parser.expect_comma()?;
        }
        parser.parse_comma_separated(|parser| {
            let percentage_first = parser.try_parse(mix_percentage).is_ok();
            palette(parser, depth + 1)?;
            if !percentage_first && !parser.is_exhausted() {
                mix_percentage(parser)?;
            }
            Ok(())
        })?;
        Ok(())
    })
}

pub(crate) fn interpolation_method<'i>(
    parser: &mut Parser<'i, '_>,
) -> Result<(), ParseError<'i, ()>> {
    let space = parser.expect_ident_cloned()?;
    if ["hsl", "hwb", "lch", "oklch"]
        .iter()
        .any(|name| space.eq_ignore_ascii_case(name))
    {
        if let Ok(hue) = parser.try_parse(|parser| parser.expect_ident_cloned()) {
            if !["shorter", "longer", "increasing", "decreasing"]
                .iter()
                .any(|name| hue.eq_ignore_ascii_case(name))
            {
                return Err(parser.new_custom_error(()));
            }
            parser.expect_ident_matching("hue")?;
        }
        Ok(())
    } else if space.starts_with("--")
        || [
            "srgb",
            "srgb-linear",
            "display-p3",
            "display-p3-linear",
            "a98-rgb",
            "prophoto-rgb",
            "rec2020",
            "lab",
            "oklab",
            "xyz",
            "xyz-d50",
            "xyz-d65",
        ]
        .iter()
        .any(|name| space.eq_ignore_ascii_case(name))
    {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

pub(crate) fn color<'i>(
    parser: &mut Parser<'i, '_>,
    depth: usize,
) -> Result<(), ParseError<'i, ()>> {
    if depth >= 128 {
        return Err(parser.new_custom_error(()));
    }
    let state = parser.state();
    let start = parser.position();
    if let Ok(name) = parser.try_parse(|parser| parser.expect_function().cloned()) {
        if let Some(function) = components::Function::from_name(&name) {
            return parser.parse_nested_block(|parser| components::parse(parser, function, depth));
        }
        if name.eq_ignore_ascii_case("contrast-color") {
            return parser.parse_nested_block(|parser| color(parser, depth + 1));
        }
        if name.eq_ignore_ascii_case("color-layers") {
            return parser.parse_nested_block(|parser| {
                if let Ok(blend) = parser.try_parse(BlendMode::parse) {
                    // Color 6 uses Compositing 1 blend modes. The plus modes
                    // in the native enum belong to the later compositing level.
                    if matches!(blend, BlendMode::PlusDarker | BlendMode::PlusLighter) {
                        return Err(parser.new_custom_error(()));
                    }
                    parser.expect_comma()?;
                }
                parser.parse_comma_separated(|parser| color(parser, depth + 1))?;
                Ok(())
            });
        }
        if name.eq_ignore_ascii_case("color-mix") {
            return parser.parse_nested_block(|parser| {
                if parser
                    .try_parse(|parser| parser.expect_ident_matching("in"))
                    .is_ok()
                {
                    interpolation_method(parser)?;
                    parser.expect_comma()?;
                }
                parser.parse_comma_separated(|parser| {
                    let percentage_first = parser.try_parse(mix_percentage).is_ok();
                    color(parser, depth + 1)?;
                    if !percentage_first && !parser.is_exhausted() {
                        mix_percentage(parser)?;
                    }
                    Ok(())
                })?;
                Ok(())
            });
        }
        if name.eq_ignore_ascii_case("light-dark") {
            return parser.parse_nested_block(|parser| {
                color(parser, depth + 1)?;
                parser.expect_comma()?;
                color(parser, depth + 1)
            });
        }
        parser.parse_nested_block(|parser| urls::consume(parser, 0, false).map(|_| ()))?;
        let source = parser.slice_from(start);
        // The native converter eagerly resolves hue math. Share the existing
        // finite stand-ins only within this color's syntax-validation context.
        let finite = crate::finite_css_math_surrogate(source);
        return CssColor::parse_string(finite.as_deref().unwrap_or(source))
            .map(|_| ())
            .map_err(|_| parser.new_custom_error(()));
    }
    parser.reset(&state);
    CssColor::parse(parser)
        .map(|_| ())
        .map_err(|_| parser.new_custom_error(()))
}

pub(crate) fn mix_percentage<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let source = numeric_component(parser)?;
    let source =
        numeric_functions::nonzero_divisors(source).map_err(|_| parser.new_custom_error(()))?;
    let value = sign::lower(&source, |_| Some(1.0)).map_err(|_| parser.new_custom_error(()))?;
    let value =
        numeric_functions::nonzero_divisors(&value).map_err(|_| parser.new_custom_error(()))?;
    let mut input = ParserInput::new(&value);
    let value = Parser::new(&mut input)
        .parse_entirely(parse_percentage_value)
        .map_err(|_| parser.new_custom_error(()))?;
    if matches!(value, PercentageValue::Percentage(value) if !(0.0..=1.0).contains(&value)) {
        Err(parser.new_custom_error(()))
    } else {
        Ok(())
    }
}
