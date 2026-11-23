//! Keep Taffy measurement and percentage-row reruns inside one solver stage.
use super::measurement::MeasurementMode;
use super::taffy_layout::TaffyLayoutTree;
use super::{FlexGridInput, PreparedFlexGrid, SolvedFlexGrid};
use crate::LayoutEngine;
use taffy::geometry::Size as TaffySize;
use taffy::prelude::AvailableSpace;

pub(super) fn solve(
    engine: &mut LayoutEngine<'_, '_>,
    input: &FlexGridInput<'_>,
    prepared: PreparedFlexGrid,
) -> SolvedFlexGrid {
    let mut taffy = TaffyLayoutTree::solve(
        MeasurementMode::Full(engine),
        input.container.kind,
        prepared.root_style,
        prepared.child_styles,
        prepared.available,
    );
    if prepared.rerun_indefinite_percentage_rows {
        // CSS Grid resolves percentage rows as auto while finding an
        // intrinsic block size, then sizes the same tracks once more against
        // that now-definite size. Taffy owns both track-sizing passes; the
        // adapter supplies the first pass's result as the second pass basis.
        let intrinsic_height = taffy.root_layout().size.height.max(0.0);
        let second_pass_available = TaffySize {
            width: prepared.available.width,
            height: AvailableSpace::Definite(intrinsic_height),
        };
        taffy.compute_with_definite_root_height(second_pass_available, intrinsic_height);
    }

    taffy.into_result()
}
