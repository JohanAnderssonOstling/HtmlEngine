use html_style_model::UsedPreferredSize as PreferredSize;

use super::super::{TaffyContainerKind, tracks::fixed_auto_track_limit};
use super::{LayoutEngine, layout_style};

pub(crate) fn grid_definite_inline_minimum_exceeds_track_limit(session: &LayoutEngine<'_, '_>, container_idx: usize, children: &[u32], kind: TaffyContainerKind, containing_width: f64) -> bool {
    if kind != TaffyContainerKind::Grid {
        return false;
    }
    let size_contained_intrinsic_minimum = containing_width <= 0.01
        && session.reader.style(container_idx).size_containment()
        && session.reader.style(container_idx).grid_template_columns().any(|component| match component {
            html_style_model::UsedGridTemplateTrack::Single(track) => intrinsic_minimum_has_definite_maximum(track),
            html_style_model::UsedGridTemplateTrack::Repeat { tracks, .. } => tracks.iter().any(intrinsic_minimum_has_definite_maximum),
        });
    if size_contained_intrinsic_minimum {
        return true;
    }
    let Some(track_limit) = fixed_auto_track_limit(session.reader.style(container_idx).grid_template_columns(), containing_width) else {
        return false;
    };
    children.iter().any(|&child| {
        let style = session.reader.style(child as usize);
        // Percentage item insets are cyclic at this track-sizing stage: their
        // definite lower-bound contribution is zero until the grid area is
        // known. Resolving them against the whole container would incorrectly
        // turn `minmax(auto, fixed)` into a fit-content track. A zero basis
        // retains fixed and calc-absolute terms without inventing a percentage
        // contribution from the container rather than the item grid area.
        let margin = style.get_horizontal_margin(0.0);
        let padding_border = style.get_horizontal_padding(0.0) + style.border_left_width() as f64 + style.border_right_width() as f64;
        let outer = |width: f32| match style.box_sizing() {
            html_style_model::BoxSizing::ContentBox => width.max(0.0) as f64 + padding_border + margin,
            html_style_model::BoxSizing::BorderBox => (width.max(0.0) as f64).max(padding_border) + margin,
        };
        let preferred = match style.width() {
            PreferredSize::Px(width) => Some(outer(width)),
            _ => None,
        };
        let minimum = match style.min_width() {
            PreferredSize::Px(width) => Some(outer(width)),
            _ => None,
        };
        // Even an auto-sized empty item has a definite outer lower bound:
        // its padding, border, and non-auto margins. Taffy otherwise loses
        // that contribution when every spanned `minmax(auto, fixed)` track
        // has a zero maximum.
        let outer_insets = outer(0.0);
        preferred.into_iter().chain(minimum).chain(std::iter::once(outer_insets)).any(|contribution| contribution > track_limit + 0.01)
    })
}

fn intrinsic_minimum_has_definite_maximum(track: html_style_model::UsedGridTrackSize) -> bool {
    matches!(
        track,
        html_style_model::UsedGridTrackSize::MinMax {
            min: html_style_model::UsedGridTrackBreadth::MinContent | html_style_model::UsedGridTrackBreadth::MaxContent,
            max: html_style_model::UsedGridTrackBreadth::Length(_),
        }
    )
}

/// An otherwise unconstrained 100% grid item is equivalent to the grid
/// item's default stretch behavior when the adapter needs to avoid a cyclic
/// percentage contribution.
pub(super) fn grid_percentage_width_is_stretch_equivalent(session: &LayoutEngine<'_, '_>, box_idx: usize, kind: TaffyContainerKind) -> bool {
    if kind != TaffyContainerKind::Grid {
        return false;
    }
    let style = session.reader.style(box_idx);
    let layout = layout_style(session, box_idx);
    matches!(style.width(), PreferredSize::Percent(value) if (value - 1.0).abs() <= f32::EPSILON)
        && matches!(style.min_width(), PreferredSize::Auto)
        && matches!(style.max_width(), PreferredSize::Auto)
        && !layout.margin_left_auto
        && !layout.margin_right_auto
        && style.margin_left().is_zero()
        && style.margin_right().is_zero()
        && style.padding_left().is_zero()
        && style.padding_right().is_zero()
        && style.border_left_width() == 0.0
        && style.border_right_width() == 0.0
}

/// Taffy 0.13 resolves percentages correctly for fixed and flexible tracks.
/// A percentage item in `minmax(auto, <fixed>)` still feeds the unresolved
/// percentage back into the intrinsic minimum, however. Keep the old
/// stretch-equivalent encoding only for that remaining Taffy cycle.
pub(super) fn grid_percentage_width_needs_intrinsic_track_workaround(session: &LayoutEngine<'_, '_>, box_idx: usize) -> bool {
    let Some(parent) = session.reader.get_parent(box_idx) else {
        return false;
    };
    session.reader.style(parent).grid_template_columns().any(|track| {
        matches!(
            track,
            html_style_model::UsedGridTemplateTrack::Single(html_style_model::UsedGridTrackSize::MinMax {
                min: html_style_model::UsedGridTrackBreadth::Auto,
                max: html_style_model::UsedGridTrackBreadth::Length(_),
            })
        )
    })
}

pub(super) fn grid_percentage_height_is_stretch_equivalent(session: &LayoutEngine<'_, '_>, box_idx: usize, kind: TaffyContainerKind) -> bool {
    if kind != TaffyContainerKind::Grid {
        return false;
    }
    let style = session.reader.style(box_idx);
    let layout = layout_style(session, box_idx);
    matches!(style.height(), PreferredSize::Percent(value) if (value - 1.0).abs() <= f32::EPSILON)
        && matches!(style.min_height(), PreferredSize::Auto)
        && matches!(style.max_height(), PreferredSize::Auto)
        && !layout.margin_top_auto
        && !layout.margin_bottom_auto
        && style.margin_top().is_zero()
        && style.margin_bottom().is_zero()
        && style.padding_top().is_zero()
        && style.padding_bottom().is_zero()
        && style.border_top_width() == 0.0
        && style.border_bottom_width() == 0.0
}

