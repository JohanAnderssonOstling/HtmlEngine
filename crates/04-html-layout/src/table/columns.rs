use crate::layout::LayoutReader;
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
            .map(|style| reader.used_style(style))
            .unwrap_or(primary)
    } else {
        primary
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
