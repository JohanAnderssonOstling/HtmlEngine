use crate::measurement::contains_full_width_percentage_table;
use crate::{ContentLayoutResult, LayoutEngine, ResolvedBoxSizing, constrain_size};
use html_engine_model::TableBox;
use html_style_model::{BorderCollapseMode, BoxSizing, TextDirection};
use kurbo::{Point, Size, Vec2};

use super::captions::{layout_caption_stack, measure_caption_stack};
use super::geometry::{TableGeometry, TableRowLayout};
use super::s01_grid::{PreparedTableGrid, prepare_table_grid};
use super::s02_intrinsic::intrinsic_sizes_for_grid;
use super::s03_column_sizing::{WidthMeasure, measure_table_columns, resolve_table_columns};
use super::s04_rows::prepare_cells;
use super::s05_row_sizing::resolve_row_heights;
use super::s06_placement::publish_table_plan;
use super::{ResolvedTablePlan, TableLayoutInput};

pub fn layout_table(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    table: &TableBox,
    content_pos: Point,
) -> ContentLayoutResult {
    let prepared = prepare_table_grid(engine, resolved.request.box_idx, table);
    let size_is_final = prepared.grid.placements.is_empty();
    let measures = measure_table_columns(engine, &prepared);
    let input = resolve_table_input(engine, resolved, content_pos, &prepared, &measures);
    let plan = resolve_table_plan(engine, input, table, prepared, measures);
    ContentLayoutResult {
        size: publish_table_plan(engine, plan),
        size_is_final,
        ..ContentLayoutResult::default()
    }
}

fn resolve_table_plan<'tree>(
    engine: &mut LayoutEngine<'_, '_>,
    input: TableLayoutInput,
    table: &'tree TableBox,
    prepared: PreparedTableGrid<'tree>,
    measures: Vec<WidthMeasure>,
) -> ResolvedTablePlan<'tree> {
    let used_grid_outer_width =
        if let Some((intrinsic_min, intrinsic_max)) = input.auto_grid_intrinsic {
            intrinsic_max.min(input.grid_outer_width).max(intrinsic_min)
        } else {
            input.grid_outer_width
        };
    let fit_to_width = engine.read.constraints.book_optimized_text();
    let used_grid_outer_width = if fit_to_width {
        used_grid_outer_width.min(input.grid_outer_width)
    } else {
        used_grid_outer_width
    };
    let grid_width = (used_grid_outer_width - prepared.outer_borders.horizontal()).max(0.0);
    let columns = resolve_table_columns(
        measures,
        grid_width,
        &prepared,
        fit_to_width,
        engine.read.reader.style(input.table_box_idx).direction() == TextDirection::Rtl,
    );
    // Captions belong to the wrapper outside the table's padding and borders.
    // Resolve the grid width first, then emit each top caption once at its
    // final width so wrapping and page gaps determine the grid's start.
    let caption_width =
        columns.table_width.max(input.content_width) + input.grid_insets.horizontal();
    let caption_origin =
        input.content_pos - Vec2::new(input.grid_insets.left, input.grid_insets.top);
    let caption_top_height = layout_caption_stack(
        engine,
        &table.captions_top,
        caption_origin,
        caption_width,
        input.parent_content_height,
    );
    let table_content_pos = Point::new(
        input.content_pos.x,
        input.content_pos.y + caption_top_height,
    );
    // Measure bottom captions in isolation so they do not emit paint before
    // their final position is known.
    let caption_bottom_height = measure_caption_stack(
        engine,
        &table.captions_bottom,
        caption_width,
        input.parent_content_height,
    );
    let definite_row_area_height = input.explicit_content_height.map(|height| {
        (height
            - caption_top_height
            - caption_bottom_height
            - prepared.outer_borders.vertical()
            - 2.0 * prepared.vertical_spacing)
            .max(0.0)
    });
    let cells = prepare_cells(
        engine,
        &prepared,
        &columns,
        table_content_pos,
        input.parent_content_height,
        definite_row_area_height,
    );
    let row_heights = resolve_row_heights(
        engine,
        &prepared,
        &cells.cells,
        input.explicit_content_height,
        definite_row_area_height,
    );

    let mut row_starts = vec![0.0f64; prepared.rows.len()];
    let mut cursor_y = prepared.vertical_spacing + prepared.outer_borders.top;
    // Row geometry is continuous. Page breaks belong to
    // page composition and must never change the table's source coordinates.
    for (start, &height) in row_starts.iter_mut().zip(&row_heights) {
        *start = cursor_y;
        cursor_y += height + prepared.vertical_spacing;
    }
    let table_height = cursor_y + prepared.outer_borders.bottom;

    debug_assert_eq!(columns.starts.len(), columns.widths.len());
    debug_assert_eq!(prepared.rows.len(), row_starts.len());
    debug_assert_eq!(prepared.rows.len(), row_heights.len());
    debug_assert_eq!(prepared.rows.len(), cells.row_baselines.len());
    debug_assert_eq!(prepared.grid.placements.len(), cells.cells.len());

    ResolvedTablePlan {
        input,
        table,
        prepared,
        geometry: TableGeometry {
            origin: table_content_pos,
            columns,
            rows: TableRowLayout {
                starts: row_starts,
                heights: row_heights,
            },
            height: table_height,
        },
        cells,
        caption_top_height,
        caption_bottom_height,
    }
}

fn resolve_table_input(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    content_pos: Point,
    prepared: &PreparedTableGrid,
    measures: &[WidthMeasure],
) -> TableLayoutInput {
    let has_assigned_width = resolved
        .request
        .assigned_border_size
        .and_then(|size| size.width)
        .is_some();
    let has_assigned_height = resolved
        .request
        .assigned_border_size
        .and_then(|size| size.height)
        .is_some();
    let available_width = resolved
        .request
        .auto_width_limit
        .unwrap_or(resolved.request.available_width)
        - resolved.horizontal.margin_padding
        - resolved.box_model.horizontal_border();
    let uses_auto_intrinsic_width = !has_assigned_width && resolved.horizontal.auto_width;
    let mut auto_grid_intrinsic = None;
    let content_width = if uses_auto_intrinsic_width {
        let intrinsic =
            intrinsic_sizes_for_grid(engine, resolved.request.box_idx, prepared, measures);
        let intrinsic_min = intrinsic.minimum;
        let mut intrinsic_max = intrinsic.maximum;
        let requires_available_width = intrinsic.full_width_percentage_grid
            || contains_full_width_percentage_table(engine, resolved.request.box_idx);
        let mut auto_table_available = resolved.horizontal.content;
        if requires_available_width {
            auto_table_available = available_width.max(0.0);
            intrinsic_max = intrinsic_max.max(auto_table_available);
        } else {
            auto_grid_intrinsic = Some((intrinsic.minimum, intrinsic.grid_maximum));
        }
        intrinsic_max
            .min(auto_table_available.max(intrinsic_min))
            .clamp(resolved.horizontal.min, resolved.horizontal.max)
            .max(0.0)
    } else {
        resolved.horizontal.content
    };
    let explicit_content_height = resolved_table_height(resolved, has_assigned_height);
    let collapsed_content_box = matches!(
        engine
            .read
            .reader
            .style(resolved.request.box_idx)
            .border_collapse(),
        BorderCollapseMode::Collapse
    ) && matches!(resolved.box_sizing, BoxSizing::ContentBox);
    let grid_border =
        if collapsed_content_box && !has_assigned_width && !resolved.horizontal.auto_width {
            prepared.outer_borders.horizontal()
        } else {
            0.0
        };
    let explicit_content_height = explicit_content_height.map(|height| {
        height
            + if collapsed_content_box
                && !has_assigned_height
                && resolved.vertical.explicit.is_some()
            {
                prepared.outer_borders.vertical()
            } else {
                0.0
            }
    });
    // Reading columns are a hard inline limit. Apply it before captions and
    // cells are laid out, including intrinsic and authored minimum widths.
    let content_width = if engine.read.constraints.book_optimized_text() {
        let available = available_width - grid_border;
        content_width.min(available.max(0.0))
    } else {
        content_width
    };
    TableLayoutInput {
        table_box_idx: resolved.request.box_idx,
        grid_insets: resolved.box_model.padding_border_insets(),
        content_pos,
        content_width,
        parent_content_height: resolved.request.parent_content_height,
        explicit_content_height,
        auto_grid_intrinsic,
        grid_outer_width: content_width + grid_border,
    }
}

fn resolved_table_height(resolved: &ResolvedBoxSizing, has_assigned_height: bool) -> Option<f64> {
    if has_assigned_height {
        return resolved.vertical.explicit;
    }
    resolved
        .vertical
        .explicit
        .map(|height| {
            constrain_size(
                height,
                resolved.vertical.min.resolve(None),
                resolved.vertical.max.resolve(None),
            )
        })
        .or(resolved.vertical.min.resolve(None))
}
