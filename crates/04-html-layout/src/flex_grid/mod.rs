//! Stage 00 carries owned results through prepare (01), solve (02), place (03), and align (04).
//! Taffy measurement and repeated solves stay within the solver stage.

#[path = "00-layout.rs"]
mod s00_layout;
#[path = "01-prepare.rs"]
mod s01_prepare;
#[path = "02-solve.rs"]
mod s02_solve;
#[path = "03-place.rs"]
mod s03_place;
#[path = "04-align.rs"]
mod s04_align;

use kurbo::{Point, Size};
use taffy::geometry::Size as TaffySize;
use taffy::prelude::{AvailableSpace, Dimension, Layout, Style};

mod compatibility;
mod container;
mod sizing;
use compatibility::ensure_grid_algorithm_root;
mod intrinsic;
mod item_sizing;
mod measurement;
pub use measurement::FlexGridState;
mod style;
mod taffy_layout;
mod tracks;
pub use intrinsic::intrinsic_widths;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaffyContainerKind {
    Flex,
    Grid,
}

pub(crate) use container::layout_container;
pub(crate) use s00_layout::layout;

pub(crate) struct FlexGridInput<'a> {
    pub(crate) container_idx: usize,
    pub(crate) children: &'a [u32],
    pub(crate) content_origin: Point,
    pub(crate) content_width: f64,
    pub(crate) explicit_height: Option<f64>,
    pub(crate) min_height: Option<f64>,
    pub(crate) max_height: Option<f64>,
    pub(super) container: sizing::ContainerFacts,
}

struct FlexGridPolicy {
    size_containment: bool,
    child_containing_height: Option<f64>,
}

struct PreparedFlexGrid {
    child_styles: Vec<(Style, Option<u32>)>,
    root_style: Style,
    available: TaffySize<AvailableSpace>,
    rerun_indefinite_percentage_rows: bool,
}

struct SolvedFlexGrid {
    first_baseline: Option<f64>,
    items: Vec<(Layout, u32)>,
    size: Size,
}

/// Only baseline-aligned children need a record after final placement.
struct BaselineItem {
    original_y: f64,
    height: f64,
    baseline: f64,
    placement: crate::PlacementId,
}

fn finite_f32(value: f64) -> f32 {
    value.clamp(0.0, f32::MAX as f64) as f32
}

fn length_dimension(value: f64) -> Dimension {
    Dimension::length(finite_f32(value))
}

fn nonnegative_size(size: TaffySize<f32>) -> Size {
    Size::new(size.width.max(0.0) as f64, size.height.max(0.0) as f64)
}

#[cfg(test)]
mod tests;
