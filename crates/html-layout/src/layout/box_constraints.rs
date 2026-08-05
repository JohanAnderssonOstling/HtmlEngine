use super::box_model::UsedBorderInsets;
use html_style_model::{TextDirection, UsedPreferredSize as PreferredSize, UsedStyleView, resolve_used_preferred_size};
use kurbo::Size;

/// `border_box_inset` is the padding+border to subtract from a specified size
/// under `box-sizing: border-box` (0 for `content-box`); `auto` is unaffected.
pub(crate) fn resolve_vertical_size(size: PreferredSize, parent_content_height: Option<f64>, border_box_inset: f64) -> Option<f64> {
    match size {
        PreferredSize::Auto | PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent => None,
        PreferredSize::Stretch => parent_content_height.map(|height| (height - border_box_inset).max(0.0)),
        PreferredSize::Px(px) => Some((px as f64 - border_box_inset).max(0.0)),
        PreferredSize::Percent(pct) => parent_content_height.map(|height| (height * pct as f64 - border_box_inset).max(0.0)),
        PreferredSize::Calc { absolute_px, percentage, percentage_dependent } => {
            if percentage_dependent {
                parent_content_height.map(|height| (absolute_px as f64 + height * percentage as f64 - border_box_inset).max(0.0))
            } else {
                Some((absolute_px as f64 - border_box_inset).max(0.0))
            }
        }
        PreferredSize::Comparison { .. } => {
            if size.percentage_dependent() {
                parent_content_height.map(|height| (resolve_used_preferred_size(size, 0.0, height) - border_box_inset).max(0.0))
            } else {
                Some((resolve_used_preferred_size(size, 0.0, 0.0) - border_box_inset).max(0.0))
            }
        }
    }
}

/// Resolve a block-axis minimum. Unlike `height` and `max-height`, an
/// unresolved percentage in `min-height` contributes zero rather than making
/// the entire value behave as `auto`. A mixed calc therefore retains its
/// absolute component when the containing block has an indefinite height.
pub(crate) fn resolve_vertical_min_size(size: PreferredSize, parent_content_height: Option<f64>, border_box_inset: f64) -> Option<f64> {
    match (size, parent_content_height) {
        (PreferredSize::Percent(_), None) => Some(0.0),
        (PreferredSize::Calc { absolute_px, percentage_dependent: true, .. }, None) => Some((absolute_px as f64 - border_box_inset).max(0.0)),
        (PreferredSize::Comparison { .. }, None) if size.percentage_dependent() => Some(0.0),
        _ => resolve_vertical_size(size, parent_content_height, border_box_inset),
    }
}

pub(super) fn resolve_content_width(width: PreferredSize, min_width: PreferredSize, max_width: PreferredSize, available_width: f64, border_box_inset: f64, auto_width: f64) -> f64 {
    let width = resolve_preferred_content_width(width, auto_width, available_width, border_box_inset);
    constrain_content_width(width, min_width, max_width, available_width, border_box_inset)
}

pub(super) fn constrain_content_width(width: f64, min_width: PreferredSize, max_width: PreferredSize, available_width: f64, border_box_inset: f64) -> f64 {
    let min_width = resolve_preferred_content_width(min_width, 0.0, available_width, border_box_inset);
    let max_width = resolve_preferred_content_width(max_width, f64::INFINITY, available_width, border_box_inset).max(min_width);
    width.clamp(min_width, max_width).max(0.0)
}

fn resolve_preferred_content_width(size: PreferredSize, auto_width: f64, available_width: f64, border_box_inset: f64) -> f64 {
    match size {
        PreferredSize::Auto => auto_width,
        PreferredSize::Stretch => (available_width - border_box_inset).max(0.0),
        _ => (resolve_used_preferred_size(size, auto_width, available_width) - border_box_inset).max(0.0),
    }
}

pub(super) fn resolve_block_margin_left(style: UsedStyleView<'_>, containing_direction: TextDirection, containing_width: f64, border_width: f64) -> f64 {
    let left = style.margin_left().resolve(containing_width);
    let right = style.margin_right().resolve(containing_width);
    let left_auto = style.margin_left_auto();
    let right_auto = style.margin_right_auto();

    // With an automatic width, the width absorbs the remaining inline space
    // and automatic margins resolve to zero.
    if matches!(style.width(), PreferredSize::Auto) {
        return if left_auto { 0.0 } else { left };
    }

    let remaining = containing_width - border_width - if left_auto { 0.0 } else { left } - if right_auto { 0.0 } else { right };
    match (left_auto, right_auto) {
        (true, true) if remaining >= 0.0 => remaining / 2.0,
        (true, true) => {
            if matches!(containing_direction, TextDirection::Rtl) {
                remaining
            } else {
                0.0
            }
        }
        (true, false) => remaining,
        (false, true) => left,
        (false, false) if matches!(containing_direction, TextDirection::Rtl) => left + remaining,
        (false, false) => left,
    }
}

#[derive(Clone, Copy)]
pub(super) struct AssignedBorderSize {
    pub(super) width: Option<f64>,
    pub(super) height: Option<f64>,
    pub(super) height_is_definite: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct BoxLayoutRequest {
    pub(super) box_idx: usize,
    pub(super) available_width: f64,
    /// Optional inline space left by adjacent floats. Percentages and margins
    /// still resolve against `available_width`; only automatic/fit-content
    /// sizing consumes this narrower limit.
    pub(super) auto_width_limit: Option<f64>,
    pub(super) parent_content_height: Option<f64>,
    pub(super) first_line_indent: f64,
    pub(super) assigned_border_size: Option<AssignedBorderSize>,
    pub(super) used_borders: Option<UsedBorderInsets>,
}

impl BoxLayoutRequest {
    pub(crate) fn normal(box_idx: usize, available_width: f64, parent_content_height: Option<f64>) -> Self {
        Self { box_idx, available_width, auto_width_limit: None, parent_content_height, first_line_indent: 0.0, assigned_border_size: None, used_borders: None }
    }

    pub(crate) fn with_auto_width_limit(mut self, auto_width_limit: f64) -> Self {
        self.auto_width_limit = Some(auto_width_limit.max(0.0));
        self
    }

    pub(crate) fn collapsed_table_cell(box_idx: usize, available_width: f64, parent_content_height: Option<f64>, used_borders: UsedBorderInsets) -> Self {
        Self { used_borders: Some(used_borders), ..Self::normal(box_idx, available_width, parent_content_height) }
    }

    pub(crate) fn assigned(box_idx: usize, containing_width: f64, assigned: Size) -> Self {
        Self {
            box_idx,
            available_width: containing_width,
            auto_width_limit: None,
            parent_content_height: Some(assigned.height),
            first_line_indent: 0.0,
            assigned_border_size: Some(AssignedBorderSize { width: Some(assigned.width), height: Some(assigned.height), height_is_definite: true }),
            used_borders: None,
        }
    }

    pub(crate) fn taffy_assigned(box_idx: usize, containing_width: f64, parent_content_height: Option<f64>, assigned: Size, height_is_definite: bool) -> Self {
        Self {
            box_idx,
            available_width: containing_width,
            auto_width_limit: None,
            parent_content_height,
            first_line_indent: 0.0,
            assigned_border_size: Some(AssignedBorderSize { width: Some(assigned.width), height: Some(assigned.height), height_is_definite }),
            used_borders: None,
        }
    }

    pub(crate) fn width_assigned(box_idx: usize, containing_width: f64, assigned_width: f64, parent_content_height: Option<f64>) -> Self {
        Self {
            box_idx,
            available_width: containing_width,
            auto_width_limit: None,
            parent_content_height,
            first_line_indent: 0.0,
            assigned_border_size: Some(AssignedBorderSize { width: Some(assigned_width), height: None, height_is_definite: false }),
            used_borders: None,
        }
    }

    pub(crate) fn absolute_assigned(box_idx: usize, containing_width: f64, containing_height: f64, assigned_width: Option<f64>, assigned_height: Option<f64>) -> Self {
        Self {
            box_idx,
            available_width: containing_width,
            auto_width_limit: None,
            parent_content_height: Some(containing_height),
            first_line_indent: 0.0,
            assigned_border_size: Some(AssignedBorderSize { width: assigned_width, height: assigned_height, height_is_definite: assigned_height.is_some() }),
            used_borders: None,
        }
    }
}
