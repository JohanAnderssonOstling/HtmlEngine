//! Intrinsic item dimensions and flex automatic minimum sizing.

use super::length_dimension;
use super::sizing::{ContainerFacts, SizingPass, border_to_specified, content_to_specified};
use super::style::dimension;
use crate::measurement::{box_content_intrinsic_widths, measure_box_isolated};
use crate::{
    BoxLayoutRequest, LayoutEngine, ReplacedMainAxis, ReplacedSizeInput, preferred_aspect_ratio,
    resolve_definite_content_size, resolve_replaced_flex_auto_min_main_size,
};
use html_style_model::{BoxSizing, FlexWrap, UsedPreferredSize as PreferredSize};
use taffy::prelude::Dimension;

/// Taffy represents `flex-basis` as only auto, length, or percentage. CSS
/// intrinsic sizing keywords therefore have to be resolved at the adapter
/// boundary instead of being collapsed to `auto` (content/max-content).
pub(super) fn resolve_intrinsic_flex_basis(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    containing_width: f64,
    containing_height: Option<f64>,
    container: ContainerFacts,
) -> Option<Dimension> {
    let style = engine.read.reader.style(box_idx);
    let sizes = engine.read.reader.sizes(box_idx);
    let mut basis = sizes.flex_basis;
    if matches!(basis, PreferredSize::Stretch) {
        let available = if container.flex_row() {
            Some(containing_width)
        } else {
            containing_height
        };
        if let Some(available) = available {
            let model = engine
                .read
                .reader
                .box_model(box_idx, containing_width)
                .with_nonnegative_padding();
            let value = container
                .main_axis
                .stretch(Some(available), model, style.box_sizing())?;
            return Some(length_dimension(value));
        }
        // An indefinite stretch basis resolves to content rather than using
        // the item's authored main size through the `auto` basis path.
        basis = PreferredSize::Content;
    }
    if matches!(basis, PreferredSize::Content) {
        if let Some(value) = content_ratio_basis(
            engine,
            box_idx,
            containing_width,
            containing_height,
            container,
        ) {
            return Some(value);
        }
        basis = PreferredSize::MaxContent;
    }
    if !basis.is_intrinsic_keyword() {
        return None;
    }
    if container.flex_row() {
        return Some(taffy_item_width_dimension(
            engine,
            box_idx,
            basis,
            SizingPass::Final(containing_width),
        ));
    }

    // Intrinsic block-axis sizes are measured with the item's allocated
    // cross size. This is observable when content wraps at that width.
    let measured = measure_box_isolated(
        engine,
        BoxLayoutRequest::normal(box_idx, containing_width.max(0.0), None)
            .for_intrinsic_block_measurement(),
    )
    .size_with_floats();
    let model = engine
        .read
        .reader
        .box_model(box_idx, containing_width)
        .with_nonnegative_padding();
    let vertical_inset = model.vertical_padding_border();
    let basis = border_to_specified(measured.height, vertical_inset, style.box_sizing());
    Some(length_dimension(basis))
}

fn content_ratio_basis(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    containing_width: f64,
    containing_height: Option<f64>,
    container: ContainerFacts,
) -> Option<Dimension> {
    let style = engine.read.reader.style(box_idx);
    let sizes = engine.read.reader.sizes(box_idx);
    let intrinsic_ratio = engine
        .read
        .reader
        .image_intrinsic(box_idx)
        .and_then(|image| image.aspect_ratio);
    let authored = style.aspect_ratio();
    let ratio = preferred_aspect_ratio(authored, intrinsic_ratio)?;
    let model = engine
        .read
        .reader
        .box_model(box_idx, containing_width)
        .with_nonnegative_padding();
    let layout = engine.read.reader.layout_style(box_idx);
    let cross_axis = container.main_axis.cross();
    let cross_size = cross_axis.select(sizes.width, sizes.height);
    let cross_basis = cross_axis.select(Some(containing_width), containing_height);
    let cross_inset = cross_axis.inset(model);
    let main_inset = container.main_axis.inset(model);
    let resolve = |size| {
        resolve_definite_content_size(
            size,
            cross_basis,
            cross_axis.margin(model),
            cross_inset,
            style.box_sizing(),
        )
    };
    let mut cross = resolve(cross_size).or_else(|| {
        (matches!(cross_size, PreferredSize::Auto)
            && container.wrap == FlexWrap::NoWrap
            && container.cross_axis_stretches(layout))
        .then(|| resolve(PreferredSize::Stretch))
        .flatten()
    })?;
    let minimum = resolve(cross_axis.select(sizes.min_width, sizes.min_height)).unwrap_or(0.0);
    let maximum = resolve(cross_axis.select(sizes.max_width, sizes.max_height))
        .unwrap_or(f64::INFINITY)
        .max(minimum);
    cross = cross.clamp(minimum, maximum);
    // Intrinsic image ratios refer to the content box; an authored ratio
    // follows box-sizing, including border-box ratios on ordinary elements.
    let border_ratio = style.box_sizing() == BoxSizing::BorderBox
        && !(authored.uses_intrinsic() && intrinsic_ratio.is_some());
    let cross = cross + if border_ratio { cross_inset } else { 0.0 };
    let main = if container.flex_row() {
        cross * ratio
    } else {
        cross / ratio
    };
    let content = (main - if border_ratio { main_inset } else { 0.0 }).max(0.0);
    let basis = content_to_specified(content, main_inset, style.box_sizing());
    Some(length_dimension(basis))
}

/// Resolve the flex-item automatic minimum from the content-size
/// suggestion, capped by a definite preferred size. Feeding Taffy `auto`
/// here makes its generic measurement path treat a specified width as the
/// minimum itself, which prevents empty fixed-size items from shrinking
/// and gives percentage-sized tables no usable share of the flex line.
pub(super) fn flex_automatic_min_content_width(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    containing_width: f64,
) -> f64 {
    let style = engine.read.reader.style(box_idx);
    let sizes = engine.read.reader.sizes(box_idx);
    if style.overflow_x().is_scrollable() {
        return 0.0;
    }

    let model = engine
        .read
        .reader
        .box_model(box_idx, containing_width)
        .with_nonnegative_padding();
    let (content_min, content_max) = box_content_intrinsic_widths(engine, box_idx, None);
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
    let preferred = match sizes.width {
        PreferredSize::MinContent => Some(content_min),
        PreferredSize::MaxContent | PreferredSize::FitContent => Some(content_max.max(content_min)),
        value => resolve_definite(value),
    };
    let mut automatic_min = preferred.map_or(content_min, |preferred| content_min.min(preferred));
    if matches!(
        sizes.max_width,
        PreferredSize::Px(_) | PreferredSize::Percent(_) | PreferredSize::Stretch
    ) && let Some(maximum) = resolve_definite(sizes.max_width)
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
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    containing_width: f64,
    containing_height: Option<f64>,
    container: ContainerFacts,
) -> Option<f64> {
    let image_intrinsic = engine.read.reader.image_intrinsic(box_idx)?;
    let intrinsic = image_intrinsic.size;
    let style = engine.read.reader.style(box_idx);
    let sizes = engine.read.reader.sizes(box_idx);
    let model = engine
        .read
        .reader
        .box_model(box_idx, containing_width)
        .with_nonnegative_padding();
    let layout = engine.read.reader.layout_style(box_idx);
    let scrollable_main = if container.flex_row() {
        style.overflow_x()
    } else {
        style.overflow_y()
    };
    if scrollable_main.is_scrollable() {
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
    let cross_axis = container.main_axis.cross();
    let cross_size = cross_axis.select(sizes.width, sizes.height);
    let cross_basis = cross_axis.select(Some(containing_width), containing_height);
    let stretch_cross_size = matches!(cross_size, PreferredSize::Auto)
        && container.cross_axis_stretches(layout)
        && cross_basis.is_some();
    Some(resolve_replaced_flex_auto_min_main_size(
        input,
        container
            .main_axis
            .select(ReplacedMainAxis::Horizontal, ReplacedMainAxis::Vertical),
        stretch_cross_size,
    ))
}

pub(super) fn taffy_item_width_dimension(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    value: PreferredSize,
    pass: SizingPass,
) -> Dimension {
    if let PreferredSize::Calc {
        absolute_px,
        percentage,
        ..
    } = value
    {
        return length_dimension((absolute_px as f64 + pass.width() * percentage as f64).max(0.0));
    }
    if !value.is_intrinsic_keyword() {
        return dimension(value);
    }
    let style = engine.read.reader.style(box_idx);
    let model = engine
        .read
        .reader
        .box_model(box_idx, pass.width())
        .with_nonnegative_padding();
    let (min, max) = box_content_intrinsic_widths(engine, box_idx, None);
    let available =
        (pass.width() - model.horizontal_margin() - model.horizontal_padding_border()).max(0.0);
    let content = match value {
        PreferredSize::MinContent => min,
        PreferredSize::MaxContent => max,
        PreferredSize::FitContent if pass.is_intrinsic() => max,
        PreferredSize::FitContent => max.min(available.max(min)),
        _ => unreachable!(),
    };
    length_dimension(content_to_specified(
        content,
        model.horizontal_padding_border(),
        style.box_sizing(),
    ))
}

/// Stable sorting preserves source order for items with the same CSS order.
pub(super) fn ordered_children(engine: &LayoutEngine<'_, '_>, children: &[u32]) -> Vec<u32> {
    let mut ordered = children.to_vec();
    ordered.sort_by_key(|&child| engine.read.reader.layout_style(child as usize).order);
    ordered
}
