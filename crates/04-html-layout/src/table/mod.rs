
mod backgrounds;
mod borders;
mod captions;
mod columns;
mod grid;
mod intrinsic;
mod layout;
mod row_sizing;
mod rows;
mod sizing;

pub use intrinsic::{table_intrinsic_sizes, table_intrinsic_widths};
pub use layout::layout_table;
pub use rows::layout_table_row;

include!("tests.rs");
