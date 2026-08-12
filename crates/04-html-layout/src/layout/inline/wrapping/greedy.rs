use super::*;

struct LineBreakState {
    x: f64,
    line_start: usize,
    last_break: Option<usize>,
    line_indent: f64,
    tab_origin: f64,
    line_width_limit: f64,
}

#[derive(Clone, Copy)]
enum LineEnding {
    Wrapped(TextAlign),
    Forced(TextAlign, Option<usize>),
    ContentEnd(TextAlign),
}

pub(in crate::layout::inline) fn line_constraints(
    paragraph: ParagraphLayout,
    available_width: f64,
    starts_indented_line: bool,
) -> LineConstraints {
    let indent = indent_for_line(
        paragraph.indent.amount,
        paragraph.indent.hanging,
        starts_indented_line,
    );
    LineConstraints {
        indent,
        tab_origin: indent,
        width_limit: (available_width - indent).max(0.0),
    }
}

/// Breaks exactly one line using caller-provided geometry.
pub(in crate::layout::inline) fn break_next_line(
    tokens: &[InlineToken],
    runs: &[InlineTokenMetrics],
    paragraph: ParagraphLayout,
    start: usize,
    constraints: LineConstraints,
) -> LineBreakResult {
    let start = trim_leading_soft_tokens(tokens, runs, start);
    let mut state = LineBreakState {
        x: 0.0,
        line_start: start,
        last_break: None,
        line_indent: constraints.indent,
        tab_origin: constraints.tab_origin,
        line_width_limit: constraints.width_limit,
    };

    let mut i = start;
    while i < tokens.len() {
        let token = &tokens[i];
        if matches!(
            token.kind(),
            InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. }
        ) {
            i += 1;
            continue;
        }
        if token.break_kind() == BreakKind::Hard {
            return LineBreakResult {
                line: finish_broken_line(
                    tokens,
                    runs,
                    &state,
                    i,
                    LineEnding::Forced(
                        paragraph.text_align_last,
                        empty_forced_break_index(token, i),
                    ),
                ),
                next_start: trim_leading_soft_tokens(tokens, runs, i + 1),
                ended_by_forced_break: true,
            };
        }

        let advance = token.advance_at(runs, state.tab_origin + state.x);
        if state.x + advance > state.line_width_limit
            && !should_trim_leading_soft_token(token, runs)
            && i > state.line_start
            && !(state.last_break.is_none() && keep_with_previous_token(tokens, runs, i))
        {
            let break_mid_word = match token.wrap() {
                TokenWrap::Anywhere => true,
                TokenWrap::BreakWord => state.last_break.is_none(),
                TokenWrap::Normal => false,
            };
            if break_mid_word && token.is_cluster_boundary() {
                return LineBreakResult {
                    line: finish_broken_line(
                        tokens,
                        runs,
                        &state,
                        i,
                        LineEnding::Wrapped(paragraph.text_align),
                    ),
                    next_start: trim_leading_soft_tokens(tokens, runs, i),
                    ended_by_forced_break: false,
                };
            }
            if let Some(break_at) = state.last_break.filter(|&bp| bp > state.line_start) {
                return LineBreakResult {
                    line: finish_broken_line(
                        tokens,
                        runs,
                        &state,
                        break_at,
                        LineEnding::Wrapped(paragraph.text_align),
                    ),
                    next_start: trim_leading_soft_tokens(tokens, runs, break_at),
                    ended_by_forced_break: false,
                };
            }
        }

        if token.break_kind() == BreakKind::Soft && token.is_cluster_boundary() {
            state.last_break = Some(i);
        } else if token.break_kind() == BreakKind::Discretionary {
            state.last_break = Some(i + 1);
        }
        state.x += advance;
        i += 1;
    }

    LineBreakResult {
        line: finish_broken_line(
            tokens,
            runs,
            &state,
            tokens.len(),
            LineEnding::ContentEnd(paragraph.text_align_last),
        ),
        next_start: tokens.len(),
        ended_by_forced_break: false,
    }
}

fn finish_broken_line(
    tokens: &[InlineToken],
    runs: &[InlineTokenMetrics],
    state: &LineBreakState,
    end: usize,
    ending: LineEnding,
) -> Option<BrokenLine> {
    let (align, is_last_line, segment_break) = match ending {
        LineEnding::Wrapped(align) => (align, false, None),
        LineEnding::Forced(align, segment_break) => (align, true, segment_break),
        LineEnding::ContentEnd(align) => (align, true, None),
    };
    let trimmed_end = trim_trailing_line_whitespace(tokens, runs, state.line_start, end);
    let empty_segment_break = (trimmed_end == state.line_start)
        .then_some(segment_break)
        .flatten();
    (trimmed_end > state.line_start || empty_segment_break.is_some()).then_some(BrokenLine {
        start: state.line_start,
        end: trimmed_end,
        empty_segment_break,
        indent: state.line_indent,
        tab_origin: state.tab_origin,
        width_limit: state.line_width_limit,
        align,
        is_last_line,
    })
}

fn trim_leading_soft_tokens(
    tokens: &[InlineToken],
    runs: &[InlineTokenMetrics],
    mut start: usize,
) -> usize {
    while start < tokens.len() && should_trim_leading_soft_token(&tokens[start], runs) {
        start += 1;
    }
    start
}

pub(in crate::layout::inline) fn break_lines(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    tokens: &[InlineToken],
    runs: &[InlineTokenMetrics],
    paragraph: ParagraphLayout,
    available_width: f64,
) -> Vec<BrokenLine> {
    let timing_started = engine.start_timing();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut starts_indented_line = true;
    while start < tokens.len() {
        let result = break_next_line(
            tokens,
            runs,
            paragraph,
            start,
            line_constraints(paragraph, available_width, starts_indented_line),
        );
        let previous_start = start;
        start = result.next_start;
        starts_indented_line = result.ended_by_forced_break && paragraph.indent.each_line;
        if let Some(line) = result.line {
            lines.push(line);
        }
        if start <= previous_start && start < tokens.len() {
            start = previous_start.saturating_add(1);
        }
    }
    engine.record_timing(|t| t.break_lines += timing_started.elapsed());
    lines
}

fn keep_with_previous_token(tokens: &[InlineToken], runs: &[InlineTokenMetrics], i: usize) -> bool {
    if i == 0 {
        return false;
    }
    let token = &tokens[i];
    if !matches!(
        token.run_metrics(runs).vertical_align,
        VerticalAlignValue::Super | VerticalAlignValue::Sub
    ) {
        return false;
    }
    tokens[i - 1].break_kind() == BreakKind::None
}
