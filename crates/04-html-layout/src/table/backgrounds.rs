use super::columns::{TableColumnLayout, span_extent};
use crate::layout::LayoutEngine;
use crate::layout::paint_program::PaintStyleSource;
use crate::layout_model::{TableColumnGroupSpan, TableColumnTrack};
use kurbo::{Point, Rect};

fn emit_table_background_rect(
    session: &mut LayoutEngine<'_, '_>,
    table_box_idx: usize,
    rect: Rect,
    source: PaintStyleSource,
) {
    crate::layout::decorations::emit_bound_background(session, table_box_idx, rect, source);
}

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
    emit_table_background_rect(
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
    h_spacing: f64,
    v_spacing: f64,
    columns: &TableColumnLayout,
    column_tracks: &[TableColumnTrack],
    column_groups: &[TableColumnGroupSpan],
) {
    let _ = (h_spacing, v_spacing);
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
        let span_width = span_extent(&columns.starts, &columns.widths, start, end - start);
        if span_width <= 0.0 {
            continue;
        }
        let x = table_content_pos.x + columns.physical_start(start, end - start);
        if session.reader.used_style(group.style).visibility()
            == html_style_model::Visibility::Hidden
        {
            continue;
        }
        emit_table_background_rect(
            session,
            table_box_idx,
            Rect::new(x, table_grid.y0, x + span_width, table_grid.y1),
            PaintStyleSource::Style {indices:group.style,context:table_box_idx as u32},
        );
    }

    for (column, track) in column_tracks.iter().enumerate() {
        let Some(style_idx) = track.background_style else {
            continue;
        };
        let Some((&width, _)) = columns.widths.get(column).zip(columns.starts.get(column))
        else {
            continue;
        };
        let x = table_content_pos.x + columns.physical_start(column, 1);
        if session.reader.used_style(style_idx).visibility() == html_style_model::Visibility::Hidden
        {
            continue;
        }
        emit_table_background_rect(
            session,
            table_box_idx,
            Rect::new(x, table_grid.y0, x + width, table_grid.y1),
            PaintStyleSource::Style {indices:style_idx,context:table_box_idx as u32},
        );
    }
}
