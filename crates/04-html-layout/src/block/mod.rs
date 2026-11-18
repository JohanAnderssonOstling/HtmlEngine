//! Stage 00 repeats stages 01–03 for each child, sharing one sibling flow state.
//! Margin and float rules operate on continuous source geometry.

#[path = "00-flow.rs"]
mod s00_flow;
#[path = "01-prepare_child.rs"]
mod s01_prepare_child;
#[path = "02-layout_child.rs"]
mod s02_layout_child;
#[path = "03-finish_child.rs"]
mod s03_finish_child;

mod float_fitting;
mod floats;
mod margins;

pub(crate) use floats::FloatState;
pub(crate) use floats::layout_float;
pub use floats::{FloatBand, FloatSide};
pub(crate) use margins::{MarginAnalysis, physical_margin_edges, stretch_margin_inset};
pub(crate) use s00_flow::layout_flow_children;

use crate::{BoxLayoutRequest, BoxLayoutResult};
use floats::Clearance;
use kurbo::Point;
pub(crate) use margins::MarginStrut;
use margins::{MarginProfile, ParentCollapseContext};

struct BlockFlowInput {
    parent_idx: usize,
    available_width: f64,
    parent_content_height: Option<f64>,
    position: Point,
    parent_ctx: ParentCollapseContext,
}

/// State carried between siblings, owned only by the flow coordinator.
#[derive(Default)]
struct BlockFlowState {
    y_offset: f64,
    max_width: f64,
    pending_margin: MarginStrut,
    trailing_margin: MarginStrut,
    // A cleared collapse-through box separates the adjoining margin chain.
    clearance_consumed_margin: Option<f64>,
    collapsed_parent_top_margin: MarginStrut,
    indent_remaining: f64,
    started_flow: bool,
    // Only the latest adjoining float source can trigger negative clearance.
    latest_adjoining_float_sides: u8,
}

struct PreparedChild {
    child_idx: usize,
    profile: MarginProfile,
    margin_adjoins_parent_top: bool,
    cleared_offset: Option<Clearance>,
    leading_clearance_rewind: Option<f64>,
    clearance_on_collapsed_through_box: bool,
}

struct LaidOutChild {
    profile: MarginProfile,
    runtime_leading_clearance: bool,
    top_collapses_with_parent: bool,
    clearance_on_collapsed_through_box: bool,
    horizontal_margin: f64,
    layout: BoxLayoutResult,
}

impl BlockFlowInput {
    fn child_request(&self, child: usize) -> BoxLayoutRequest {
        BoxLayoutRequest::normal(child, self.available_width, self.parent_content_height)
    }
}

#[cfg(test)]
mod tests;
