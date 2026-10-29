//! Intrinsic item dimensions and flex automatic minimum sizing.

use super::finite_f32;
use super::style::{dimension, layout_style};
use crate::layout::{
    BoxLayoutRequest, LayoutEngine, ReplacedMainAxis, ReplacedSizeInput, ResolvedBoxModel,
    box_content_intrinsic_widths, measure_box_isolated, preferred_aspect_ratio,
    resolve_definite_content_size, resolve_replaced_flex_auto_min_main_size,
};
use html_style_model::{
    BoxSizing, FlexWrap, ItemAlignment, OverflowMode, UsedPreferredSize as PreferredSize,
};
use taffy::prelude::Dimension;

/// Taffy represents `flex-basis` as only auto, length, or percentage. CSS
/// intrinsic sizing keywords therefore have to be resolved at the adapter
/// boundary instead of being collapsed to `auto` (content/max-content).
pub(super) fn resolve_intrinsic_flex_basis(
    session: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    containing_width: f64,
    containing_height: Option<f64>,
    horizontal_main_axis: bool,
) -> Option<Dimension> {
    let style = session.reader.style(box_idx);
    let model = ResolvedBoxModel::new(style, containing_width).with_nonnegative_padding();
    let mut basis = style.flex_basis();
    if matches!(basis, PreferredSize::Stretch) {
        let available = if horizontal_main_axis {
            Some(containing_width)
        } else {
            containing_height
        };
        if let Some(available) = available {
            let (margin, inset) = if horizontal_main_axis {
                (model.horizontal_margin(), model.horizontal_padding_border())
            } else {
                (model.vertical_margin(), model.vertical_padding_border())
            };
            let border_box = (available - margin).max(inset);
            let value = match style.box_sizing() {
                BoxSizing::ContentBox => border_box - inset,
                BoxSizing::BorderBox => border_box,
            };
            return Some(Dimension::length(finite_f32(value)));
        }
        // An indefinite stretch basis resolves to content rather than using
        // the item's authored main size through the `auto` basis path.
        basis = PreferredSize::Content;
    }
    if matches!(basis, PreferredSize::Content) {
        if let Some(value) = content_ratio_basis(
            session,
            box_idx,
            containing_width,
            containing_height,
            horizontal_main_axis,
        ) {
            return Some(value);
        }
    }
    if !matches!(
        basis,
        PreferredSize::Content
            | PreferredSize::MinContent
            | PreferredSize::MaxContent
            | PreferredSize::FitContent
    ) {
        return None;
    }
    if horizontal_main_axis {
        return Some(taffy_item_width_dimension(
            session,
            box_idx,
            if matches!(basis, PreferredSize::Content) {
                PreferredSize::MaxContent
            } else {
                basis
            },
            containing_width,
            false,
        ));
    }

    // Intrinsic block-axis sizes are measured with the item's allocated
    // cross size. This is observable when content wraps at that width.
    let measured = measure_box_isolated(
        session,
        BoxLayoutRequest::normal(box_idx, containing_width.max(0.0), None)
            .for_intrinsic_block_measurement(),
    )
    .size_with_floats();
    let vertical_inset = model.vertical_padding_border();
    let basis = match style.box_sizing() {
        BoxSizing::ContentBox => (measured.height - vertical_inset).max(0.0),
        BoxSizing::BorderBox => measured.height.max(vertical_inset),
    };
    Some(Dimension::length(finite_f32(basis)))
}

fn content_ratio_basis(
    session: &LayoutEngine<'_, '_>,
    box_idx: usize,
    containing_width: f64,
    containing_height: Option<f64>,
    horizontal_main_axis: bool,
) -> Option<Dimension> {
    let style = session.reader.style(box_idx);
    let model = ResolvedBoxModel::new(style, containing_width).with_nonnegative_padding();
    let intrinsic_ratio = session
        .reader
        .image_intrinsic(box_idx)
        .and_then(|image| image.aspect_ratio);
    let authored = style.aspect_ratio();
    let ratio = preferred_aspect_ratio(authored, intrinsic_ratio)?;
    let horizontal_inset = model.horizontal_padding_border();
    let vertical_inset = model.vertical_padding_border();
    let layout = layout_style(session, box_idx);
    let (
        cross_size,
        cross_min,
        cross_max,
        cross_basis,
        cross_margin,
        cross_inset,
        main_inset,
        auto_margins,
    ) = if horizontal_main_axis {
        (
            style.height(),
            style.min_height(),
            style.max_height(),
            containing_height,
            model.vertical_margin(),
            vertical_inset,
            horizontal_inset,
            layout.margin_top_auto || layout.margin_bottom_auto,
        )
    } else {
        (
            style.width(),
            style.min_width(),
            style.max_width(),
            Some(containing_width),
            model.horizontal_margin(),
            horizontal_inset,
            vertical_inset,
            layout.margin_left_auto || layout.margin_right_auto,
        )
    };
    let resolve = |size| {
        resolve_definite_content_size(
            size,
            cross_basis,
            cross_margin,
            cross_inset,
            style.box_sizing(),
        )
    };
    let mut cross = resolve(cross_size).or_else(|| {
        if !matches!(cross_size, PreferredSize::Auto) || auto_margins {
            return None;
        }
        let parent = layout_style(session, session.reader.get_parent(box_idx)?);
        let align = if layout.align_self == ItemAlignment::Auto {
            parent.align_items
        } else {
            layout.align_self
        };
        (parent.flex_wrap == FlexWrap::NoWrap
            && matches!(align, ItemAlignment::Normal | ItemAlignment::Stretch))
        .then(|| resolve(PreferredSize::Stretch))
        .flatten()
    })?;
    let minimum = resolve(cross_min).unwrap_or(0.0);
    let maximum = resolve(cross_max).unwrap_or(f64::INFINITY).max(minimum);
    cross = cross.clamp(minimum, maximum);
    // Intrinsic image ratios refer to the content box; an authored ratio
    // follows box-sizing, including border-box ratios on ordinary elements.
    let border_ratio = style.box_sizing() == BoxSizing::BorderBox
        && !(authored.uses_intrinsic() && intrinsic_ratio.is_some());
    let cross = cross + if border_ratio { cross_inset } else { 0.0 };
    let main = if horizontal_main_axis {
        cross * ratio
    } else {
        cross / ratio
    };
    let content = (main - if border_ratio { main_inset } else { 0.0 }).max(0.0);
    let basis = content
        + if style.box_sizing() == BoxSizing::BorderBox {
            main_inset
        } else {
            0.0
        };
    Some(Dimension::length(finite_f32(basis)))
}

/// Resolve the flex-item automatic minimum from the content-size
/// suggestion, capped by a definite preferred size. Feeding Taffy `auto`
/// here makes its generic measurement path treat a specified width as the
/// minimum itself, which prevents empty fixed-size items from shrinking
/// and gives percentage-sized tables no usable share of the flex line.
pub(super) fn flex_automatic_min_content_width(
    session: &LayoutEngine<'_, '_>,
    box_idx: usize,
    containing_width: f64,
) -> f64 {
    let style = session.reader.style(box_idx);
    let model = ResolvedBoxModel::new(style, containing_width).with_nonnegative_padding();
    if matches!(
        style.overflow_x(),
        OverflowMode::Hidden | OverflowMode::Scroll | OverflowMode::Auto
    ) {
        return 0.0;
    }

    let (content_min, content_max) = box_content_intrinsic_widths(session, box_idx, None);
    let content_min = content_min.max(0.0);
    let padding_border = model.horizontal_padding_border();
    let horizontal_margin = model.horizontal_margin();
    let resolve_definite = |value| {
        resolve_definite_content_size(
            value,
            Some(containing_width),
            horizontal_margin,
            padding_border,
            style.box_sizing(),
        )
    };
    let preferred = match style.width() {
        PreferredSize::MinContent => Some(content_min),
        PreferredSize::MaxContent | PreferredSize::FitContent => Some(content_max.max(content_min)),
        value => resolve_definite(value),
    };
    let mut automatic_min = preferred.map_or(content_min, |preferred| content_min.min(preferred));
    if matches!(
        style.max_width(),
        PreferredSize::Px(_) | PreferredSize::Percent(_) | PreferredSize::Stretch
    ) && let Some(maximum) = resolve_definite(style.max_width())
    {
        automatic_min = automatic_min.min(maximum);
    }
    automatic_min.max(0.0)
}

/// Compute the content-based automatic minimum of a replaced flex item.
/// Taffy's generic leaf minimum starts with the raw intrinsic size, but CSS
/// first converts a definite (including stretched) cross size and cross
/// min/max constraints through the preferred aspect ratio.
pub(super) fn flex_replaced_automatic_minimum(
    session: &LayoutEngine<'_, '_>,
    box_idx: usize,
    containing_width: f64,
    containing_height: Option<f64>,
    horizontal_main_axis: bool,
) -> Option<f64> {
    let image_intrinsic = session.reader.image_intrinsic(box_idx)?;
    let intrinsic = image_intrinsic.size;
    let style = session.reader.style(box_idx);
    let model = ResolvedBoxModel::new(style, containing_width).with_nonnegative_padding();
    let layout = layout_style(session, box_idx);
    let scrollable_main = if horizontal_main_axis {
        style.overflow_x()
    } else {
        style.overflow_y()
    };
    if matches!(
        scrollable_main,
        OverflowMode::Hidden | OverflowMode::Scroll | OverflowMode::Auto
    ) {
        return Some(0.0);
    }

    let ratio = preferred_aspect_ratio(style.aspect_ratio(), image_intrinsic.aspect_ratio)
        .filter(|ratio| ratio.is_finite() && *ratio > 0.0);

    let input =
        ReplacedSizeInput::from_style(style, intrinsic, ratio, containing_width, containing_height)
            .with_box_model(
                model.horizontal_margin(),
                model.vertical_margin(),
                model.horizontal_padding_border(),
                model.vertical_padding_border(),
            );
    let (main_axis, cross_size, cross_basis, cross_auto_margins) = if horizontal_main_axis {
        (
            ReplacedMainAxis::Horizontal,
            style.height(),
            containing_height,
            layout.margin_top_auto || layout.margin_bottom_auto,
        )
    } else {
        (
            ReplacedMainAxis::Vertical,
            style.width(),
            Some(containing_width),
            layout.margin_left_auto || layout.margin_right_auto,
        )
    };
    let mut stretch_cross_size = false;
    if matches!(cross_size, PreferredSize::Auto) && !cross_auto_margins {
        let parent_alignment = session
            .reader
            .get_parent(box_idx)
            .map(|parent| layout_style(session, parent).align_items)
            .unwrap_or(ItemAlignment::Normal);
        let effective_alignment = if layout.align_self == ItemAlignment::Auto {
            parent_alignment
        } else {
            layout.align_self
        };
        if matches!(
            effective_alignment,
            ItemAlignment::Normal | ItemAlignment::Stretch
        ) {
            stretch_cross_size = cross_basis.is_some();
        }
    }
    Some(resolve_replaced_flex_auto_min_main_size(
        input,
        main_axis,
        stretch_cross_size,
    ))
}

pub(super) fn taffy_item_width_dimension(
    session: &LayoutEngine<'_, '_>,
    box_idx: usize,
    value: PreferredSize,
    containing_width: f64,
    intrinsic_inline_sizing: bool,
) -> Dimension {
    if let PreferredSize::Calc {
        absolute_px,
        percentage,
        ..
    } = value
    {
        return Dimension::length(finite_f32(
            (absolute_px as f64 + containing_width * percentage as f64).max(0.0),
        ));
    }
    if !matches!(
        value,
        PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent
    ) {
        return dimension(value);
    }
    let style = session.reader.style(box_idx);
    let model = ResolvedBoxModel::new(style, containing_width).with_nonnegative_padding();
    let (min, max) = box_content_intrinsic_widths(session, box_idx, None);
    let available =
        (containing_width - model.horizontal_margin() - model.horizontal_padding_border()).max(0.0);
    let content = match value {
        PreferredSize::MinContent => min,
        PreferredSize::MaxContent => max,
        PreferredSize::FitContent if intrinsic_inline_sizing => max,
        PreferredSize::FitContent => max.min(available.max(min)),
        _ => unreachable!(),
    };
    let border_box_inset = match style.box_sizing() {
        BoxSizing::ContentBox => 0.0,
        BoxSizing::BorderBox => model.horizontal_padding_border(),
    };
    Dimension::length(finite_f32(content + border_box_inset))
}

/// Stable sorting preserves source order for items with the same CSS order.
pub(super) fn ordered_children(session: &LayoutEngine<'_, '_>, children: &[u32]) -> Vec<u32> {
    let mut ordered = children.to_vec();
    ordered.sort_by_key(|&child| super::style::layout_style(session, child as usize).order);
    ordered
}
