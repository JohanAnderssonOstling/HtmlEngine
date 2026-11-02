use super::columns::{TableCellPlacement, TableColumnLayout, span_extent};
use super::grid::PreparedTableGrid;
use crate::layout::LayoutEngine;
use crate::layout::decorations::record_bound_background;
use crate::layout::paint_program::PaintStyleSource;
use html_style_model::{StyleIndices, Visibility};
use kurbo::{Point, Rect};

pub(super) fn emit_collapsed_table_background(
    session: &mut LayoutEngine<'_, '_>,
    table_box_idx: usize,
    rect: Rect,
) {
    if rect.width() <= 0.0
        || rect.height() <= 0.0
        || !matches!(
            session.reader.style(table_box_idx).border_collapse(),
            html_style_model::BorderCollapseMode::Collapse
        )
    {
        return;
    }
    record_bound_background(
        session,
        table_box_idx,
        rect,
        PaintStyleSource::Box(table_box_idx as u32),
    );
}

pub(super) fn emit_table_layers(
    session: &mut LayoutEngine<'_, '_>,
    table_box_idx: usize,
    table_content_pos: Point,
    table_height: f64,
    columns: &TableColumnLayout,
    prepared: &PreparedTableGrid,
    row_starts: &[f64],
    row_heights: &[f64],
) {
    let grid_top = table_content_pos.y;
    let grid_height = table_height.max(0.0);
    let grid_left = table_content_pos.x;
    let grid_right = table_content_pos.x + columns.table_width;
    let table_grid = Rect::new(grid_left, grid_top, grid_right, grid_top + grid_height);

    if grid_height <= 0.0 || table_grid.width() <= 0.0 {
        return;
    }

    // In the collapsed model the grid canvas is the table background area.
    // Emit it here, before column, row, cell, and border layers. Separate
    // tables retain their ordinary full border-box background path because
    // their padding and border-spacing also belong to the background area.
    emit_collapsed_table_background(session, table_box_idx, table_grid);

    // Column layers cover their originating cells, including spans, while
    // border-spacing remains part of the table's own background. Look up the
    // source per cell instead of scanning the grid separately for each column.
    emit_table_column_background(
        session,
        table_box_idx,
        prepared,
        table_content_pos,
        columns,
        row_starts,
        row_heights,
        |cell| {
            let index = prepared
                .column_groups
                .partition_point(|group| group.start <= cell.col);
            let index = index.checked_sub(1)?;
            let group = prepared.column_groups.get(index)?;
            (cell.col < group.start.saturating_add(group.span)).then_some((index, group.style))
        },
    );
    emit_table_column_background(
        session,
        table_box_idx,
        prepared,
        table_content_pos,
        columns,
        row_starts,
        row_heights,
        |cell| Some((cell.col, prepared.columns.get(cell.col)?.background_style?)),
    );
}

fn emit_table_column_background(
    session: &mut LayoutEngine<'_, '_>,
    table_box_idx: usize,
    prepared: &PreparedTableGrid,
    table_content_pos: Point,
    columns: &TableColumnLayout,
    row_starts: &[f64],
    row_heights: &[f64],
    style_for_cell: impl Fn(&TableCellPlacement) -> Option<(usize, StyleIndices)>,
) {
    let mut pending = None::<(usize, StyleIndices, Rect)>;
    for cell in &prepared.grid.placements {
        let Some((part, style)) = style_for_cell(cell) else {
            continue;
        };
        if session.reader.used_style(style).visibility() == Visibility::Hidden {
            continue;
        }
        let Some(rect) =
            cell_background_rect(cell, table_content_pos, columns, row_starts, row_heights)
        else {
            continue;
        };
        if let Some((previous_part, previous_style, previous)) = &mut pending {
            let adjacent = (previous.y0 == rect.y0
                && previous.y1 == rect.y1
                && (previous.x1 == rect.x0 || rect.x1 == previous.x0))
                || (previous.x0 == rect.x0
                    && previous.x1 == rect.x1
                    && (previous.y1 == rect.y0 || rect.y1 == previous.y0));
            if *previous_part == part && *previous_style == style && adjacent {
                *previous = previous.union(rect);
                continue;
            }
            record_bound_background(
                session,
                table_box_idx,
                *previous,
                PaintStyleSource::Style {
                    indices: *previous_style,
                    context: table_box_idx as u32,
                },
            );
        }
        pending = Some((part, style, rect));
    }
    if let Some((_, style, rect)) = pending {
        record_bound_background(
            session,
            table_box_idx,
            rect,
            PaintStyleSource::Style {
                indices: style,
                context: table_box_idx as u32,
            },
        );
    }
}

pub(super) fn emit_table_row_layer_background<'a>(
    session: &mut LayoutEngine<'_, '_>,
    owner_box_idx: usize,
    placements: impl Iterator<Item = &'a TableCellPlacement>,
    table_content_pos: Point,
    columns: &TableColumnLayout,
    row_starts: &[f64],
    row_heights: &[f64],
) {
    for placement in placements {
        let Some(rect) = cell_background_rect(
            placement,
            table_content_pos,
            columns,
            row_starts,
            row_heights,
        ) else {
            continue;
        };
        record_bound_background(
            session,
            owner_box_idx,
            rect,
            PaintStyleSource::Box(owner_box_idx as u32),
        );
    }
}

fn cell_background_rect(
    placement: &TableCellPlacement,
    table_content_pos: Point,
    columns: &TableColumnLayout,
    row_starts: &[f64],
    row_heights: &[f64],
) -> Option<Rect> {
    let width = columns.span_width(placement.col, placement.colspan);
    let height = span_extent(row_starts, row_heights, placement.row, placement.rowspan);
    columns.starts.get(placement.col)?;
    let y = *row_starts.get(placement.row)?;
    let x = columns.physical_start(placement.col, placement.colspan);
    (width > 0.0 && height > 0.0).then(|| {
        Rect::new(
            table_content_pos.x + x,
            table_content_pos.y + y,
            table_content_pos.x + x + width,
            table_content_pos.y + y + height,
        )
    })
}
