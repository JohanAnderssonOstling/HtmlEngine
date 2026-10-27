use super::*;
use crate::layout::inline::fragments::TrackingCapacityIndex;
use crate::layout::inline::justification::tracking_width_limit;
use std::cell::{Cell, OnceCell};
#[cfg(any(test, feature = "testing"))]
use crate::layout::inline::fragments::measured_tracking_boundary_capacity;

/// Knuth–Plass optimal line breaking over the shared immutable token stream.
pub(in crate::layout::inline) fn break_lines_knuth(
    engine: &crate::layout::LayoutEngine<'_, '_>, tokens: &[InlineToken], runs: &[InlineTokenMetrics], paragraph: ParagraphLayout, available_width: f64, cached_plan: Option<&KpPlan>, ragged: bool, mut lines: Vec<BrokenLine>,
    scratch: &mut knuth_plass::CompactScratch,
) -> Vec<BrokenLine> {
    profile_scope!("lines.knuth_dispatch");
    let timing_started = engine.start_timing();
    use super::knuth_plass as kp;
    let fallback_plan;
    let plan = if let Some(plan) = cached_plan {
        plan
    } else {
        fallback_plan = KpPlan::build(tokens, runs, engine.config.book_optimized_text());
        &fallback_plan
    };
    let seg_count = plan.segments.len();
    lines.clear();
    let mut global_line = 0usize;

    for (seg_idx, segment) in plan.segments.iter().enumerate() {
        let (start, end) = (segment.token_start, segment.token_end);
        if start >= end {
            if seg_idx + 1 < seg_count {
                let indent = indent_for_line(
                    paragraph.indent.amount,
                    paragraph.indent.hanging,
                    (global_line == 0 && paragraph.indent.first_line) || paragraph.indent.each_line,
                );
                let empty_segment_break = tokens
                    .get(end)
                    .and_then(|token| empty_forced_break_index(token, end));
                if empty_segment_break.is_some() {
                    lines.push(BrokenLine {
                        start,
                        end,
                        empty_segment_break,
                        indent,
                        tab_origin: indent,
                        width_limit: (available_width - indent).max(0.0),
                        align: paragraph.text_align_last,
                        is_last_line: true,
                    });
                    global_line += 1;
                }
            }
            continue;
        }

        let seg_base_line = global_line;
        let line_width = |line: usize| (available_width - indent_for_line(paragraph.indent.amount, paragraph.indent.hanging, (seg_base_line + line == 0 && paragraph.indent.first_line) || paragraph.indent.each_line)).max(0.0);
        // Build boundary/metric summaries only when a candidate needs them.
        // Histories at one breakpoint often share the same token range.
        let tracking_index = OnceCell::new();
        let tracking_cache = Cell::new(None);
        let tracking_allowed = paragraph.text_align == TextAlign::Justify
            || (engine.config.force_justify() && paragraph.text_align == TextAlign::Left);
        let admit_tracking = |start: u32, end: u32, width: f64, required: f64| {
            if !tracking_allowed { return false; }
            let start = (start as usize).max(segment.token_start);
            let end = trim_trailing_line_whitespace(tokens, runs, start, end as usize);
            let boundary_capacity = match tracking_cache.get() {
                Some((cached_start, cached_end, capacity)) if cached_start == start && cached_end == end => capacity,
                _ => {
                    let index = tracking_index.get_or_init(|| TrackingCapacityIndex::new(
                        engine, &tokens[segment.token_start..segment.token_end], runs,
                    ));
                    let capacity = index.boundary_capacity(start - segment.token_start, end - segment.token_start);
                    #[cfg(any(test, feature = "testing"))]
                    assert_eq!(capacity, measured_tracking_boundary_capacity(engine, &tokens[start..end], runs),
                        "indexed capacity must match final-placement measurement");
                    tracking_cache.set(Some((start, end, capacity)));
                    capacity
                }
            };
            required <= tracking_width_limit(width).min(boundary_capacity)
        };
        let breaks = kp::break_compact_reusing_with_tracking(&segment.breakpoints, &line_width, KP_TOLERANCE, ragged, engine.config.hyphenation_quality(), &admit_tracking, scratch);
        let Some(breaks) = breaks else {
            return greedy::break_lines_into(engine, tokens, runs, paragraph, available_width, lines);
        };

        let break_count = breaks.len();
        let mut line_start = start;
        for (index, breakpoint_index) in breaks.iter().copied().enumerate() {
            let is_segment_last = index + 1 == break_count;
            let breakpoint = segment
                .breakpoints
                .get(breakpoint_index)
                .expect("compact Knuth-Plass breakpoint index is valid");
            let line_end =
                trim_trailing_line_whitespace(tokens, runs, line_start, breakpoint.line_end());
            let indent = indent_for_line(
                paragraph.indent.amount,
                paragraph.indent.hanging,
                (global_line == 0 && paragraph.indent.first_line) || paragraph.indent.each_line,
            );
            let (align, is_last_line) = if is_segment_last {
                (paragraph.text_align_last, true)
            } else {
                (paragraph.text_align, false)
            };
            if line_end > line_start {
                lines.push(BrokenLine {
                    start: line_start,
                    end: line_end,
                    empty_segment_break: None,
                    indent,
                    tab_origin: indent,
                    width_limit: (available_width - indent).max(0.0),
                    align,
                    is_last_line,
                });
                global_line += 1;
            }
            line_start = breakpoint.next_line_start();
        }
    }

    engine.record_timing(|timings| timings.break_lines_knuth += timing_started.elapsed());
    lines
}
