//! Stage 00 coordinates grid preparation, intrinsic/column sizing, cells, rows, and placement.
//! Intrinsic queries and cell probes can revisit earlier stages; numbering is a reading order.
//! Shared stage contracts and helpers support each stage.
//!
//! Keep numbered stages overview-first: imports and data contracts, then one or
//! two stage entry points, followed by private implementation helpers. Put helpers
//! shared between stages in the supporting modules instead of expanding stage APIs.

use crate::boxes::absolute_positioning;
#[path = "00-layout.rs"]
mod s00_layout;
#[path = "01-grid.rs"]
mod s01_grid;
#[path = "02-intrinsic.rs"]
mod s02_intrinsic;
#[path = "03-column_sizing.rs"]
mod s03_column_sizing;
#[path = "04-rows.rs"]
mod s04_rows;
#[path = "05-row_sizing.rs"]
mod s05_row_sizing;
#[path = "06-placement.rs"]
mod s06_placement;

mod backgrounds;
mod borders;
mod captions;
mod cell;
mod columns;
mod geometry;
mod grid;

pub use grid::{TableCellPlacement, place_table_cells};
pub(crate) use s04_rows::CellHeightDependency;

use crate::boxes::constraints::resolve_margin_left_for_used_width;
use crate::boxes::translate_laid_out_output;
use crate::{BoxLayoutResult, LayoutEngine, ResolvedBoxSizing, UsedBorderInsets};
use geometry::TableGeometry;
use html_engine_model::{LayoutMode, TableBox};
use kurbo::{Point, Vec2};
use s01_grid::PreparedTableGrid;
use s04_rows::PreparedTableCells;

struct TableLayoutInput {
    table_box_idx: usize,
    grid_insets: UsedBorderInsets,
    content_pos: Point,
    content_width: f64,
    parent_content_height: Option<f64>,
    explicit_content_height: Option<f64>,
    auto_grid_intrinsic: Option<(f64, f64)>,
    grid_outer_width: f64,
}

/// Complete resolved decision for a table, including caption-only tables.
/// Cell-free tables carry no row or column tracks into publication.
///
/// Preparation retains reusable cell output in provisional local coordinates
/// while block-basis-dependent cells remain isolated probes. Publication
/// consumes this value in one direction, applies final row transforms, and
/// never recomputes ordinary cell layout.
struct ResolvedTablePlan<'tree> {
    input: TableLayoutInput,
    table: &'tree TableBox,
    prepared: PreparedTableGrid<'tree>,
    geometry: TableGeometry,
    cells: PreparedTableCells,
    caption_top_height: f64,
    caption_bottom_height: f64,
}

pub use s00_layout::layout_table;
pub use s02_intrinsic::table_intrinsic_widths;
pub use s04_rows::layout_table_row;

fn defer_row_out_of_flow(
    engine: &mut LayoutEngine<'_, '_>,
    row_box_idx: usize,
    static_position: Point,
) {
    let reader = engine.read.reader;
    let Some(LayoutMode::TableRow(row)) = reader.box_layout_mode(row_box_idx) else {
        return;
    };
    for &child in &row.out_of_flow {
        absolute_positioning::defer(engine, child as usize, static_position);
    }
}

#[cfg(test)]
mod tests;

fn weighted_share(weight: f64, total: f64, count: usize) -> f64 {
    if total > 0.0 {
        weight / total
    } else {
        1.0 / count as f64
    }
}

pub(crate) fn align_in_block_flow(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    layout: &BoxLayoutResult,
) {
    let child_style = engine.read.reader.style(resolved.box_idx());
    // A table's intrinsic width is finalized by its formatting context,
    // after generic block sizing and float fitting have placed it.
    let available_width = resolved
        .request
        .auto_width_limit
        .unwrap_or(resolved.request.available_width);
    let used_left = resolve_margin_left_for_used_width(
        child_style,
        engine
            .read
            .reader
            .parent_direction(resolved.box_idx())
            .unwrap_or(child_style.direction()),
        resolved.request.available_width,
        available_width,
        layout.size.width,
    );
    let offset = Vec2::new(used_left - resolved.horizontal_margins().left, 0.0);
    translate_laid_out_output(engine, &layout.output, offset);
}

/// End of the consecutive rows sharing the start row's formatting parent.
pub(crate) fn row_group_end(reader: &crate::LayoutReader<'_>, rows: &[u32], start: usize) -> usize {
    let parent = reader.get_parent(rows[start] as usize);
    start
        + rows[start..]
            .iter()
            .take_while(|&&row| reader.get_parent(row as usize) == parent)
            .count()
}
