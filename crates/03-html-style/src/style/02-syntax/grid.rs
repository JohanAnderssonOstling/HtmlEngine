//! Renderer limits and validity checks on the native grid AST.

use super::substitution::{contains_function, is_random_math};
use super::value::parsed_property;
use lightningcss::properties::custom::{Token as CssToken, TokenList, TokenOrValue};
use lightningcss::properties::grid::{
    GridLine, GridTemplateAreas, RepeatCount, TrackBreadth, TrackListItem, TrackSize, TrackSizing,
};
use lightningcss::properties::{Property, PropertyId};
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
    let property = parsed_property(id.clone(), value)?;
    Some(match &property {
        Property::Unparsed(_) | Property::Custom(_) => false,
        Property::GridTemplateAreas(areas) => areas_are_rectangular(areas),
        Property::GridTemplate(grid) => {
            areas_are_rectangular(&grid.areas) && !native_value_is_unsupported(&property)
        }
        Property::Grid(grid) => {
            areas_are_rectangular(&grid.areas) && !native_value_is_unsupported(&property)
        }
        _ => !native_value_is_unsupported(&property),
    })
}

fn reserved_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("span") || name.eq_ignore_ascii_case("auto")
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
