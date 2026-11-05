use crate::layout::{LayoutReader, resolve_definite_outer_inline_size};
use crate::layout_model::TableColumnWidthHint;
pub(super) use crate::layout_model::{TableCellPlacement, TableGrid, build_table_grid};
use html_style_model::{UsedPreferredSize, UsedStyleView};
use std::ops::Range;

pub(super) fn column_width_style<'a>(
    reader: &LayoutReader<'a>,
    hint: TableColumnWidthHint,
) -> UsedStyleView<'a> {
    let primary = reader.used_style(hint.style);
    if matches!(primary.width(), UsedPreferredSize::Auto) {
        hint.fallback_style
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
    hint: TableColumnWidthHint,
) -> (f64, Option<f64>) {
    let primary = reader.used_style(hint.style);
    let (own_minimum, own_maximum, _) = definite_width_constraints(primary, 0.0);
    let (minimum, _, width) = definite_width_constraints(column_width_style(reader, hint), 0.0);
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

pub(super) fn row_group_range(
    reader: &LayoutReader<'_>,
    rows: &[u32],
    group_idx: usize,
) -> Option<Range<usize>> {
    let mut positions = rows.iter().enumerate().filter_map(|(row, &row_idx)| {
        (reader.get_parent(row_idx as usize) == Some(group_idx)).then_some(row)
    });
    let first = positions.next()?;
    let last = positions.next_back().unwrap_or(first);
    Some(first..last + 1)
}

pub(super) struct TableColumnLayout {
    pub(super) widths: Vec<f64>,
    pub(super) starts: Vec<f64>,
    pub(super) table_width: f64,
    pub(super) rtl: bool,
}

impl TableColumnLayout {
    pub(super) fn span_width(&self, column: usize, span: usize) -> f64 {
        span_extent(&self.starts, &self.widths, column, span)
    }

    pub(super) fn physical_start(&self, column: usize, span: usize) -> f64 {
        let start = self.starts.get(column).copied().unwrap_or(0.0);
        if self.rtl {
            self.table_width - start - self.span_width(column, span)
        } else {
            start
        }
    }
}

pub(super) fn span_extent(starts: &[f64], sizes: &[f64], start: usize, span: usize) -> f64 {
    let start = start.min(sizes.len());
    let end = start.saturating_add(span).min(sizes.len());
    if start >= end {
        return 0.0;
    }
    let last = end - 1;
    (starts[last] + sizes[last] - starts[start]).max(0.0)
}
