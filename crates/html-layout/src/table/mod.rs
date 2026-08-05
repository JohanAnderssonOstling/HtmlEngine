use crate::layout::LayoutEngine;
use crate::layout_model::LayoutMode;
use kurbo::{Point, Rect, Size, Vec2};
use std::time::Instant;

pub(crate) fn parse_positive_span(value: Option<&str>) -> usize {
    value.and_then(|value| value.trim().parse::<usize>().ok()).filter(|&value| value > 0).unwrap_or(1)
}

mod backgrounds;
mod borders;
mod captions;
mod columns;
mod rows;
mod sizing;

use backgrounds::{emit_collapsed_table_background, emit_table_layers};
use borders::CollapsedBorderGrid;
use captions::{intrinsic_widths as caption_intrinsic_widths, layout_caption_stack};
use columns::{TableGrid, build_table_grid, span_extent};
pub(crate) use rows::layout_table_row;
use rows::{cell_has_visible_content, cell_vertical_offset, measure_cells, resolve_row_heights};
use sizing::{intrinsic_column_widths, resolve_table_columns};

fn table_grid(session: &LayoutEngine<'_, '_>, rows: &[u32]) -> TableGrid {
    build_table_grid(rows.iter().map(|&row_idx| {
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
                    (cell_idx, colspan, rowspan)
                })
                .collect(),
            _ => Vec::new(),
        }
    }))
}

fn layout_empty_grid(
    session: &mut LayoutEngine<'_, '_>, table_box_idx: usize, captions_top: &[u32], captions_bottom: &[u32], content_pos: Point, content_width: f64, parent_content_height: Option<f64>, explicit_content_height: Option<f64>,
) -> Size {
    let caption_top_height = layout_caption_stack(session, captions_top, content_pos, content_width, parent_content_height);
    let measured_bottom_height = captions_bottom.iter().map(|&caption_idx| crate::layout::measure_box_isolated(session, caption_idx as usize, content_width, parent_content_height).height).sum::<f64>();
    let grid_height = explicit_content_height.map_or(0.0, |height| (height - caption_top_height - measured_bottom_height).max(0.0));
    emit_collapsed_table_background(session, table_box_idx, Rect::new(content_pos.x, content_pos.y + caption_top_height, content_pos.x + content_width, content_pos.y + caption_top_height + grid_height));
    let caption_bottom_height = layout_caption_stack(session, captions_bottom, Point::new(content_pos.x, content_pos.y + caption_top_height + grid_height), content_width, parent_content_height);
    let size = Size::new(content_width, caption_top_height + grid_height + caption_bottom_height);
    session.geometry.set_size(table_box_idx, size);
    size
}

pub(crate) fn layout_table(
    session: &mut LayoutEngine<'_, '_>, table_box_idx: usize, rows: Vec<u32>, content_pos: Point, content_width: f64, parent_content_height: Option<f64>, explicit_content_height: Option<f64>, uses_auto_intrinsic_width: bool,
) -> Size {
    let timing_started = Instant::now();
    let (row_groups, captions_top, captions_bottom, columns_meta, column_groups, column_width_hints) = match session.reader.box_layout_mode(table_box_idx) {
        Some(LayoutMode::Table(table)) => (table.row_groups.clone(), table.captions_top.clone(), table.captions_bottom.clone(), table.columns.clone(), table.column_groups.clone(), table.column_width_hints.clone()),
        _ => (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()),
    };
    if rows.is_empty() {
        return layout_empty_grid(session, table_box_idx, &captions_top, &captions_bottom, content_pos, content_width, parent_content_height, explicit_content_height);
    }

    let grid = table_grid(session, &rows);
    if grid.column_count == 0 {
        for &row_box_idx in &rows {
            let row_box_idx = row_box_idx as usize;
            session.geometry.set_point(row_box_idx, content_pos);
            session.geometry.set_size(row_box_idx, Size::ZERO);
            defer_row_out_of_flow(session, row_box_idx, content_pos);
        }
        return layout_empty_grid(session, table_box_idx, &captions_top, &captions_bottom, content_pos, content_width, parent_content_height, explicit_content_height);
    }
    let caption_top_height = layout_caption_stack(session, &captions_top, content_pos, content_width, parent_content_height);
    let table_content_pos = Point::new(content_pos.x, content_pos.y + caption_top_height);

    let table_style = session.reader.style(table_box_idx);
    let collapsed_borders = matches!(table_style.border_collapse(), html_style_model::BorderCollapseMode::Collapse);
    let h_spacing = if collapsed_borders { 0.0 } else { table_style.border_spacing_horizontal().max(0.0) as f64 };
    let v_spacing = if collapsed_borders { 0.0 } else { table_style.border_spacing_vertical().max(0.0) as f64 };
    let collapsed_grid = collapsed_borders.then(|| CollapsedBorderGrid::build(&session.reader, table_box_idx, &rows, &row_groups, &grid.placements, &columns_meta, &column_groups));
    let outer_borders = collapsed_grid.as_ref().map_or_else(crate::layout::UsedBorderInsets::default, CollapsedBorderGrid::outer_insets);
    // The width entering table layout is the complete used table width for
    // both authored and intrinsic sizing. Collapsed outer edges straddle the
    // grid boundary, so reserve their outward-facing halves before resolving
    // the column tracks.
    let used_grid_outer_width = if uses_auto_intrinsic_width && !crate::layout::contains_full_width_percentage_table(session, table_box_idx) {
        let (min_widths, max_widths) = intrinsic_column_widths(session, &grid.placements, grid.column_count, &column_width_hints, h_spacing, collapsed_grid.as_ref());
        let spacing = h_spacing * (grid.column_count as f64 + 1.0);
        let intrinsic_min = min_widths.iter().sum::<f64>() + spacing + outer_borders.horizontal();
        let intrinsic_max = max_widths.iter().sum::<f64>() + spacing + outer_borders.horizontal();
        intrinsic_max.min(content_width).max(intrinsic_min)
    } else {
        content_width
    };
    let grid_width = (used_grid_outer_width - outer_borders.horizontal()).max(0.0);
    let mut columns = resolve_table_columns(session, &grid.placements, grid.column_count, &column_width_hints, grid_width, h_spacing, collapsed_grid.as_ref());
    for start in &mut columns.starts {
        *start += outer_borders.left;
    }
    columns.table_width += outer_borders.horizontal();
    let measurements = measure_cells(session, &grid.placements, rows.len(), &columns, table_content_pos, v_spacing, parent_content_height, collapsed_grid.as_ref());
    // Measure bottom captions in isolation so they do not emit paint before
    // their final position is known.
    let caption_bottom_height = captions_bottom.iter().map(|&caption_idx| crate::layout::measure_box_isolated(session, caption_idx as usize, content_width, parent_content_height).height).sum::<f64>();
    let row_heights = resolve_row_heights(session, &rows, &row_groups, measurements.row_heights, v_spacing, explicit_content_height, caption_top_height + caption_bottom_height, outer_borders.vertical());

    let mut row_starts = vec![0.0f64; rows.len()];
    let mut cursor_y = v_spacing + outer_borders.top;
    for i in 0..rows.len() {
        row_starts[i] = cursor_y;
        cursor_y += row_heights[i] + v_spacing;
    }
    let table_height = cursor_y + outer_borders.bottom;

    // Captions participate in the anonymous table wrapper, not in the table
    // grid's border box. Keep the wrapper size for normal-flow placement, but
    // paint the table's own background and borders on the authoritative grid
    // rectangle rather than across a wider/taller caption stack.
    if !captions_top.is_empty() || !captions_bottom.is_empty() {
        let box_point = session.geometry.point(table_box_idx);
        session
            .geometry
            .set_decoration_rect(table_box_idx, Rect::new(table_content_pos.x - box_point.x, table_content_pos.y - box_point.y, table_content_pos.x - box_point.x + columns.table_width, table_content_pos.y - box_point.y + table_height));
    }

    session.geometry.set_size(table_box_idx, Size::new(columns.table_width.max(content_width), caption_top_height + table_height + caption_bottom_height));

    for (row_idx, &row_box_idx) in rows.iter().enumerate() {
        let row_box_idx = row_box_idx as usize;
        session.geometry.set_point(row_box_idx, table_content_pos + Vec2::new(h_spacing + outer_borders.left, row_starts[row_idx]));
        session.geometry.set_size(row_box_idx, Size::new(columns.table_width - 2.0 * h_spacing - outer_borders.horizontal(), row_heights[row_idx]));
        defer_row_out_of_flow(session, row_box_idx, session.geometry.point(row_box_idx));
    }

    emit_table_layers(session, table_box_idx, table_content_pos, table_height, h_spacing, v_spacing, &columns, &columns_meta, &column_groups);

    for group_idx in row_groups {
        let group_rows: Vec<usize> = rows.iter().copied().map(|row| row as usize).filter(|&row| session.reader.get_parent(row) == Some(group_idx as usize)).collect();
        let (Some(first), Some(last)) = (group_rows.first().copied(), group_rows.last().copied()) else {
            continue;
        };
        let first_row = rows.iter().position(|&row| row as usize == first).expect("group row belongs to table");
        let last_row = rows.iter().position(|&row| row as usize == last).expect("group row belongs to table");
        let y = row_starts[first_row];
        let height = row_starts[last_row] + row_heights[last_row] - y;
        session.geometry.set_point(group_idx as usize, Point::new(table_content_pos.x + h_spacing, table_content_pos.y + y));
        session.geometry.set_size(group_idx as usize, Size::new((columns.table_width - 2.0 * h_spacing).max(0.0), height.max(0.0)));
        emit_table_row_layer_background(session, group_idx as usize, grid.placements.iter().filter(|placement| (first_row..=last_row).contains(&placement.row)), table_content_pos, &columns, &row_starts, &row_heights);
        if !collapsed_borders {
            crate::layout::emit_block_border_and_outline(session, group_idx as usize);
        }
    }

    for (row_idx, &row_box_idx) in rows.iter().enumerate() {
        emit_table_row_layer_background(session, row_box_idx as usize, grid.placements.iter().filter(|placement| placement.row == row_idx), table_content_pos, &columns, &row_starts, &row_heights);
        if !collapsed_borders {
            crate::layout::emit_block_border_and_outline(session, row_box_idx as usize);
        }
    }

    for (i, placement) in grid.placements.iter().enumerate() {
        let span_width = span_extent(&columns.starts, &columns.widths, placement.col, placement.colspan);
        let span_height = span_extent(&row_starts, &row_heights, placement.row, placement.rowspan);

        session.geometry.set_point(placement.cell_idx, Point::new(table_content_pos.x + columns.starts[placement.col], table_content_pos.y + row_starts[placement.row]));
        let cell_layout = session.without_fragmentation(|session| {
            if let Some(grid) = collapsed_grid.as_ref() {
                session.layout_box(crate::layout::BoxLayoutRequest::collapsed_table_cell(placement.cell_idx, span_width, parent_content_height, grid.cell_insets(*placement)))
            } else {
                session.layout_box(crate::layout::BoxLayoutRequest::normal(placement.cell_idx, span_width, parent_content_height))
            }
        });
        // Cell `height` is a minimum for the cell box, not part of the natural
        // content height used by `vertical-align`. Otherwise an explicitly
        // tall cell has no remaining alignment space and bottom-aligned text
        // incorrectly stays at the top.
        let cell_style = session.reader.style(placement.cell_idx);
        let natural_height = session.natural_content_height(placement.cell_idx) + cell_style.get_vertical_padding(span_width) + cell_style.border_top_width() as f64 + cell_style.border_bottom_width() as f64;
        let vertical_offset = cell_vertical_offset(session, placement.cell_idx, natural_height, span_height, measurements.row_baselines[placement.row], measurements.baseline_offsets[i]);
        let cell_has_content = cell_has_visible_content(session, placement.cell_idx, &cell_layout.output);
        if vertical_offset.abs() > 0.01 {
            crate::layout::translate_laid_out_subtree_output(session, placement.cell_idx, false, &cell_layout.output, Vec2::new(0.0, vertical_offset));
        }
        let mut size = session.geometry.size(placement.cell_idx);
        size.width = span_width;
        size.height = size.height.max(span_height);
        session.geometry.set_size(placement.cell_idx, size);
        let hide_empty_borders = matches!(cell_style.empty_cells(), html_style_model::EmptyCellsMode::Hide) && !cell_has_content;
        if !hide_empty_borders {
            if collapsed_borders {
                crate::layout::emit_block_background(session, placement.cell_idx);
            } else {
                crate::layout::emit_block_decorations(session, placement.cell_idx);
            }
        }
    }

    if let Some(grid) = collapsed_grid.as_ref() {
        grid.emit(&mut session.fragments, table_box_idx, table_content_pos, &columns.starts, &columns.widths, &row_starts, &row_heights);
    }

    let caption_bottom_height = layout_caption_stack(session, &captions_bottom, Point::new(content_pos.x, table_content_pos.y + table_height), content_width, parent_content_height);
    let size = Size::new(columns.table_width.max(content_width), caption_top_height + table_height + caption_bottom_height);
    session.record_timing(|t| t.layout_table += timing_started.elapsed());
    size
}

fn emit_table_row_layer_background<'a>(
    session: &mut LayoutEngine<'_, '_>, owner_box_idx: usize, placements: impl Iterator<Item = &'a columns::TableCellPlacement>, table_content_pos: Point, columns: &columns::TableColumnLayout, row_starts: &[f64], row_heights: &[f64],
) {
    let color = session.reader.style(owner_box_idx).background_color();
    if color & 0xFF == 0 {
        return;
    }
    for placement in placements {
        let width = span_extent(&columns.starts, &columns.widths, placement.col, placement.colspan);
        let height = span_extent(row_starts, row_heights, placement.row, placement.rowspan);
        let Some((&x, &y)) = columns.starts.get(placement.col).zip(row_starts.get(placement.row)) else {
            continue;
        };
        crate::layout::emit_color_rect_for_owner(session, owner_box_idx, Rect::new(table_content_pos.x + x, table_content_pos.y + y, table_content_pos.x + x + width, table_content_pos.y + y + height), color);
    }
}

fn defer_row_out_of_flow(session: &mut LayoutEngine<'_, '_>, row_box_idx: usize, static_position: Point) {
    let children = match session.reader.box_layout_mode(row_box_idx) {
        Some(LayoutMode::TableRow(row)) => row.out_of_flow.clone(),
        _ => Vec::new(),
    };
    for child in children {
        session.defer_absolute_box(child as usize, static_position);
    }
}

pub(crate) fn table_intrinsic_widths(session: &LayoutEngine<'_, '_>, table_box_idx: usize) -> (f64, f64) {
    let Some(LayoutMode::Table(table)) = session.reader.box_layout_mode(table_box_idx) else {
        return (0.0, 0.0);
    };
    let grid = table_grid(session, &table.rows);
    if grid.column_count == 0 {
        return caption_intrinsic_widths(session, &table.captions_top, &table.captions_bottom);
    }

    let style = session.reader.style(table_box_idx);
    let h_spacing = if matches!(style.border_collapse(), html_style_model::BorderCollapseMode::Collapse) { 0.0 } else { style.border_spacing_horizontal().max(0.0) as f64 };
    let collapsed_grid = matches!(style.border_collapse(), html_style_model::BorderCollapseMode::Collapse)
        .then(|| CollapsedBorderGrid::build(&session.reader, table_box_idx, &table.rows, &table.row_groups, &grid.placements, &table.columns, &table.column_groups));
    let (min_widths, max_widths) = intrinsic_column_widths(session, &grid.placements, grid.column_count, &table.column_width_hints, h_spacing, collapsed_grid.as_ref());
    let spacing = h_spacing * (grid.column_count as f64 + 1.0);
    // Intrinsic widths describe the complete used outer width, matching the
    // width contract consumed by final table layout.
    let outer_width = collapsed_grid.as_ref().map_or(0.0, |grid| grid.outer_insets().horizontal());
    let mut min = min_widths.iter().sum::<f64>() + spacing + outer_width;
    let mut max = max_widths.iter().sum::<f64>() + spacing + outer_width;
    let (caption_min, caption_max) = caption_intrinsic_widths(session, &table.captions_top, &table.captions_bottom);
    min = min.max(caption_min);
    max = max.max(caption_max);
    (min, max.max(min))
}

include!("tests.rs");
