use crate::layout::{
    LayoutEngine, ResolvedBoxModel, ResolvedBoxSizing, ResolvedTableBoxSizing, UsedBorderInsets,
    constrain_content_width, contains_full_width_percentage_table, resolve_vertical_size,
};
use crate::layout_model::{LayoutMode, TableColumnGroupSpan, TableColumnTrack};
use html_style_model::{BorderCollapseMode, BoxSizing, UsedPreferredSize as PreferredSize};
use kurbo::{Point, Rect, Size, Vec2};

use super::backgrounds::{emit_collapsed_table_background, emit_table_layers};
use super::borders::CollapsedBorderGrid;
use super::captions::layout_caption_stack;
use super::columns::{self, TableColumnLayout, TableGrid, span_extent};
use super::grid::{effective_table_grid, table_grid};
use super::row_sizing::resolve_row_heights;
use super::rows::{
    PreparedCellLayout, PreparedTableCells, cell_has_visible_content, cell_vertical_offset,
    prepare_cells,
};
use super::sizing::resolve_table_columns;

#[derive(Clone, Copy, Eq, PartialEq)]
enum TableDimensionBasis {
    UsedBorderBox,
    AuthoredContentBox,
}

#[derive(Clone, Copy)]
enum CaptionMeasurement {
    BorderBoxes,
    MarginStack,
}

struct TableLayoutInput {
    table_box_idx: usize,
    rows: Vec<u32>,
    content_pos: Point,
    content_width: f64,
    parent_content_height: Option<f64>,
    explicit_content_height: Option<f64>,
    auto_grid_intrinsic: Option<(f64, f64)>,
    inline_basis: TableDimensionBasis,
    block_basis: TableDimensionBasis,
}

struct TableStructure {
    row_groups: Vec<u32>,
    captions_top: Vec<u32>,
    captions_bottom: Vec<u32>,
    columns: Vec<TableColumnTrack>,
    column_groups: Vec<TableColumnGroupSpan>,
    column_width_hints: Vec<crate::layout_model::TableColumnWidthHint>,
}

/// Complete resolved decision for a non-empty table grid.
///
/// Preparation retains reusable cell output in provisional local coordinates
/// while block-basis-dependent cells remain isolated probes. Publication
/// consumes this value in one direction, applies final row transforms, and
/// never recomputes ordinary cell layout.
struct ResolvedTablePlan {
    input: TableLayoutInput,
    structure: TableStructure,
    grid: TableGrid,
    effective_columns: Vec<TableColumnTrack>,
    effective_column_groups: Vec<TableColumnGroupSpan>,
    collapsed_borders: bool,
    collapsed_grid: Option<CollapsedBorderGrid>,
    outer_borders: UsedBorderInsets,
    horizontal_spacing: f64,
    vertical_spacing: f64,
    columns: TableColumnLayout,
    cells: PreparedTableCells,
    row_heights: Vec<f64>,
    row_starts: Vec<f64>,
    caption_top_height: f64,
    caption_bottom_height: f64,
    table_content_pos: Point,
    table_height: f64,
}

fn table_structure(session: &LayoutEngine<'_, '_>, table_box_idx: usize) -> TableStructure {
    match session.reader.box_layout_mode(table_box_idx) {
        Some(LayoutMode::Table(table)) => TableStructure {
            row_groups: table.row_groups.clone(),
            captions_top: table.captions_top.clone(),
            captions_bottom: table.captions_bottom.clone(),
            columns: table.columns.clone(),
            column_groups: table.column_groups.clone(),
            column_width_hints: table.column_width_hints.clone(),
        },
        _ => TableStructure {
            row_groups: Vec::new(),
            captions_top: Vec::new(),
            captions_bottom: Vec::new(),
            columns: Vec::new(),
            column_groups: Vec::new(),
            column_width_hints: Vec::new(),
        },
    }
}

fn resolve_table_input(
    session: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    rows: Vec<u32>,
    content_pos: Point,
) -> TableLayoutInput {
    let sizing = resolved.table_box_sizing();
    let uses_auto_intrinsic_width =
        !sizing.has_assigned_width && matches!(sizing.preferred_width, PreferredSize::Auto);
    let mut auto_grid_intrinsic = None;
    let content_width = if uses_auto_intrinsic_width {
        let intrinsic = crate::table::table_intrinsic_sizes(session, sizing.box_idx);
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
            auto_grid_intrinsic = Some((intrinsic.grid_minimum, intrinsic.grid_maximum));
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
        .map(|height| {
            let height = sizing
                .max_height
                .map_or(height, |maximum| height.min(maximum));
            sizing
                .min_height
                .map_or(height, |minimum| height.max(minimum))
        })
        .or(sizing.min_height)
}

fn measure_caption_stack(
    session: &mut LayoutEngine<'_, '_>,
    captions: &[u32],
    content_width: f64,
    parent_content_height: Option<f64>,
    measurement: CaptionMeasurement,
) -> f64 {
    captions
        .iter()
        .map(|&caption_idx| {
            let size = crate::layout::measure_box_isolated(
                session,
                caption_idx as usize,
                content_width,
                parent_content_height,
            );
            let margins = match measurement {
                CaptionMeasurement::BorderBoxes => 0.0,
                CaptionMeasurement::MarginStack => {
                    let box_model = ResolvedBoxModel::new(
                        session.reader.style(caption_idx as usize),
                        content_width,
                    );
                    box_model.margin_top + box_model.margin_bottom
                }
            };
            size.height + margins
        })
        .sum()
}

fn layout_empty_grid(
    session: &mut LayoutEngine<'_, '_>,
    table_box_idx: usize,
    row_groups: &[u32],
    captions_top: &[u32],
    captions_bottom: &[u32],
    content_pos: Point,
    content_width: f64,
    parent_content_height: Option<f64>,
    explicit_content_height: Option<f64>,
) -> Size {
    let caption_top_height = layout_caption_stack(
        session,
        captions_top,
        content_pos,
        content_width,
        parent_content_height,
    );
    let measured_bottom_height = captions_bottom
        .iter()
        .map(|&caption_idx| {
            crate::layout::measure_box_isolated(
                session,
                caption_idx as usize,
                content_width,
                parent_content_height,
            )
            .height
        })
        .sum::<f64>();
    let mut group_heights = row_groups
        .iter()
        .map(|&group_idx| {
            resolve_vertical_size(
                session.reader.style(group_idx as usize).height(),
                explicit_content_height,
                0.0,
            )
            .unwrap_or(0.0)
        })
        .collect::<Vec<_>>();
    let natural_grid_height = group_heights.iter().sum::<f64>();
    let grid_height = explicit_content_height.map_or(natural_grid_height, |height| {
        (height - caption_top_height - measured_bottom_height).max(natural_grid_height)
    });
    if !group_heights.is_empty() && grid_height > natural_grid_height {
        let extra = (grid_height - natural_grid_height) / group_heights.len() as f64;
        for height in &mut group_heights {
            *height += extra;
        }
    }
    let mut group_y = caption_top_height;
    for (&group_idx, height) in row_groups.iter().zip(group_heights) {
        session.geometry.set_point(
            group_idx as usize,
            Point::new(content_pos.x, content_pos.y + group_y),
        );
        session
            .geometry
            .set_size(group_idx as usize, Size::new(content_width, height));
        group_y += height;
    }
    emit_collapsed_table_background(
        session,
        table_box_idx,
        Rect::new(
            content_pos.x,
            content_pos.y + caption_top_height,
            content_pos.x + content_width,
            content_pos.y + caption_top_height + grid_height,
        ),
    );
    let caption_bottom_height = layout_caption_stack(
        session,
        captions_bottom,
        Point::new(
            content_pos.x,
            content_pos.y + caption_top_height + grid_height,
        ),
        content_width,
        parent_content_height,
    );
    let size = Size::new(
        content_width,
        caption_top_height + grid_height + caption_bottom_height,
    );
    session.geometry.set_size(table_box_idx, size);
    size
}

fn layout_zero_column_grid(
    session: &mut LayoutEngine<'_, '_>,
    table_box_idx: usize,
    rows: &[u32],
    row_groups: &[u32],
    captions_top: &[u32],
    captions_bottom: &[u32],
    content_pos: Point,
    content_width: f64,
    parent_content_height: Option<f64>,
    explicit_content_height: Option<f64>,
) -> Size {
    let caption_top_height = layout_caption_stack(
        session,
        captions_top,
        content_pos,
        content_width,
        parent_content_height,
    );
    let table_content_pos = Point::new(content_pos.x, content_pos.y + caption_top_height);
    let style = session.reader.style(table_box_idx);
    let spacing = if matches!(
        style.border_collapse(),
        html_style_model::BorderCollapseMode::Collapse
    ) {
        0.0
    } else {
        style.border_spacing_vertical().max(0.0) as f64
    };
    let measured_bottom_height = captions_bottom
        .iter()
        .map(|&caption_idx| {
            crate::layout::measure_box_isolated(
                session,
                caption_idx as usize,
                content_width,
                parent_content_height,
            )
            .height
        })
        .sum::<f64>();
    let row_heights = resolve_row_heights(
        session,
        rows,
        row_groups,
        &[],
        &[],
        spacing,
        explicit_content_height,
        caption_top_height + measured_bottom_height,
        0.0,
    );

    // Border spacing surrounds an actual row area. An auto-height table whose
    // zero-column rows all remain zero-sized has no row area to surround.
    let used_spacing =
        if explicit_content_height.is_some() || row_heights.iter().any(|height| *height > 0.0) {
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
        session
            .geometry
            .set_size(row_box_idx, Size::new(content_width, row_heights[row]));
        defer_row_out_of_flow(session, row_box_idx, point);
        if !matches!(
            style.border_collapse(),
            html_style_model::BorderCollapseMode::Collapse
        ) {
            crate::layout::emit_block_border_and_outline(session, row_box_idx);
        }
    }

    for &group_idx in row_groups {
        let group_rows = rows
            .iter()
            .enumerate()
            .filter_map(|(row, &row_idx)| {
                (session.reader.get_parent(row_idx as usize) == Some(group_idx as usize))
                    .then_some(row)
            })
            .collect::<Vec<_>>();
        let (Some(&first), Some(&last)) = (group_rows.first(), group_rows.last()) else {
            continue;
        };
        let y = row_starts[first];
        let height = row_starts[last] + row_heights[last] - y;
        session.geometry.set_point(
            group_idx as usize,
            Point::new(table_content_pos.x, table_content_pos.y + y),
        );
        session
            .geometry
            .set_size(group_idx as usize, Size::new(content_width, height));
        if !matches!(
            style.border_collapse(),
            html_style_model::BorderCollapseMode::Collapse
        ) {
            crate::layout::emit_block_border_and_outline(session, group_idx as usize);
        }
    }

    let caption_bottom_height = layout_caption_stack(
        session,
        captions_bottom,
        Point::new(content_pos.x, table_content_pos.y + table_height),
        content_width,
        parent_content_height,
    );
    let size = Size::new(
        content_width,
        caption_top_height + table_height + caption_bottom_height,
    );
    session.geometry.set_size(table_box_idx, size);
    size
}

pub(crate) fn layout_table(
    session: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    rows: Vec<u32>,
    content_pos: Point,
) -> Size {
    let timing_started = session.start_timing();
    let input = resolve_table_input(session, resolved, rows, content_pos);
    let structure = table_structure(session, input.table_box_idx);
    if input.rows.is_empty() {
        return layout_empty_grid(
            session,
            input.table_box_idx,
            &structure.row_groups,
            &structure.captions_top,
            &structure.captions_bottom,
            input.content_pos,
            input.content_width,
            input.parent_content_height,
            input.explicit_content_height,
        );
    }

    let raw_grid = table_grid(session, &input.rows);
    let table_style = session.reader.style(input.table_box_idx);
    let fixed_mode = matches!(
        table_style.table_layout(),
        html_style_model::TableLayoutMode::Fixed
    ) && !matches!(
        table_style.width(),
        html_style_model::UsedPreferredSize::Auto
    );
    let (grid, track_sources, logical_to_effective) =
        effective_table_grid(session, raw_grid, &structure.column_width_hints, fixed_mode);
    if grid.column_count == 0 {
        return layout_zero_column_grid(
            session,
            input.table_box_idx,
            &input.rows,
            &structure.row_groups,
            &structure.captions_top,
            &structure.captions_bottom,
            input.content_pos,
            input.content_width,
            input.parent_content_height,
            input.explicit_content_height,
        );
    }
    let plan = resolve_table_plan(
        session,
        input,
        structure,
        grid,
        track_sources,
        logical_to_effective,
        fixed_mode,
    );
    let size = publish_table_plan(session, plan);
    session.record_timing(|t| t.layout_table += timing_started.elapsed());
    size
}

fn resolve_table_plan(
    session: &mut LayoutEngine<'_, '_>,
    input: TableLayoutInput,
    structure: TableStructure,
    grid: TableGrid,
    track_sources: Vec<usize>,
    logical_to_effective: Vec<usize>,
    fixed_mode: bool,
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
    let table_style = session.reader.style(input.table_box_idx);

    let collapsed_borders = matches!(
        table_style.border_collapse(),
        html_style_model::BorderCollapseMode::Collapse
    );
    let h_spacing = if collapsed_borders {
        0.0
    } else {
        table_style.border_spacing_horizontal().max(0.0) as f64
    };
    let v_spacing = if collapsed_borders {
        0.0
    } else {
        table_style.border_spacing_vertical().max(0.0) as f64
    };
    let effective_hints = track_sources
        .iter()
        .filter_map(|&source| structure.column_width_hints.get(source).copied())
        .collect::<Vec<_>>();
    let effective_columns = track_sources
        .iter()
        .filter_map(|&source| structure.columns.get(source).cloned())
        .collect::<Vec<_>>();
    let effective_column_groups = structure
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
            Some(crate::layout_model::TableColumnGroupSpan {
                style: group.style,
                start,
                span: end - start,
            })
        })
        .collect::<Vec<_>>();
    let collapsed_grid = collapsed_borders.then(|| {
        CollapsedBorderGrid::build(
            &session.reader,
            input.table_box_idx,
            &input.rows,
            &structure.row_groups,
            &grid.placements,
            &effective_columns,
            &effective_column_groups,
        )
    });
    let outer_borders = collapsed_grid.as_ref().map_or_else(
        crate::layout::UsedBorderInsets::default,
        CollapsedBorderGrid::outer_insets,
    );
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
    let mut columns = resolve_table_columns(
        session,
        &grid.placements,
        grid.column_count,
        &effective_hints,
        grid_width,
        h_spacing,
        collapsed_grid.as_ref(),
        fixed_mode,
    );
    for start in &mut columns.starts {
        *start += outer_borders.left;
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
        &grid.placements,
        input.rows.len(),
        &columns,
        table_content_pos,
        v_spacing,
        input.parent_content_height,
        collapsed_grid.as_ref(),
        definite_row_area_height,
    );
    let row_heights = resolve_row_heights(
        session,
        &input.rows,
        &structure.row_groups,
        &grid.placements,
        &cells.cell_heights,
        v_spacing,
        explicit_grid_height,
        caption_top_height + caption_bottom_height,
        outer_borders.vertical(),
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
    debug_assert_eq!(grid.placements.len(), cells.cell_heights.len());
    debug_assert_eq!(grid.placements.len(), cells.baseline_offsets.len());
    debug_assert_eq!(grid.placements.len(), cells.layouts.len());

    ResolvedTablePlan {
        input,
        structure,
        grid,
        effective_columns,
        effective_column_groups,
        collapsed_borders,
        collapsed_grid,
        outer_borders,
        horizontal_spacing: h_spacing,
        vertical_spacing: v_spacing,
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

fn publish_table_plan(session: &mut LayoutEngine<'_, '_>, plan: ResolvedTablePlan) -> Size {
    let input = &plan.input;
    let structure = &plan.structure;
    let grid = &plan.grid;
    let columns = &plan.columns;
    let row_heights = &plan.row_heights;
    let row_starts = &plan.row_starts;
    let table_content_pos = plan.table_content_pos;
    let table_height = plan.table_height;
    let h_spacing = plan.horizontal_spacing;
    let v_spacing = plan.vertical_spacing;
    let outer_borders = plan.outer_borders;
    let collapsed_borders = plan.collapsed_borders;
    let collapsed_grid = plan.collapsed_grid.as_ref();
    let cells = &plan.cells;
    let table_box_idx = input.table_box_idx;
    let rows = &input.rows;
    let content_pos = input.content_pos;
    let content_width = input.content_width;
    let parent_content_height = input.parent_content_height;
    let caption_top_height = layout_caption_stack(
        session,
        &structure.captions_top,
        content_pos,
        content_width,
        parent_content_height,
    );
    debug_assert!((caption_top_height - plan.caption_top_height).abs() < 0.01);

    // Captions participate in the anonymous table wrapper, not in the table
    // grid's border box. Keep the wrapper size for normal-flow placement, but
    // paint the table's own background and borders on the authoritative grid
    // rectangle rather than across a wider/taller caption stack.
    if !structure.captions_top.is_empty() || !structure.captions_bottom.is_empty() {
        let box_point = session.geometry.point(table_box_idx);
        session.geometry.set_decoration_rect(
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
    session.geometry.set_size(
        table_box_idx,
        Size::new(
            used_table_width,
            caption_top_height + table_height + plan.caption_bottom_height,
        ),
    );

    for (row_idx, &row_box_idx) in rows.iter().enumerate() {
        let row_box_idx = row_box_idx as usize;
        session.geometry.set_point(
            row_box_idx,
            table_content_pos + Vec2::new(h_spacing + outer_borders.left, row_starts[row_idx]),
        );
        session.geometry.set_size(
            row_box_idx,
            Size::new(
                columns.table_width - 2.0 * h_spacing - outer_borders.horizontal(),
                row_heights[row_idx],
            ),
        );
        defer_row_out_of_flow(session, row_box_idx, session.geometry.point(row_box_idx));
    }

    emit_table_layers(
        session,
        table_box_idx,
        table_content_pos,
        table_height,
        h_spacing,
        v_spacing,
        columns,
        &plan.effective_columns,
        &plan.effective_column_groups,
    );

    for &group_idx in &structure.row_groups {
        let group_rows: Vec<usize> = rows
            .iter()
            .copied()
            .map(|row| row as usize)
            .filter(|&row| session.reader.get_parent(row) == Some(group_idx as usize))
            .collect();
        let (Some(first), Some(last)) = (group_rows.first().copied(), group_rows.last().copied())
        else {
            continue;
        };
        let first_row = rows
            .iter()
            .position(|&row| row as usize == first)
            .expect("group row belongs to table");
        let last_row = rows
            .iter()
            .position(|&row| row as usize == last)
            .expect("group row belongs to table");
        let y = row_starts[first_row];
        let height = row_starts[last_row] + row_heights[last_row] - y;
        session.geometry.set_point(
            group_idx as usize,
            Point::new(table_content_pos.x + h_spacing, table_content_pos.y + y),
        );
        session.geometry.set_size(
            group_idx as usize,
            Size::new(
                (columns.table_width - 2.0 * h_spacing).max(0.0),
                height.max(0.0),
            ),
        );
        emit_table_row_layer_background(
            session,
            group_idx as usize,
            grid.placements
                .iter()
                .filter(|placement| (first_row..=last_row).contains(&placement.row)),
            table_content_pos,
            columns,
            row_starts,
            row_heights,
        );
        if !collapsed_borders {
            crate::layout::emit_block_border_and_outline(session, group_idx as usize);
        }
    }

    for (row_idx, &row_box_idx) in rows.iter().enumerate() {
        emit_table_row_layer_background(
            session,
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
            crate::layout::emit_block_border_and_outline(session, row_box_idx as usize);
        }
    }

    for (i, placement) in grid.placements.iter().enumerate() {
        let span_width = span_extent(
            &columns.starts,
            &columns.widths,
            placement.col,
            placement.colspan,
        );
        let span_height = span_extent(&row_starts, &row_heights, placement.row, placement.rowspan);

        let final_point = Point::new(
            table_content_pos.x + columns.starts[placement.col],
            table_content_pos.y + row_starts[placement.row],
        );
        let (cell_output, natural_height, cell_has_content) = match &cells.layouts[i] {
            PreparedCellLayout::Retained(retained) => {
                crate::layout::translate_laid_out_output(
                    session,
                    &retained.output,
                    final_point - retained.provisional_point,
                );
                (
                    retained.output.clone(),
                    retained.natural_border_height,
                    retained.has_visible_content,
                )
            }
            PreparedCellLayout::Relayout {
                assign_final_height,
            } => {
                session.geometry.set_point(placement.cell_idx, final_point);
                let cell_layout = session.without_fragmentation(|session| {
                    let used_borders = collapsed_grid.map(|grid| grid.cell_insets(*placement));
                    let mut request = crate::layout::BoxLayoutRequest::table_cell(
                        placement.cell_idx,
                        span_width,
                        parent_content_height,
                        used_borders,
                    );
                    if *assign_final_height {
                        request = request.with_assigned_border_height(span_height);
                    }
                    session.layout_box(request)
                });
                let cell_style = session.reader.style(placement.cell_idx);
                let cell_box_model = collapsed_grid.map_or_else(
                    || ResolvedBoxModel::new(cell_style, span_width),
                    |grid| {
                        ResolvedBoxModel::new(cell_style, span_width)
                            .with_used_borders(grid.cell_insets(*placement))
                    },
                );
                let natural_height = session.natural_content_height(placement.cell_idx)
                    + cell_box_model.vertical_padding_border();
                let cell_has_content =
                    cell_has_visible_content(session, placement.cell_idx, &cell_layout.output);
                (cell_layout.output, natural_height, cell_has_content)
            }
        };
        // Cell `height` is a minimum for the cell box, not part of the natural
        // content height used by `vertical-align`. Otherwise an explicitly
        // tall cell has no remaining alignment space and bottom-aligned text
        // incorrectly stays at the top.
        let cell_style = session.reader.style(placement.cell_idx);
        let vertical_offset = cell_vertical_offset(
            session,
            placement.cell_idx,
            natural_height,
            span_height,
            cells.row_baselines[placement.row],
            cells.baseline_offsets[i],
        );
        if vertical_offset.abs() > 0.01 {
            crate::layout::translate_laid_out_content(
                session,
                &cell_output,
                Vec2::new(0.0, vertical_offset),
            );
        }
        let mut size = session.geometry.size(placement.cell_idx);
        size.width = span_width;
        // A table cell occupies its grid area even when final percentage
        // resolution makes its contents overflow that area.
        size.height = span_height;
        session.geometry.set_size(placement.cell_idx, size);
        let hide_empty_borders = matches!(
            cell_style.empty_cells(),
            html_style_model::EmptyCellsMode::Hide
        ) && !cell_has_content;
        if !hide_empty_borders {
            session.select_placement_group(cell_output.placement.box_group);
            if collapsed_borders {
                crate::layout::emit_block_background(session, placement.cell_idx);
            } else {
                crate::layout::emit_block_decorations(session, placement.cell_idx);
            }
            session.select_placement_group(cell_output.placement.previous_group());
        }
    }

    if let Some(grid) = collapsed_grid {
        grid.emit(
            session,
            table_box_idx,
            table_content_pos,
            &columns.starts,
            &columns.widths,
            row_starts,
            row_heights,
        );
    }

    let caption_bottom_height = layout_caption_stack(
        session,
        &structure.captions_bottom,
        Point::new(content_pos.x, table_content_pos.y + table_height),
        content_width,
        parent_content_height,
    );
    let size = Size::new(
        used_table_width,
        caption_top_height + table_height + caption_bottom_height,
    );
    size
}

fn emit_table_row_layer_background<'a>(
    session: &mut LayoutEngine<'_, '_>,
    owner_box_idx: usize,
    placements: impl Iterator<Item = &'a columns::TableCellPlacement>,
    table_content_pos: Point,
    columns: &columns::TableColumnLayout,
    row_starts: &[f64],
    row_heights: &[f64],
) {
    let color = session.reader.style(owner_box_idx).background_color();
    if color & 0xFF == 0 {
        return;
    }
    for placement in placements {
        let width = span_extent(
            &columns.starts,
            &columns.widths,
            placement.col,
            placement.colspan,
        );
        let height = span_extent(row_starts, row_heights, placement.row, placement.rowspan);
        let Some((&x, &y)) = columns
            .starts
            .get(placement.col)
            .zip(row_starts.get(placement.row))
        else {
            continue;
        };
        crate::layout::emit_color_rect_for_owner(
            session,
            owner_box_idx,
            Rect::new(
                table_content_pos.x + x,
                table_content_pos.y + y,
                table_content_pos.x + x + width,
                table_content_pos.y + y + height,
            ),
            color,
        );
    }
}

fn defer_row_out_of_flow(
    session: &mut LayoutEngine<'_, '_>,
    row_box_idx: usize,
    static_position: Point,
) {
    let children = match session.reader.box_layout_mode(row_box_idx) {
        Some(LayoutMode::TableRow(row)) => row.out_of_flow.clone(),
        _ => Vec::new(),
    };
    for child in children {
        session.defer_absolute_box(child as usize, static_position);
    }
}
