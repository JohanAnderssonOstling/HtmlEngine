//! Finish baseline alignment without re-entering the solver.
use super::{BaselineItem, FlexGridInput};
use crate::layout::LayoutEngine;
use kurbo::{Size, Vec2};

pub(super) fn align(
    session: &mut LayoutEngine<'_, '_>,
    input: &FlexGridInput<'_>,
    size_containment: bool,
    mut items: Vec<BaselineItem>,
    mut result: Size,
) -> Size {
    items.sort_unstable_by(|left, right| {
        (left.original_y + left.height).total_cmp(&(right.original_y + right.height))
    });
    let mut remaining = items.as_slice();
    while let Some(first) = remaining.first() {
        let group_end_coordinate = first.original_y + first.height;
        let group_end = remaining
            .iter()
            .position(|item| (item.original_y + item.height - group_end_coordinate).abs() > 0.01)
            .unwrap_or(remaining.len());
        let (group, rest) = remaining.split_at(group_end);
        remaining = rest;
        if group.len() <= 1 {
            continue;
        }
        let baseline_origin = group
            .iter()
            .map(|item| item.original_y)
            .fold(f64::INFINITY, f64::min);
        let max_baseline = group
            .iter()
            .map(|item| item.baseline)
            .fold(0.0f64, f64::max);
        for item in group {
            let desired_y = baseline_origin + max_baseline - item.baseline;
            let delta = desired_y - item.original_y;
            if delta.abs() > f64::EPSILON {
                session
                    .placement
                    .translate(item.placement, Vec2::new(0.0, delta));
            }
            if !size_containment {
                result.height = result
                    .height
                    .max(desired_y + item.height - input.content_origin.y);
            }
        }
    }
    result
}
