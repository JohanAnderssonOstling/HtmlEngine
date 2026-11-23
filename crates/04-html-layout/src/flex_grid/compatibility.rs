//! Corrections at the Taffy boundary, kept separate from CSS sizing policy.
use super::length_dimension;
use super::sizing::specified_to_content;
use super::sizing::{Axis, ContainerFacts, SizingPass};
use html_style_model::UsedPreferredSize as PreferredSize;
use html_style_model::{
    GridRepeatCount, LayoutStyle, UsedGridTemplateTrack, UsedGridTrackBreadth, UsedGridTrackSize,
    UsedStyleView,
};
use taffy::geometry::Size as TaffySize;
use taffy::prelude::{Dimension, Style};
use taffy::style::Display as TaffyDisplay;

use super::{TaffyContainerKind, tracks::fixed_auto_track_limit};
use crate::LayoutEngine;

pub fn grid_definite_inline_minimum_exceeds_track_limit(
    engine: &LayoutEngine<'_, '_>,
    container_idx: usize,
    children: &[u32],
    kind: TaffyContainerKind,
    containing_width: f64,
) -> bool {
    if kind != TaffyContainerKind::Grid {
        return false;
    }
    let size_contained_intrinsic_minimum = containing_width <= 0.01
        && engine.read.reader.style(container_idx).size_containment()
        && engine
            .read
            .reader
            .style(container_idx)
            .grid_template_columns()
            .any(|component| match component {
                UsedGridTemplateTrack::Single(track) => {
                    intrinsic_minimum_has_definite_maximum(track)
                }
                UsedGridTemplateTrack::Repeat { tracks, .. } => {
                    tracks.iter().any(intrinsic_minimum_has_definite_maximum)
                }
            });
    if size_contained_intrinsic_minimum {
        return true;
    }
    let Some(track_limit) = fixed_auto_track_limit(
        engine
            .read
            .reader
            .style(container_idx)
            .grid_template_columns(),
        containing_width,
    ) else {
        return false;
    };
    children.iter().any(|&child| {
        let style = engine.read.reader.style(child as usize);
        // Percentage item insets are cyclic at this track-sizing stage: their
        // definite lower-bound contribution is zero until the grid area is
        // known. Resolving them against the whole container would incorrectly
        // turn `minmax(auto, fixed)` into a fit-content track. A zero basis
        // retains fixed and calc-absolute terms without inventing a percentage
        // contribution from the container rather than the item grid area.
        let margin = style.get_horizontal_margin(0.0);
        let padding_border = style.get_horizontal_padding_border(0.0);
        let outer = |width: f32| match style.box_sizing() {
            html_style_model::BoxSizing::ContentBox => {
                width.max(0.0) as f64 + padding_border + margin
            }
            html_style_model::BoxSizing::BorderBox => {
                (width.max(0.0) as f64).max(padding_border) + margin
            }
        };
        // Even an auto-sized empty item has a definite outer lower bound:
        // its padding, border, and non-auto margins. Taffy otherwise loses
        // that contribution when every spanned `minmax(auto, fixed)` track
        // has a zero maximum.
        [style.width(), style.min_width(), PreferredSize::Px(0.0)]
            .into_iter()
            .filter_map(|size| match size {
                PreferredSize::Px(width) => Some(outer(width)),
                _ => None,
            })
            .any(|contribution| contribution > track_limit + 0.01)
    })
}

fn intrinsic_minimum_has_definite_maximum(track: UsedGridTrackSize) -> bool {
    matches!(
        track,
        UsedGridTrackSize::MinMax {
            min: UsedGridTrackBreadth::MinContent | UsedGridTrackBreadth::MaxContent,
            max: UsedGridTrackBreadth::Length(_),
        }
    )
}

/// An otherwise unconstrained 100% grid item is equivalent to the grid
/// item's default stretch behavior when the adapter needs to avoid a cyclic
/// percentage contribution.
pub(super) fn grid_percentage_stretch(
    style: UsedStyleView<'_>,
    layout: &LayoutStyle,
    kind: TaffyContainerKind,
) -> TaffySize<bool> {
    TaffySize { width: Axis::Horizontal, height: Axis::Vertical }.map(|axis| {
        kind == TaffyContainerKind::Grid
            && matches!(axis.select(style.width(), style.height()), PreferredSize::Percent(value) if (value - 1.0).abs() <= f32::EPSILON)
            && matches!(axis.select(style.min_width(), style.min_height()), PreferredSize::Auto)
            && matches!(axis.select(style.max_width(), style.max_height()), PreferredSize::Auto)
            && !axis.auto_margin(layout)
            && axis.select(style.margin_left(), style.margin_top()).is_zero()
            && axis.select(style.margin_right(), style.margin_bottom()).is_zero()
            && axis.select(style.padding_left(), style.padding_top()).is_zero()
            && axis.select(style.padding_right(), style.padding_bottom()).is_zero()
            && axis.select(style.border_left_width(), style.border_top_width()) == 0.0
            && axis.select(style.border_right_width(), style.border_bottom_width()) == 0.0
    })
}

/// Taffy 0.13 resolves percentages correctly for fixed and flexible tracks.
/// A percentage item in `minmax(auto, <fixed>)` still feeds the unresolved
/// percentage back into the intrinsic minimum, however. Keep the old
/// stretch-equivalent encoding only for that remaining Taffy cycle.
pub(super) fn grid_percentage_track_workaround(style: html_style_model::UsedStyleView<'_>) -> bool {
    style.grid_template_columns().any(|track| {
        matches!(
            track,
            UsedGridTemplateTrack::Single(UsedGridTrackSize::MinMax {
                min: UsedGridTrackBreadth::Auto,
                max: UsedGridTrackBreadth::Length(_),
            })
        )
    })
}

/// Preserve the interoperable intrinsic main-size behavior for an auto-height
/// single-line column whose items all have definite flex bases and minimums.
/// This is intentionally narrower than the general intrinsic main-size
/// algorithm: uncertain content-based minimums remain Taffy-owned.
pub(super) fn fixed_basis_auto_column_height(
    engine: &LayoutEngine<'_, '_>,
    parent: usize,
    children: &[u32],
    containing_width: f64,
    container: ContainerFacts,
) -> Option<f64> {
    if container.wrap != html_style_model::FlexWrap::NoWrap {
        return None;
    }
    let mut item_count = 0usize;
    let mut total = 0.0;
    for &child in children {
        let child_idx = child as usize;
        let style = engine.read.reader.style(child_idx);
        if style.position() == html_style_model::PositionMode::Absolute {
            continue;
        }
        let PreferredSize::Px(basis) = style.flex_basis() else {
            return None;
        };
        let PreferredSize::Px(minimum) = style.min_height() else {
            return None;
        };
        let padding_border = style.get_vertical_padding_border(containing_width);
        let as_content =
            |value: f32| specified_to_content(f64::from(value), padding_border, style.box_sizing());
        let minimum = as_content(minimum);
        let mut content = as_content(basis).max(minimum);
        if let PreferredSize::Px(maximum) = style.max_height() {
            content = content.min(as_content(maximum).max(minimum));
        } else if !matches!(style.max_height(), PreferredSize::Auto) {
            return None;
        }
        total += content
            + padding_border
            + style.margin_top().resolve(containing_width)
            + style.margin_bottom().resolve(containing_width);
        item_count += 1;
    }
    if item_count > 1 {
        total += engine
            .read
            .reader
            .style(parent)
            .row_gap()
            .resolve(containing_width)
            * (item_count - 1) as f64;
    }
    (item_count > 0).then_some(total.max(0.0))
}

pub(super) fn adjust_intrinsic_grid_root(
    engine: &mut LayoutEngine<'_, '_>,
    container_idx: usize,
    sizing_children: &[u32],
    available_width: Option<f64>,
    root_style: &mut Style,
) {
    let reader = engine.read.reader;
    let used = reader.style(container_idx);
    let mut has_auto_repeat = false;
    let mut has_intrinsic_min_fixed_max = false;
    for track in used.grid_template_columns() {
        match track {
            UsedGridTemplateTrack::Repeat {
                count: GridRepeatCount::AutoFill | GridRepeatCount::AutoFit,
                ..
            } => has_auto_repeat = true,
            UsedGridTemplateTrack::Single(track) => {
                has_intrinsic_min_fixed_max |= intrinsic_minimum_has_definite_maximum(track);
            }
            _ => {}
        }
        if has_auto_repeat && has_intrinsic_min_fixed_max {
            break;
        }
    }
    if !has_auto_repeat {
        return;
    }

    if available_width.is_some() || used.width().is_intrinsic_keyword() {
        root_style.min_size.width =
            intrinsic_grid_inline_constraint(used.min_width(), available_width, 0.0);
    }
    // An indefinite auto-repeat uses the container's definite
    // maximum to choose its repetition count. This applies to
    // fixed maxima as well as percentages resolved against an
    // available inline size.
    // Taffy's root represents our content box, so a border-box
    // maximum must first lose its fixed padding and borders.
    // Percentage padding remains cyclic and contributes zero at
    // this intrinsic-sizing stage.
    let padding_basis = if used.width().is_intrinsic_keyword() && available_width.is_some() {
        used.max_width()
            .resolve_definite(available_width)
            .unwrap_or(0.0)
    } else {
        0.0
    };
    let border_box_inset = if used.box_sizing() == html_style_model::BoxSizing::BorderBox {
        used.get_horizontal_padding_border(padding_basis)
    } else {
        0.0
    };
    root_style.max_size.width =
        intrinsic_grid_inline_constraint(used.max_width(), available_width, border_box_inset);
    // Taffy starts an indefinite auto-repeat with one repetition.
    // Preserve the definite contribution of an in-flow item as
    // the intrinsic sizing floor so Taffy can choose the repeat
    // count against the same inline size used by final layout.
    if has_intrinsic_min_fixed_max
        && matches!(
            used.width(),
            PreferredSize::MinContent | PreferredSize::MaxContent
        )
    {
        let use_max_contribution = !matches!(used.width(), PreferredSize::MinContent);
        let item_floor = sizing_children
            .iter()
            .filter(|&&child| !reader.box_facts(child as usize).absolute)
            .map(|&child| {
                let (minimum, maximum) =
                    crate::measurement::box_intrinsic_widths(engine, child as usize);
                if use_max_contribution {
                    maximum
                } else {
                    minimum
                }
            })
            .fold(0.0f64, f64::max);
        if item_floor > 0.0 {
            root_style.min_size.width = length_dimension(item_floor);
        }
    }
}

fn intrinsic_grid_inline_constraint(
    value: PreferredSize,
    available_width: Option<f64>,
    border_box_inset: f64,
) -> Dimension {
    // `content` historically contributes a zero length in this constraint.
    if matches!(value, PreferredSize::Content) {
        return length_dimension((-border_box_inset).max(0.0));
    }
    value
        .resolve_definite(available_width)
        .map_or_else(Dimension::auto, |size| {
            length_dimension((size - border_box_inset).max(0.0))
        })
}

/// Taffy dispatches every childless node through generic leaf layout even
/// when its display mode is Grid. A non-generated sentinel keeps empty grids
/// on the Grid path without occupying an auto-fit track.
pub(super) fn ensure_grid_algorithm_root(
    children: &mut Vec<(Style, Option<u32>)>,
    kind: TaffyContainerKind,
) {
    if kind == TaffyContainerKind::Grid && children.is_empty() {
        children.push((
            Style {
                display: TaffyDisplay::None,
                ..Style::default()
            },
            None,
        ));
    }
}

pub(super) fn intrinsic_column_ratio_basis(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    container: ContainerFacts,
    pass: SizingPass,
    aspect_ratio: Option<f32>,
) -> Option<Dimension> {
    let style = engine.read.reader.style(box_idx);
    let basis = (container.kind == TaffyContainerKind::Flex
        && pass.is_intrinsic()
        && !container.flex_row()
        && matches!(style.flex_basis(), PreferredSize::Auto)
        && matches!(style.height(), PreferredSize::Auto)
        && style.width().percentage_dependent())
    .then(|| {
        let ratio = aspect_ratio?;
        let (_, max_content_width) =
            crate::measurement::box_content_intrinsic_widths(engine, box_idx, None);
        (ratio > 0.0).then(|| length_dimension(max_content_width / f64::from(ratio)))
    })
    .flatten();
    basis
}
