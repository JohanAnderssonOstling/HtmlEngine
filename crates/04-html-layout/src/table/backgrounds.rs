use super::columns::{TableCellPlacement, TableColumnLayout, span_extent};
use crate::layout::LayoutEngine;
use crate::layout::decorations::record_bound_background;
use crate::layout::paint_program::PaintStyleSource;
use crate::layout_model::{TableColumnGroupSpan, TableColumnTrack};
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
    column_tracks: &[TableColumnTrack],
    column_groups: &[TableColumnGroupSpan],
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

    for group in column_groups {
        if group.span == 0 {
            continue;
        }
        let start = group.start.min(columns.starts.len());
        let end = start.saturating_add(group.span).min(columns.starts.len());
        if start >= end {
            continue;
        }
        let span_width = columns.span_width(start, end - start);
        if span_width <= 0.0 {
            continue;
        }
        let x = table_content_pos.x + columns.physical_start(start, end - start);
        if session.reader.used_style(group.style).visibility()
            == html_style_model::Visibility::Hidden
        {
            continue;
        }
        record_bound_background(
            session,
            table_box_idx,
            Rect::new(x, table_grid.y0, x + span_width, table_grid.y1),
            PaintStyleSource::Style {
                indices: group.style,
                context: table_box_idx as u32,
            },
        );
    }

    for (column, track) in column_tracks.iter().enumerate() {
        let Some(style_idx) = track.background_style else {
            continue;
        };
        let Some((&width, _)) = columns.widths.get(column).zip(columns.starts.get(column)) else {
            continue;
        };
        let x = table_content_pos.x + columns.physical_start(column, 1);
        if session.reader.used_style(style_idx).visibility() == html_style_model::Visibility::Hidden
        {
            continue;
        }
        record_bound_background(
            session,
            table_box_idx,
            Rect::new(x, table_grid.y0, x + width, table_grid.y1),
            PaintStyleSource::Style {
                indices: style_idx,
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
        let width = columns.span_width(placement.col, placement.colspan);
        let height = span_extent(row_starts, row_heights, placement.row, placement.rowspan);
        let Some((_, &y)) = columns
            .starts
            .get(placement.col)
            .zip(row_starts.get(placement.row))
        else {
            continue;
        };
        let x = columns.physical_start(placement.col, placement.colspan);
        record_bound_background(
            session,
            owner_box_idx,
            Rect::new(
                table_content_pos.x + x,
                table_content_pos.y + y,
                table_content_pos.x + x + width,
                table_content_pos.y + y + height,
            ),
            PaintStyleSource::Box(owner_box_idx as u32),
        );
    }
}
