use std::ops::Range;

#[derive(Clone, Copy)]
pub(super) struct TableCellPlacement {
    pub(super) cell_idx: usize,
    pub(super) row: usize,
    pub(super) col: usize,
    pub(super) colspan: usize,
    pub(super) source_colspan: usize,
    pub(super) rowspan: usize,
}

impl TableCellPlacement {
    pub(super) fn column_range(self, column_count: usize) -> Option<Range<usize>> {
        let start = self.col.min(column_count);
        let end = self.col.saturating_add(self.colspan).min(column_count);
        (start < end).then_some(start..end)
    }
}

pub(super) struct TableGrid {
    pub(super) placements: Vec<TableCellPlacement>,
    pub(super) column_count: usize,
}

pub(super) struct TableColumnLayout {
    pub(super) widths: Vec<f64>,
    pub(super) starts: Vec<f64>,
    pub(super) table_width: f64,
}

pub(super) fn build_table_grid(rows: impl IntoIterator<Item = Vec<(usize, usize, usize)>>) -> TableGrid {
    let mut placements = Vec::new();
    let mut carry_rowspans: Vec<usize> = Vec::new();
    let mut max_columns = 0usize;

    for (row_idx, cells) in rows.into_iter().enumerate() {
        if row_idx > 0 {
            for span in &mut carry_rowspans {
                if *span > 0 {
                    *span -= 1;
                }
            }
            while matches!(carry_rowspans.last(), Some(0)) {
                carry_rowspans.pop();
            }
        }

        let mut col = 0usize;
        for (cell_idx, colspan, rowspan) in cells {
            while col < carry_rowspans.len() && carry_rowspans[col] > 0 {
                col += 1;
            }

            let colspan = colspan.max(1);
            let rowspan = rowspan.max(1);
            let end_col = col + colspan;
            if carry_rowspans.len() < end_col {
                carry_rowspans.resize(end_col, 0);
            }
            for covered_col in col..end_col {
                carry_rowspans[covered_col] = carry_rowspans[covered_col].max(rowspan);
            }
            placements.push(TableCellPlacement { cell_idx, row: row_idx, col, colspan, source_colspan: colspan, rowspan });
            col = end_col;
            max_columns = max_columns.max(end_col);
        }
    }

    TableGrid { placements, column_count: max_columns }
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
