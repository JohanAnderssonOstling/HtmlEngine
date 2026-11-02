use super::super::context::ResolutionContext;
use super::lengths::computed_length_pct;
use super::non_negative_length_pct;
use crate::style::syntax::grid::auto_repeat_is_supported;
use html_style_model::{
    ComputedStylesBuilder, GridAutoFlow, GridPlacement, GridRepeatCount, GridTemplateArea,
    GridTemplateTrack, GridTrackBreadth, GridTrackSize, StyleStringId,
};
use lightningcss::values::ident::CustomIdentList;
use std::num::{NonZeroI16, NonZeroU16};

pub(in crate::style::cascade::resolver) fn grid_auto_flow(
    value: lightningcss::properties::grid::GridAutoFlow,
) -> GridAutoFlow {
    use lightningcss::properties::grid::GridAutoFlow as Lc;
    match (value.contains(Lc::Column), value.contains(Lc::Dense)) {
        (false, false) => GridAutoFlow::Row,
        (true, false) => GridAutoFlow::Column,
        (false, true) => GridAutoFlow::RowDense,
        (true, true) => GridAutoFlow::ColumnDense,
    }
}

pub(in crate::style::cascade::resolver) fn grid_template_tracks(
    styles: &mut ComputedStylesBuilder,
    value: &lightningcss::properties::grid::TrackSizing<'_>,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<(Vec<GridTemplateTrack>, Vec<Vec<StyleStringId>>)> {
    use lightningcss::properties::grid::{RepeatCount, TrackListItem, TrackSizing};
    let TrackSizing::TrackList(list) = value else {
        return Some((Vec::new(), Vec::new()));
    };
    if list.items.is_empty() {
        return None;
    }
    let mut intern_names = |sets: &[CustomIdentList<'_>]| -> Vec<Vec<StyleStringId>> {
        sets.iter()
            .map(|set| {
                set.iter()
                    .map(|name| styles.intern_string(name.0.as_ref()))
                    .collect()
            })
            .collect()
    };
    let names = intern_names(&list.line_names);
    let mut tracks = Vec::with_capacity(list.items.len());
    for item in &list.items {
        tracks.push(match item {
            TrackListItem::TrackSize(size) => GridTemplateTrack::Single(grid_track_size(
                size,
                font_size,
                root_font_size,
                resolution,
            )?),
            TrackListItem::TrackRepeat(repeat) => {
                let count = match repeat.count {
                    RepeatCount::Number(value) => {
                        GridRepeatCount::Count(NonZeroU16::new(u16::try_from(value).ok()?)?)
                    }
                    RepeatCount::AutoFill => GridRepeatCount::AutoFill,
                    RepeatCount::AutoFit => GridRepeatCount::AutoFit,
                };
                let tracks = repeat
                    .track_sizes
                    .iter()
                    .map(|size| grid_track_size(size, font_size, root_font_size, resolution))
                    .collect::<Option<Vec<_>>>()?;
                if tracks.is_empty() {
                    return None;
                }
                let line_names = intern_names(&repeat.line_names);
                GridTemplateTrack::Repeat {
                    count,
                    tracks,
                    line_names,
                }
            }
        });
    }
    if !auto_repeat_is_supported(list) {
        return None;
    }
    Some((tracks, names))
}

pub(in crate::style::cascade::resolver) fn grid_auto_tracks(
    value: &lightningcss::properties::grid::TrackSizeList,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<Vec<GridTrackSize>> {
    value
        .0
        .iter()
        .map(|size| grid_track_size(size, font_size, root_font_size, resolution))
        .collect()
}

pub(in crate::style::cascade::resolver) fn grid_track_size(
    value: &lightningcss::properties::grid::TrackSize,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<GridTrackSize> {
    use lightningcss::properties::grid::TrackSize;
    Some(match value {
        TrackSize::TrackBreadth(value) => GridTrackSize::Breadth(grid_track_breadth(
            value,
            font_size,
            root_font_size,
            resolution,
        )?),
        TrackSize::MinMax { min, max } => GridTrackSize::MinMax {
            min: grid_track_breadth(min, font_size, root_font_size, resolution)?,
            max: grid_track_breadth(max, font_size, root_font_size, resolution)?,
        },
        TrackSize::FitContent(value) => GridTrackSize::FitContent(non_negative_length_pct(
            computed_length_pct(value, font_size, root_font_size, resolution)?,
        )?),
    })
}

pub(in crate::style::cascade::resolver) fn grid_track_breadth(
    value: &lightningcss::properties::grid::TrackBreadth,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<GridTrackBreadth> {
    use lightningcss::properties::grid::TrackBreadth;
    Some(match value {
        TrackBreadth::Length(value) => GridTrackBreadth::Length(non_negative_length_pct(
            computed_length_pct(value, font_size, root_font_size, resolution)?,
        )?),
        TrackBreadth::Flex(value) if value.is_finite() && *value >= 0.0 => {
            GridTrackBreadth::Flex(*value)
        }
        TrackBreadth::Flex(_) => return None,
        TrackBreadth::MinContent => GridTrackBreadth::MinContent,
        TrackBreadth::MaxContent => GridTrackBreadth::MaxContent,
        TrackBreadth::Auto => GridTrackBreadth::Auto,
    })
}

pub(in crate::style::cascade::resolver) fn grid_placement(
    styles: &mut ComputedStylesBuilder,
    value: &lightningcss::properties::grid::GridLine<'_>,
) -> Option<GridPlacement> {
    use lightningcss::properties::grid::GridLine;
    Some(match value {
        GridLine::Auto => GridPlacement::Auto,
        GridLine::Area { name } => GridPlacement::NamedLine {
            name: styles.intern_string(name.0.as_ref()),
            index: 0,
        },
        GridLine::Line { index, name: None } => {
            GridPlacement::Line(NonZeroI16::new(i16::try_from(*index).ok()?)?)
        }
        GridLine::Line {
            index,
            name: Some(name),
        } => GridPlacement::NamedLine {
            name: styles.intern_string(name.0.as_ref()),
            index: i16::try_from(*index).ok()?,
        },
        GridLine::Span { index, name: None } => {
            GridPlacement::Span(NonZeroU16::new(u16::try_from(*index).ok()?)?)
        }
        GridLine::Span {
            index,
            name: Some(name),
        } => GridPlacement::NamedSpan {
            name: styles.intern_string(name.0.as_ref()),
            count: NonZeroU16::new(u16::try_from(*index).ok()?)?,
        },
    })
}

pub(in crate::style::cascade::resolver) fn grid_template_areas(
    styles: &mut ComputedStylesBuilder,
    value: &lightningcss::properties::grid::GridTemplateAreas,
) -> Option<(Vec<GridTemplateArea>, u16, u16)> {
    use lightningcss::properties::grid::GridTemplateAreas;
    let GridTemplateAreas::Areas { columns, areas } = value else {
        return Some((Vec::new(), 0, 0));
    };
    let columns = usize::try_from(*columns).ok()?;
    if columns == 0 || areas.len() % columns != 0 {
        return None;
    }
    let rows = areas.len() / columns;
    let mut found: Vec<(&str, usize, usize, usize, usize, usize)> = Vec::new();
    for (row, cells) in areas.chunks_exact(columns).enumerate() {
        for (column, name) in cells.iter().enumerate() {
            let Some(name) = name.as_deref() else {
                continue;
            };
            if let Some((_, _, row_end, column_start, column_end, count)) =
                found.iter_mut().find(|entry| entry.0 == name)
            {
                *row_end = row + 1;
                *count += 1;
                *column_start = (*column_start).min(column);
                *column_end = (*column_end).max(column + 1);
            } else {
                found.push((name, row, row + 1, column, column + 1, 1));
            }
        }
    }
    let mut output = Vec::with_capacity(found.len());
    for (name, row_start, row_end, column_start, column_end, count) in found {
        // Every occurrence lies within these bounds. Matching their area means
        // the name fills the rectangle without holes or disconnected cells.
        if count != (row_end - row_start) * (column_end - column_start) {
            return None;
        }
        output.push(GridTemplateArea {
            name: styles.intern_string(name),
            // Renderer grid coordinates are CSS grid-line numbers (one based),
            // matching Taffy's named-area resolver rather than the zero-based
            // row/column offsets used while validating the source matrix.
            row_start: u16::try_from(row_start + 1).ok()?,
            row_end: u16::try_from(row_end + 1).ok()?,
            column_start: u16::try_from(column_start + 1).ok()?,
            column_end: u16::try_from(column_end + 1).ok()?,
        });
    }
    Some((
        output,
        u16::try_from(rows).ok()?,
        u16::try_from(columns).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocation_test_support::count_allocations;
    use html_parse::parse_dom_document;
    use lightningcss::properties::grid::GridTemplateAreas;
    use lightningcss::traits::Parse;

    #[test]
    fn named_areas_keep_order_and_coordinates_without_copying_names() {
        let doc = parse_dom_document("<p></p>").unwrap();
        let mut styles = ComputedStylesBuilder::new(&doc);
        let expected: Vec<_> = [
            ("head", 1, 2, 1, 4),
            ("side", 2, 3, 1, 2),
            ("main", 2, 3, 2, 4),
            ("foot", 3, 4, 1, 4),
        ]
        .into_iter()
        .map(
            |(name, row_start, row_end, column_start, column_end)| GridTemplateArea {
                name: styles.intern_string(name),
                row_start,
                row_end,
                column_start,
                column_end,
            },
        )
        .collect();
        let input: GridTemplateAreas =
            Parse::parse_string("'head head head' 'side main main' 'foot foot foot'").unwrap();
        let (actual, allocations) = count_allocations(|| grid_template_areas(&mut styles, &input));
        assert_eq!(actual, Some((expected, 3, 3)));
        assert_eq!(allocations, 2);
    }

    #[test]
    fn named_area_shapes_reject_holes_and_disconnected_cells() {
        let doc = parse_dom_document("<p></p>").unwrap();
        let mut styles = ComputedStylesBuilder::new(&doc);
        for css in [
            "'a a' 'a b'",
            "'a . a'",
            "'a .' '. a'",
            "'a a a' 'a . a' 'a a a'",
            "'a a' 'a a' 'b a'",
            "'a' '.' 'a'",
        ] {
            let input: GridTemplateAreas = Parse::parse_string(css).unwrap();
            assert_eq!(grid_template_areas(&mut styles, &input), None, "{css}");
        }
        for (css, expected_count, rows, columns) in [
            ("none", 0, 0, 0),
            ("'. .' '. .'", 0, 2, 2),
            ("'a A' 'a A'", 2, 2, 2),
            ("'. . .' '. a a' '. a a'", 1, 3, 3),
        ] {
            let input: GridTemplateAreas = Parse::parse_string(css).unwrap();
            let (areas, actual_rows, actual_columns) =
                grid_template_areas(&mut styles, &input).unwrap();
            assert_eq!(
                (areas.len(), actual_rows, actual_columns),
                (expected_count, rows, columns),
                "{css}"
            );
        }
    }
}
