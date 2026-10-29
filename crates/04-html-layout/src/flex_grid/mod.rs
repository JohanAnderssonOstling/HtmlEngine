//! Owned stage results carry preparation through solving, placement, and alignment.
//! Taffy callbacks and repeated solves stay within the solver stage.

use html_style_model::ItemAlignment;
use kurbo::{Point, Size};
use taffy::geometry::Size as TaffySize;
use taffy::prelude::{AvailableSpace, Layout, Style};
use taffy::style::Display as TaffyDisplay;

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

#[path = "04-align.rs"]
mod alignment;
#[path = "00-layout.rs"]
mod coordinator;
#[path = "03-place.rs"]
mod placement;
#[path = "01-prepare.rs"]
mod preparation;
#[path = "02-solve.rs"]
mod solver;
pub use coordinator::layout;

struct FlexGridInput<'a> {
    container_idx: usize,
    children: &'a [u32],
    content_origin: Point,
    content_width: f64,
    explicit_height: Option<f64>,
    min_height: Option<f64>,
    max_height: Option<f64>,
    kind: TaffyContainerKind,
}

struct FlexGridPolicy {
    flex_row: bool,
    parent_align_items: ItemAlignment,
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
    placement: crate::layout::PlacementId,
}

fn finite_f32(value: f64) -> f32 {
    value.clamp(0.0, f32::MAX as f64) as f32
}

/// Taffy dispatches every childless node through generic leaf layout even
/// when its display mode is Grid. A non-generated sentinel keeps empty grids
/// on the Grid path without occupying an auto-fit track.
fn ensure_grid_algorithm_root(children: &mut Vec<(Style, Option<u32>)>, kind: TaffyContainerKind) {
    if kind == TaffyContainerKind::Grid && children.is_empty() {
        children.push((
            Style {
                display: TaffyDisplay::None,
                ..Style::default()
            },
            None,
        ));
    }
}

include!("tests.rs");
