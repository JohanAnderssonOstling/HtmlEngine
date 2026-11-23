//! Limits of the renderer's native grid AST conversion.
use lightningcss::properties::Property;
use lightningcss::properties::grid::{
    RepeatCount, TrackBreadth, TrackList, TrackListItem, TrackSize, TrackSizing,
};
pub(crate) fn reserved_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("span") || name.eq_ignore_ascii_case("auto")
}

pub(crate) fn native_value_is_valid(property: &Property<'_>) -> bool {
    match property {
        Property::GridTemplateRows(tracks) | Property::GridTemplateColumns(tracks) => {
            tracks_are_valid(tracks)
        }
        Property::GridTemplate(value) => {
            tracks_are_valid(&value.rows) && tracks_are_valid(&value.columns)
        }
        Property::Grid(value) => tracks_are_valid(&value.rows) && tracks_are_valid(&value.columns),
        _ => true,
    }
}

fn tracks_are_valid(value: &TrackSizing<'_>) -> bool {
    let TrackSizing::TrackList(list) = value else {
        return true;
    };
    !list
        .line_names
        .iter()
        .flatten()
        .any(|name| reserved_name(name.0.as_ref()))
        && list.items.iter().all(|item| match item {
            TrackListItem::TrackRepeat(repeat) => !repeat
                .line_names
                .iter()
                .flatten()
                .any(|name| reserved_name(name.0.as_ref())),
            _ => true,
        })
        && auto_repeat_count(list) <= 1
}

/// Check the complete auto-repeat shape before committing converted tracks.
pub(crate) fn auto_repeat_is_supported(list: &TrackList<'_>) -> bool {
    let count = auto_repeat_count(list);
    count <= 1
        && (count == 0
            || list.items.iter().all(|item| match item {
                TrackListItem::TrackSize(size) => native_fixed_size(size),
                TrackListItem::TrackRepeat(repeat) => {
                    repeat.track_sizes.iter().all(native_fixed_size)
                }
            }))
}

fn auto_repeat_count(list: &TrackList<'_>) -> usize {
    list.items
        .iter()
        .filter(|item| {
            matches!(item, TrackListItem::TrackRepeat(repeat)
            if matches!(repeat.count, RepeatCount::AutoFill | RepeatCount::AutoFit))
        })
        .count()
}

fn native_fixed_size(size: &TrackSize) -> bool {
    match size {
        TrackSize::TrackBreadth(TrackBreadth::Length(_)) => true,
        TrackSize::MinMax { min, max } => {
            matches!(min, TrackBreadth::Length(_)) || matches!(max, TrackBreadth::Length(_))
        }
        _ => false,
    }
}
