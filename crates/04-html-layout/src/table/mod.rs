//! Stage 00 coordinates grid preparation, intrinsic/column sizing, cells, rows, and placement.
//! Intrinsic queries and cell probes can revisit earlier stages; numbering is a reading order.
//! Shared stage contracts and helpers support each stage.

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
mod columns;

use columns::TableColumnLayout;
use crate::layout::UsedBorderInsets;
use s01_grid::PreparedTableGrid;
use kurbo::Point;
use s04_rows::PreparedTableCells;

#[derive(Clone, Copy, Eq, PartialEq)]
enum TableDimensionBasis {
    UsedBorderBox,
    AuthoredContentBox,
}

struct TableLayoutInput<'tree> {
    table_box_idx: usize,
    grid_insets: UsedBorderInsets,
    rows: &'tree [u32],
    content_pos: Point,
    content_width: f64,
    parent_content_height: Option<f64>,
    explicit_content_height: Option<f64>,
    auto_grid_intrinsic: Option<(f64, f64)>,
    inline_basis: TableDimensionBasis,
    block_basis: TableDimensionBasis,
}

struct TableStructure<'tree> {
    row_groups: &'tree [u32],
    captions_top: &'tree [u32],
    captions_bottom: &'tree [u32],
}

/// Complete resolved decision for a non-empty table grid.
///
/// Preparation retains reusable cell output in provisional local coordinates
/// while block-basis-dependent cells remain isolated probes. Publication
/// consumes this value in one direction, applies final row transforms, and
/// never recomputes ordinary cell layout.
struct ResolvedTablePlan<'tree> {
    input: TableLayoutInput<'tree>,
    structure: TableStructure<'tree>,
    prepared: PreparedTableGrid,
    columns: TableColumnLayout,
    cells: PreparedTableCells,
    row_heights: Vec<f64>,
    row_starts: Vec<f64>,
    caption_top_height: f64,
    caption_bottom_height: f64,
    table_content_pos: Point,
    table_height: f64,
}

pub use s02_intrinsic::table_intrinsic_widths;
pub use s00_layout::layout_table;
pub use s04_rows::layout_table_row;

include!("tests.rs");
