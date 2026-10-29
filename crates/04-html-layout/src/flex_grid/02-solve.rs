//! Keep Taffy callbacks and percentage-row reruns inside one solver stage.
use super::measurement::measure_item_with_baseline;
use super::taffy_layout::TaffyLayoutTree;
use super::{FlexGridInput, PreparedFlexGrid, SolvedFlexGrid};
use crate::layout::LayoutEngine;
use kurbo::Size;
use taffy::geometry::Size as TaffySize;
use taffy::prelude::AvailableSpace;

pub(super) fn solve(
    session: &mut LayoutEngine<'_, '_>,
    input: &FlexGridInput<'_>,
    prepared: PreparedFlexGrid,
) -> SolvedFlexGrid {
    let PreparedFlexGrid {
        child_styles,
        root_style,
        available,
        rerun_indefinite_percentage_rows,
    } = prepared;
    let mut taffy = TaffyLayoutTree::new(
        |box_idx, known, available| measure_item_with_baseline(session, box_idx, known, available),
        input.kind,
        root_style,
        child_styles,
    );
    taffy.compute(available);
    if rerun_indefinite_percentage_rows {
        // CSS Grid resolves percentage rows as auto while finding an
        // intrinsic block size, then sizes the same tracks once more against
        // that now-definite size. Taffy owns both track-sizing passes; the
        // adapter supplies the first pass's result as the second pass basis.
        let intrinsic_height = taffy.root_layout().size.height.max(0.0);
        let second_pass_available = TaffySize {
            width: available.width,
            height: AvailableSpace::Definite(intrinsic_height),
        };
        taffy.compute_with_definite_root_height(second_pass_available, intrinsic_height);
    }

    let taffy_items = taffy.item_layouts().collect();
    let root_layout = taffy.root_layout();
    let root_first_baseline = taffy.root_first_baseline().map(f64::from);
    drop(taffy);
    SolvedFlexGrid {
        first_baseline: root_first_baseline,
        items: taffy_items,
        size: Size::new(
            root_layout.size.width.max(0.0) as f64,
            root_layout.size.height.max(0.0) as f64,
        ),
    }
}
