use super::*;
use crate::layout::inline::fragments::TrackingCapacityIndex;
use crate::layout::inline::justification::tracking_width_limit;
use crate::layout::inline::hanging;
#[cfg(any(test, feature = "testing"))]
use crate::layout::inline::fragments::measured_tracking_boundary_capacity;

/// Knuth–Plass optimal line breaking over the shared immutable token stream.
pub(in crate::layout::inline) fn break_lines_knuth(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    tokens: TokenSpan<'_>,
    runs: &[InlineTokenMetrics],
    metrics: MetricsView<'_>,
    paragraph: ParagraphLayout,
    available_width: f64,
    cached_plan: Option<&KpPlan>,
    ragged: bool,
    mut lines: Vec<BrokenLine>,
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
    let hanging_start = hanging::first(engine, tokens, paragraph);
    let hanging_end = hanging::last(engine, tokens, paragraph);
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
                let empty_segment_break = (end < tokens.len())
                    .then(|| tokens.metadata_at(end))
                    .and_then(|token| empty_forced_break_index(&token, end));
                if empty_segment_break.is_some() {
                    lines.push(BrokenLine {
                        start,
                        end,
                        empty_segment_break,
                        hyphen: None,
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
        // These constraints depend only on whether the segment's first line
        // is also the paragraph's first line. Resolve them once rather than
        // rereading indent policy and subtracting it for every active node.
        let first_indent = indent_for_line(
            paragraph.indent.amount,
            paragraph.indent.hanging,
            (seg_base_line == 0 && paragraph.indent.first_line) || paragraph.indent.each_line,
        );
        let later_indent = indent_for_line(
            paragraph.indent.amount,
            paragraph.indent.hanging,
            paragraph.indent.each_line,
        );
        let first_hang = if seg_base_line == 0 { hanging_start } else { 0.0 };
        let first_width = (available_width - first_indent).max(0.0) + first_hang;
        let later_width = (available_width - later_indent).max(0.0);
        let line_width = move |line: usize| {
            if line == 0 { first_width } else { later_width }
        };
        // Build boundary/metric summaries only when a candidate needs them.
        // Histories at one breakpoint often share the same token range.
        let mut tracking_index = None;
        let mut tracking_cache = None;
        let tracking_allowed = paragraph.text_align == TextAlign::Justify
            || (engine.config.force_justify() && paragraph.text_align == TextAlign::Left);
        let mut admit_tracking = |start: u32, end: u32, width: f64, required: f64| {
            if hanging_end.is_some_and(|(index, hang)| end as usize > index && required <= hang) {
                return true;
            }
            if !tracking_allowed {
                return false;
            }
            let start = (start as usize).max(segment.token_start);
            let end = (end as usize).max(start);
            let boundary_capacity = match tracking_cache {
                Some((cached_start, cached_end, capacity))
                    if cached_start == start && cached_end == end =>
                {
                    capacity
                }
                _ => {
                    let index = tracking_index.get_or_insert_with(|| {
                        TrackingCapacityIndex::new(
                            engine,
                            tokens.slice(segment.token_start..segment.token_end),
                            metrics.shifted(segment.token_start),
                        )
                    });
                    let capacity = index
                        .boundary_capacity(start - segment.token_start, end - segment.token_start);
                    #[cfg(any(test, feature = "testing"))]
                    assert_eq!(
                        capacity,
                        measured_tracking_boundary_capacity(engine, tokens.slice(start..end), runs),
                        "indexed capacity must match final-placement measurement"
                    );
                    tracking_cache = Some((start, end, capacity));
                    capacity
                }
            };
            required <= tracking_width_limit(width).min(boundary_capacity)
        };
        let breaks = kp::break_compact_reusing_with_tracking(
            &segment.breakpoints,
            &line_width,
            KP_TOLERANCE,
            ragged,
            engine.config.hyphenation_quality(),
            &mut admit_tracking,
            scratch,
        );
        let Some(breaks) = breaks else {
            return greedy::break_lines_into(
                engine,
                tokens,
                metrics,
                paragraph,
                available_width,
                lines,
                None,
            );
        };

        let break_count = breaks.len();
        let mut line_start = start;
        for (index, breakpoint_index) in breaks.iter().copied().enumerate() {
            let is_segment_last = index + 1 == break_count;
            let breakpoint = segment
                .breakpoints
                .get(breakpoint_index)
                .expect("compact Knuth-Plass breakpoint index is valid");
            let line_end = breakpoint.line_end().max(line_start);
            let indent = indent_for_line(
                paragraph.indent.amount,
                paragraph.indent.hanging,
                (global_line == 0 && paragraph.indent.first_line) || paragraph.indent.each_line,
            );
            let first_hang = if global_line == 0 { hanging_start } else { 0.0 };
            let last_hang = if is_segment_last && seg_idx + 1 == seg_count {
                hanging_end.map_or(0.0, |(_, width)| width)
            } else { 0.0 };
            let origin = indent - if paragraph.direction == TextDirection::Ltr { first_hang } else { last_hang };
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
                    hyphen: None,
                    indent: origin,
                    tab_origin: origin,
                    width_limit: (available_width - indent).max(0.0) + first_hang + last_hang,
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
