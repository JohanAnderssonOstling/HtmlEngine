//! Publish resolved row and cell geometry, then emit table decorations.

use super::backgrounds::{emit_table_layers, emit_table_row_layer_background};
use super::captions::layout_caption_stack;
use super::cell::retain_cell;
use super::s04_rows::{CellHeightDependency, PreparedCellLayout};
use super::{ResolvedTablePlan, defer_row_out_of_flow};
use crate::boxes::{translate_laid_out_content, translate_laid_out_output};
use crate::decorations::{
    record_block_background, record_block_border_and_outline, record_block_decorations,
};
use crate::{BoxLayoutRequest, LayoutEngine};
use html_style_model::{EmptyCellsMode, VerticalAlignValue};
use kurbo::{Point, Rect, Size, Vec2};

pub(super) fn publish_table_plan(
    engine: &mut LayoutEngine<'_, '_>,
    plan: ResolvedTablePlan<'_>,
) -> Size {
    // Captions participate in the anonymous table wrapper, not in the table
    // grid's border box. Keep the wrapper size for normal-flow placement, but
    // paint the table's own background and borders on the authoritative grid
    // rectangle rather than across a wider/taller caption stack.
    if !plan.prepared.grid.placements.is_empty()
        && (!plan.table.captions_top.is_empty() || !plan.table.captions_bottom.is_empty())
    {
        let box_point = engine.geometry.point(plan.input.table_box_idx);
        engine.geometry.set_decoration_rect(
            plan.input.table_box_idx,
            Rect::new(
                plan.geometry.origin.x - box_point.x - plan.input.grid_insets.left,
                plan.geometry.origin.y - box_point.y - plan.input.grid_insets.top,
                plan.geometry.origin.x - box_point.x
                    + plan.geometry.columns.table_width
                    + plan.input.grid_insets.right,
                plan.geometry.origin.y - box_point.y
                    + plan.geometry.height
                    + plan.input.grid_insets.bottom,
            ),
        );
    }

    let used_table_width = plan
        .geometry
        .columns
        .table_width
        .max(plan.input.content_width);
    engine.geometry.set_size(
        plan.input.table_box_idx,
        Size::new(
            used_table_width,
            plan.caption_top_height + plan.geometry.height + plan.caption_bottom_height,
        ),
    );

    for (row_idx, &row_box_idx) in plan.prepared.rows.iter().enumerate() {
        let row_box_idx = row_box_idx as usize;
        engine.placement.bind_box(row_box_idx);
        engine.geometry.set_point(
            row_box_idx,
            plan.geometry.origin
                + Vec2::new(
                    plan.geometry.columns.spacing + plan.prepared.outer_borders.left,
                    plan.geometry.rows.starts[row_idx],
                ),
        );
        engine.geometry.set_size(
            row_box_idx,
            Size::new(
                plan.geometry.columns.table_width
                    - 2.0 * plan.geometry.columns.spacing
                    - plan.prepared.outer_borders.horizontal(),
                plan.geometry.rows.heights[row_idx],
            ),
        );
        defer_row_out_of_flow(engine, row_box_idx, engine.geometry.point(row_box_idx));
    }

    emit_table_layers(
        engine,
        plan.input.table_box_idx,
        &plan.geometry,
        &plan.prepared,
    );

    for (group_idx, range) in &plan.prepared.row_groups {
        let height = plan.geometry.rows.span_height(range.start, range.len());
        engine.placement.bind_box(*group_idx as usize);
        engine.geometry.set_point(
            *group_idx as usize,
            Point::new(
                plan.geometry.origin.x + plan.geometry.columns.spacing,
                plan.geometry.origin.y + plan.geometry.rows.starts[range.start],
            ),
        );
        engine.geometry.set_size(
            *group_idx as usize,
            Size::new(
                (plan.geometry.columns.table_width - 2.0 * plan.geometry.columns.spacing).max(0.0),
                height.max(0.0),
            ),
        );
        let first_cell = plan
            .prepared
            .grid
            .placements
            .partition_point(|cell| cell.row < range.start);
        let end_cell = plan
            .prepared
            .grid
            .placements
            .partition_point(|cell| cell.row < range.end);
        emit_table_row_layer_background(
            engine,
            *group_idx as usize,
            &plan.prepared.grid.placements[first_cell..end_cell],
            &plan.geometry,
        );
        if plan.prepared.collapsed_borders.is_none() {
            record_block_border_and_outline(engine, *group_idx as usize);
        }
    }

    let mut cell_start = 0;
    for (row_idx, &row_box_idx) in plan.prepared.rows.iter().enumerate() {
        let cell_end = cell_start
            + plan.prepared.grid.placements[cell_start..]
                .iter()
                .take_while(|cell| cell.row == row_idx)
                .count();
        emit_table_row_layer_background(
            engine,
            row_box_idx as usize,
            &plan.prepared.grid.placements[cell_start..cell_end],
            &plan.geometry,
        );
        cell_start = cell_end;
        if plan.prepared.collapsed_borders.is_none() {
            record_block_border_and_outline(engine, row_box_idx as usize);
        }
    }

    for (placement, cell) in plan.prepared.grid.placements.iter().zip(plan.cells.cells) {
        let rect = plan.geometry.cell_rect(placement);
        let span_width = rect.width();
        let span_height = rect.height();
        let final_point = rect.origin();
        let retained = match cell.layout {
            PreparedCellLayout::Retained(retained) => {
                translate_laid_out_output(
                    engine,
                    &retained.output,
                    final_point - retained.provisional_point,
                );
                retained
            }
            PreparedCellLayout::Relayout(dependency) => {
                let used_borders = plan
                    .prepared
                    .collapsed_borders
                    .as_ref()
                    .map(|grid| grid.cell_insets(*placement));
                let mut request = BoxLayoutRequest::table_cell(
                    placement.cell_idx,
                    span_width,
                    plan.input.parent_content_height,
                    used_borders,
                );
                if dependency == CellHeightDependency::AssignedHeight {
                    request = request.with_table_cell_height(span_height);
                }
                retain_cell(engine, final_point, request, used_borders).2
            }
        };
        // Cell `height` is a minimum for the cell box, not part of the natural
        // content height used by `vertical-align`. Otherwise an explicitly
        // tall cell has no remaining alignment space and bottom-aligned text
        // incorrectly stays at the top.
        let cell_style = engine.read.reader.style(placement.cell_idx);
        let vertical_offset = cell_vertical_offset(
            engine,
            placement.cell_idx,
            retained.natural_border_height,
            span_height,
            plan.cells.row_baselines[placement.row],
            cell.baseline,
        );
        if vertical_offset.abs() > 0.01 {
            translate_laid_out_content(engine, &retained.output, Vec2::new(0.0, vertical_offset));
        }
        let mut size = engine.geometry.size(placement.cell_idx);
        size.width = span_width;
        // A table cell occupies its grid area even when final percentage
        // resolution makes its contents overflow that area.
        size.height = span_height;
        engine.geometry.set_size(placement.cell_idx, size);
        let hide_empty_borders = matches!(cell_style.empty_cells(), EmptyCellsMode::Hide)
            && !retained.has_visible_content;
        if !hide_empty_borders {
            engine.placement.select(retained.output.placement.box_group);
            if plan.prepared.collapsed_borders.is_some() {
                record_block_background(engine, placement.cell_idx);
            } else {
                record_block_decorations(engine, placement.cell_idx);
            }
            engine
                .placement
                .select(retained.output.placement.previous_group());
        }
    }

    if let Some(grid) = plan.prepared.collapsed_borders.as_ref() {
        grid.emit(engine, &plan.geometry, plan.prepared.rows);
    }

    let caption_bottom_height = layout_caption_stack(
        engine,
        &plan.table.captions_bottom,
        Point::new(
            plan.input.content_pos.x - plan.input.grid_insets.left,
            plan.geometry.origin.y + plan.geometry.height + plan.input.grid_insets.bottom,
        ),
        used_table_width + plan.input.grid_insets.horizontal(),
        plan.input.parent_content_height,
    );
    Size::new(
        used_table_width,
        plan.caption_top_height + plan.geometry.height + caption_bottom_height,
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
    match engine.read.reader.style(cell_idx).vertical_align() {
        VerticalAlignValue::Top => 0.0,
        VerticalAlignValue::Bottom | VerticalAlignValue::TextBottom => extra,
        VerticalAlignValue::Middle => extra * 0.5,
        _ => match (row_baseline, cell_baseline) {
            (Some(row), Some(cell)) => (row - cell).max(0.0),
            _ => 0.0,
        },
    }
}
