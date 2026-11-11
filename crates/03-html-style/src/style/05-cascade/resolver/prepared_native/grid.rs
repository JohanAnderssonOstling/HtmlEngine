use super::super::context::PropertyContext;
use super::{components, length_pct};
use crate::style::syntax::grid::auto_repeat_is_supported;
use crate::style::syntax::prepared_length::{LengthMode, PreparedLength};
use html_style_model::{
    ComputedStylesBuilder, GridPlacement, GridRepeatCount, GridTemplateArea, GridTemplateTrack,
    GridTrackBreadth, GridTrackSize, StyleStringId,
};
use lightningcss::properties::Property;
use lightningcss::properties::grid::{
    GridLine, GridTemplateAreas, RepeatCount, TrackBreadth, TrackListItem, TrackSize, TrackSizing,
};
use lightningcss::values::ident::CustomIdentList;
use std::num::{NonZeroI16, NonZeroU16};

#[derive(Clone, Debug)]
enum Breadth {
    Length(PreparedLength),
    Flex(f32),
    MinContent,
    MaxContent,
    Auto,
}
#[derive(Clone, Debug)]
enum Track {
    Breadth(Breadth),
    MinMax(Breadth, Breadth),
    FitContent(PreparedLength),
}
#[derive(Clone, Debug)]
enum TemplateTrack {
    Single(Track),
    Repeat(GridRepeatCount, Vec<Track>, Vec<Vec<Box<str>>>),
}
#[derive(Clone, Debug)]
enum Placement {
    Auto,
    Line(NonZeroI16),
    Span(NonZeroU16),
    NamedLine(Box<str>, i16),
    NamedSpan(Box<str>, NonZeroU16),
}
#[derive(Clone, Debug)]
struct Area {
    name: Box<str>,
    rows: [u16; 2],
    columns: [u16; 2],
}
#[derive(Clone, Debug)]
pub(crate) struct PreparedGrid(GridValue);
#[derive(Clone, Debug)]
enum GridValue {
    Template(Vec<TemplateTrack>, Vec<Vec<Box<str>>>),
    Auto(Vec<Track>),
    Placement(Placement),
    Areas(Vec<Area>, u16, u16),
}

impl Breadth {
    fn prepare(value: &TrackBreadth) -> Option<Self> {
        Some(match value {
            TrackBreadth::Length(v) => Self::Length(PreparedLength::nonnegative_percentage(
                v,
                LengthMode::Affine,
            )?),
            TrackBreadth::Flex(v) if v.is_finite() && *v >= 0.0 => Self::Flex(*v),
            TrackBreadth::Flex(_) => return None,
            TrackBreadth::MinContent => Self::MinContent,
            TrackBreadth::MaxContent => Self::MaxContent,
            TrackBreadth::Auto => Self::Auto,
        })
    }
    fn evaluate(&self, context: &mut PropertyContext<'_>) -> GridTrackBreadth {
        match self {
            Self::Length(v) => {
                GridTrackBreadth::Length(length_pct(v, components(v, context), true))
            }
            Self::Flex(v) => GridTrackBreadth::Flex(*v),
            Self::MinContent => GridTrackBreadth::MinContent,
            Self::MaxContent => GridTrackBreadth::MaxContent,
            Self::Auto => GridTrackBreadth::Auto,
        }
    }
    fn needs_height(&self) -> bool {
        matches!(self,Self::Length(v) if v.viewport_height)
    }
}
impl Track {
    fn prepare(value: &TrackSize) -> Option<Self> {
        Some(match value {
            TrackSize::TrackBreadth(v) => Self::Breadth(Breadth::prepare(v)?),
            TrackSize::MinMax { min, max } => {
                Self::MinMax(Breadth::prepare(min)?, Breadth::prepare(max)?)
            }
            TrackSize::FitContent(v) => Self::FitContent(PreparedLength::nonnegative_percentage(
                v,
                LengthMode::Affine,
            )?),
        })
    }
    fn evaluate(&self, context: &mut PropertyContext<'_>) -> GridTrackSize {
        match self {
            Self::Breadth(v) => GridTrackSize::Breadth(v.evaluate(context)),
            Self::MinMax(min, max) => GridTrackSize::MinMax {
                min: min.evaluate(context),
                max: max.evaluate(context),
            },
            Self::FitContent(v) => {
                GridTrackSize::FitContent(length_pct(v, components(v, context), true))
            }
        }
    }
    fn needs_height(&self) -> bool {
        match self {
            Self::Breadth(v) => v.needs_height(),
            Self::MinMax(a, b) => a.needs_height() || b.needs_height(),
            Self::FitContent(v) => v.viewport_height,
        }
    }
}
fn names(sets: &[CustomIdentList<'_>]) -> Vec<Vec<Box<str>>> {
    sets.iter()
        .map(|set| {
            set.iter()
                .map(|name| Box::<str>::from(name.0.as_ref()))
                .collect()
        })
        .collect()
}
fn intern_names(
    sets: &[Vec<Box<str>>],
    styles: &mut ComputedStylesBuilder,
) -> Vec<Vec<StyleStringId>> {
    sets.iter()
        .map(|set| set.iter().map(|name| styles.intern_string(name)).collect())
        .collect()
}
impl PreparedGrid {
    pub(crate) fn prepare(property: &Property<'_>) -> Result<Option<Self>, ()> {
        let value = match property {
            Property::GridTemplateRows(v) | Property::GridTemplateColumns(v) => {
                let TrackSizing::TrackList(list) = v else {
                    return Ok(Some(Self(GridValue::Template(Vec::new(), Vec::new()))));
                };
                if list.items.is_empty() || !auto_repeat_is_supported(list) {
                    return Err(());
                }
                let mut tracks = Vec::with_capacity(list.items.len());
                for item in &list.items {
                    tracks.push(match item {
                        TrackListItem::TrackSize(v) => {
                            TemplateTrack::Single(Track::prepare(v).ok_or(())?)
                        }
                        TrackListItem::TrackRepeat(v) => {
                            let count = match v.count {
                                RepeatCount::Number(v) => GridRepeatCount::Count(
                                    NonZeroU16::new(u16::try_from(v).map_err(|_| ())?).ok_or(())?,
                                ),
                                RepeatCount::AutoFill => GridRepeatCount::AutoFill,
                                RepeatCount::AutoFit => GridRepeatCount::AutoFit,
                            };
                            if v.track_sizes.is_empty() {
                                return Err(());
                            }
                            TemplateTrack::Repeat(
                                count,
                                v.track_sizes
                                    .iter()
                                    .map(Track::prepare)
                                    .collect::<Option<Vec<_>>>()
                                    .ok_or(())?,
                                names(&v.line_names),
                            )
                        }
                    });
                }
                GridValue::Template(tracks, names(&list.line_names))
            }
            Property::GridAutoRows(v) | Property::GridAutoColumns(v) => GridValue::Auto(
                v.0.iter()
                    .map(Track::prepare)
                    .collect::<Option<Vec<_>>>()
                    .ok_or(())?,
            ),
            Property::GridRowStart(v)
            | Property::GridRowEnd(v)
            | Property::GridColumnStart(v)
            | Property::GridColumnEnd(v) => GridValue::Placement(match v {
                GridLine::Auto => Placement::Auto,
                GridLine::Area { name } => Placement::NamedLine(Box::from(name.0.as_ref()), 0),
                GridLine::Line { index, name: None } => Placement::Line(
                    NonZeroI16::new(i16::try_from(*index).map_err(|_| ())?).ok_or(())?,
                ),
                GridLine::Line {
                    index,
                    name: Some(name),
                } => Placement::NamedLine(
                    Box::from(name.0.as_ref()),
                    i16::try_from(*index).map_err(|_| ())?,
                ),
                GridLine::Span { index, name: None } => Placement::Span(
                    NonZeroU16::new(u16::try_from(*index).map_err(|_| ())?).ok_or(())?,
                ),
                GridLine::Span {
                    index,
                    name: Some(name),
                } => Placement::NamedSpan(
                    Box::from(name.0.as_ref()),
                    NonZeroU16::new(u16::try_from(*index).map_err(|_| ())?).ok_or(())?,
                ),
            }),
            Property::GridTemplateAreas(v) => {
                let GridTemplateAreas::Areas { columns, areas } = v else {
                    return Ok(Some(Self(GridValue::Areas(Vec::new(), 0, 0))));
                };
                let columns = usize::try_from(*columns).map_err(|_| ())?;
                if columns == 0 || areas.len() % columns != 0 {
                    return Err(());
                }
                let rows = areas.len() / columns;
                let mut found: Vec<(&str, usize, usize, usize, usize, usize)> = Vec::new();
                for (row, cells) in areas.chunks_exact(columns).enumerate() {
                    for (column, name) in cells.iter().enumerate() {
                        let Some(name) = name.as_deref() else {
                            continue;
                        };
                        if let Some((_, _, end, left, right, count)) =
                            found.iter_mut().find(|entry| entry.0 == name)
                        {
                            *end = row + 1;
                            *left = (*left).min(column);
                            *right = (*right).max(column + 1);
                            *count += 1;
                        } else {
                            found.push((name, row, row + 1, column, column + 1, 1));
                        }
                    }
                }
                let mut output = Vec::with_capacity(found.len());
                for (name, top, bottom, left, right, count) in found {
                    if count != (bottom - top) * (right - left) {
                        return Err(());
                    }
                    output.push(Area {
                        name: Box::from(name),
                        rows: [
                            u16::try_from(top + 1).map_err(|_| ())?,
                            u16::try_from(bottom + 1).map_err(|_| ())?,
                        ],
                        columns: [
                            u16::try_from(left + 1).map_err(|_| ())?,
                            u16::try_from(right + 1).map_err(|_| ())?,
                        ],
                    });
                }
                GridValue::Areas(
                    output,
                    u16::try_from(rows).map_err(|_| ())?,
                    u16::try_from(columns).map_err(|_| ())?,
                )
            }
            _ => return Ok(None),
        };
        Ok(Some(Self(value)))
    }
    pub(crate) fn requires_viewport_height(&self) -> bool {
        match &self.0 {
            GridValue::Template(tracks, _) => tracks.iter().any(|track| match track {
                TemplateTrack::Single(v) => v.needs_height(),
                TemplateTrack::Repeat(_, v, _) => v.iter().any(Track::needs_height),
            }),
            GridValue::Auto(v) => v.iter().any(Track::needs_height),
            _ => false,
        }
    }
    pub(in crate::style::cascade::resolver) fn apply(
        &self,
        property: &Property<'_>,
        context: &mut PropertyContext<'_>,
    ) {
        match &self.0 {
            GridValue::Template(tracks, names) => {
                let names = intern_names(names, context.styles);
                let tracks = tracks
                    .iter()
                    .map(|track| match track {
                        TemplateTrack::Single(v) => GridTemplateTrack::Single(v.evaluate(context)),
                        TemplateTrack::Repeat(count, tracks, names) => GridTemplateTrack::Repeat {
                            count: *count,
                            tracks: tracks.iter().map(|v| v.evaluate(context)).collect(),
                            line_names: intern_names(names, context.styles),
                        },
                    })
                    .collect();
                if matches!(property, Property::GridTemplateRows(_)) {
                    context.style.layout.grid_template_rows = tracks;
                    context.style.layout.grid_template_row_names = names;
                } else {
                    context.style.layout.grid_template_columns = tracks;
                    context.style.layout.grid_template_column_names = names;
                }
            }
            GridValue::Auto(tracks) => {
                let tracks = tracks.iter().map(|v| v.evaluate(context)).collect();
                if matches!(property, Property::GridAutoRows(_)) {
                    context.style.layout.grid_auto_rows = tracks;
                } else {
                    context.style.layout.grid_auto_columns = tracks;
                }
            }
            GridValue::Placement(v) => {
                let value = match v {
                    Placement::Auto => GridPlacement::Auto,
                    Placement::Line(v) => GridPlacement::Line(*v),
                    Placement::Span(v) => GridPlacement::Span(*v),
                    Placement::NamedLine(name, index) => GridPlacement::NamedLine {
                        name: context.styles.intern_string(name),
                        index: *index,
                    },
                    Placement::NamedSpan(name, count) => GridPlacement::NamedSpan {
                        name: context.styles.intern_string(name),
                        count: *count,
                    },
                };
                *match property {
                    Property::GridRowStart(_) => &mut context.style.layout.grid_row.start,
                    Property::GridRowEnd(_) => &mut context.style.layout.grid_row.end,
                    Property::GridColumnStart(_) => &mut context.style.layout.grid_column.start,
                    _ => &mut context.style.layout.grid_column.end,
                } = value;
            }
            GridValue::Areas(areas, rows, columns) => {
                context.style.layout.grid_template_areas = intern_areas(areas, context.styles);
                context.style.layout.grid_template_area_rows = *rows;
                context.style.layout.grid_template_area_columns = *columns;
            }
        }
    }
}

fn intern_areas(areas: &[Area], styles: &mut ComputedStylesBuilder) -> Vec<GridTemplateArea> {
    areas
        .iter()
        .map(|area| GridTemplateArea {
            name: styles.intern_string(&area.name),
            row_start: area.rows[0],
            row_end: area.rows[1],
            column_start: area.columns[0],
            column_end: area.columns[1],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocation_test_support::count_allocations;
    use html_parse::parse_dom_document;
    use lightningcss::properties::grid::GridTemplateAreas;
    use lightningcss::traits::Parse;

    #[test]
    fn named_areas_keep_order_and_coordinates_after_preparation() {
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
        let prepared = PreparedGrid::prepare(&Property::GridTemplateAreas(input))
            .unwrap()
            .unwrap();
        let GridValue::Areas(areas, rows, columns) = &prepared.0 else {
            panic!("prepared areas");
        };
        let (actual, allocations) = count_allocations(|| intern_areas(areas, &mut styles));
        assert_eq!((actual, *rows, *columns), (expected, 3, 3));
        assert_eq!(allocations, 1);
    }

    #[test]
    fn named_area_shapes_reject_holes_and_disconnected_cells() {
        for css in [
            "'a a' 'a b'",
            "'a . a'",
            "'a .' '. a'",
            "'a a a' 'a . a' 'a a a'",
            "'a a' 'a a' 'b a'",
            "'a' '.' 'a'",
        ] {
            let input: GridTemplateAreas = Parse::parse_string(css).unwrap();
            assert!(
                PreparedGrid::prepare(&Property::GridTemplateAreas(input)).is_err(),
                "{css}"
            );
        }
        for (css, expected_count, rows, columns) in [
            ("none", 0, 0, 0),
            ("'. .' '. .'", 0, 2, 2),
            ("'a A' 'a A'", 2, 2, 2),
            ("'. . .' '. a a' '. a a'", 1, 3, 3),
        ] {
            let input: GridTemplateAreas = Parse::parse_string(css).unwrap();
            let prepared = PreparedGrid::prepare(&Property::GridTemplateAreas(input))
                .unwrap()
                .unwrap();
            let GridValue::Areas(areas, actual_rows, actual_columns) = prepared.0 else {
                panic!("prepared areas");
            };
            assert_eq!(
                (areas.len(), actual_rows, actual_columns),
                (expected_count, rows, columns),
                "{css}"
            );
        }
    }
}
