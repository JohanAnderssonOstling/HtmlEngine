pub(crate) fn parse_positive_span(value: Option<&str>) -> usize {
    value.and_then(|value| value.trim().parse::<usize>().ok()).filter(|&value| value > 0).unwrap_or(1)
}

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

pub(crate) use intrinsic::{table_intrinsic_sizes, table_intrinsic_widths};
pub(crate) use layout::layout_table;
pub(crate) use rows::layout_table_row;

include!("tests.rs");
