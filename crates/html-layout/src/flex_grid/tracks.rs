use html_style_model::{ComputedStyles, GridPlacement, GridRepeatCount, UsedGridTemplateTrack, UsedGridTrackBreadth, UsedGridTrackSize, UsedLengthPct};
use taffy::prelude::{LengthPercentage, LengthPercentageAuto};
use taffy::style::{
    GridPlacement as TaffyGridPlacement, GridTemplateComponent as TaffyGridTemplateComponent, GridTemplateRepetition as TaffyGridTemplateRepetition, MaxTrackSizingFunction, MinTrackSizingFunction, RepetitionCount, TrackSizingFunction,
};

pub(super) fn line_names(styles: &ComputedStyles, source: &[Vec<html_style_model::StyleStringId>]) -> Vec<Vec<String>> {
    source.iter().map(|names| names.iter().filter_map(|&name| styles.string(name).map(str::to_owned)).collect()).collect()
}

pub(super) fn template_track(styles: &ComputedStyles, source: &UsedGridTemplateTrack<'_>, floor_fixed_auto_maximum: bool) -> TaffyGridTemplateComponent<String> {
    match source {
        UsedGridTemplateTrack::Single(size) => TaffyGridTemplateComponent::Single(track_size(size, floor_fixed_auto_maximum)),
        UsedGridTemplateTrack::Repeat { count, tracks, line_names: names } => {
            let auto_repeat = matches!(count, GridRepeatCount::AutoFill | GridRepeatCount::AutoFit);
            TaffyGridTemplateComponent::Repeat(TaffyGridTemplateRepetition {
                count: match count {
                    GridRepeatCount::Count(value) => RepetitionCount::Count(value.get()),
                    GridRepeatCount::AutoFill => RepetitionCount::AutoFill,
                    GridRepeatCount::AutoFit => RepetitionCount::AutoFit,
                    GridRepeatCount::Once => RepetitionCount::Count(1),
                },
                tracks: tracks.iter().map(|track| if auto_repeat { auto_repeat_track(&track, floor_fixed_auto_maximum) } else { track_size(&track, floor_fixed_auto_maximum) }).collect(),
                line_names: line_names(styles, names),
            })
        }
    }
}

fn auto_repeat_track(source: &UsedGridTrackSize, floor_fixed_auto_maximum: bool) -> TrackSizingFunction {
    // CSS Grid floors an auto-repeat track to a small UA-defined size when
    // counting repetitions. Supplying the suggested 1px floor keeps Taffy's
    // repetition input finite while allowing auto-fit to collapse empty tracks.
    match source {
        UsedGridTrackSize::Breadth(UsedGridTrackBreadth::Length(value)) if value.is_zero() => breadth_pair(UsedGridTrackBreadth::Length(UsedLengthPct::Px(1.0))),
        UsedGridTrackSize::MinMax { min: UsedGridTrackBreadth::Length(value), max } if value.is_zero() => TrackSizingFunction { min: MinTrackSizingFunction::length(1.0), max: max_breadth(*max) },
        _ => track_size(source, floor_fixed_auto_maximum),
    }
}

pub(super) fn track_size(source: &UsedGridTrackSize, floor_fixed_auto_maximum: bool) -> TrackSizingFunction {
    match source {
        UsedGridTrackSize::Auto => TrackSizingFunction { min: MinTrackSizingFunction::auto(), max: MaxTrackSizingFunction::auto() },
        UsedGridTrackSize::Breadth(value) => breadth_pair(*value),
        UsedGridTrackSize::MinMax { min, max } => TrackSizingFunction { min: min_breadth(*min), max: if floor_fixed_auto_maximum { max_breadth_with_auto_minimum_floor(*min, *max) } else { max_breadth(*max) } },
        UsedGridTrackSize::FitContent(value) => {
            let max = match value {
                UsedLengthPct::Px(value) => MaxTrackSizingFunction::fit_content_px(*value),
                UsedLengthPct::Pct(value) => MaxTrackSizingFunction::fit_content_percent(*value),
                UsedLengthPct::Calc { absolute_px, .. } => MaxTrackSizingFunction::fit_content_px(*absolute_px),
            };
            TrackSizingFunction { min: MinTrackSizingFunction::auto(), max }
        }
    }
}

pub(super) fn placement(styles: &ComputedStyles, source: GridPlacement) -> TaffyGridPlacement<String> {
    match source {
        GridPlacement::Auto => TaffyGridPlacement::Auto,
        GridPlacement::Line(index) => TaffyGridPlacement::Line(index.get().into()),
        GridPlacement::NamedLine { name, index } => styles.string(name).map(|name| TaffyGridPlacement::NamedLine(name.to_owned(), index)).unwrap_or(TaffyGridPlacement::Auto),
        GridPlacement::Span(count) => TaffyGridPlacement::Span(count.get()),
        GridPlacement::NamedSpan { name, count } => styles.string(name).map(|name| TaffyGridPlacement::NamedSpan(name.to_owned(), count.get())).unwrap_or(TaffyGridPlacement::Auto),
    }
}

pub(super) fn fixed_auto_track_limit<'a>(tracks: impl IntoIterator<Item = UsedGridTemplateTrack<'a>>, containing_width: f64) -> Option<f64> {
    fn track_limit(track: &UsedGridTrackSize, containing_width: f64) -> Option<f64> {
        match track {
            UsedGridTrackSize::MinMax { min: UsedGridTrackBreadth::Auto, max: UsedGridTrackBreadth::Length(UsedLengthPct::Px(value)) } => Some(value.max(0.0) as f64),
            UsedGridTrackSize::MinMax { min: UsedGridTrackBreadth::Auto, max: UsedGridTrackBreadth::Length(UsedLengthPct::Pct(value)) } => Some(containing_width.max(0.0) * value.max(0.0) as f64),
            _ => None,
        }
    }

    tracks.into_iter().try_fold(0.0, |total, component| match component {
        UsedGridTemplateTrack::Single(track) => Some(total + track_limit(&track, containing_width)?),
        UsedGridTemplateTrack::Repeat { count, tracks, .. } => {
            let repetitions = match count {
                GridRepeatCount::Count(value) => value.get() as f64,
                GridRepeatCount::Once => 1.0,
                GridRepeatCount::AutoFill | GridRepeatCount::AutoFit => return None,
            };
            let repeated = tracks.iter().try_fold(0.0, |sum, track| Some(sum + track_limit(&track, containing_width)?))?;
            Some(total + repetitions * repeated)
        }
    })
}

fn breadth_pair(value: UsedGridTrackBreadth) -> TrackSizingFunction {
    TrackSizingFunction { min: min_breadth(value), max: max_breadth(value) }
}

fn min_breadth(value: UsedGridTrackBreadth) -> MinTrackSizingFunction {
    match value {
        UsedGridTrackBreadth::Length(UsedLengthPct::Px(value)) => MinTrackSizingFunction::length(value),
        UsedGridTrackBreadth::Length(UsedLengthPct::Pct(value)) => MinTrackSizingFunction::percent(value),
        UsedGridTrackBreadth::Length(UsedLengthPct::Calc { absolute_px, .. }) => MinTrackSizingFunction::length(absolute_px),
        UsedGridTrackBreadth::Flex(_) => MinTrackSizingFunction::auto(),
        UsedGridTrackBreadth::MinContent => MinTrackSizingFunction::min_content(),
        UsedGridTrackBreadth::MaxContent => MinTrackSizingFunction::max_content(),
        UsedGridTrackBreadth::Auto => MinTrackSizingFunction::auto(),
    }
}

fn max_breadth(value: UsedGridTrackBreadth) -> MaxTrackSizingFunction {
    match value {
        UsedGridTrackBreadth::Length(UsedLengthPct::Px(value)) => MaxTrackSizingFunction::length(value),
        UsedGridTrackBreadth::Length(UsedLengthPct::Pct(value)) => MaxTrackSizingFunction::percent(value),
        UsedGridTrackBreadth::Length(UsedLengthPct::Calc { absolute_px, .. }) => MaxTrackSizingFunction::length(absolute_px),
        UsedGridTrackBreadth::Flex(value) => MaxTrackSizingFunction::fr(value),
        UsedGridTrackBreadth::MinContent => MaxTrackSizingFunction::min_content(),
        UsedGridTrackBreadth::MaxContent => MaxTrackSizingFunction::max_content(),
        UsedGridTrackBreadth::Auto => MaxTrackSizingFunction::auto(),
    }
}

fn max_breadth_with_auto_minimum_floor(min: UsedGridTrackBreadth, max: UsedGridTrackBreadth) -> MaxTrackSizingFunction {
    match (min, max) {
        (UsedGridTrackBreadth::Auto, UsedGridTrackBreadth::Length(UsedLengthPct::Px(value))) => MaxTrackSizingFunction::fit_content_px(value),
        (UsedGridTrackBreadth::Auto, UsedGridTrackBreadth::Length(UsedLengthPct::Pct(value))) => MaxTrackSizingFunction::fit_content_percent(value),
        (_, max) => max_breadth(max),
    }
}

pub(super) fn used_length_percentage(value: UsedLengthPct, percentage_basis: f64) -> LengthPercentage {
    match value {
        UsedLengthPct::Px(value) => LengthPercentage::length(value.max(0.0)),
        UsedLengthPct::Pct(value) => LengthPercentage::percent(value.max(0.0)),
        UsedLengthPct::Calc { absolute_px, percentage, .. } => LengthPercentage::length((absolute_px + percentage_basis as f32 * percentage).max(0.0)),
    }
}

pub(super) fn length_percentage_auto(value: UsedLengthPct, auto: bool, percentage_basis: f64) -> LengthPercentageAuto {
    if auto {
        return LengthPercentageAuto::auto();
    }
    match value {
        UsedLengthPct::Px(value) => LengthPercentageAuto::length(value),
        UsedLengthPct::Pct(value) => LengthPercentageAuto::percent(value),
        UsedLengthPct::Calc { absolute_px, percentage, .. } => LengthPercentageAuto::length(absolute_px + percentage_basis as f32 * percentage),
    }
}
