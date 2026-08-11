use super::*;

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
) -> Option<(Vec<GridTemplateTrack>, Vec<Vec<StyleStringId>>)> {
    use lightningcss::properties::grid::{RepeatCount, TrackListItem, TrackSizing};
    let TrackSizing::TrackList(list) = value else {
        return Some((Vec::new(), Vec::new()));
    };
    if list.items.is_empty() {
        return None;
    }
    let names = list
        .line_names
        .iter()
        .map(|set| {
            set.iter()
                .map(|name| styles.intern_string(name.0.as_ref()))
                .collect()
        })
        .collect();
    let mut tracks = Vec::with_capacity(list.items.len());
    for item in &list.items {
        tracks.push(match item {
            TrackListItem::TrackSize(size) => {
                GridTemplateTrack::Single(grid_track_size(size, font_size, root_font_size)?)
            }
            TrackListItem::TrackRepeat(repeat) => {
                let count = match repeat.count {
                    RepeatCount::Number(value) => GridRepeatCount::Count(
                        std::num::NonZeroU16::new(u16::try_from(value).ok()?)?,
                    ),
                    RepeatCount::AutoFill => GridRepeatCount::AutoFill,
                    RepeatCount::AutoFit => GridRepeatCount::AutoFit,
                };
                let tracks = repeat
                    .track_sizes
                    .iter()
                    .map(|size| grid_track_size(size, font_size, root_font_size))
                    .collect::<Option<Vec<_>>>()?;
                if tracks.is_empty() {
                    return None;
                }
                let line_names = repeat
                    .line_names
                    .iter()
                    .map(|set| {
                        set.iter()
                            .map(|name| styles.intern_string(name.0.as_ref()))
                            .collect()
                    })
                    .collect();
                GridTemplateTrack::Repeat {
                    count,
                    tracks,
                    line_names,
                }
            }
        });
    }
    let auto_repeat_count = tracks
        .iter()
        .filter(|track| {
            matches!(
                track,
                GridTemplateTrack::Repeat {
                    count: GridRepeatCount::AutoFill | GridRepeatCount::AutoFit,
                    ..
                }
            )
        })
        .count();
    if auto_repeat_count > 1
        || (auto_repeat_count == 1 && !tracks.iter().all(grid_auto_repeat_compatible))
    {
        return None;
    }
    Some((tracks, names))
}

pub(in crate::style::cascade::resolver) fn grid_auto_repeat_compatible(
    track: &GridTemplateTrack,
) -> bool {
    match track {
        GridTemplateTrack::Single(size) => grid_fixed_track_size(size),
        GridTemplateTrack::Repeat { tracks, .. } => tracks.iter().all(grid_fixed_track_size),
    }
}

pub(in crate::style::cascade::resolver) fn grid_fixed_track_size(size: &GridTrackSize) -> bool {
    match size {
        GridTrackSize::Breadth(value) => grid_fixed_breadth(value),
        GridTrackSize::MinMax { min, max } => {
            grid_fixed_breadth(min) || (grid_inflexible_breadth(min) && grid_fixed_breadth(max))
        }
        GridTrackSize::Auto | GridTrackSize::FitContent(_) => false,
    }
}

pub(in crate::style::cascade::resolver) fn grid_fixed_breadth(value: &GridTrackBreadth) -> bool {
    matches!(value, GridTrackBreadth::Length(_))
}

pub(in crate::style::cascade::resolver) fn grid_inflexible_breadth(
    value: &GridTrackBreadth,
) -> bool {
    !matches!(value, GridTrackBreadth::Flex(_))
}

pub(in crate::style::cascade::resolver) fn grid_auto_tracks(
    value: &lightningcss::properties::grid::TrackSizeList,
    font_size: f32,
    root_font_size: f32,
) -> Option<Vec<GridTrackSize>> {
    value
        .0
        .iter()
        .map(|size| grid_track_size(size, font_size, root_font_size))
        .collect()
}

pub(in crate::style::cascade::resolver) fn grid_track_size(
    value: &lightningcss::properties::grid::TrackSize,
    font_size: f32,
    root_font_size: f32,
) -> Option<GridTrackSize> {
    use lightningcss::properties::grid::TrackSize;
    Some(match value {
        TrackSize::TrackBreadth(value) => {
            GridTrackSize::Breadth(grid_track_breadth(value, font_size, root_font_size)?)
        }
        TrackSize::MinMax { min, max } => GridTrackSize::MinMax {
            min: grid_track_breadth(min, font_size, root_font_size)?,
            max: grid_track_breadth(max, font_size, root_font_size)?,
        },
        TrackSize::FitContent(value) => GridTrackSize::FitContent(non_negative_length_pct(
            computed_length_pct(value, font_size, root_font_size)?,
        )?),
    })
}

pub(in crate::style::cascade::resolver) fn grid_track_breadth(
    value: &lightningcss::properties::grid::TrackBreadth,
    font_size: f32,
    root_font_size: f32,
) -> Option<GridTrackBreadth> {
    use lightningcss::properties::grid::TrackBreadth;
    Some(match value {
        TrackBreadth::Length(value) => GridTrackBreadth::Length(non_negative_length_pct(
            computed_length_pct(value, font_size, root_font_size)?,
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
            GridPlacement::Line(std::num::NonZeroI16::new(i16::try_from(*index).ok()?)?)
        }
        GridLine::Line {
            index,
            name: Some(name),
        } => GridPlacement::NamedLine {
            name: styles.intern_string(name.0.as_ref()),
            index: i16::try_from(*index).ok()?,
        },
        GridLine::Span { index, name: None } => {
            GridPlacement::Span(std::num::NonZeroU16::new(u16::try_from(*index).ok()?)?)
        }
        GridLine::Span {
            index,
            name: Some(name),
        } => GridPlacement::NamedSpan {
            name: styles.intern_string(name.0.as_ref()),
            count: std::num::NonZeroU16::new(u16::try_from(*index).ok()?)?,
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
    let mut found: Vec<(String, usize, usize, usize, usize)> = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let Some(name) = areas[row * columns + column].as_ref() else {
                continue;
            };
            if let Some((_, row_start, row_end, column_start, column_end)) =
                found.iter_mut().find(|entry| entry.0 == *name)
            {
                *row_start = (*row_start).min(row);
                *row_end = (*row_end).max(row + 1);
                *column_start = (*column_start).min(column);
                *column_end = (*column_end).max(column + 1);
            } else {
                found.push((name.clone(), row, row + 1, column, column + 1));
            }
        }
    }
    let mut output = Vec::with_capacity(found.len());
    for (name, row_start, row_end, column_start, column_end) in found {
        for row in row_start..row_end {
            for column in column_start..column_end {
                if areas[row * columns + column].as_deref() != Some(name.as_str()) {
                    return None;
                }
            }
        }
        output.push(GridTemplateArea {
            name: styles.intern_string(&name),
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
