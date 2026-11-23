use super::grid::TableCellPlacement;
use kurbo::{Point, Rect};

pub(super) struct TableColumnLayout {
    pub(super) widths: Vec<f64>,
    pub(super) starts: Vec<f64>,
    pub(super) table_width: f64,
    pub(super) rtl: bool,
    pub(super) spacing: f64,
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

pub(super) struct TableGeometry {
    pub(super) origin: Point,
    pub(super) columns: TableColumnLayout,
    pub(super) rows: TableRowLayout,
    pub(super) height: f64,
}

pub(super) struct TableRowLayout {
    pub(super) starts: Vec<f64>,
    pub(super) heights: Vec<f64>,
}

impl TableRowLayout {
    pub(super) fn span_height(&self, row: usize, span: usize) -> f64 {
        span_extent(&self.starts, &self.heights, row, span)
    }
}

impl TableGeometry {
    pub(super) fn cell_rect(&self, cell: &TableCellPlacement) -> Rect {
        let x = self.origin.x + self.columns.physical_start(cell.col, cell.colspan);
        let y = self.origin.y + self.rows.starts[cell.row];
        Rect::new(
            x,
            y,
            x + self.columns.span_width(cell.col, cell.colspan),
            y + self.rows.span_height(cell.row, cell.rowspan),
        )
    }
}
