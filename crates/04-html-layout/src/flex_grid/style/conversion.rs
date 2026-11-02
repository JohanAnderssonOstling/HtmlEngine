use crate::layout::resolve_definite_size_value;
use html_style_model::{
    ContentAlignment, ItemAlignment, OverflowMode, UsedLengthPct,
    UsedPreferredSize as PreferredSize,
};
use taffy::prelude::{Dimension, LengthPercentageAuto};
use taffy::style::{
    AlignContent as TaffyAlignContent, AlignItems as TaffyAlignItems, Overflow as TaffyOverflow,
};

use super::super::TaffyContainerKind;
use super::super::tracks::length_percentage_auto;

pub fn dimension(value: PreferredSize) -> Dimension {
    match value {
        // Percentages remain symbolic for Taffy. Stretch participates through
        // alignment, so it must not become a percentage of an indefinite size.
        PreferredSize::Percent(value) => Dimension::percent(value.max(0.0)),
        PreferredSize::Auto
        | PreferredSize::Content
        | PreferredSize::MinContent
        | PreferredSize::MaxContent
        | PreferredSize::FitContent
        | PreferredSize::Stretch => Dimension::auto(),
        // Callers with a definite basis resolve percentage-dependent sizes
        // before this fallback; use the shared numeric resolver otherwise.
        PreferredSize::Px(_) | PreferredSize::Calc { .. } | PreferredSize::Comparison { .. } => {
            resolve_definite_size_value(value, None).map_or_else(Dimension::auto, |value| {
                Dimension::length(value.max(0.0) as f32)
            })
        }
    }
}

pub(super) fn absolute_grid_inset(
    value: Option<UsedLengthPct>,
    percentage_basis: f64,
    taffy_handles_absolute: bool,
    stretch_area: bool,
) -> LengthPercentageAuto {
    match value {
        Some(value) if taffy_handles_absolute => {
            length_percentage_auto(value, false, percentage_basis)
        }
        None if taffy_handles_absolute && stretch_area => LengthPercentageAuto::length(0.0),
        _ => LengthPercentageAuto::auto(),
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
    rtl: bool,
) -> Option<TaffyAlignContent> {
    Some(match value {
        ContentAlignment::Normal if kind == TaffyContainerKind::Flex && main_axis => {
            TaffyAlignContent::FLEX_START
        }
        ContentAlignment::Normal | ContentAlignment::Stretch => TaffyAlignContent::STRETCH,
        ContentAlignment::Start => TaffyAlignContent::START,
        ContentAlignment::End => TaffyAlignContent::END,
        ContentAlignment::Left => if rtl { TaffyAlignContent::END } else { TaffyAlignContent::START },
        ContentAlignment::Right => if rtl { TaffyAlignContent::START } else { TaffyAlignContent::END },
        ContentAlignment::FlexStart => TaffyAlignContent::FLEX_START,
        ContentAlignment::FlexEnd => TaffyAlignContent::FLEX_END,
        ContentAlignment::Center => TaffyAlignContent::CENTER,
        ContentAlignment::SpaceBetween => TaffyAlignContent::SPACE_BETWEEN,
        ContentAlignment::SpaceAround => TaffyAlignContent::SPACE_AROUND,
        ContentAlignment::SpaceEvenly => TaffyAlignContent::SPACE_EVENLY,
    })
}

pub(super) fn item_alignment(value: ItemAlignment, rtl: bool) -> Option<TaffyAlignItems> {
    Some(match value {
        ItemAlignment::Auto => return None,
        ItemAlignment::Normal | ItemAlignment::Stretch => TaffyAlignItems::STRETCH,
        ItemAlignment::Start => TaffyAlignItems::START,
        ItemAlignment::End => TaffyAlignItems::END,
        ItemAlignment::FlexStart => TaffyAlignItems::FLEX_START,
        ItemAlignment::Left | ItemAlignment::LegacyLeft => {
            if rtl {
                TaffyAlignItems::END
            } else {
                TaffyAlignItems::START
            }
        }
        ItemAlignment::Right | ItemAlignment::LegacyRight => {
            if rtl {
                TaffyAlignItems::START
            } else {
                TaffyAlignItems::END
            }
        }
        ItemAlignment::SelfStart => TaffyAlignItems::SELF_START,
        ItemAlignment::SelfEnd => TaffyAlignItems::SELF_END,
        ItemAlignment::FlexEnd => TaffyAlignItems::FLEX_END,
        ItemAlignment::Center | ItemAlignment::LegacyCenter => TaffyAlignItems::CENTER,
        ItemAlignment::Baseline => TaffyAlignItems::BASELINE,
    })
}
