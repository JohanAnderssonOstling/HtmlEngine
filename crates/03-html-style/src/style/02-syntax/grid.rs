//! Grid value grammars independent of layout and native parser recovery.

use super::sign;
use super::substitution::{contains_function, is_random_math};
use super::value::{
    IntegerValue, length_percentage_is_valid, literal_numeric_tokens_are_valid, numeric_component,
    parse_integer_value, parse_signed_integer,
};
use cssparser::{ParseError, Parser, ParserInput, Token};
use lightningcss::properties::custom::{Token as CssToken, TokenList, TokenOrValue};
use lightningcss::properties::grid::{
    GridLine, GridTemplateAreas, RepeatCount, TrackBreadth, TrackListItem, TrackSize, TrackSizing,
};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::traits::Parse;
use lightningcss::values::ident::CustomIdent;
use lightningcss::values::length::LengthPercentage;
use std::collections::HashMap;

pub(crate) fn supports(id: &PropertyId<'_>) -> bool {
    matches!(
        id,
        PropertyId::GridTemplateRows
            | PropertyId::GridTemplateColumns
            | PropertyId::GridAutoRows
            | PropertyId::GridAutoColumns
            | PropertyId::GridAutoFlow
            | PropertyId::GridTemplateAreas
            | PropertyId::GridTemplate
            | PropertyId::Grid
            | PropertyId::GridRowStart
            | PropertyId::GridRowEnd
            | PropertyId::GridColumnStart
            | PropertyId::GridColumnEnd
            | PropertyId::GridRow
            | PropertyId::GridColumn
            | PropertyId::GridArea
    )
}

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    if !supports(id) {
        return None;
    }
    let Ok(value) = sign::lower(value, |_| Some(1.0)) else {
        return Some(false);
    };
    let mut input = ParserInput::new(&value);
    Some(
        Parser::new(&mut input)
            .parse_entirely(|parser| match id {
                PropertyId::GridTemplateRows | PropertyId::GridTemplateColumns => axis(parser),
                PropertyId::GridAutoRows | PropertyId::GridAutoColumns => auto_tracks(parser, true),
                PropertyId::GridAutoFlow => auto_flow(parser, false),
                PropertyId::GridTemplateAreas => {
                    let areas = GridTemplateAreas::parse(parser)
                        .map_err(|_| parser.new_custom_error(()))?;
                    areas_are_rectangular(&areas)
                        .then_some(())
                        .ok_or_else(|| parser.new_custom_error(()))
                }
                PropertyId::GridTemplate => template(parser),
                PropertyId::Grid => {
                    if parser.try_parse(template).is_ok() {
                        return Ok(());
                    }
                    if parser
                        .try_parse(|parser| {
                            before_slash(parser, axis)?;
                            parser.expect_delim('/')?;
                            implicit_axis(parser)?;
                            parser.expect_exhausted()?;
                            Ok::<_, ParseError<'_, ()>>(())
                        })
                        .is_ok()
                    {
                        return Ok(());
                    }
                    before_slash(parser, implicit_axis)?;
                    parser.expect_delim('/')?;
                    axis(parser)
                }
                _ => {
                    let limit = match id {
                        PropertyId::GridArea => 4,
                        PropertyId::GridRow | PropertyId::GridColumn => 2,
                        _ => 1,
                    };
                    for count in 0..limit {
                        if count != 0 {
                            parser.expect_delim('/')?;
                        }
                        before_slash(parser, line)?;
                        if parser.is_exhausted() {
                            return Ok(());
                        }
                    }
                    Err(parser.new_custom_error(()))
                }
            })
            .is_ok(),
    )
}

#[derive(Default)]
struct Tracks {
    count: usize,
    all_fixed: bool,
    auto_repeats: usize,
}

fn before_slash<'i>(
    parser: &mut Parser<'i, '_>,
    parse: impl FnOnce(&mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>>,
) -> Result<(), ParseError<'i, ()>> {
    let start = parser.position();
    loop {
        let state = parser.state();
        match parser.next().cloned() {
            Ok(Token::Delim('/')) => {
                parser.reset(&state);
                break;
            }
            Ok(
                Token::Function(_)
                | Token::SquareBracketBlock
                | Token::ParenthesisBlock
                | Token::CurlyBracketBlock,
            ) => {
                parser.parse_nested_block(|parser| {
                    super::urls::consume(parser, 0, false).map(|_| ())
                })?;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let mut input = ParserInput::new(parser.slice_from(start));
    Parser::new(&mut input).parse_entirely(parse)
}

fn axis<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("none"))
        .is_ok()
    {
        return Ok(());
    }
    if parser
        .try_parse(|parser| parser.expect_ident_matching("subgrid"))
        .is_ok()
    {
        let mut auto_repeats = 0;
        while !parser.is_exhausted() {
            if parser.try_parse(line_names).is_ok() {
                continue;
            }
            parser.expect_function_matching("repeat")?;
            auto_repeats += usize::from(parser.parse_nested_block(|parser| {
                let automatic = repeat_count(parser, true)?;
                parser.expect_comma()?;
                line_names(parser)?;
                while !parser.is_exhausted() {
                    line_names(parser)?;
                }
                Ok(automatic)
            })?);
            if auto_repeats > 1 {
                return Err(parser.new_custom_error(()));
            }
        }
        return Ok(());
    }
    track_list(parser, true).map(|_| ())
}

fn line_names<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    parser.expect_square_bracket_block()?;
    parser.parse_nested_block(|parser| {
        while !parser.is_exhausted() {
            line_name(parser)?;
        }
        Ok(())
    })
}

fn line_name<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let name = CustomIdent::parse(parser).map_err(|_| parser.new_custom_error(()))?;
    if reserved_name(name.0.as_ref()) {
        return Err(parser.new_custom_error(()));
    }
    Ok(())
}

fn reserved_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("span") || name.eq_ignore_ascii_case("auto")
}

fn track_list<'i>(
    parser: &mut Parser<'i, '_>,
    repeats: bool,
) -> Result<Tracks, ParseError<'i, ()>> {
    let mut result = Tracks {
        all_fixed: true,
        ..Tracks::default()
    };
    let _ = parser.try_parse(line_names);
    while !parser.is_exhausted() {
        let fixed = if repeats
            && parser
                .try_parse(|parser| parser.expect_function_matching("repeat"))
                .is_ok()
        {
            let (automatic, tracks) = parser.parse_nested_block(|parser| {
                let automatic = repeat_count(parser, false)?;
                parser.expect_comma()?;
                let tracks = track_list(parser, false)?;
                Ok((automatic, tracks))
            })?;
            result.auto_repeats += usize::from(automatic);
            // Grid 3 relaxes the auto-repeat fragment to <track-size>+.
            // Tracks outside it (including integer repeats) must stay fixed.
            automatic || tracks.all_fixed
        } else {
            track_size(parser)?
        };
        result.count += 1;
        result.all_fixed &= fixed;
        let _ = parser.try_parse(line_names);
    }
    if result.count == 0
        || result.auto_repeats > 1
        || (result.auto_repeats != 0 && !result.all_fixed)
    {
        return Err(parser.new_custom_error(()));
    }
    Ok(result)
}

fn repeat_count<'i>(parser: &mut Parser<'i, '_>, names: bool) -> Result<bool, ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("auto-fill"))
        .is_ok()
    {
        return Ok(true);
    }
    if !names
        && parser
            .try_parse(|parser| parser.expect_ident_matching("auto-fit"))
            .is_ok()
    {
        return Ok(true);
    }
    parse_integer_value(parser, 1)?;
    Ok(false)
}

#[derive(Clone, Copy, PartialEq)]
enum Breadth {
    Fixed,
    Flex,
    Intrinsic,
}

fn breadth<'i>(parser: &mut Parser<'i, '_>, flex: bool) -> Result<Breadth, ParseError<'i, ()>> {
    let state = parser.state();
    if let Ok(name) = parser.try_parse(|parser| parser.expect_ident_cloned()) {
        if ["auto", "min-content", "max-content"]
            .iter()
            .any(|word| name.eq_ignore_ascii_case(word))
        {
            return Ok(Breadth::Intrinsic);
        }
        parser.reset(&state);
    }
    if flex {
        if let Ok(Token::Dimension { value, unit, .. }) =
            parser.try_parse(|parser| parser.next().cloned())
        {
            if unit.eq_ignore_ascii_case("fr") && value >= 0.0 {
                return Ok(Breadth::Flex);
            }
        }
        parser.reset(&state);
    }
    length(parser)?;
    Ok(Breadth::Fixed)
}

fn length<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let start = parser.position();
    let value = LengthPercentage::parse(parser).map_err(|_| parser.new_custom_error(()))?;
    if !length_percentage_is_valid(&value)
        || !literal_numeric_tokens_are_valid(parser.slice_from(start), true, false)
    {
        return Err(parser.new_custom_error(()));
    }
    Ok(())
}

fn track_size<'i>(parser: &mut Parser<'i, '_>) -> Result<bool, ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_function_matching("minmax"))
        .is_ok()
    {
        return parser.parse_nested_block(|parser| {
            let min = breadth(parser, false)?;
            parser.expect_comma()?;
            let max = breadth(parser, true)?;
            Ok(min == Breadth::Fixed || max == Breadth::Fixed)
        });
    }
    if parser
        .try_parse(|parser| parser.expect_function_matching("fit-content"))
        .is_ok()
    {
        parser.parse_nested_block(length)?;
        return Ok(false);
    }
    breadth(parser, true).map(|value| value == Breadth::Fixed)
}

fn auto_tracks<'i>(parser: &mut Parser<'i, '_>, required: bool) -> Result<(), ParseError<'i, ()>> {
    if required {
        track_size(parser)?;
    }
    while !parser.is_exhausted() {
        track_size(parser)?;
    }
    Ok(())
}

fn auto_flow<'i>(parser: &mut Parser<'i, '_>, implicit: bool) -> Result<(), ParseError<'i, ()>> {
    let (mut direction, mut dense) = (false, false);
    while !parser.is_exhausted() {
        let state = parser.state();
        let Ok(name) = parser.try_parse(|parser| parser.expect_ident_cloned()) else {
            break;
        };
        if name.eq_ignore_ascii_case("dense") && !dense {
            dense = true;
        } else if !direction
            && ((implicit && name.eq_ignore_ascii_case("auto-flow"))
                || (!implicit
                    && (name.eq_ignore_ascii_case("row") || name.eq_ignore_ascii_case("column"))))
        {
            direction = true;
        } else {
            parser.reset(&state);
            break;
        }
    }
    if (!implicit && (direction || dense)) || (implicit && direction) {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

fn implicit_axis<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    auto_flow(parser, true)?;
    auto_tracks(parser, false)
}

fn template<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| {
            parser.expect_ident_matching("none")?;
            parser.expect_exhausted()
        })
        .is_ok()
    {
        return Ok(());
    }
    if parser.try_parse(area_template).is_ok() {
        return Ok(());
    }
    before_slash(parser, axis)?;
    parser.expect_delim('/')?;
    axis(parser)
}

fn area_template<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let mut rows = Vec::new();
    while let Ok(row) = parser.try_parse(|parser| {
        let _ = parser.try_parse(line_names);
        let start = parser.position();
        parser.expect_string()?;
        let row = parser.slice_from(start).to_owned();
        let _ = parser.try_parse(track_size);
        let _ = parser.try_parse(line_names);
        Ok::<_, ParseError<'i, ()>>(row)
    }) {
        rows.push(row);
    }
    if rows.is_empty() {
        return Err(parser.new_custom_error(()));
    }
    let areas: GridTemplateAreas =
        Parse::parse_string(&rows.join(" ")).map_err(|_| parser.new_custom_error(()))?;
    if !areas_are_rectangular(&areas) {
        return Err(parser.new_custom_error(()));
    }
    if parser.try_parse(|parser| parser.expect_delim('/')).is_ok() {
        track_list(parser, false)?;
    }
    parser.expect_exhausted()?;
    Ok(())
}

fn areas_are_rectangular(value: &GridTemplateAreas) -> bool {
    let GridTemplateAreas::Areas { columns, areas } = value else {
        return true;
    };
    let columns = *columns as usize;
    if columns == 0 || areas.is_empty() || areas.len() % columns != 0 {
        return false;
    }
    let mut bounds = HashMap::<&str, (usize, usize, usize, usize, usize)>::new();
    for (index, name) in areas.iter().enumerate() {
        if let Some(name) = name {
            let (row, column) = (index / columns, index % columns);
            let entry = bounds.entry(name).or_insert((row, row, column, column, 0));
            entry.0 = entry.0.min(row);
            entry.1 = entry.1.max(row);
            entry.2 = entry.2.min(column);
            entry.3 = entry.3.max(column);
            entry.4 += 1;
        }
    }
    bounds
        .values()
        .all(|(r0, r1, c0, c1, count)| (r1 - r0 + 1) * (c1 - c0 + 1) == *count)
}

fn line<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let (mut span, mut number, mut name) = (false, None, false);
    while !parser.is_exhausted() {
        if number.is_none() {
            if let Ok(value) = parser.try_parse(|parser| {
                let source = numeric_component(parser)?;
                parse_signed_integer(source).ok_or_else(|| parser.new_custom_error(()))
            }) {
                number = Some(value);
                continue;
            }
        }
        if !span
            && parser
                .try_parse(|parser| parser.expect_ident_matching("span"))
                .is_ok()
        {
            // span && [integer || name] keeps the inner number/name group
            // together; span may precede or follow it, but not split it.
            if number.is_some() || name {
                parser.expect_exhausted()?;
            }
            span = true;
            continue;
        }
        if !name
            && !span
            && number.is_none()
            && parser
                .try_parse(|parser| {
                    parser.expect_ident_matching("auto")?;
                    parser.expect_exhausted()
                })
                .is_ok()
        {
            return Ok(());
        }
        if name {
            return Err(parser.new_custom_error(()));
        }
        line_name(parser)?;
        name = true;
    }
    if number.is_none() && !name {
        return Err(parser.new_custom_error(()));
    }
    if let Some(IntegerValue::Integer(index)) = number {
        if index == 0 || (span && index < 1) {
            return Err(parser.new_custom_error(()));
        }
    }
    Ok(())
}

pub(crate) fn raw_is_unsupported(id: &PropertyId<'_>, tokens: &TokenList<'_>) -> bool {
    supports(id)
        && (tokens_use_subgrid(id, tokens)
            || !contains_function(tokens, |name| {
                is_random_math(name)
                    || name.eq_ignore_ascii_case("sibling-index")
                    || name.eq_ignore_ascii_case("sibling-count")
            }))
}

fn tokens_use_subgrid(id: &PropertyId<'_>, tokens: &TokenList<'_>) -> bool {
    if !matches!(
        id,
        PropertyId::GridTemplateRows
            | PropertyId::GridTemplateColumns
            | PropertyId::GridTemplate
            | PropertyId::Grid
    ) {
        return false;
    }
    let mut depth = 0usize;
    for token in &tokens.0 {
        match token {
            TokenOrValue::Token(
                CssToken::SquareBracketBlock
                | CssToken::ParenthesisBlock
                | CssToken::CurlyBracketBlock,
            ) => depth += 1,
            TokenOrValue::Token(
                CssToken::CloseSquareBracket
                | CssToken::CloseParenthesis
                | CssToken::CloseCurlyBracket,
            ) => depth = depth.saturating_sub(1),
            TokenOrValue::Token(CssToken::Ident(name))
                if depth == 0 && name.eq_ignore_ascii_case("subgrid") =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

pub(crate) fn native_value_is_unsupported(property: &Property<'_>) -> bool {
    let bad = |line: &GridLine<'_>| match line {
        GridLine::Auto => false,
        GridLine::Area { name } => reserved_name(name.0.as_ref()),
        GridLine::Line { index, name } => {
            *index == 0
                || i16::try_from(*index).is_err()
                || name
                    .as_ref()
                    .is_some_and(|name| reserved_name(name.0.as_ref()))
        }
        GridLine::Span { index, name } => {
            *index <= 0
                || u16::try_from(*index).is_err()
                || name
                    .as_ref()
                    .is_some_and(|name| reserved_name(name.0.as_ref()))
        }
    };
    match property {
        Property::GridTemplateRows(tracks) | Property::GridTemplateColumns(tracks) => {
            unsupported_tracks(tracks)
        }
        Property::GridTemplate(value) => {
            unsupported_tracks(&value.rows) || unsupported_tracks(&value.columns)
        }
        Property::Grid(value) => {
            unsupported_tracks(&value.rows) || unsupported_tracks(&value.columns)
        }
        Property::GridRowStart(line)
        | Property::GridRowEnd(line)
        | Property::GridColumnStart(line)
        | Property::GridColumnEnd(line) => bad(line),
        Property::GridRow(value) => bad(&value.start) || bad(&value.end),
        Property::GridColumn(value) => bad(&value.start) || bad(&value.end),
        Property::GridArea(value) => {
            bad(&value.row_start)
                || bad(&value.row_end)
                || bad(&value.column_start)
                || bad(&value.column_end)
        }
        _ => false,
    }
}

fn unsupported_tracks(value: &TrackSizing<'_>) -> bool {
    let TrackSizing::TrackList(list) = value else {
        return false;
    };
    list.items.iter().any(|item| {
        matches!(item, TrackListItem::TrackRepeat(repeat)
            if matches!(repeat.count, RepeatCount::AutoFill | RepeatCount::AutoFit)
                && !repeat.track_sizes.iter().all(native_fixed_size))
    })
}

fn native_fixed_size(size: &TrackSize) -> bool {
    match size {
        TrackSize::TrackBreadth(TrackBreadth::Length(_)) => true,
        TrackSize::MinMax { min, max } => {
            matches!(min, TrackBreadth::Length(_)) || matches!(max, TrackBreadth::Length(_))
        }
        _ => false,
    }
}
