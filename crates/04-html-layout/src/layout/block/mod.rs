//! Per-child stages share one coordinator-owned sibling flow state.
//! Margin, float, and fragmentation rules remain reusable helpers.

mod decorations;
#[path = "03-finish_child.rs"]
mod finish_child;
mod floats;
#[path = "00-flow.rs"]
mod flow;
mod fragmentation;
#[path = "02-layout_child.rs"]
mod layout_child;
mod margin_cache;
mod margins;
mod paragraph_spacing;
#[path = "01-prepare_child.rs"]
mod prepare_child;
pub(in crate::layout) use paragraph_spacing::ParagraphSpacing;

pub(in crate::layout) use floats::FloatState;
pub(in crate::layout) use floats::layout_float;
pub use floats::{FloatBand, FloatSide};
pub(in crate::layout) use flow::layout_block_children;
pub(in crate::layout) use margins::{
    MarginAnalysis, establishes_formatting_context, stretch_margin_inset,
};

use crate::layout::{BoxLayoutResult, PlacementId};
use html_style_model::BreakBetween;
use floats::Clearance;
use kurbo::Point;
pub(crate) use margins::MarginStrut;
use margins::{MarginProfile, ParentCollapseContext};

struct BlockFlowInput {
    parent_idx: usize,
    available_width: f64,
    parent_cb_width: f64,
    parent_content_height: Option<f64>,
    fragment_height: Option<f64>,
    position: Point,
    parent_ctx: ParentCollapseContext,
}

struct PreviousBlock {
    top: f64,
    placement: PlacementId,
    break_after: BreakBetween,
}

/// State carried between siblings, owned only by the flow coordinator.
#[derive(Default)]
struct BlockFlowState {
    y_offset: f64,
    max_width: f64,
    pending_margin: MarginStrut,
    trailing_margin: MarginStrut,
    // A cleared collapse-through box separates the adjoining margin chain.
    clearance_margin_barrier: bool,
    clearance_consumed_margin: f64,
    collapsed_parent_top_margin: MarginStrut,
    indent_remaining: f64,
    started_flow: bool,
    previous_block: Option<PreviousBlock>,
    // Only the latest adjoining float source can trigger negative clearance.
    latest_adjoining_float_sides: u8,
}

struct PreparedChild {
    child_idx: usize,
    profile: MarginProfile,
    is_anonymous: bool,
    cleared_offset: Option<Clearance>,
    runtime_leading_clearance: bool,
    runtime_leading_clearance_rewind: f64,
}

struct LaidOutChild {
    child_idx: usize,
    profile: MarginProfile,
    runtime_leading_clearance: bool,
    top_collapses_with_parent: bool,
    clearance_on_collapsed_through_box: bool,
    point: Point,
    horizontal_margin: f64,
    layout: BoxLayoutResult,
}
