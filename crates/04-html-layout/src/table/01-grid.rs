use crate::layout::{LayoutEngine, UsedBorderInsets};
use crate::layout_model::{
    LayoutMode, TableColumnGroupSpan, TableColumnTrack, TableColumnWidthHint,
};
use html_style_model::{BorderCollapseMode, TableLayoutMode, UsedPreferredSize};

use super::borders::CollapsedBorderGrid;
use super::columns::{TableGrid, build_table_grid, column_width_style};

/// Shared grid preparation for intrinsic sizing and final table layout.
pub(super) struct PreparedTableGrid {
    pub(super) grid: TableGrid,
    pub(super) columns: Vec<TableColumnTrack>,
    pub(super) column_groups: Vec<TableColumnGroupSpan>,
    pub(super) column_width_hints: Vec<TableColumnWidthHint>,
    pub(super) collapsed_borders: Option<CollapsedBorderGrid>,
    pub(super) outer_borders: UsedBorderInsets,
    pub(super) horizontal_spacing: f64,
    pub(super) vertical_spacing: f64,
    pub(super) fixed_mode: bool,
}

pub(super) fn prepare_table_grid(
    session: &LayoutEngine<'_, '_>,
    table_box_idx: usize,
    rows: &[u32],
) -> PreparedTableGrid {
    let style = session.reader.style(table_box_idx);
    let fixed_mode = matches!(style.table_layout(), TableLayoutMode::Fixed)
        && !matches!(style.width(), UsedPreferredSize::Auto);
    let collapsed = matches!(style.border_collapse(), BorderCollapseMode::Collapse);
    let (row_groups, columns, column_groups, hints) =
        match session.reader.box_layout_mode(table_box_idx) {
            Some(LayoutMode::Table(table)) => (
                table.row_groups.as_slice(),
                table.columns.as_slice(),
                table.column_groups.as_slice(),
                table.column_width_hints.as_slice(),
            ),
            _ => (&[][..], &[][..], &[][..], &[][..]),
        };
    let (grid, track_sources, logical_to_effective) =
        effective_table_grid(session, table_grid(session, rows), hints, fixed_mode);
    let column_width_hints = track_sources
        .iter()
        .filter_map(|&source| hints.get(source).copied())
        .collect();
    let columns = track_sources
        .iter()
        .filter_map(|&source| columns.get(source).cloned())
        .collect::<Vec<_>>();
    let column_groups = column_groups
        .iter()
        .filter_map(|group| {
            let logical_end = group
                .start
                .saturating_add(group.span)
                .min(logical_to_effective.len());
            if logical_end <= group.start {
                return None;
            }
            let start = logical_to_effective[group.start];
            let end = logical_to_effective[logical_end - 1] + 1;
            Some(TableColumnGroupSpan {
                style: group.style,
                start,
                span: end - start,
            })
        })
        .collect::<Vec<_>>();
    let collapsed_borders = (collapsed && grid.column_count > 0).then(|| {
        CollapsedBorderGrid::build(
            &session.reader,
            table_box_idx,
            rows,
            row_groups,
            &grid.placements,
            &columns,
            &column_groups,
        )
    });
    let outer_borders = collapsed_borders
        .as_ref()
        .map_or_else(UsedBorderInsets::default, CollapsedBorderGrid::outer_insets);
    PreparedTableGrid {
        grid,
        columns,
        column_groups,
        column_width_hints,
        collapsed_borders,
        outer_borders,
        horizontal_spacing: if collapsed {
            0.0
        } else {
            style.border_spacing_horizontal().max(0.0) as f64
        },
        vertical_spacing: if collapsed {
            0.0
        } else {
            style.border_spacing_vertical().max(0.0) as f64
        },
        fixed_mode,
    }
}

fn table_grid(session: &LayoutEngine<'_, '_>, rows: &[u32]) -> TableGrid {
    let mut group_end = 0;
    build_table_grid(rows.iter().enumerate().map(|(row_position, &row_idx)| {
        // A cell cannot span beyond its row group. Clamp here so both track
        // placement and height distribution use the same effective rowspan.
        if row_position == group_end {
            let row_parent = session.reader.get_parent(row_idx as usize);
            group_end += rows[row_position..]
                .iter()
                .take_while(|&&candidate| {
                    session.reader.get_parent(candidate as usize) == row_parent
                })
                .count();
        }
        let remaining_group_rows = group_end - row_position;
        let cells = match session.reader.box_layout_mode(row_idx as usize) {
            Some(LayoutMode::TableRow(row)) => row.cells.as_slice(),
            _ => &[],
        };
        cells.iter().map(move |&cell_idx| {
            let cell_idx = cell_idx as usize;
            let (colspan, rowspan) = match session.reader.box_layout_mode(cell_idx) {
                Some(LayoutMode::TableCell(cell)) => (cell.colspan, cell.rowspan),
                _ => (1, 1),
            };
            (cell_idx, colspan, rowspan.min(remaining_group_rows))
        })
    }))
}

fn size_keeps_empty_track(size: UsedPreferredSize) -> bool {
    use UsedPreferredSize as Size;
    match size {
        Size::Auto | Size::Content | Size::Stretch => false,
        Size::Px(value) => value > 0.0,
        Size::Percent(value) => value > 0.0,
        Size::Calc {
            absolute_px,
            percentage,
            ..
        } => absolute_px > 0.0 || percentage > 0.0,
        Size::MinContent | Size::MaxContent | Size::FitContent | Size::Comparison { .. } => true,
    }
}

fn effective_table_grid(
    session: &LayoutEngine<'_, '_>,
    mut grid: TableGrid,
    hints: &[TableColumnWidthHint],
    fixed_mode: bool,
) -> (TableGrid, Vec<usize>, Vec<usize>) {
    let logical_count = grid.column_count.max(hints.len());
    if logical_count == 0 {
        return (grid, Vec::new(), Vec::new());
    }
    if fixed_mode {
        grid.column_count = logical_count;
        let identity = (0..logical_count).collect::<Vec<_>>();
        return (grid, identity.clone(), identity);
    }

    // Mark originating columns, then replace each mark with its final track
    // index in one forward pass. Removed columns map to the preceding retained
    // track, or the first track when there is no preceding one.
    let mut logical_to_effective = vec![0; logical_count];
    for placement in &grid.placements {
        if placement.col < logical_count {
            logical_to_effective[placement.col] = usize::MAX;
        }
    }
    let mut retained = Vec::new();
    for (column, effective) in logical_to_effective.iter_mut().enumerate() {
        if *effective == usize::MAX
            || hints.get(column).is_some_and(|hint| {
                let primary = session.reader.used_style(hint.style);
                let width = column_width_style(&session.reader, *hint).width();
                size_keeps_empty_track(width) || size_keeps_empty_track(primary.min_width())
            })
        {
            retained.push(column);
        }
        *effective = retained.len().saturating_sub(1);
    }
    if retained.is_empty() {
        grid.column_count = 0;
        return (grid, retained, Vec::new());
    }

    for placement in &mut grid.placements {
        let logical_end = placement
            .col
            .saturating_add(placement.colspan)
            .min(logical_count);
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
