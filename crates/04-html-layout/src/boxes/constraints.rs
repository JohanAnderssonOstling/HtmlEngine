use html_style_model::{
    BoxSizing, TextDirection, UsedPreferredSize as PreferredSize, UsedStyleView,
    resolve_used_preferred_size,
};

/// `border_box_inset` is the padding+border to subtract from a specified size
/// under `box-sizing: border-box` (0 for `content-box`); `auto` is unaffected.
pub fn resolve_vertical_size(
    size: PreferredSize,
    parent_content_height: Option<f64>,
    border_box_inset: f64,
) -> Option<f64> {
    resolve_definite_content_size_with_insets(
        size,
        parent_content_height,
        border_box_inset,
        border_box_inset,
    )
}

/// Resolve a content-box size from a CSS size and explicit box-model insets.
/// `stretch` fits the margin box, so its inset may differ from the inset used
/// to convert a border-box authored size.
pub fn resolve_definite_content_size_with_insets(
    size: PreferredSize,
    percentage_basis: Option<f64>,
    border_box_inset: f64,
    stretch_inset: f64,
) -> Option<f64> {
    if matches!(size, PreferredSize::Stretch) {
        return percentage_basis.map(|basis| (basis - stretch_inset).max(0.0));
    }
    size.resolve_definite(percentage_basis)
        .map(|value| (value - border_box_inset).max(0.0))
}

/// Resolve a content-box size when the caller has margins, padding, borders,
/// and `box-sizing` rather than precomputed conversion insets.
pub fn resolve_definite_content_size(
    size: PreferredSize,
    percentage_basis: Option<f64>,
    margin: f64,
    padding_border: f64,
    box_sizing: BoxSizing,
) -> Option<f64> {
    let border_box_inset = if matches!(box_sizing, BoxSizing::BorderBox) {
        padding_border
    } else {
        0.0
    };
    resolve_definite_content_size_with_insets(
        size,
        percentage_basis,
        border_box_inset,
        margin + padding_border,
    )
}

/// Resolve an inline size that is independent of its containing block and
/// convert it to an outer size. Formatting contexts can share this conversion
/// while retaining ownership of their own auto/percentage sizing algorithms.
pub fn resolve_definite_outer_inline_size(
    size: PreferredSize,
    box_sizing: BoxSizing,
    padding_border: f64,
    margin: f64,
) -> Option<f64> {
    let content = resolve_definite_content_size(size, None, margin, padding_border, box_sizing)?;
    Some(content + padding_border + margin)
}

/// Resolve a block-axis minimum. Unlike `height` and `max-height`, an
/// unresolved percentage in `min-height` contributes zero rather than making
/// the entire value behave as `auto`. A mixed calc therefore retains its
/// absolute component when the containing block has an indefinite height.
pub fn resolve_vertical_min_size_with_stretch_inset(
    size: PreferredSize,
    parent_content_height: Option<f64>,
    border_box_inset: f64,
    stretch_inset: f64,
) -> Option<f64> {
    match (size, parent_content_height) {
        (PreferredSize::Percent(_), None) => Some(0.0),
        (
            PreferredSize::Calc {
                absolute_px,
                percentage_dependent: true,
                ..
            },
            None,
        ) => Some((absolute_px as f64 - border_box_inset).max(0.0)),
        (PreferredSize::Comparison { .. }, None) if size.percentage_dependent() => Some(0.0),
        _ => resolve_definite_content_size_with_insets(
            size,
            parent_content_height,
            border_box_inset,
            stretch_inset,
        ),
    }
}

/// Apply definite bounds, with the minimum winning when they conflict.
pub fn constrain_size(size: f64, minimum: Option<f64>, maximum: Option<f64>) -> f64 {
    let size = maximum.map_or(size, |maximum| size.min(maximum));
    minimum.map_or(size, |minimum| size.max(minimum))
}

pub fn constrain_content_width(
    width: f64,
    min_width: PreferredSize,
    max_width: PreferredSize,
    available_width: f64,
    border_box_inset: f64,
) -> f64 {
    let (minimum, maximum) =
        resolve_content_width_limits(min_width, max_width, available_width, border_box_inset);
    width.clamp(minimum, maximum).max(0.0)
}

pub(crate) fn resolve_content_width_limits(
    min_width: PreferredSize,
    max_width: PreferredSize,
    available_width: f64,
    border_box_inset: f64,
) -> (f64, f64) {
    let min_width =
        resolve_preferred_content_width(min_width, 0.0, available_width, border_box_inset);
    let max_width = resolve_preferred_content_width(
        max_width,
        f64::INFINITY,
        available_width,
        border_box_inset,
    )
    .max(min_width);
    (min_width, max_width)
}

pub(crate) fn resolve_preferred_content_width(
    size: PreferredSize,
    auto_width: f64,
    available_width: f64,
    border_box_inset: f64,
) -> f64 {
    match size {
        PreferredSize::Auto => auto_width,
        PreferredSize::Stretch => (available_width - border_box_inset).max(0.0),
        _ => (resolve_used_preferred_size(size, auto_width, available_width) - border_box_inset)
            .max(0.0),
    }
}

/// Tables resolve automatic margins against their final intrinsic width,
/// even when their authored width is `auto`.
pub(crate) fn resolve_margin_left_for_used_width(
    style: UsedStyleView<'_>,
    containing_direction: TextDirection,
    containing_width: f64,
    available_width: f64,
    border_width: f64,
) -> f64 {
    let left = style.margin_left().resolve(containing_width);
    let right = style.margin_right().resolve(containing_width);
    let left_auto = style.margin_left_auto();
    let right_auto = style.margin_right_auto();

    resolve_margin_left_from_edges(
        left,
        right,
        left_auto,
        right_auto,
        containing_direction,
        available_width,
        border_width,
    )
}

pub(crate) fn resolve_margin_left_from_edges(
    left: f64,
    right: f64,
    left_auto: bool,
    right_auto: bool,
    containing_direction: TextDirection,
    available_width: f64,
    border_width: f64,
) -> f64 {
    if !left_auto && !right_auto && containing_direction == TextDirection::Rtl {
        return left + (available_width - border_width - left - right);
    }
    resolve_auto_start_margin(
        (left, right),
        (left_auto, right_auto),
        available_width - border_width,
        if containing_direction == TextDirection::Rtl {
            AutoMarginOverflow::Start
        } else {
            AutoMarginOverflow::End
        },
    )
}

#[derive(Clone, Copy)]
pub(crate) enum AutoMarginOverflow {
    Start,
    End,
    Split,
    Preserve,
}

/// Distribute the space remaining after the border box and any fixed insets.
pub(crate) fn resolve_auto_start_margin(
    margins: (f64, f64),
    automatic: (bool, bool),
    available: f64,
    overflow: AutoMarginOverflow,
) -> f64 {
    let remaining = available
        - if automatic.0 { 0.0 } else { margins.0 }
        - if automatic.1 { 0.0 } else { margins.1 };
    if matches!(overflow, AutoMarginOverflow::Preserve) && !(remaining > 0.0) {
        return margins.0;
    }
    match automatic {
        (true, true) if remaining >= 0.0 => remaining / 2.0,
        (true, true) => match overflow {
            AutoMarginOverflow::Start => remaining,
            AutoMarginOverflow::End => 0.0,
            AutoMarginOverflow::Split => remaining / 2.0,
            AutoMarginOverflow::Preserve => margins.0,
        },
        (true, false) => remaining,
        _ => margins.0,
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_definite_content_size, resolve_definite_outer_inline_size};
    use crate::{BoxLayoutRequest, UsedBorderInsets};
    use html_style_model::{BoxSizing, UsedPreferredSize as PreferredSize};

    #[test]
    fn request_modes_make_table_placement_and_measurement_semantics_explicit() {
        let ordinary = BoxLayoutRequest::normal(0, 100.0, None);
        assert!(!ordinary.splits_content_placement());
        assert!(ordinary.used_borders().is_none());
        assert!(!ordinary.suppresses_authored_height_basis());
        assert!(!ordinary.is_intrinsic_block_measurement());

        let borders = UsedBorderInsets {
            top: 1.0,
            right: 2.0,
            bottom: 3.0,
            left: 4.0,
        };
        let cell = BoxLayoutRequest::table_cell(1, 100.0, None, Some(borders));
        assert!(cell.splits_content_placement());
        assert_eq!(cell.used_borders().expect("table-cell borders").left, 4.0);

        let table_measurement =
            BoxLayoutRequest::table_cell(1, 100.0, None, None).for_table_intrinsic_measurement();
        assert!(table_measurement.splits_content_placement());
        assert!(table_measurement.suppresses_authored_height_basis());
        assert!(!table_measurement.is_intrinsic_block_measurement());

        let intrinsic = ordinary.for_intrinsic_block_measurement();
        assert!(intrinsic.suppresses_authored_height_basis());
        assert!(intrinsic.is_intrinsic_block_measurement());
    }

    #[test]
    fn definite_outer_inline_size_is_shared_across_formatting_contexts() {
        assert_eq!(
            resolve_definite_outer_inline_size(
                PreferredSize::Px(20.0),
                BoxSizing::ContentBox,
                6.0,
                4.0
            ),
            Some(30.0)
        );
        assert_eq!(
            resolve_definite_outer_inline_size(
                PreferredSize::Px(4.0),
                BoxSizing::BorderBox,
                6.0,
                4.0
            ),
            Some(10.0)
        );
        assert_eq!(
            resolve_definite_outer_inline_size(
                PreferredSize::Calc {
                    absolute_px: 12.0,
                    percentage: 0.0,
                    percentage_dependent: false
                },
                BoxSizing::ContentBox,
                3.0,
                0.0,
            ),
            Some(15.0)
        );
        assert_eq!(
            resolve_definite_outer_inline_size(
                PreferredSize::Percent(0.5),
                BoxSizing::ContentBox,
                6.0,
                4.0
            ),
            None
        );
    }

    #[test]
    fn definite_size_resolution_keeps_context_keywords_outside_the_shared_core() {
        assert_eq!(
            PreferredSize::Percent(0.5).resolve_definite(Some(200.0)),
            Some(100.0)
        );
        assert_eq!(PreferredSize::Percent(0.5).resolve_definite(None), None);
        assert_eq!(PreferredSize::Stretch.resolve_definite(Some(200.0)), None);
        assert_eq!(
            resolve_definite_content_size(
                PreferredSize::Px(80.0),
                Some(200.0),
                10.0,
                12.0,
                BoxSizing::BorderBox
            ),
            Some(68.0)
        );
        assert_eq!(
            resolve_definite_content_size(
                PreferredSize::Stretch,
                Some(200.0),
                10.0,
                12.0,
                BoxSizing::ContentBox
            ),
            Some(178.0)
        );
    }
}
