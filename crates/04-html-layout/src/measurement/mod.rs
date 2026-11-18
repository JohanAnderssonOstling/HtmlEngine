//! Shared size queries and isolated layout probes for all formatting contexts.
//! Intrinsic sizing caches content contributions; isolated probes run against
//! temporary output without publishing geometry into the active layout pass.

mod intrinsic;
mod isolated;

pub(crate) use intrinsic::{
    IntrinsicFloats, IntrinsicWidths, box_content_intrinsic_widths, box_intrinsic_widths,
    contains_full_width_percentage_table, percentage_height_image_width,
};
pub(crate) use isolated::{
    measure_box_isolated, measure_resolved_box_isolated, with_isolated_measurement,
};
