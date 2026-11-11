//! Publish resolved row and cell geometry, then emit table decorations.

use super::ResolvedTablePlan;
use super::backgrounds::{emit_table_layers, emit_table_row_layer_background};
use super::captions::layout_caption_stack;
use super::columns::{row_group_range, span_extent};
use super::s04_rows::{PreparedCellLayout, cell_content_metrics, layout_cell};
use crate::layout::{
    BoxLayoutRequest, LayoutEngine, record_block_background, record_block_border_and_outline,
    record_block_decorations, translate_laid_out_content, translate_laid_out_output,
};
use html_engine_model::LayoutMode;
use html_style_model::{EmptyCellsMode, VerticalAlignValue};
use kurbo::{Point, Rect, Size, Vec2};

pub(super) fn publish_table_plan(
    engine: &mut LayoutEngine<'_, '_>,
    plan: ResolvedTablePlan<'_>,
) -> Size {
    let input = &plan.input;
    let structure = &plan.structure;
    let prepared = &plan.prepared;
    let grid = &prepared.grid;
    let columns = &plan.columns;
    let row_heights = &plan.row_heights;
    let row_starts = &plan.row_starts;
    let table_content_pos = plan.table_content_pos;
    let table_height = plan.table_height;
    let h_spacing = prepared.horizontal_spacing;
    let collapsed_grid = prepared.collapsed_borders.as_ref();
    let collapsed_borders = collapsed_grid.is_some();
    let outer_borders = prepared.outer_borders;
    let row_baselines = &plan.cells.row_baselines;
    let table_box_idx = input.table_box_idx;
    let rows = input.rows;
    let content_pos = input.content_pos;
    let content_width = input.content_width;
    let parent_content_height = input.parent_content_height;
    let caption_top_height = plan.caption_top_height;

    // Captions participate in the anonymous table wrapper, not in the table
    // grid's border box. Keep the wrapper size for normal-flow placement, but
    // paint the table's own background and borders on the authoritative grid
    // rectangle rather than across a wider/taller caption stack.
    if !structure.captions_top.is_empty() || !structure.captions_bottom.is_empty() {
        let box_point = engine.geometry.point(table_box_idx);
        engine.geometry.set_decoration_rect(
            table_box_idx,
            Rect::new(
                table_content_pos.x - box_point.x,
                table_content_pos.y - box_point.y,
                table_content_pos.x - box_point.x + columns.table_width,
                table_content_pos.y - box_point.y + table_height,
            ),
        );
    }

    let used_table_width = columns.table_width.max(content_width);
    engine.geometry.set_size(
        table_box_idx,
        Size::new(
            used_table_width,
            caption_top_height + table_height + plan.caption_bottom_height,
        ),
    );

    for (row_idx, &row_box_idx) in rows.iter().enumerate() {
        let row_box_idx = row_box_idx as usize;
        engine.placement.bind_box(row_box_idx);
        engine.geometry.set_point(
            row_box_idx,
            table_content_pos + Vec2::new(h_spacing + outer_borders.left, row_starts[row_idx]),
        );
        engine.geometry.set_size(
            row_box_idx,
            Size::new(
                columns.table_width - 2.0 * h_spacing - outer_borders.horizontal(),
                row_heights[row_idx],
            ),
        );
        defer_row_out_of_flow(engine, row_box_idx, engine.geometry.point(row_box_idx));
    }

    emit_table_layers(
        engine,
        table_box_idx,
        table_content_pos,
        table_height,
        columns,
        prepared,
        row_starts,
        row_heights,
    );

    for &group_idx in structure.row_groups {
        let Some(range) = row_group_range(&engine.reader, rows, group_idx as usize) else {
            continue;
        };
        let y = row_starts[range.start];
        let height = span_extent(row_starts, row_heights, range.start, range.len());
        engine.placement.bind_box(group_idx as usize);
        engine.geometry.set_point(
            group_idx as usize,
            Point::new(table_content_pos.x + h_spacing, table_content_pos.y + y),
        );
        engine.geometry.set_size(
            group_idx as usize,
            Size::new(
                (columns.table_width - 2.0 * h_spacing).max(0.0),
                height.max(0.0),
            ),
        );
        emit_table_row_layer_background(
            engine,
            group_idx as usize,
            grid.placements
                .iter()
                .filter(|placement| range.contains(&placement.row)),
            table_content_pos,
            columns,
            row_starts,
            row_heights,
        );
        if !collapsed_borders {
            record_block_border_and_outline(engine, group_idx as usize);
        }
    }

    for (row_idx, &row_box_idx) in rows.iter().enumerate() {
        emit_table_row_layer_background(
            engine,
            row_box_idx as usize,
            grid.placements
                .iter()
                .filter(|placement| placement.row == row_idx),
            table_content_pos,
            columns,
            row_starts,
            row_heights,
        );
        if !collapsed_borders {
            record_block_border_and_outline(engine, row_box_idx as usize);
        }
    }

    for (placement, cell) in grid.placements.iter().zip(plan.cells.cells) {
        let span_width = columns.span_width(placement.col, placement.colspan);
        let span_height = span_extent(row_starts, row_heights, placement.row, placement.rowspan);

        let final_point = Point::new(
            table_content_pos.x + columns.physical_start(placement.col, placement.colspan),
            table_content_pos.y + row_starts[placement.row],
        );
        let (cell_output, natural_height, cell_has_content) = match cell.layout {
            PreparedCellLayout::Retained(retained) => {
                translate_laid_out_output(
                    engine,
                    &retained.output,
                    final_point - retained.provisional_point,
                );
                (
                    retained.output,
                    retained.natural_border_height,
                    retained.has_visible_content,
                )
            }
            PreparedCellLayout::Relayout {
                assign_final_height,
            } => {
                let used_borders = collapsed_grid.map(|grid| grid.cell_insets(*placement));
                let mut request = BoxLayoutRequest::table_cell(
                    placement.cell_idx,
                    span_width,
                    parent_content_height,
                    used_borders,
                );
                if assign_final_height {
                    request = request.with_assigned_border_height(span_height);
                }
                let cell_layout = layout_cell(engine, final_point, request);
                let (natural_height, cell_has_content) = cell_content_metrics(
                    engine,
                    placement.cell_idx,
                    span_width,
                    used_borders,
                    &cell_layout,
                );
                (cell_layout.output, natural_height, cell_has_content)
            }
        };
        // Cell `height` is a minimum for the cell box, not part of the natural
        // content height used by `vertical-align`. Otherwise an explicitly
        // tall cell has no remaining alignment space and bottom-aligned text
        // incorrectly stays at the top.
        let cell_style = engine.reader.style(placement.cell_idx);
        let vertical_offset = cell_vertical_offset(
            engine,
            placement.cell_idx,
            natural_height,
            span_height,
            row_baselines[placement.row],
            cell.baseline,
        );
        if vertical_offset.abs() > 0.01 {
            translate_laid_out_content(engine, &cell_output, Vec2::new(0.0, vertical_offset));
        }
        let mut size = engine.geometry.size(placement.cell_idx);
        size.width = span_width;
        // A table cell occupies its grid area even when final percentage
        // resolution makes its contents overflow that area.
        size.height = span_height;
        engine.geometry.set_size(placement.cell_idx, size);
        let hide_empty_borders =
            matches!(cell_style.empty_cells(), EmptyCellsMode::Hide) && !cell_has_content;
        if !hide_empty_borders {
            engine.placement.select(cell_output.placement.box_group);
            if collapsed_borders {
                record_block_background(engine, placement.cell_idx);
            } else {
                record_block_decorations(engine, placement.cell_idx);
            }
            engine
                .placement
                .select(cell_output.placement.previous_group());
        }
    }

    if let Some(grid) = collapsed_grid {
        grid.emit(engine, table_content_pos, columns, row_starts, row_heights, rows);
    }

    let caption_bottom_height = layout_caption_stack(
        engine,
        structure.captions_bottom,
        Point::new(
            content_pos.x - input.grid_insets.left,
            table_content_pos.y + table_height + input.grid_insets.bottom,
        ),
        used_table_width + input.grid_insets.horizontal(),
        parent_content_height,
    );
    Size::new(
        used_table_width,
        caption_top_height + table_height + caption_bottom_height,
    )
}

fn cell_vertical_offset(
    engine: &LayoutEngine<'_, '_>,
    cell_idx: usize,
    natural_height: f64,
    span_height: f64,
    row_baseline: Option<f64>,
    cell_baseline: Option<f64>,
) -> f64 {
    let extra = (span_height - natural_height).max(0.0);
    match engine.reader.style(cell_idx).vertical_align() {
        VerticalAlignValue::Top => 0.0,
        VerticalAlignValue::Bottom | VerticalAlignValue::TextBottom => extra,
        VerticalAlignValue::Middle => extra * 0.5,
        _ => match (row_baseline, cell_baseline) {
            (Some(row), Some(cell)) => (row - cell).max(0.0),
            _ => 0.0,
        },
    }
}

pub(super) fn defer_row_out_of_flow(
    engine: &mut LayoutEngine<'_, '_>,
    row_box_idx: usize,
    static_position: Point,
) {
    let reader = engine.reader;
    let Some(LayoutMode::TableRow(row)) = reader.box_layout_mode(row_box_idx) else {
        return;
    };
    for &child in &row.out_of_flow {
        engine.defer_absolute_box(child as usize, static_position);
    }
}
