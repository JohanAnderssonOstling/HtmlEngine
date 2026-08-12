use super::*;

/// Knuth–Plass optimal line breaking over the shared immutable token stream.
pub(in crate::layout::inline) fn break_lines_knuth(
    engine: &crate::layout::LayoutEngine<'_, '_>, tokens: &[InlineToken], runs: &[InlineTokenMetrics], paragraph: ParagraphLayout, available_width: f64, cached_plan: Option<&KpPlan>, ragged: bool,
) -> Vec<BrokenLine> {
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
    let mut lines = Vec::new();
    let mut global_line = 0usize;

    for (seg_idx, segment) in plan.segments.iter().enumerate() {
        let (start, end) = (segment.token_start, segment.token_end);
        if start >= end {
            if seg_idx + 1 < seg_count {
                let indent = indent_for_line(paragraph.indent.amount, paragraph.indent.hanging, global_line == 0 || paragraph.indent.each_line);
                let empty_segment_break = tokens.get(end).and_then(|token| empty_forced_break_index(token, end));
                if empty_segment_break.is_some() {
                    lines.push(BrokenLine { start, end, empty_segment_break, indent, tab_origin: indent, width_limit: (available_width - indent).max(0.0), align: paragraph.text_align_last, is_last_line: true });
                    global_line += 1;
                }
            }
            continue;
        }

        let seg_base_line = global_line;
        let line_width = |line: usize| (available_width - indent_for_line(paragraph.indent.amount, paragraph.indent.hanging, seg_base_line + line == 0 || paragraph.indent.each_line)).max(0.0);
        let breaks = if ragged {
            if engine.config.hyphenation_quality() { kp::break_compact_ragged(&segment.breakpoints, &line_width) } else { kp::break_compact_ragged_unrestricted(&segment.breakpoints, &line_width) }
        } else if engine.config.hyphenation_quality() {
            kp::break_compact_book(&segment.breakpoints, &line_width, KP_TOLERANCE).or_else(|| kp::break_compact_emergency_book(&segment.breakpoints, &line_width, KP_TOLERANCE))
        } else {
            kp::break_compact(&segment.breakpoints, &line_width, KP_TOLERANCE).or_else(|| kp::break_compact_emergency(&segment.breakpoints, &line_width, KP_TOLERANCE))
        };
        let Some(breaks) = breaks else {
            return greedy::break_lines(engine, tokens, runs, paragraph, available_width);
        };

        let break_count = breaks.len();
        let mut line_start = start;
        for (index, breakpoint_index) in breaks.into_iter().enumerate() {
            let is_segment_last = index + 1 == break_count;
            let breakpoint = segment.breakpoints.get(breakpoint_index).expect("compact Knuth-Plass breakpoint index is valid");
            let line_end = trim_trailing_line_whitespace(tokens, runs, line_start, breakpoint.line_end());
            let indent = indent_for_line(paragraph.indent.amount, paragraph.indent.hanging, global_line == 0 || paragraph.indent.each_line);
            let (align, is_last_line) = if is_segment_last { (paragraph.text_align_last, true) } else { (paragraph.text_align, false) };
            if line_end > line_start {
                lines.push(BrokenLine { start: line_start, end: line_end, empty_segment_break: None, indent, tab_origin: indent, width_limit: (available_width - indent).max(0.0), align, is_last_line });
                global_line += 1;
            }
            line_start = breakpoint.next_line_start();
        }
    }

    engine.record_timing(|timings| timings.break_lines_knuth += timing_started.elapsed());
    lines
}
