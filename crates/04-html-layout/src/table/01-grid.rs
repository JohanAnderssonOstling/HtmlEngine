//! Prepare cell placements, effective columns, borders, and spacing for sizing and layout.

use crate::{LayoutEngine, UsedBorderInsets};
use html_engine_model::{LayoutMode, TableBox, TableColumnGroupSpan, TableColumnTrack};
use html_style_model::{BorderCollapseMode, TableLayoutMode, UsedPreferredSize};

use super::borders::CollapsedBorderGrid;
use super::columns::column_width_constraints;
use super::grid::{TableGrid, build_table_grid};

/// Shared grid preparation for intrinsic sizing and final table layout.
pub(super) struct PreparedTableGrid<'tree> {
    pub(super) rows: &'tree [u32],
    pub(super) row_groups: Vec<(u32, std::ops::Range<usize>)>,
    pub(super) grid: TableGrid,
    pub(super) columns: Vec<TableColumnTrack>,
    pub(super) column_groups: Vec<TableColumnGroupSpan>,
    pub(super) collapsed_borders: Option<CollapsedBorderGrid>,
    pub(super) outer_borders: UsedBorderInsets,
    pub(super) horizontal_spacing: f64,
    pub(super) vertical_spacing: f64,
    pub(super) fixed_mode: bool,
}

pub(super) fn prepare_table_grid<'tree>(
    engine: &LayoutEngine<'_, '_>,
    table_box_idx: usize,
    table: &'tree TableBox,
) -> PreparedTableGrid<'tree> {
    let style = engine.read.reader.style(table_box_idx);
    let fixed_mode = matches!(style.table_layout(), TableLayoutMode::Fixed)
        && !matches!(style.width(), UsedPreferredSize::Auto);
    let collapsed = matches!(style.border_collapse(), BorderCollapseMode::Collapse);
    let (grid, track_sources, logical_to_effective) = effective_table_grid(
        engine,
        table_grid(engine, &table.rows),
        &table.columns,
        fixed_mode,
    );
    let rows = if grid.placements.is_empty() {
        &[][..]
    } else {
        table.rows.as_slice()
    };
    let mut parent_ranges = rustc_data_structures::fx::FxHashMap::default();
    for (row, &row_idx) in rows.iter().enumerate() {
        if let Some(parent) = engine.read.reader.get_parent(row_idx as usize) {
            parent_ranges.entry(parent).or_insert(row..row + 1).end = row + 1;
        }
    }
    let row_groups = table
        .row_groups
        .iter()
        .filter_map(|&group| {
            parent_ranges
                .get(&(group as usize))
                .cloned()
                .map(|range| (group, range))
        })
        .collect::<Vec<_>>();
    let columns = track_sources
        .iter()
        .filter_map(|&source| table.columns.get(source).cloned())
        .collect::<Vec<_>>();
    let column_groups = table
        .column_groups
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
            &engine.read.reader,
            table_box_idx,
            rows,
            &row_groups,
            grid.column_count,
            &grid.placements,
            &columns,
            &column_groups,
        )
    });
    let outer_borders = collapsed_borders
        .as_ref()
        .map_or_else(UsedBorderInsets::default, CollapsedBorderGrid::outer_insets);
    PreparedTableGrid {
        rows,
        row_groups,
        grid,
        columns,
        column_groups,
        collapsed_borders,
        outer_borders,
        horizontal_spacing: if collapsed || rows.is_empty() {
            0.0
        } else {
            style.border_spacing_horizontal().max(0.0) as f64
        },
        vertical_spacing: if collapsed || rows.is_empty() {
            0.0
        } else {
            style.border_spacing_vertical().max(0.0) as f64
        },
        fixed_mode,
    }
}

fn table_grid(engine: &LayoutEngine<'_, '_>, rows: &[u32]) -> TableGrid {
    let mut group_end = 0;
    build_table_grid(rows.iter().enumerate().map(|(row_position, &row_idx)| {
        // A cell cannot span beyond its row group. Clamp here so both track
        // placement and height distribution use the same effective rowspan.
        if row_position == group_end {
            group_end = super::row_group_end(&engine.read.reader, rows, row_position);
        }
        let remaining_group_rows = group_end - row_position;
        let cells = match engine.read.reader.box_layout_mode(row_idx as usize) {
            Some(LayoutMode::TableRow(row)) => row.cells.as_slice(),
            _ => &[],
        };
        cells.iter().map(move |&cell_idx| {
            let cell_idx = cell_idx as usize;
            let (colspan, rowspan) = match engine.read.reader.box_layout_mode(cell_idx) {
                Some(LayoutMode::TableCell(cell)) => (cell.colspan, cell.rowspan),
                _ => (1, 1),
            };
            (cell_idx, colspan, rowspan.min(remaining_group_rows))
        })
    }))
}

fn effective_table_grid(
    engine: &LayoutEngine<'_, '_>,
    mut grid: TableGrid,
    columns: &[TableColumnTrack],
    fixed_mode: bool,
) -> (TableGrid, Vec<usize>, Vec<usize>) {
    if grid.placements.is_empty() {
        grid.column_count = 0;
        return (grid, Vec::new(), Vec::new());
    }
    let logical_count = grid.column_count.max(columns.len());
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
            || columns.get(column).is_some_and(|track| {
                let (minimum, width) = column_width_constraints(&engine.read.reader, track);
                minimum > 0.0 || width.is_some_and(|width| width > 0.0)
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
