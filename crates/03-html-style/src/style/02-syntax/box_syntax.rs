use super::alignment;
use super::flex;
use super::font;
use super::property_id::RendererPropertyId;
use super::renderer_owned;
use super::sign;
use super::tree_counting;
use super::value::{
    css_wide_keyword, length_is_valid, length_percentage_is_valid,
    length_percentage_or_number_is_valid, literal_numeric_tokens_are_valid, may_have_substitutions,
    parse_signed_integer, parsed_property, substitutions,
};
use super::values::text_spacing;
use cssparser::{CowRcStr, ParseError, Parser, ParserInput, Token};
use lightningcss::properties::align::GapValue;
use lightningcss::properties::background::BackgroundSize;
use lightningcss::properties::border::{BorderSideWidth, LineStyle};
use lightningcss::properties::font::{FontSize, LineHeight};
use lightningcss::properties::list::{CounterStyle, ListStyleType, Symbol, SymbolsType};
use lightningcss::properties::outline::OutlineStyle;
use lightningcss::properties::size::{MaxSize, Size};
use lightningcss::properties::text::{TextTransform, WordBreak};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::traits::{Parse, TrySign};
use lightningcss::values::color::CssColor;
use lightningcss::values::image::Image;
use lightningcss::values::length::{Length, LengthPercentage, LengthPercentageOrAuto};
use lightningcss::values::position::Position;
use lightningcss::values::size::Size2D;
use std::borrow::Cow;

/// Strict web-syntax validation for box properties where Lightning CSS either
/// treats an unknown property as a custom declaration (`float`/`clear`) or
/// deliberately parses values more permissively than the property grammar.
/// `None` means this module does not own the property's extra validation.
pub fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    if !id.name().starts_with("--")
        && (!may_have_substitutions(value) || substitutions(value) == Ok(false))
    {
        match tree_counting::lower(value, |_| 1) {
            Ok(Cow::Owned(lowered)) => return property_value_is_valid(id, &lowered),
            Err(()) => return Some(false),
            Ok(Cow::Borrowed(_)) => {}
        }
        match super::random_math::for_syntax(value) {
            Ok(Cow::Owned(lowered)) => return property_value_is_valid(id, &lowered),
            Err(()) => return Some(false),
            Ok(Cow::Borrowed(_)) => {}
        }
    }
    if let Some(id) = RendererPropertyId::from_lightning(id) {
        return renderer_property_value_is_valid(id, value);
    }
    if !targeted_property(id) && !super::grid::supports(id) {
        return None;
    }
    let value = value.trim();
    if css_wide_keyword(value) {
        return Some(true);
    }
    if may_have_substitutions(value) {
        match substitutions(value) {
            Ok(true) => return Some(true),
            Err(()) => return Some(false),
            Ok(false) => {}
        }
    }

    if let Some(valid) = super::grid::property_value_is_valid(id, value) {
        return Some(valid);
    }
    if let Some(valid) = super::inline_values::property_value_is_valid(id, value) {
        return Some(valid);
    }
    if let Some(valid) = super::unit_algebra::property_value_is_valid(id, value) {
        return Some(valid);
    }
    if let Some(valid) = flex::wrap_value_is_valid(id, value) {
        return Some(valid);
    }

    if sign::supports(id) {
        // Each sign atom is checked independently as a number, including
        // dimensional consistency of its argument. The native property
        // grammar then checks how that number combines with the outer value.
        match sign::lower(value, |_| Some(1.0)) {
            Ok(Cow::Owned(lowered)) => return property_value_is_valid(id, &lowered),
            // Other argument types can already be supported by the native
            // grammar. Keep its validation path when this adapter cannot
            // lower a value, rather than narrowing existing syntax support.
            Err(()) | Ok(Cow::Borrowed(_)) => {}
        }
    }

    if alignment::legacy_items(id, value).is_some() {
        return Some(true);
    }

    match id {
        PropertyId::TextTransform => return Some(text_transform_is_valid(value)),
        PropertyId::WordBreak => return Some(word_break_is_valid(value)),
        PropertyId::FontWeight => return Some(font::weight_is_valid(value)),
        PropertyId::ZIndex => {
            let mut input = ParserInput::new(value);
            let mut parser = Parser::new(&mut input);
            let auto = parser.expect_ident_matching("auto").is_ok() && parser.is_exhausted();
            return Some(auto || parse_signed_integer(value).is_some());
        }
        PropertyId::Order(_) => return Some(parse_signed_integer(value).is_some()),
        PropertyId::Flex(_) => return Some(flex::parse(value).is_some()),
        PropertyId::FlexBasis(_) => return Some(flex::basis(value).is_some()),
        PropertyId::FlexGrow(_) | PropertyId::FlexShrink(_) => {
            return Some(flex::factor_is_valid(value));
        }
        PropertyId::LetterSpacing | PropertyId::WordSpacing => {
            return Some(text_spacing::parse(value).is_some());
        }
        _ => {}
    }

    // Lightning CSS currently accepts a non-zero unitless number as a length.
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let nonzero_number =
        parser.expect_number().is_ok_and(|number| number != 0.0) && parser.is_exhausted();
    if !matches!(id, PropertyId::AspectRatio | PropertyId::LineHeight) && nonzero_number {
        return Some(false);
    }

    if matches!(id, PropertyId::Display) && repeated_keyword(value, Some("list-item")) {
        return Some(false);
    }
    if matches!(id, PropertyId::TextDecorationLine(_))
        && (repeated_keyword(value, None) || exclusive_decoration_keyword(value))
    {
        return Some(false);
    }

    let property = match parsed_property(id.clone(), value) {
        Some(property) if !matches!(property, Property::Unparsed(_) | Property::Custom(_)) => {
            property
        }
        _ => return Some(false),
    };
    Some(
        property_has_valid_range(&property, value)
            && !matches!(
                property,
                Property::OutlineStyle(OutlineStyle::LineStyle(LineStyle::Hidden))
            ),
    )
}

/// The native parser accepts a color on non-final background layers and
/// negative literal sizes. Keep these source checks to preserve fallbacks.
fn background_layers_are_valid(source: &str) -> bool {
    let mut input = ParserInput::new(source);
    let result = Parser::new(&mut input).parse_comma_separated(
        |parser| -> Result<bool, ParseError<'_, ()>> {
            let mut color = false;
            while !parser.is_exhausted() {
                if parser.try_parse(CssColor::parse).is_ok() {
                    color = true;
                    continue;
                }
                match parser.next()?.clone() {
                    Token::Ident(name)
                        if ["border", "padding", "content"]
                            .iter()
                            .any(|keyword| name.eq_ignore_ascii_case(keyword)) =>
                    {
                        return Err(parser.new_custom_error(()));
                    }
                    Token::Delim('/') => {
                        let start = parser.position();
                        BackgroundSize::parse(parser).map_err(|_| parser.new_custom_error(()))?;
                        if !literal_numeric_tokens_are_valid(parser.slice_from(start), true, false)
                        {
                            return Err(parser.new_custom_error(()));
                        }
                    }
                    Token::Function(_)
                    | Token::ParenthesisBlock
                    | Token::SquareBracketBlock
                    | Token::CurlyBracketBlock => {
                        parser.parse_nested_block(|nested| {
                            super::urls::consume(nested, 0, false).map(|_| ())
                        })?;
                    }
                    _ => {}
                }
            }
            Ok(color)
        },
    );
    result.is_ok_and(|layers| {
        !layers[..layers.len().saturating_sub(1)]
            .iter()
            .any(|color| *color)
    })
}

fn repeated_keyword(value: &str, only: Option<&str>) -> bool {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let mut seen = Vec::new();
    while let Ok(Token::Ident(ident)) = parser.next().cloned() {
        if only.is_some_and(|only| !ident.eq_ignore_ascii_case(only)) {
            continue;
        }
        if seen
            .iter()
            .any(|word: &CowRcStr<'_>| word.eq_ignore_ascii_case(&ident))
        {
            return true;
        }
        seen.push(ident);
    }
    false
}

fn text_transform_is_valid(source: &str) -> bool {
    if repeated_keyword(source, None) {
        return false;
    }
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    let (mut count, mut exclusive) = (0, false);
    while !parser.is_exhausted() {
        let Ok(name) = parser.expect_ident() else {
            return false;
        };
        count += 1;
        exclusive |= name.eq_ignore_ascii_case("none") || name.eq_ignore_ascii_case("math-auto");
    }
    if exclusive {
        count == 1
    } else {
        count != 0 && TextTransform::parse_string(source).is_ok()
    }
}

fn word_break_is_valid(source: &str) -> bool {
    if WordBreak::parse_string(source).is_ok() {
        return true;
    }
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    parser.expect_ident().is_ok_and(|name| {
        name.eq_ignore_ascii_case("manual") || name.eq_ignore_ascii_case("auto-phrase")
    }) && parser.is_exhausted()
}

fn list_style_type_is_valid(value: &ListStyleType<'_>) -> bool {
    let ListStyleType::CounterStyle(CounterStyle::Symbols { system, symbols }) = value else {
        return true;
    };
    symbols.len()
        >= if matches!(system, SymbolsType::Alphabetic | SymbolsType::Numeric) {
            2
        } else {
            1
        }
        && symbols
            .iter()
            .all(|symbol| !matches!(symbol, Symbol::Image(Image::None)))
}

fn exclusive_decoration_keyword(value: &str) -> bool {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let mut count = 0;
    let mut exclusive = false;
    while let Ok(Token::Ident(ident)) = parser.next() {
        count += 1;
        exclusive |= ["none", "spelling-error", "grammar-error"]
            .iter()
            .any(|keyword| ident.eq_ignore_ascii_case(keyword));
    }
    exclusive && count > 1
}

fn strict_length(value: &str) -> bool {
    if !literal_numeric_tokens_are_valid(value, false, false) {
        return false;
    }
    Length::parse_string(value).is_ok_and(|value| length_is_valid(&value))
}

fn valid_border_width(value: &BorderSideWidth) -> bool {
    match value {
        BorderSideWidth::Length(value) => length_is_valid(value),
        _ => true,
    }
}

fn valid_length_or_auto(value: &LengthPercentageOrAuto) -> bool {
    match value {
        LengthPercentageOrAuto::Auto => true,
        LengthPercentageOrAuto::LengthPercentage(value) => length_percentage_is_valid(value),
    }
}

fn top_level_component_count(value: &str) -> usize {
    let mut depth = 0usize;
    let mut in_component = false;
    let mut count = 0usize;
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '/' && characters.peek() == Some(&'*') {
            let _ = characters.next();
            let mut previous = '\0';
            for comment in characters.by_ref() {
                if previous == '*' && comment == '/' {
                    break;
                }
                previous = comment;
            }
            continue;
        }
        match character {
            '(' => {
                if depth == 0 && !in_component {
                    count += 1;
                    in_component = true;
                }
                depth += 1;
            }
            ')' => depth = depth.saturating_sub(1),
            c if c.is_ascii_whitespace() && depth == 0 => in_component = false,
            _ if !in_component => {
                count += 1;
                in_component = true;
            }
            _ => {}
        }
    }
    count
}

// These renderer properties have no typed IDs in Lightning CSS.
pub(crate) fn renderer_property_value_is_valid(
    id: RendererPropertyId,
    value: &str,
) -> Option<bool> {
    if let Some(canonical) = id.gap_property() {
        return property_value_is_valid(&canonical, value);
    }
    if !matches!(
        id,
        RendererPropertyId::Float
            | RendererPropertyId::Clear
            | RendererPropertyId::ObjectFit
            | RendererPropertyId::ObjectPosition
            | RendererPropertyId::OutlineOffset
            | RendererPropertyId::FontFeatureSettings
            | RendererPropertyId::FontKerning
            | RendererPropertyId::FontVariantNumeric
            | RendererPropertyId::FontVariantLigatures
            | RendererPropertyId::BreakBefore
            | RendererPropertyId::BreakAfter
            | RendererPropertyId::BreakInside
            | RendererPropertyId::PageBreakBefore
            | RendererPropertyId::PageBreakAfter
            | RendererPropertyId::PageBreakInside
            | RendererPropertyId::Widows
            | RendererPropertyId::Orphans
    ) {
        return None;
    }
    if css_wide_keyword(value) {
        return Some(true);
    }
    if may_have_substitutions(value) {
        match substitutions(value) {
            Ok(true) => return Some(true),
            Err(()) => return Some(false),
            Ok(false) => {}
        }
    }
    match id {
        RendererPropertyId::Float
        | RendererPropertyId::Clear
        | RendererPropertyId::ObjectFit
        | RendererPropertyId::FontFeatureSettings
        | RendererPropertyId::FontKerning
        | RendererPropertyId::FontVariantNumeric
        | RendererPropertyId::FontVariantLigatures
        | RendererPropertyId::BreakBefore
        | RendererPropertyId::BreakAfter
        | RendererPropertyId::BreakInside
        | RendererPropertyId::PageBreakBefore
        | RendererPropertyId::PageBreakAfter
        | RendererPropertyId::PageBreakInside
        | RendererPropertyId::Widows
        | RendererPropertyId::Orphans => {
            Some(renderer_owned::property_value_is_valid(id, value) == Some(true))
        }
        RendererPropertyId::ObjectPosition => {
            let top_level_components = top_level_component_count(value);
            Some(top_level_components != 3 && Position::parse_string(value).is_ok())
        }
        RendererPropertyId::OutlineOffset => {
            let mut input = ParserInput::new(value);
            let mut parser = Parser::new(&mut input);
            let inset = parser.expect_ident_matching("inset").is_ok() && parser.is_exhausted();
            Some(inset || strict_length(value))
        }
        _ => unreachable!(),
    }
}

fn targeted_property(id: &PropertyId<'_>) -> bool {
    matches!(
        id,
        PropertyId::Background
            | PropertyId::ZIndex
            | PropertyId::VerticalAlign
            | PropertyId::TextOverflow(_)
            | PropertyId::TextTransform
            | PropertyId::WordBreak
            | PropertyId::TextIndent
            | PropertyId::ListStyle
            | PropertyId::ListStyleType
            | PropertyId::JustifyItems
            | PropertyId::PlaceItems
            | PropertyId::FlexBasis(_)
            | PropertyId::FlexWrap(_)
            | PropertyId::FlexFlow(_)
            | PropertyId::Flex(_)
            | PropertyId::FlexGrow(_)
            | PropertyId::FlexShrink(_)
            | PropertyId::BorderRadius(_)
            | PropertyId::BorderTopLeftRadius(_)
            | PropertyId::BorderTopRightRadius(_)
            | PropertyId::BorderBottomRightRadius(_)
            | PropertyId::BorderBottomLeftRadius(_)
            | PropertyId::BorderStartStartRadius
            | PropertyId::BorderStartEndRadius
            | PropertyId::BorderEndStartRadius
            | PropertyId::BorderEndEndRadius
            | PropertyId::Width
            | PropertyId::Height
            | PropertyId::AspectRatio
            | PropertyId::MinWidth
            | PropertyId::MinHeight
            | PropertyId::MaxWidth
            | PropertyId::MaxHeight
            | PropertyId::BlockSize
            | PropertyId::InlineSize
            | PropertyId::MinBlockSize
            | PropertyId::MinInlineSize
            | PropertyId::MaxBlockSize
            | PropertyId::MaxInlineSize
            | PropertyId::BorderSpacing
            | PropertyId::RowGap
            | PropertyId::ColumnGap
            | PropertyId::Gap
            | PropertyId::Padding
            | PropertyId::PaddingTop
            | PropertyId::PaddingRight
            | PropertyId::PaddingBottom
            | PropertyId::PaddingLeft
            | PropertyId::PaddingBlock
            | PropertyId::PaddingBlockStart
            | PropertyId::PaddingBlockEnd
            | PropertyId::PaddingInline
            | PropertyId::PaddingInlineStart
            | PropertyId::PaddingInlineEnd
            | PropertyId::Top
            | PropertyId::Right
            | PropertyId::Bottom
            | PropertyId::Left
            | PropertyId::Inset
            | PropertyId::InsetBlock
            | PropertyId::InsetInline
            | PropertyId::InsetBlockStart
            | PropertyId::InsetBlockEnd
            | PropertyId::InsetInlineStart
            | PropertyId::InsetInlineEnd
            | PropertyId::Display
            | PropertyId::TextDecorationLine(_)
            | PropertyId::TextDecorationThickness
            | PropertyId::Outline
            | PropertyId::OutlineWidth
            | PropertyId::OutlineStyle
            | PropertyId::All
            | PropertyId::FontSize
            | PropertyId::Font
            | PropertyId::FontStyle
            | PropertyId::FontWeight
            | PropertyId::Order(_)
            | PropertyId::LineHeight
            | PropertyId::LetterSpacing
            | PropertyId::WordSpacing
            | PropertyId::Border
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
            | PropertyId::Margin
            | PropertyId::MarginTop
            | PropertyId::MarginRight
            | PropertyId::MarginBottom
            | PropertyId::MarginLeft
            | PropertyId::MarginBlock
            | PropertyId::MarginBlockStart
            | PropertyId::MarginBlockEnd
            | PropertyId::MarginInline
            | PropertyId::MarginInlineStart
            | PropertyId::MarginInlineEnd
    )
}

fn property_has_valid_range(property: &Property<'_>, source: &str) -> bool {
    if matches!(property, Property::Outline(value)
        if matches!(value.style, OutlineStyle::LineStyle(LineStyle::Hidden)))
    {
        return false;
    }
    let border_width = match property {
        Property::Outline(value) => Some(&value.width),
        Property::Border(value) => Some(&value.width),
        Property::BorderTop(value) => Some(&value.width),
        Property::BorderRight(value) => Some(&value.width),
        Property::BorderBottom(value) => Some(&value.width),
        Property::BorderLeft(value) => Some(&value.width),
        Property::BorderBlock(value) => Some(&value.width),
        Property::BorderInline(value) => Some(&value.width),
        Property::BorderBlockStart(value) => Some(&value.width),
        Property::BorderBlockEnd(value) => Some(&value.width),
        Property::BorderInlineStart(value) => Some(&value.width),
        Property::BorderInlineEnd(value) => Some(&value.width),
        _ => None,
    };
    if let Some(width) = border_width {
        return literal_numeric_tokens_are_valid(source, true, false) && valid_border_width(width);
    }
    match property {
        Property::Background(_) => background_layers_are_valid(source),
        Property::ListStyleType(value) => list_style_type_is_valid(value),
        Property::ListStyle(value) => list_style_type_is_valid(&value.list_style_type),
        Property::TextIndent(value) => {
            literal_numeric_tokens_are_valid(source, false, false)
                && length_percentage_is_valid(&value.value)
        }
        Property::BorderRadius(value, _) => {
            literal_numeric_tokens_are_valid(source, true, false)
                && [
                    &value.top_left,
                    &value.top_right,
                    &value.bottom_right,
                    &value.bottom_left,
                ]
                .into_iter()
                .all(valid_radius)
        }
        Property::BorderTopLeftRadius(value, _)
        | Property::BorderTopRightRadius(value, _)
        | Property::BorderBottomRightRadius(value, _)
        | Property::BorderBottomLeftRadius(value, _)
        | Property::BorderStartStartRadius(value)
        | Property::BorderStartEndRadius(value)
        | Property::BorderEndStartRadius(value)
        | Property::BorderEndEndRadius(value) => {
            literal_numeric_tokens_are_valid(source, true, false) && valid_radius(value)
        }
        Property::RowGap(value) | Property::ColumnGap(value) => {
            literal_numeric_tokens_are_valid(source, true, false) && valid_gap(value)
        }
        Property::Gap(value) => {
            literal_numeric_tokens_are_valid(source, true, false)
                && valid_gap(&value.row)
                && valid_gap(&value.column)
        }
        Property::BorderSpacing(value) => {
            literal_numeric_tokens_are_valid(source, true, false)
                && length_is_valid(&value.0)
                && length_is_valid(&value.1)
        }
        Property::BorderBlockWidth(value) => {
            literal_numeric_tokens_are_valid(source, true, false)
                && valid_border_width(&value.start)
                && valid_border_width(&value.end)
        }
        Property::BorderInlineWidth(value) => {
            literal_numeric_tokens_are_valid(source, true, false)
                && valid_border_width(&value.start)
                && valid_border_width(&value.end)
        }
        Property::FontSize(value) => {
            literal_numeric_tokens_are_valid(source, true, false)
                && match value {
                    FontSize::Length(value) => length_percentage_is_valid(value),
                    _ => true,
                }
        }
        Property::Font(_) => font::shorthand_literals_are_valid(source),
        Property::LineHeight(value) => {
            literal_numeric_tokens_are_valid(source, true, true)
                && match value {
                    LineHeight::Length(value) => length_percentage_or_number_is_valid(value),
                    _ => true,
                }
        }
        Property::BorderWidth(value) => {
            literal_numeric_tokens_are_valid(source, true, false)
                && [&value.top, &value.right, &value.bottom, &value.left]
                    .into_iter()
                    .all(valid_border_width)
        }
        Property::OutlineWidth(value)
        | Property::BorderTopWidth(value)
        | Property::BorderRightWidth(value)
        | Property::BorderBottomWidth(value)
        | Property::BorderLeftWidth(value)
        | Property::BorderBlockStartWidth(value)
        | Property::BorderBlockEndWidth(value)
        | Property::BorderInlineStartWidth(value)
        | Property::BorderInlineEndWidth(value) => {
            literal_numeric_tokens_are_valid(source, true, false) && valid_border_width(value)
        }
        Property::MarginTop(value)
        | Property::MarginRight(value)
        | Property::MarginBottom(value)
        | Property::MarginLeft(value)
        | Property::MarginBlockStart(value)
        | Property::MarginBlockEnd(value)
        | Property::MarginInlineStart(value)
        | Property::MarginInlineEnd(value)
        | Property::Top(value)
        | Property::Right(value)
        | Property::Bottom(value)
        | Property::Left(value)
        | Property::InsetBlockStart(value)
        | Property::InsetBlockEnd(value)
        | Property::InsetInlineStart(value)
        | Property::InsetInlineEnd(value) => {
            valid_length_or_auto(value) && literal_numeric_tokens_are_valid(source, false, false)
        }
        Property::Margin(value) => {
            literal_numeric_tokens_are_valid(source, false, false)
                && [&value.top, &value.right, &value.bottom, &value.left]
                    .into_iter()
                    .all(valid_length_or_auto)
        }
        Property::MarginBlock(value) => {
            literal_numeric_tokens_are_valid(source, false, false)
                && [&value.block_start, &value.block_end]
                    .into_iter()
                    .all(valid_length_or_auto)
        }
        Property::MarginInline(value) => {
            literal_numeric_tokens_are_valid(source, false, false)
                && [&value.inline_start, &value.inline_end]
                    .into_iter()
                    .all(valid_length_or_auto)
        }
        Property::Inset(value) => {
            literal_numeric_tokens_are_valid(source, false, false)
                && [&value.top, &value.right, &value.bottom, &value.left]
                    .into_iter()
                    .all(valid_length_or_auto)
        }
        Property::InsetBlock(value) => {
            literal_numeric_tokens_are_valid(source, false, false)
                && [&value.block_start, &value.block_end]
                    .into_iter()
                    .all(valid_length_or_auto)
        }
        Property::InsetInline(value) => {
            literal_numeric_tokens_are_valid(source, false, false)
                && [&value.inline_start, &value.inline_end]
                    .into_iter()
                    .all(valid_length_or_auto)
        }
        Property::Width(value)
        | Property::Height(value)
        | Property::MinWidth(value)
        | Property::MinHeight(value)
        | Property::BlockSize(value)
        | Property::InlineSize(value)
        | Property::MinBlockSize(value)
        | Property::MinInlineSize(value) => size_value_is_valid(value, source),
        Property::MaxWidth(value)
        | Property::MaxHeight(value)
        | Property::MaxBlockSize(value)
        | Property::MaxInlineSize(value) => max_size_value_is_valid(value, source),
        Property::AspectRatio(_) => literal_numeric_tokens_are_valid(source, true, true),
        Property::PaddingTop(value)
        | Property::PaddingRight(value)
        | Property::PaddingBottom(value)
        | Property::PaddingLeft(value)
        | Property::PaddingBlockStart(value)
        | Property::PaddingBlockEnd(value)
        | Property::PaddingInlineStart(value)
        | Property::PaddingInlineEnd(value) => padding_value_is_non_negative(value),
        Property::Padding(value) => [&value.top, &value.right, &value.bottom, &value.left]
            .into_iter()
            .all(padding_value_is_non_negative),
        Property::PaddingBlock(value) => [&value.block_start, &value.block_end]
            .into_iter()
            .all(padding_value_is_non_negative),
        Property::PaddingInline(value) => [&value.inline_start, &value.inline_end]
            .into_iter()
            .all(padding_value_is_non_negative),
        _ => true,
    }
}

fn valid_radius(value: &Size2D<LengthPercentage>) -> bool {
    length_percentage_is_valid(&value.0) && length_percentage_is_valid(&value.1)
}

fn valid_gap(value: &GapValue) -> bool {
    match value {
        GapValue::Normal => true,
        GapValue::LengthPercentage(value) => length_percentage_is_valid(value),
    }
}

fn size_value_is_valid(value: &Size, source: &str) -> bool {
    match value {
        Size::LengthPercentage(length) | Size::FitContentFunction(length) => {
            sizing_length_is_valid(length, source)
        }
        _ => true,
    }
}

fn max_size_value_is_valid(value: &MaxSize, source: &str) -> bool {
    match value {
        MaxSize::LengthPercentage(length) | MaxSize::FitContentFunction(length) => {
            sizing_length_is_valid(length, source)
        }
        _ => true,
    }
}

fn sizing_length_is_valid(length: &LengthPercentage, source: &str) -> bool {
    if !length_percentage_is_valid(length) || !literal_numeric_tokens_are_valid(source, true, false)
    {
        return false;
    }
    // fit-content() wraps a restricted length. Math functions inside it still
    // accept negative operands and clamp their result at computed/used time.
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    if parser
        .try_parse(|parser| parser.expect_function_matching("fit-content"))
        .is_err()
    {
        return true;
    }
    parser
        .parse_nested_block(|nested| {
            let start = nested.position();
            while nested.next().is_ok() {}
            if literal_numeric_tokens_are_valid(nested.slice_from(start), true, false) {
                Ok(())
            } else {
                Err(nested.new_custom_error::<(), ()>(()))
            }
        })
        .is_ok()
}

fn padding_value_is_non_negative(value: &LengthPercentageOrAuto) -> bool {
    match value {
        LengthPercentageOrAuto::Auto => false,
        LengthPercentageOrAuto::LengthPercentage(length) => !length_is_negative(length),
    }
}

fn length_is_negative(value: &LengthPercentage) -> bool {
    // CSS range restrictions compare the numeric value with zero. IEEE -0.0
    // therefore remains a valid zero rather than a negative length.
    value.try_sign().is_some_and(|sign| sign < 0.0)
}

#[cfg(test)]
mod tests {
    use lightningcss::properties::PropertyId;

    fn property_value_is_valid(name: &str, value: &str) -> Option<bool> {
        super::property_value_is_valid(&PropertyId::from(name), value)
    }

    #[test]
    fn rejects_recovered_box_values_without_rejecting_valid_ranges() {
        assert_eq!(property_value_is_valid("float", "left right"), Some(false));
        assert_eq!(property_value_is_valid("float", "inline-start"), Some(true));
        assert_eq!(property_value_is_valid("width", "-10px"), Some(false));
        assert_eq!(property_value_is_valid("max-height", "-1px"), Some(false));
        assert_eq!(property_value_is_valid("max-height", "-0px"), Some(true));
        assert_eq!(property_value_is_valid("width", "60"), Some(false));
        assert_eq!(property_value_is_valid("width", "20%"), Some(true));
        assert_eq!(property_value_is_valid("padding", "auto"), Some(false));
        assert_eq!(property_value_is_valid("padding", "10px 20%"), Some(true));
        assert_eq!(
            property_value_is_valid("object-fit", "scale-down"),
            Some(true)
        );
        assert_eq!(
            property_value_is_valid("object-fit", "contain cover"),
            Some(false)
        );
        assert_eq!(
            property_value_is_valid("object-position", "right 10px bottom 20%"),
            Some(true)
        );
        assert_eq!(
            property_value_is_valid("object-position", "left right"),
            Some(false)
        );
        assert_eq!(
            property_value_is_valid("outline-offset", "-2px"),
            Some(true)
        );
        assert_eq!(
            property_value_is_valid("outline-offset", "inset"),
            Some(true)
        );
        assert_eq!(
            property_value_is_valid("outline-offset", "20%"),
            Some(false)
        );
        assert_eq!(property_value_is_valid("color", "red"), None);
    }
}
