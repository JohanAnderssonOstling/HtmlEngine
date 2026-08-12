use html_style_model::{
    ContentAlignment, ItemAlignment, OverflowMode, UsedPreferredSize as PreferredSize,
};
use taffy::prelude::Dimension;
use taffy::style::{
    AlignContent as TaffyAlignContent, AlignItems as TaffyAlignItems, Overflow as TaffyOverflow,
};

use super::super::TaffyContainerKind;
use super::super::tracks::length_percentage_auto;

pub(crate) fn dimension(value: PreferredSize) -> Dimension {
    match value {
        PreferredSize::Auto
        | PreferredSize::MinContent
        | PreferredSize::MaxContent
        | PreferredSize::FitContent => Dimension::auto(),
        // Stretch participates through flex/grid alignment; encoding it as
        // 100% would resolve an indefinite cross size against the container.
        PreferredSize::Stretch => Dimension::auto(),
        PreferredSize::Px(value) => Dimension::length(value.max(0.0)),
        PreferredSize::Percent(value) => Dimension::percent(value.max(0.0)),
        // Callers with a definite percentage basis resolve calc before this
        // fallback. With an indefinite basis, only the absolute term remains.
        PreferredSize::Calc {
            absolute_px,
            percentage_dependent: false,
            ..
        } => Dimension::length(absolute_px.max(0.0)),
        PreferredSize::Calc {
            percentage_dependent: true,
            ..
        } => Dimension::auto(),
        PreferredSize::Comparison { .. } if value.percentage_dependent() => Dimension::auto(),
        PreferredSize::Comparison { .. } => Dimension::length(
            html_style_model::resolve_used_preferred_size(value, 0.0, 0.0).max(0.0) as f32,
        ),
    }
}
fn optional_length_percentage_auto(
    value: Option<html_style_model::UsedLengthPct>,
    percentage_basis: f64,
) -> taffy::prelude::LengthPercentageAuto {
    value.map_or_else(taffy::prelude::LengthPercentageAuto::auto, |value| {
        length_percentage_auto(value, false, percentage_basis)
    })
}

pub(super) fn absolute_grid_inset(
    value: Option<html_style_model::UsedLengthPct>,
    percentage_basis: f64,
    taffy_handles_absolute: bool,
    stretch_area: bool,
) -> taffy::prelude::LengthPercentageAuto {
    if !taffy_handles_absolute {
        return taffy::prelude::LengthPercentageAuto::auto();
    }
    if stretch_area && value.is_none() {
        taffy::prelude::LengthPercentageAuto::length(0.0)
    } else {
        optional_length_percentage_auto(value, percentage_basis)
    }
}

pub(super) fn overflow(value: OverflowMode) -> TaffyOverflow {
    match value {
        OverflowMode::Visible => TaffyOverflow::Visible,
        OverflowMode::Clip => TaffyOverflow::Clip,
        OverflowMode::Hidden | OverflowMode::Auto => TaffyOverflow::Hidden,
        OverflowMode::Scroll => TaffyOverflow::Scroll,
    }
}

pub(super) fn content_alignment(
    value: ContentAlignment,
    kind: TaffyContainerKind,
    main_axis: bool,
) -> Option<TaffyAlignContent> {
    Some(match value {
        ContentAlignment::Normal if kind == TaffyContainerKind::Flex && main_axis => {
            TaffyAlignContent::FLEX_START
        }
        ContentAlignment::Normal => TaffyAlignContent::STRETCH,
        ContentAlignment::Start => TaffyAlignContent::START,
        ContentAlignment::End => TaffyAlignContent::END,
        ContentAlignment::FlexStart => TaffyAlignContent::FLEX_START,
        ContentAlignment::FlexEnd => TaffyAlignContent::FLEX_END,
        ContentAlignment::Center => TaffyAlignContent::CENTER,
        ContentAlignment::Stretch => TaffyAlignContent::STRETCH,
        ContentAlignment::SpaceBetween => TaffyAlignContent::SPACE_BETWEEN,
        ContentAlignment::SpaceAround => TaffyAlignContent::SPACE_AROUND,
        ContentAlignment::SpaceEvenly => TaffyAlignContent::SPACE_EVENLY,
    })
}

pub(super) fn item_alignment(
    value: ItemAlignment,
    _grid_inline_axis: bool,
) -> Option<TaffyAlignItems> {
    match value {
        ItemAlignment::Auto => None,
        ItemAlignment::Normal => Some(TaffyAlignItems::STRETCH),
        ItemAlignment::Start | ItemAlignment::SelfStart | ItemAlignment::Left => {
            Some(TaffyAlignItems::START)
        }
        ItemAlignment::End | ItemAlignment::SelfEnd | ItemAlignment::Right => {
            Some(TaffyAlignItems::END)
        }
        ItemAlignment::FlexStart => Some(TaffyAlignItems::FLEX_START),
        ItemAlignment::FlexEnd => Some(TaffyAlignItems::FLEX_END),
        ItemAlignment::Center => Some(TaffyAlignItems::CENTER),
        ItemAlignment::Stretch => Some(TaffyAlignItems::STRETCH),
        ItemAlignment::Baseline => Some(TaffyAlignItems::BASELINE),
    }
}
