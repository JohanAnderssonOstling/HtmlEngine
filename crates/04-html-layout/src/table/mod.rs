//! Reading order: coordinator (00), sizing stages (01–05), final placement (06).
//! Shared stage contracts, helpers, and tests are unnumbered.

#[path = "03-column_sizing.rs"]
mod column_sizing;
#[path = "01-grid.rs"]
mod grid;
#[path = "02-intrinsic.rs"]
mod intrinsic;
#[path = "00-layout.rs"]
mod layout;
#[path = "06-placement.rs"]
mod placement;
#[path = "05-row_sizing.rs"]
mod row_sizing;
#[path = "04-rows.rs"]
mod rows;

mod backgrounds;
mod borders;
mod captions;
mod columns;

use columns::TableColumnLayout;
use grid::PreparedTableGrid;
use kurbo::Point;
use rows::PreparedTableCells;

#[derive(Clone, Copy, Eq, PartialEq)]
enum TableDimensionBasis {
    UsedBorderBox,
    AuthoredContentBox,
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

pub use intrinsic::table_intrinsic_widths;
pub use layout::layout_table;
pub use rows::layout_table_row;

include!("tests.rs");
