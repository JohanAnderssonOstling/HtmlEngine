use crate::{LayoutReader, resolve_definite_outer_inline_size};
use html_engine_model::TableColumnTrack;
use html_style_model::{UsedPreferredSize, UsedStyleView};

pub(super) fn column_width_style<'a>(
    reader: &LayoutReader<'a>,
    column: &TableColumnTrack,
) -> UsedStyleView<'a> {
    let primary = reader.used_style(column.width_style);
    if matches!(primary.width(), UsedPreferredSize::Auto) {
        column
            .fallback_style
            .map_or(primary, |style| reader.used_style(style))
    } else {
        primary
    }
}

pub(super) fn definite_width_constraints(
    style: UsedStyleView<'_>,
    extras: f64,
) -> (f64, f64, Option<f64>) {
    let resolve = |size| resolve_definite_outer_inline_size(size, style.box_sizing(), extras, 0.0);
    let minimum = resolve(style.min_width()).unwrap_or(0.0);
    let maximum = resolve(style.max_width())
        .unwrap_or(f64::INFINITY)
        .max(minimum);
    let width = resolve(style.width()).map(|width| width.clamp(minimum, maximum));
    (minimum, maximum, width)
}

pub(super) fn column_width_constraints(
    reader: &LayoutReader<'_>,
    column: &TableColumnTrack,
) -> (f64, Option<f64>) {
    let primary = reader.used_style(column.width_style);
    let (own_minimum, own_maximum, _) = definite_width_constraints(primary, 0.0);
    let (minimum, _, width) = definite_width_constraints(column_width_style(reader, column), 0.0);
    // A group's width fallback must still respect the column's own bounds.
    let minimum = minimum.max(own_minimum);
    let width = width.map(|width| width.clamp(minimum, own_maximum.max(minimum)));
    (minimum, width)
}

pub(super) fn percentage_component(value: UsedPreferredSize) -> Option<f64> {
    match value {
        UsedPreferredSize::Percent(percentage) => Some(percentage as f64),
        UsedPreferredSize::Calc {
            absolute_px: 0.0,
            percentage,
            ..
        } => Some(percentage as f64),
        _ => None,
    }
}
