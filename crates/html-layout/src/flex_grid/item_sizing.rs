//! Intrinsic item dimensions and flex automatic minimum sizing.

use super::finite_f32;
use super::style::{dimension, layout_style};
use crate::layout::{
    LayoutEngine, ReplacedFlexAutoMinInput, ReplacedMainAxis, measure_box_isolated, preferred_aspect_ratio, resolve_definite_content_size, resolve_replaced_flex_auto_min_main_size,
};
use html_style_model::{ItemAlignment, OverflowMode, UsedPreferredSize as PreferredSize};
use taffy::prelude::Dimension;

/// Taffy represents `flex-basis` as only auto, length, or percentage. CSS
/// intrinsic sizing keywords therefore have to be resolved at the adapter
/// boundary instead of being collapsed to `auto` (content/max-content).
pub(super) fn resolve_intrinsic_flex_basis(session: &mut LayoutEngine<'_, '_>, box_idx: usize, containing_width: f64, horizontal_main_axis: bool) -> Option<Dimension> {
    let basis = session.reader.style(box_idx).flex_basis();
    if !matches!(basis, PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent) {
        return None;
    }
    if horizontal_main_axis {
        return Some(taffy_item_width_dimension(session, box_idx, basis, containing_width, false));
    }

    // Intrinsic block-axis sizes are measured with the item's allocated
    // cross size. This is observable when content wraps at that width.
    let measured = measure_box_isolated(session, box_idx, containing_width, None);
    let style = session.reader.style(box_idx);
    let vertical_inset = style.get_vertical_padding(containing_width) + style.border_top_width() as f64 + style.border_bottom_width() as f64;
    let basis = match style.box_sizing() {
        html_style_model::BoxSizing::ContentBox => (measured.height - vertical_inset).max(0.0),
        html_style_model::BoxSizing::BorderBox => measured.height.max(vertical_inset),
    };
    Some(Dimension::length(finite_f32(basis)))
}

/// Resolve the flex-item automatic minimum from the content-size
/// suggestion, capped by a definite preferred size. Feeding Taffy `auto`
/// here makes its generic measurement path treat a specified width as the
/// minimum itself, which prevents empty fixed-size items from shrinking
/// and gives percentage-sized tables no usable share of the flex line.
pub(super) fn flex_automatic_min_content_width(session: &LayoutEngine<'_, '_>, box_idx: usize, containing_width: f64) -> f64 {
    let style = session.reader.style(box_idx);
    if matches!(style.overflow_x(), OverflowMode::Hidden | OverflowMode::Scroll | OverflowMode::Auto) {
        return 0.0;
    }

    let content_min = crate::layout::box_content_intrinsic_widths(session, box_idx).0.max(0.0);
    let padding_border = style.get_horizontal_padding(containing_width) + style.border_left_width() as f64 + style.border_right_width() as f64;
    let horizontal_margin = style.get_horizontal_margin(containing_width);
    let resolve_definite = |value| resolve_definite_content_size(value, Some(containing_width), horizontal_margin, padding_border, style.box_sizing());
    let preferred = match style.width() {
        PreferredSize::MinContent => Some(content_min),
        PreferredSize::MaxContent | PreferredSize::FitContent => Some(crate::layout::box_content_intrinsic_widths(session, box_idx).1.max(content_min)),
        value => resolve_definite(value),
    };
    let mut automatic_min = preferred.map_or(content_min, |preferred| content_min.min(preferred));
    if matches!(style.max_width(), PreferredSize::Px(_) | PreferredSize::Percent(_) | PreferredSize::Stretch)
        && let Some(maximum) = resolve_definite(style.max_width())
    {
        automatic_min = automatic_min.min(maximum);
    }
    automatic_min.max(0.0)
}

/// Compute the content-based automatic minimum of a replaced flex item.
/// Taffy's generic leaf minimum starts with the raw intrinsic size, but CSS
/// first converts a definite (including stretched) cross size and cross
/// min/max constraints through the preferred aspect ratio.
pub(super) fn flex_replaced_automatic_minimum(session: &LayoutEngine<'_, '_>, box_idx: usize, containing_width: f64, containing_height: Option<f64>, horizontal_main_axis: bool) -> Option<f64> {
    let image_intrinsic = session.reader.image_intrinsic(box_idx)?;
    let intrinsic = image_intrinsic.size;
    let style = session.reader.style(box_idx);
    let layout = layout_style(session, box_idx);
    let scrollable_main = if horizontal_main_axis { style.overflow_x() } else { style.overflow_y() };
    if matches!(scrollable_main, OverflowMode::Hidden | OverflowMode::Scroll | OverflowMode::Auto) {
        return Some(0.0);
    }

    let ratio = preferred_aspect_ratio(style.aspect_ratio(), image_intrinsic.ratio).filter(|ratio| ratio.is_finite() && *ratio > 0.0);

    let horizontal_padding_border = style.get_horizontal_padding(containing_width) + style.border_left_width() as f64 + style.border_right_width() as f64;
    let vertical_padding_border = style.get_vertical_padding(containing_width) + style.border_top_width() as f64 + style.border_bottom_width() as f64;
    let horizontal_margin = style.get_horizontal_margin(containing_width);
    let vertical_margin = style.margin_top().resolve(containing_width) + style.margin_bottom().resolve(containing_width);
    let (main_axis, main_size, cross_size, min_main_size, min_cross_size, max_main_size, max_cross_size, main_basis, cross_basis, main_margin, cross_margin, main_padding_border, cross_padding_border, cross_auto_margins) =
        if horizontal_main_axis {
            (
                ReplacedMainAxis::Horizontal,
                style.width(),
                style.height(),
                style.min_width(),
                style.min_height(),
                style.max_width(),
                style.max_height(),
                Some(containing_width),
                containing_height,
                horizontal_margin,
                vertical_margin,
                horizontal_padding_border,
                vertical_padding_border,
                layout.margin_top_auto || layout.margin_bottom_auto,
            )
        } else {
            (
                ReplacedMainAxis::Vertical,
                style.height(),
                style.width(),
                style.min_height(),
                style.min_width(),
                style.max_height(),
                style.max_width(),
                containing_height,
                Some(containing_width),
                vertical_margin,
                horizontal_margin,
                vertical_padding_border,
                horizontal_padding_border,
                layout.margin_left_auto || layout.margin_right_auto,
            )
        };
    let mut stretch_cross_size = false;
    if matches!(cross_size, PreferredSize::Auto) && !cross_auto_margins {
        let parent_alignment = session.reader.get_parent(box_idx).map(|parent| layout_style(session, parent).align_items).unwrap_or(ItemAlignment::Normal);
        let effective_alignment = if layout.align_self == ItemAlignment::Auto { parent_alignment } else { layout.align_self };
        if matches!(effective_alignment, ItemAlignment::Normal | ItemAlignment::Stretch) {
            stretch_cross_size = cross_basis.is_some();
        }
    }
    Some(resolve_replaced_flex_auto_min_main_size(ReplacedFlexAutoMinInput {
        intrinsic,
        aspect_ratio: ratio,
        main_axis,
        main_size,
        cross_size,
        min_main_size,
        min_cross_size,
        max_main_size,
        max_cross_size,
        main_basis,
        cross_basis,
        main_margin,
        cross_margin,
        main_padding_border,
        cross_padding_border,
        box_sizing: style.box_sizing(),
        stretch_cross_size,
    }))
}

pub(super) fn taffy_item_width_dimension(session: &LayoutEngine<'_, '_>, box_idx: usize, value: PreferredSize, containing_width: f64, intrinsic_inline_sizing: bool) -> Dimension {
    if let PreferredSize::Calc { absolute_px, percentage, .. } = value {
        return Dimension::length(finite_f32((absolute_px as f64 + containing_width * percentage as f64).max(0.0)));
    }
    if !matches!(value, PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent) {
        return dimension(value);
    }
    let style = session.reader.style(box_idx);
    let (min, max) = crate::layout::box_content_intrinsic_widths(session, box_idx);
    let available = (containing_width - style.get_horizontal_margin_padding(containing_width) - style.border_left_width() as f64 - style.border_right_width() as f64).max(0.0);
    let content = match value {
        PreferredSize::MinContent => min,
        PreferredSize::MaxContent => max,
        PreferredSize::FitContent if intrinsic_inline_sizing => max,
        PreferredSize::FitContent => max.min(available.max(min)),
        _ => unreachable!(),
    };
    let border_box_inset = match style.box_sizing() {
        html_style_model::BoxSizing::ContentBox => 0.0,
        html_style_model::BoxSizing::BorderBox => style.get_horizontal_padding(containing_width) + style.border_left_width() as f64 + style.border_right_width() as f64,
    };
    Dimension::length(finite_f32(content + border_box_inset))
}
