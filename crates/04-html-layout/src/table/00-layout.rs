use crate::layout::{
    LayoutEngine, ResolvedBoxSizing, ResolvedTableBoxSizing, constrain_content_width,
    constrain_size, contains_full_width_percentage_table, resolve_vertical_size,
};
use crate::layout_model::TableBox;
use html_style_model::{
    BorderCollapseMode, BoxSizing, TextDirection, UsedPreferredSize as PreferredSize,
};
use kurbo::{Point, Rect, Size, Vec2};

use super::backgrounds::emit_collapsed_table_background;
use super::captions::{CaptionMeasurement, layout_caption_stack, measure_caption_stack};
use super::column_sizing::{WidthMeasure, measure_table_columns, resolve_table_columns};
use super::columns::{row_group_range, span_extent};
use super::grid::{PreparedTableGrid, prepare_table_grid};
use super::intrinsic::intrinsic_sizes_for_grid;
use super::placement::{defer_row_out_of_flow, publish_table_plan};
use super::row_sizing::resolve_row_heights;
use super::rows::prepare_cells;
use super::{ResolvedTablePlan, TableDimensionBasis, TableLayoutInput, TableStructure};

fn resolve_table_input(
    session: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    rows: Vec<u32>,
    content_pos: Point,
    prepared: &PreparedTableGrid,
    measures: &[WidthMeasure],
) -> TableLayoutInput {
    let sizing = resolved.table_box_sizing();
    let uses_auto_intrinsic_width =
        !sizing.has_assigned_width && matches!(sizing.preferred_width, PreferredSize::Auto);
    let mut auto_grid_intrinsic = None;
    let content_width = if uses_auto_intrinsic_width {
        let intrinsic = intrinsic_sizes_for_grid(session, sizing.box_idx, prepared, measures);
        let intrinsic_min = intrinsic.minimum;
        let mut intrinsic_max = intrinsic.maximum;
        let requires_available_width = intrinsic.full_width_percentage_grid
            || contains_full_width_percentage_table(session, sizing.box_idx);
        let mut auto_table_available = sizing.content_width;
        if requires_available_width {
            auto_table_available = (sizing.auto_width_limit.unwrap_or(sizing.available_width)
                - sizing.horizontal_margin_padding
                - sizing.horizontal_border)
                .max(0.0);
            intrinsic_max = intrinsic_max.max(auto_table_available);
        } else {
            auto_grid_intrinsic = Some((intrinsic.minimum, intrinsic.grid_maximum));
        }
        constrain_content_width(
            intrinsic_max.min(auto_table_available.max(intrinsic_min)),
            sizing.min_width,
            sizing.max_width,
            sizing.available_width,
            sizing.horizontal_border_box_inset,
        )
    } else {
        sizing.content_width
    };
    let explicit_content_height = resolved_table_height(sizing);
    let collapsed_content_box = matches!(
        session.reader.style(sizing.box_idx).border_collapse(),
        BorderCollapseMode::Collapse
    ) && matches!(sizing.box_sizing, BoxSizing::ContentBox);
    let inline_basis = if collapsed_content_box
        && !sizing.has_assigned_width
        && !matches!(sizing.preferred_width, PreferredSize::Auto)
    {
        TableDimensionBasis::AuthoredContentBox
    } else {
        TableDimensionBasis::UsedBorderBox
    };
    let block_basis =
        if collapsed_content_box && !sizing.has_assigned_height && sizing.explicit_height.is_some()
        {
            TableDimensionBasis::AuthoredContentBox
        } else {
            TableDimensionBasis::UsedBorderBox
        };
    TableLayoutInput {
        table_box_idx: sizing.box_idx,
        rows,
        content_pos,
        content_width,
        parent_content_height: sizing.parent_content_height,
        explicit_content_height,
        auto_grid_intrinsic,
        inline_basis,
        block_basis,
    }
}

fn resolved_table_height(sizing: ResolvedTableBoxSizing) -> Option<f64> {
    if sizing.has_assigned_height {
        return sizing.explicit_height;
    }
    sizing
        .explicit_height
        .map(|height| constrain_size(height, sizing.min_height, sizing.max_height))
        .or(sizing.min_height)
}

fn layout_empty_table(
    session: &mut LayoutEngine<'_, '_>,
    input: &TableLayoutInput,
    structure: &TableStructure,
    prepared: &PreparedTableGrid,
) -> Size {
    let caption_top_height = layout_caption_stack(
        session,
        &structure.captions_top,
        input.content_pos,
        input.content_width,
        input.parent_content_height,
    );
    let measured_bottom_height = measure_caption_stack(
        session,
        &structure.captions_bottom,
        input.content_width,
        input.parent_content_height,
        CaptionMeasurement::BorderBoxes,
    );
    let table_content_pos = input.content_pos + Vec2::new(0.0, caption_top_height);
    let caption_height = caption_top_height + measured_bottom_height;
    let grid_height = if input.rows.is_empty() {
        layout_empty_grid(
            session,
            input,
            &structure.row_groups,
            table_content_pos,
            caption_height,
        )
    } else {
        layout_zero_column_grid(
            session,
            input,
            &structure.row_groups,
            table_content_pos,
            caption_height,
            prepared,
        )
    };
    let caption_bottom_height = layout_caption_stack(
        session,
        &structure.captions_bottom,
        table_content_pos + Vec2::new(0.0, grid_height),
        input.content_width,
        input.parent_content_height,
    );
    let size = Size::new(
        input.content_width,
        caption_top_height + grid_height + caption_bottom_height,
    );
    session.geometry.set_size(input.table_box_idx, size);
    size
}

fn layout_empty_grid(
    session: &mut LayoutEngine<'_, '_>,
    input: &TableLayoutInput,
    row_groups: &[u32],
    table_content_pos: Point,
    caption_height: f64,
) -> f64 {
    let mut group_heights = row_groups
        .iter()
        .map(|&group_idx| {
            resolve_vertical_size(
                session.reader.style(group_idx as usize).height(),
                input.explicit_content_height,
                0.0,
            )
            .unwrap_or(0.0)
        })
        .collect::<Vec<_>>();
    let natural_grid_height = group_heights.iter().sum::<f64>();
    let grid_height = input
        .explicit_content_height
        .map_or(natural_grid_height, |height| {
            (height - caption_height).max(natural_grid_height)
        });
    if !group_heights.is_empty() && grid_height > natural_grid_height {
        let extra = (grid_height - natural_grid_height) / group_heights.len() as f64;
        for height in &mut group_heights {
            *height += extra;
        }
    }
    let mut group_y = 0.0;
    for (&group_idx, height) in row_groups.iter().zip(group_heights) {
        session.geometry.set_point(
            group_idx as usize,
            table_content_pos + Vec2::new(0.0, group_y),
        );
        session
            .geometry
            .set_size(group_idx as usize, Size::new(input.content_width, height));
        group_y += height;
    }
    emit_collapsed_table_background(
        session,
        input.table_box_idx,
        Rect::from_origin_size(
            table_content_pos,
            Size::new(input.content_width, grid_height),
        ),
    );
    grid_height
}

fn layout_zero_column_grid(
    session: &mut LayoutEngine<'_, '_>,
    input: &TableLayoutInput,
    row_groups: &[u32],
    table_content_pos: Point,
    caption_height: f64,
    prepared: &PreparedTableGrid,
) -> f64 {
    let spacing = prepared.vertical_spacing;
    let rows = &input.rows;
    let collapsed_borders = matches!(
        session.reader.style(input.table_box_idx).border_collapse(),
        BorderCollapseMode::Collapse
    );
    let row_heights = resolve_row_heights(
        session,
        prepared,
        rows,
        row_groups,
        &[],
        input.explicit_content_height,
        caption_height,
    );

    // Border spacing surrounds an actual row area. An auto-height table whose
    // zero-column rows all remain zero-sized has no row area to surround.
    let used_spacing = if input.explicit_content_height.is_some()
        || row_heights.iter().any(|height| *height > 0.0)
    {
        spacing
    } else {
        0.0
    };
    let mut row_starts = vec![0.0; rows.len()];
    let mut cursor_y = used_spacing;
    for (row, height) in row_heights.iter().copied().enumerate() {
        row_starts[row] = cursor_y;
        cursor_y += height + used_spacing;
    }
    let table_height = cursor_y;

    for (row, &row_box_idx) in rows.iter().enumerate() {
        let row_box_idx = row_box_idx as usize;
        let point = Point::new(table_content_pos.x, table_content_pos.y + row_starts[row]);
        session.geometry.set_point(row_box_idx, point);
        session.geometry.set_size(
            row_box_idx,
            Size::new(input.content_width, row_heights[row]),
        );
        defer_row_out_of_flow(session, row_box_idx, point);
        if !collapsed_borders {
            crate::layout::record_block_border_and_outline(session, row_box_idx);
        }
    }

    for &group_idx in row_groups {
        let Some(range) = row_group_range(&session.reader, rows, group_idx as usize) else {
            continue;
        };
        let y = row_starts[range.start];
        let height = span_extent(&row_starts, &row_heights, range.start, range.len());
        session.geometry.set_point(
            group_idx as usize,
            Point::new(table_content_pos.x, table_content_pos.y + y),
        );
        session
            .geometry
            .set_size(group_idx as usize, Size::new(input.content_width, height));
        if !collapsed_borders {
            crate::layout::record_block_border_and_outline(session, group_idx as usize);
        }
    }

    table_height
}

pub fn layout_table(
    session: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    table: TableBox,
    content_pos: Point,
) -> Size {
    let timing_started = session.start_timing();
    let TableBox {
        rows,
        row_groups,
        captions_top,
        captions_bottom,
        ..
    } = table;
    let structure = TableStructure {
        row_groups,
        captions_top,
        captions_bottom,
    };
    let prepared = prepare_table_grid(session, resolved.table_box_sizing().box_idx, &rows);
    let measures = measure_table_columns(session, &prepared);
    let input = resolve_table_input(session, resolved, rows, content_pos, &prepared, &measures);
    if input.rows.is_empty() || prepared.grid.column_count == 0 {
        return layout_empty_table(session, &input, &structure, &prepared);
    }
    let plan = resolve_table_plan(session, input, structure, prepared, measures);
    let size = publish_table_plan(session, plan);
    session.record_timing(|t| t.layout_table += timing_started.elapsed());
    size
}

fn resolve_table_plan(
    session: &mut LayoutEngine<'_, '_>,
    input: TableLayoutInput,
    structure: TableStructure,
    prepared: PreparedTableGrid,
    measures: Vec<WidthMeasure>,
) -> ResolvedTablePlan {
    let caption_top_height = measure_caption_stack(
        session,
        &structure.captions_top,
        input.content_width,
        input.parent_content_height,
        CaptionMeasurement::MarginStack,
    );
    let table_content_pos = Point::new(
        input.content_pos.x,
        input.content_pos.y + caption_top_height,
    );
    let grid = &prepared.grid;
    let h_spacing = prepared.horizontal_spacing;
    let v_spacing = prepared.vertical_spacing;
    let outer_borders = prepared.outer_borders;
    // The width entering table layout is the complete used table width for
    // both authored and intrinsic sizing. Collapsed outer edges straddle the
    // grid boundary, so reserve their outward-facing halves before resolving
    // the column tracks.
    let target_outer_width = input.content_width
        + if input.inline_basis == TableDimensionBasis::AuthoredContentBox {
            outer_borders.horizontal()
        } else {
            0.0
        };
    let used_grid_outer_width =
        if let Some((intrinsic_min, intrinsic_max)) = input.auto_grid_intrinsic {
            intrinsic_max.min(target_outer_width).max(intrinsic_min)
        } else {
            target_outer_width
        };
    let grid_width = (used_grid_outer_width - outer_borders.horizontal()).max(0.0);
    let mut columns = resolve_table_columns(measures, grid_width, h_spacing, prepared.fixed_mode);
    columns.rtl = session.reader.style(input.table_box_idx).direction() == TextDirection::Rtl;
    for start in &mut columns.starts {
        *start += if columns.rtl {
            outer_borders.right
        } else {
            outer_borders.left
        };
    }
    columns.table_width += outer_borders.horizontal();
    // Measure bottom captions in isolation so they do not emit paint before
    // their final position is known.
    let caption_bottom_height = measure_caption_stack(
        session,
        &structure.captions_bottom,
        input.content_width,
        input.parent_content_height,
        CaptionMeasurement::BorderBoxes,
    );
    let explicit_grid_height = input.explicit_content_height.map(|height| {
        height
            + if input.block_basis == TableDimensionBasis::AuthoredContentBox {
                outer_borders.vertical()
            } else {
                0.0
            }
    });
    let definite_row_area_height = explicit_grid_height.map(|height| {
        (height
            - caption_top_height
            - caption_bottom_height
            - outer_borders.vertical()
            - 2.0 * v_spacing)
            .max(0.0)
    });
    let cells = prepare_cells(
        session,
        &prepared,
        input.rows.len(),
        &columns,
        table_content_pos,
        input.parent_content_height,
        definite_row_area_height,
    );
    let row_heights = resolve_row_heights(
        session,
        &prepared,
        &input.rows,
        &structure.row_groups,
        &cells.cells,
        explicit_grid_height,
        caption_top_height + caption_bottom_height,
    );

    let mut row_starts = vec![0.0f64; input.rows.len()];
    let mut cursor_y = v_spacing + outer_borders.top;
    for i in 0..input.rows.len() {
        row_starts[i] = cursor_y;
        cursor_y += row_heights[i] + v_spacing;
    }
    let table_height = cursor_y + outer_borders.bottom;

    debug_assert_eq!(columns.starts.len(), columns.widths.len());
    debug_assert_eq!(input.rows.len(), row_starts.len());
    debug_assert_eq!(input.rows.len(), row_heights.len());
    debug_assert_eq!(input.rows.len(), cells.row_baselines.len());
    debug_assert_eq!(grid.placements.len(), cells.cells.len());

    ResolvedTablePlan {
        input,
        structure,
        prepared,
        columns,
        cells,
        row_heights,
        row_starts,
        caption_top_height,
        caption_bottom_height,
        table_content_pos,
        table_height,
    }
}
