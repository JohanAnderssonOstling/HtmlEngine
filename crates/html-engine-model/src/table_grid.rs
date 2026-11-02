//! Source-order table-grid placement shared by layout and document queries.

use std::ops::Range;

/// One source cell placed in a logical table grid.
#[derive(Clone, Copy)]
pub struct TableCellPlacement {
    /// Identity supplied by the caller (for example a box or DOM node index).
    pub cell_idx: usize,
    pub row: usize,
    pub col: usize,
    pub colspan: usize,
    pub source_colspan: usize,
    pub rowspan: usize,
}

impl TableCellPlacement {
    pub fn column_range(self, column_count: usize) -> Option<Range<usize>> {
        let start = self.col.min(column_count);
        let end = self.col.saturating_add(self.colspan).min(column_count);
        (start < end).then_some(start..end)
    }
}

/// Logical cell placements and the maximum column count across all rows.
pub struct TableGrid {
    pub placements: Vec<TableCellPlacement>,
    pub column_count: usize,
}

/// Collect streamed table-cell placements into an owned grid.
pub fn build_table_grid<Cells: IntoIterator<Item = (usize, usize, usize)>>(
    rows: impl IntoIterator<Item = Cells>,
) -> TableGrid {
    let mut placements = Vec::new();
    let column_count = place_table_cells(rows, |placement| placements.push(placement));
    TableGrid {
        placements,
        column_count,
    }
}

/// Visit streamed rows of `(cell identity, colspan, rowspan)` in source order,
/// returning the maximum column count without retaining a placement buffer.
/// Overlapping spans preserve the longest remaining reservation in each column.
/// Callers apply row-group limits and other stage-specific span policies first.
pub fn place_table_cells<Cells: IntoIterator<Item = (usize, usize, usize)>>(
    rows: impl IntoIterator<Item = Cells>,
    mut visit: impl FnMut(TableCellPlacement),
) -> usize {
    let mut carry_rowspans: Vec<usize> = Vec::new();
    let mut max_columns = 0usize;

    for (row_idx, cells) in rows.into_iter().enumerate() {
        if row_idx > 0 {
            for span in &mut carry_rowspans {
                *span = span.saturating_sub(1);
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
            for span in &mut carry_rowspans[col..end_col] {
                *span = (*span).max(rowspan);
            }
            visit(TableCellPlacement {
                cell_idx,
                row: row_idx,
                col,
                colspan,
                source_colspan: colspan,
                rowspan,
            });
            col = end_col;
            max_columns = max_columns.max(end_col);
        }
    }

    max_columns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placements_assign_columns_in_row_order() {
        let rows = vec![
            vec![(10usize, 2usize, 1usize), (11usize, 1usize, 2usize)],
            vec![(20usize, 1usize, 1usize), (21usize, 1usize, 1usize)],
        ];
        let TableGrid {
            placements,
            column_count: cols,
        } = build_table_grid(rows);
        assert_eq!(cols, 3);
        assert_eq!(placements.len(), 4);
        let first: TableCellPlacement = placements[0];
        let second: TableCellPlacement = placements[1];
        let third: TableCellPlacement = placements[2];
        let fourth: TableCellPlacement = placements[3];
        assert_eq!(
            (first.row, first.col, first.colspan, first.cell_idx),
            (0, 0, 2, 10)
        );
        assert_eq!(
            (second.row, second.col, second.rowspan, second.cell_idx),
            (0, 2, 2, 11)
        );
        assert_eq!((third.row, third.col, third.cell_idx), (1, 0, 20));
        assert_eq!((fourth.row, fourth.col, fourth.cell_idx), (1, 1, 21));
    }

    #[test]
    fn placements_respect_rowspan_reserved_slots() {
        let rows = vec![
            vec![(10usize, 1usize, 2usize), (11usize, 1usize, 1usize)],
            vec![(20usize, 1usize, 1usize)],
        ];
        let TableGrid {
            placements,
            column_count: cols,
        } = build_table_grid(rows);
        assert_eq!(cols, 2);
        assert_eq!(placements.len(), 3);
        assert_eq!((placements[0].row, placements[0].col), (0, 0));
        assert_eq!((placements[1].row, placements[1].col), (0, 1));
        assert_eq!((placements[2].row, placements[2].col), (1, 1));
    }
}
