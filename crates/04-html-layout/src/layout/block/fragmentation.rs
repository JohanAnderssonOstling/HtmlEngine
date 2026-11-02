use super::PreviousBlock;
use crate::layout::{LayoutEngine, OutputRanges};
use html_style_model::{BreakBetween, BreakInside};
use kurbo::{Point, Size, Vec2};

pub(super) fn forced_break_offset(
    rule: BreakBetween,
    edge: f64,
    fragment_height: Option<f64>,
) -> f64 {
    match fragment_height {
        Some(height) if rule.is_forced() => next_fragment_delta(edge, height),
        _ => 0.0,
    }
}

pub(super) fn offset_after_layout(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    size: Size,
    output: &OutputRanges,
    previous: Option<&PreviousBlock>,
    fragment_height: Option<f64>,
    point: &mut Point,
) -> f64 {
    let Some(fragment_height) = fragment_height else {
        return 0.0;
    };
    let style = engine.reader.style(box_idx);
    let mut offset = 0.0;

    if let Some(previous) = previous
        && keep_with_previous(style.break_before(), previous.break_after)
    {
        let required_bottom = point.y + size.height;
        let required_height = required_bottom - previous.top;
        if required_bottom > previous.top
            && required_height <= fragment_height
            && crosses_fragment(previous.top, required_bottom, fragment_height)
        {
            let delta = next_fragment_delta(previous.top, fragment_height);
            let translation = Vec2::new(0.0, delta);
            engine.placement.translate(previous.placement, translation);
            engine
                .placement
                .translate(output.placement.box_group, translation);
            *point += translation;
            offset += delta;
        }
    }

    let child_bottom = point.y + size.height;
    let line_count = output.lines.len();
    let violates_line_limits =
        if line_count > 1 && crosses_fragment(point.y, child_bottom, fragment_height) {
            let boundary = ((point.y / fragment_height).floor() + 1.0) * fragment_height;
            let before = engine
                .fragments
                .lines_before_y(output.lines.clone(), boundary - 0.01);
            let after = line_count.saturating_sub(before);
            before > 0
                && after > 0
                && (before < style.orphans() as usize || after < style.widows() as usize)
        } else {
            false
        };
    if (style.break_inside() == BreakInside::Avoid || violates_line_limits)
        && size.height <= fragment_height
        && crosses_fragment(point.y, child_bottom, fragment_height)
    {
        let delta = next_fragment_delta(point.y, fragment_height);
        let translation = Vec2::new(0.0, delta);
        engine
            .placement
            .translate(output.placement.box_group, translation);
        *point += translation;
        offset += delta;
    }
    offset
}

fn keep_with_previous(before: BreakBetween, after: BreakBetween) -> bool {
    !before.is_forced()
        && !after.is_forced()
        && (before == BreakBetween::Avoid || after == BreakBetween::Avoid)
}

fn next_fragment_delta(y: f64, fragment_height: f64) -> f64 {
    let local = y.rem_euclid(fragment_height);
    if local <= 0.01 || fragment_height - local <= 0.01 {
        0.0
    } else {
        fragment_height - local
    }
}

fn crosses_fragment(top: f64, bottom: f64, fragment_height: f64) -> bool {
    (top / fragment_height).floor() != ((bottom - 0.01).max(top) / fragment_height).floor()
}
