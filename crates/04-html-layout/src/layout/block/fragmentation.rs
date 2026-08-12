use crate::layout::translate_laid_out_output;
use crate::layout::{LayoutEngine, OutputRanges};
use html_style_model::{BreakBetween, BreakInside};
use kurbo::{Point, Size, Vec2};

pub(super) struct PreviousBlock {
    point: Point,
    output: OutputRanges,
    keep_with_next: bool,
    forced_break_after: bool,
}

pub(super) fn offset_before_layout(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    top: f64,
    fragment_height: Option<f64>,
) -> f64 {
    if engine.config.book_optimized_text() {
        return 0.0;
    }
    let Some(fragment_height) = fragment_height else {
        return 0.0;
    };
    let break_before = engine.reader.style(box_idx).break_before();
    if break_before.is_forced() {
        return next_fragment_delta(top, fragment_height);
    }
    0.0
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
    if engine.config.book_optimized_text() {
        return 0.0;
    }
    let Some(fragment_height) = fragment_height else {
        return 0.0;
    };
    let mut offset = 0.0;

    if let Some(previous) = previous
        && keep_with_previous(engine, box_idx, previous)
    {
        let group_bottom = point.y + size.height;
        let required_bottom = group_bottom;
        let required_height = required_bottom - previous.point.y;
        if required_bottom > previous.point.y
            && required_height <= fragment_height
            && crosses_fragment(previous.point.y, required_bottom, fragment_height)
        {
            let delta = next_fragment_delta(previous.point.y, fragment_height);
            let translation = Vec2::new(0.0, delta);
            translate_laid_out_output(engine, &previous.output, translation);
            translate_laid_out_output(engine, output, translation);
            *point += translation;
            offset += delta;
        }
    }

    let style = engine.reader.style(box_idx);
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
        translate_laid_out_output(engine, output, translation);
        *point += translation;
        offset += delta;
    }
    offset
}

pub(super) fn offset_for_forced_break_after(
    break_after: BreakBetween,
    bottom: f64,
    fragment_height: Option<f64>,
    book_optimized: bool,
) -> f64 {
    if book_optimized {
        return 0.0;
    }
    match (break_after, fragment_height) {
        (break_after, Some(fragment_height)) if break_after.is_forced() => {
            next_fragment_delta(bottom, fragment_height)
        }
        _ => 0.0,
    }
}

pub(super) fn make_previous_block(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    point: Point,
    output: OutputRanges,
) -> PreviousBlock {
    let break_after = engine.reader.style(box_idx).break_after();
    PreviousBlock {
        point,
        output,
        keep_with_next: break_after == BreakBetween::Avoid,
        forced_break_after: break_after.is_forced(),
    }
}

fn keep_with_previous(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    previous: &PreviousBlock,
) -> bool {
    let break_before = engine.reader.style(box_idx).break_before();
    if break_before.is_forced() || previous.forced_break_after {
        false
    } else if break_before == BreakBetween::Avoid {
        true
    } else {
        previous.keep_with_next
    }
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
