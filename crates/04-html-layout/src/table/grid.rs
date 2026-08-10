use crate::layout::LayoutEngine;
use crate::layout_model::LayoutMode;

use super::columns::{TableGrid, build_table_grid};

pub(super) fn table_grid(session: &LayoutEngine<'_, '_>, rows: &[u32]) -> TableGrid {
    build_table_grid(rows.iter().enumerate().map(|(row_position, &row_idx)| {
        // A cell cannot span beyond its row group. Clamp here so both track
        // placement and height distribution use the same effective rowspan.
        let row_parent = session.reader.get_parent(row_idx as usize);
        let remaining_group_rows = rows[row_position..]
            .iter()
            .take_while(|&&candidate| session.reader.get_parent(candidate as usize) == row_parent)
            .count()
            .max(1);
        match session.reader.box_layout_mode(row_idx as usize) {
            Some(LayoutMode::TableRow(row)) => row
                .cells
                .iter()
                .map(|&cell_idx| {
                    let cell_idx = cell_idx as usize;
                    let (colspan, rowspan) = match session.reader.box_layout_mode(cell_idx) {
                        Some(LayoutMode::TableCell(cell)) => (cell.colspan, cell.rowspan),
                        _ => (1, 1),
                    };
                    (cell_idx, colspan, rowspan.min(remaining_group_rows))
                })
                .collect(),
            _ => Vec::new(),
        }
    }))
}

fn size_keeps_empty_track(size: html_style_model::UsedPreferredSize) -> bool {
    use html_style_model::UsedPreferredSize as Size;
    match size {
        Size::Auto | Size::Stretch => false,
        Size::Px(value) => value > 0.0,
        Size::Percent(value) => value > 0.0,
        Size::Calc { absolute_px, percentage, .. } => absolute_px > 0.0 || percentage > 0.0,
        Size::MinContent | Size::MaxContent | Size::FitContent | Size::Comparison { .. } => true,
    }
}

pub(super) fn effective_table_grid(session: &LayoutEngine<'_, '_>, mut grid: TableGrid, hints: &[crate::layout_model::TableColumnWidthHint], fixed_mode: bool) -> (TableGrid, Vec<usize>, Vec<usize>) {
    let logical_count = grid.column_count.max(hints.len());
    if logical_count == 0 {
        return (grid, Vec::new(), Vec::new());
    }
    if fixed_mode {
        grid.column_count = logical_count;
        let identity = (0..logical_count).collect::<Vec<_>>();
        return (grid, identity.clone(), identity);
    }

    let mut originating = vec![false; logical_count];
    for placement in &grid.placements {
        if placement.col < logical_count {
            originating[placement.col] = true;
        }
    }
    let retained = (0..logical_count)
        .filter(|&column| {
            originating[column]
                || hints.get(column).is_some_and(|hint| {
                    let primary = session.reader.used_style(hint.style);
                    let width = if matches!(primary.width(), html_style_model::UsedPreferredSize::Auto) { hint.fallback_style.map(|style| session.reader.used_style(style).width()).unwrap_or(primary.width()) } else { primary.width() };
                    size_keeps_empty_track(width) || size_keeps_empty_track(primary.min_width())
                })
        })
        .collect::<Vec<_>>();
    if retained.is_empty() {
        grid.column_count = 0;
        return (grid, Vec::new(), Vec::new());
    }

    let mut logical_to_effective = vec![usize::MAX; logical_count];
    let mut effective = 0usize;
    for logical in 0..logical_count {
        if retained.binary_search(&logical).is_ok() {
            effective = retained.binary_search(&logical).expect("retained track is searchable");
        }
        logical_to_effective[logical] = effective;
    }
    let first = retained[0];
    for mapped in &mut logical_to_effective[..first] {
        *mapped = 0;
    }

    for placement in &mut grid.placements {
        let logical_end = placement.col.saturating_add(placement.colspan).min(logical_count);
        if logical_end <= placement.col {
            continue;
        }
        let start = logical_to_effective[placement.col];
        let end = logical_to_effective[logical_end - 1] + 1;
        placement.col = start;
        placement.colspan = end - start;
    }
    grid.column_count = retained.len();
    (grid, retained, logical_to_effective)
}


