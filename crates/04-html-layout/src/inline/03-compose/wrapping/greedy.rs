use super::{
    BreakKind, BrokenLine, InlineTokenKind, InlineTokenMetrics, InlineVerticalAlign,
    LineBreakResult, LineConstraints, MetricsView, ParagraphLayout, TextAlign, TextDirection,
    TokenSpan, WhiteSpace, empty_forced_break_index, trim_leading_soft_tokens,
    token_is_collapsible_line_edge_space,
};
use crate::LayoutEngine;
use crate::inline::boundaries::BoundaryReader;
use crate::inline::s03_compose::hanging;
use crate::inline::s03_compose::hyphenation::DeferredHyphenation;
use html_engine_model::GlyphId;

struct LineBreakState {
    x: f64,
    line_start: usize,
    content_end: usize,
    last_break: Option<BreakCandidate>,
    last_emergency_break: Option<BreakCandidate>,
    line_indent: f64,
    tab_origin: f64,
    line_width_limit: f64,
}

#[derive(Clone, Copy)]
struct BreakCandidate {
    line_end: usize,
    next_start: usize,
    hyphen: Option<(GlyphId, f32)>,
}

#[derive(Clone, Copy)]
enum LineEnding {
    Wrapped(TextAlign),
    Forced(TextAlign, Option<usize>),
    ContentEnd(TextAlign),
}

/// Breaks exactly one line using caller-provided geometry.
pub(in crate::inline) fn break_next_line<'input>(
    engine: &mut LayoutEngine<'input, '_>,
    tokens: TokenSpan<'_>,
    metrics_view: MetricsView<'_>,
    paragraph: ParagraphLayout,
    start: usize,
    constraints: LineConstraints,
    mut deferred: Option<&mut DeferredHyphenation<'input, '_>>,
) -> LineBreakResult {
    let reader = engine.read.reader;
    let hanging_start = if start == 0 {
        hanging::first(engine, tokens, paragraph)
    } else {
        0.0
    };
    let hanging_end = hanging::last(engine, tokens, paragraph);
    let hanging_left = if paragraph.direction == TextDirection::Ltr {
        hanging_start
    } else {
        0.0
    };
    let start = trim_leading_soft_tokens(tokens, metrics_view, start);
    let mut metrics_cursor = metrics_view.cursor(tokens.len());
    let mut state = LineBreakState {
        x: 0.0,
        line_start: start,
        content_end: start,
        last_break: None,
        last_emergency_break: None,
        line_indent: constraints.indent - hanging_left,
        tab_origin: constraints.tab_origin - hanging_left,
        line_width_limit: constraints.width_limit + hanging_start,
    };

    let boundaries = BoundaryReader::new(tokens, metrics_view);
    let mut i = start;
    while i < tokens.len() {
        let token = tokens.metadata_at(i);
        let boundary = boundaries.before(i);
        if boundary.normal && i > state.line_start {
            // A prepared normal boundary retains the preceding content,
            // including hanging preserved spaces, without adding a token.
            state.content_end = i;
            state.last_break = Some(BreakCandidate {
                line_end: i,
                next_start: i,
                hyphen: None,
            });
        }
        // A fitted emergency boundary remains available if later text changes
        // its wrapping policy. Ordinary opportunities retain priority.
        if i > state.line_start && state.x <= state.line_width_limit && boundary.emergency {
            state.last_emergency_break = Some(BreakCandidate {
                line_end: state.content_end,
                next_start: i,
                hyphen: None,
            });
        }
        // Fitting ordinary glyphs need no further whitespace metrics.
        // Keep their dependent width sum in source order.
        if token.is_unbroken_glyph() {
            let x = state.x + f64::from(tokens.stored_width_at(i));
            if x <= state.line_width_limit {
                state.x = x;
                state.content_end = i + 1;
                i += 1;
                continue;
            }
        }
        if matches!(
            token.kind(),
            InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. }
        ) {
            // Anchors remain in the line range even though they have no advance.
            state.content_end = i + 1;
            i += 1;
            continue;
        }
        if boundaries.kind(i) == BreakKind::Hard {
            return LineBreakResult {
                line: finish_broken_line(
                    &state,
                    state.content_end,
                    LineEnding::Forced(
                        paragraph.text_align_last,
                        empty_forced_break_index(&token, i),
                    ),
                ),
                next_start: trim_leading_soft_tokens(tokens, metrics_view, i + 1),
                ended_by_forced_break: true,
            };
        }

        // Ordinary glyphs need only their header and width. Resolve metrics
        // where whitespace, tab stops or overflow alignment actually use them.
        let collapsible = token_is_collapsible_line_edge_space(token, &mut metrics_cursor);
        let advance = if token.is_tab() {
            token.advance_with_metrics(
                tokens.stored_width_at(i),
                &reader,
                metrics_cursor.at(token),
                state.tab_origin + state.x,
            )
        } else {
            token.width(tokens.stored_width_at(i))
        };
        let fitting_limit = state.line_width_limit
            + hanging_end
                .filter(|(index, _)| *index == i)
                .map_or(0.0, |(_, width)| width);
        // A pre-wrap space sequence stays on the preceding line, even when
        // it overflows. Its after-space opportunity must be recorded before
        // the next visible character triggers line breaking.
        let white_space = metrics_cursor.at(token).white_space;
        let hanging_space = (white_space == WhiteSpace::PreWrap
            && (token.is_space()
                || token.is_tab()
                || matches!(token.kind(), InlineTokenKind::ZeroWidthBreak)))
            || ((white_space.collapses_spaces() || white_space == WhiteSpace::PreWrap)
                && matches!(token.kind(), InlineTokenKind::Glyph { glyph_idx }
                    if html_text::is_other_space_separator(engine.read.text.character(glyph_idx))));
        if state.x + advance > fitting_limit
            && !matches!(token.kind(), InlineTokenKind::ZeroWidthBreak)
            && !collapsible
            && !hanging_space
            && i > state.line_start
            && let Some(deferred) = deferred.as_deref_mut()
        {
            // Internal hyphens in words that fit never become candidates.
            // Resolve only the still-unbroken prefix at an actual overflow.
            let candidate_start = state
                .last_break
                .map_or(state.line_start, |candidate| candidate.next_start);
            let first = (candidate_start..i).find_map(|index| {
                let token = tokens.metadata_at(index);
                match token.kind() {
                    InlineTokenKind::Glyph { glyph_idx }
                        if token.break_kind() != BreakKind::Soft =>
                    {
                        Some(glyph_idx)
                    }
                    _ => None,
                }
            });
            let last = (candidate_start..i).rev().find_map(|index| {
                match tokens.metadata_at(index).kind() {
                    InlineTokenKind::Glyph { glyph_idx } => Some(glyph_idx),
                    _ => None,
                }
            });
            if let Some((first, last)) = first.zip(last) {
                let allow_overflow = state.last_break.is_none();
                let mut end = last + 1;
                while first < end {
                    let Some(glyph) = deferred.last_break(engine, first..end) else {
                        break;
                    };
                    end = glyph;
                    let Some(index) = (candidate_start..i).rev().find(|&index| {
                        matches!(tokens.metadata_at(index).kind(), InlineTokenKind::Glyph { glyph_idx } if glyph_idx == glyph)
                    }) else {
                        break;
                    };
                    let Some(hyphen) = engine.read.text.hyphen_glyph(
                        metrics_cursor
                            .at(tokens.metadata_at(index))
                            .owner_box_idx as usize,
                    ) else {
                        continue;
                    };
                    let hyphen_width = engine.read.text.glyph_metric(hyphen).advance();
                    // The source prefix fits before overflow, but inserting its
                    // hyphen can still exceed the line. Try earlier dictionary
                    // breaks until both fit, preserving tab-stop advances.
                    let width = (state.line_start..=index).fold(0.0, |x, index| {
                        let token = tokens.metadata_at(index);
                        x + if token.is_tab() {
                            token.advance_with_metrics(
                                tokens.stored_width_at(index),
                                &reader,
                                metrics_cursor.at(token),
                                state.tab_origin + x,
                            )
                        } else {
                            token.width(tokens.stored_width_at(index))
                        }
                    });
                    let fits = width + f64::from(hyphen_width) <= state.line_width_limit;
                    // An unbreakable prefix wider than an empty line still
                    // needs its first hyphen opportunity as an overflow fallback.
                    if fits || allow_overflow {
                        state.last_break = Some(BreakCandidate {
                            line_end: index + 1,
                            next_start: index + 1,
                            hyphen: Some((hyphen, hyphen_width)),
                        });
                    }
                    if fits {
                        break;
                    }
                }
            }
        }
        if state.x + advance > fitting_limit
            && !matches!(token.kind(), InlineTokenKind::ZeroWidthBreak)
            && !collapsible
            && !hanging_space
            && i > state.line_start
            && !(state.last_break.is_none()
                && keep_with_previous_token(tokens, metrics_cursor.at(token), i))
        {
            let break_mid_word = boundary.emergency_from_current && state.last_break.is_none();
            if break_mid_word && token.is_cluster_boundary() {
                return LineBreakResult {
                    line: finish_broken_line(
                        &state,
                        state.content_end,
                        LineEnding::Wrapped(paragraph.text_align),
                    ),
                    next_start: trim_leading_soft_tokens(tokens, metrics_view, i),
                    ended_by_forced_break: false,
                };
            }
            if let Some(candidate) = state
                .last_break
                .or(state.last_emergency_break)
                .filter(|candidate| candidate.next_start > state.line_start)
            {
                return LineBreakResult {
                    line: finish_broken_line(
                        &state,
                        candidate.line_end,
                        LineEnding::Wrapped(paragraph.text_align),
                    )
                    .map(|mut line| {
                        line.hyphen = candidate.hyphen;
                        line
                    }),
                    next_start: trim_leading_soft_tokens(
                        tokens,
                        metrics_view,
                        candidate.next_start,
                    ),
                    ended_by_forced_break: false,
                };
            }
        }

        if boundaries.kind(i) == BreakKind::Soft && token.is_cluster_boundary() {
            state.last_break = Some(BreakCandidate {
                line_end: state.content_end,
                next_start: i,
                hyphen: None,
            });
        } else if boundaries.kind(i) == BreakKind::Discretionary
            && (state.last_break.is_none()
                || !matches!(token.kind(), InlineTokenKind::Discretionary { .. })
                || state.x + token.discretionary_width(tokens.stored_width_at(i))
                    <= state.line_width_limit)
        {
            state.last_break = Some(BreakCandidate {
                line_end: i + 1,
                next_start: i + 1,
                hyphen: match token.kind() {
                    InlineTokenKind::Discretionary { glyph } => {
                        Some((glyph, tokens.stored_width_at(i)))
                    }
                    _ => None,
                },
            });
        }
        if !collapsible {
            state.content_end = i + 1;
        }
        state.x += advance;
        i += 1;
    }

    state.line_width_limit += hanging_end.map_or(0.0, |(_, width)| width);
    if paragraph.direction == TextDirection::Rtl {
        let hanging_left = hanging_end.map_or(0.0, |(_, width)| width);
        state.line_indent -= hanging_left;
        state.tab_origin -= hanging_left;
    }
    LineBreakResult {
        line: finish_broken_line(
            &state,
            state.content_end,
            LineEnding::ContentEnd(paragraph.text_align_last),
        ),
        next_start: tokens.len(),
        ended_by_forced_break: false,
    }
}

fn finish_broken_line(
    state: &LineBreakState,
    end: usize,
    ending: LineEnding,
) -> Option<BrokenLine> {
    let (align, is_last_line, segment_break) = match ending {
        LineEnding::Wrapped(align) => (align, false, None),
        LineEnding::Forced(align, segment_break) => (align, true, segment_break),
        LineEnding::ContentEnd(align) => (align, true, None),
    };
    let empty_segment_break = (end == state.line_start).then_some(segment_break).flatten();
    (end > state.line_start || empty_segment_break.is_some()).then_some(BrokenLine {
        start: state.line_start,
        end,
        hyphen: None,
        empty_segment_break,
        indent: state.line_indent,
        tab_origin: state.tab_origin,
        width_limit: state.line_width_limit,
        align,
        is_last_line,
    })
}

pub(in crate::inline) fn break_lines_into<'input>(
    engine: &mut LayoutEngine<'input, '_>,
    tokens: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    paragraph: ParagraphLayout,
    available_width: f64,
    mut lines: Vec<BrokenLine>,
    mut deferred: Option<&mut DeferredHyphenation<'input, '_>>,
) -> Vec<BrokenLine> {
    lines.clear();
    let mut start = 0usize;
    let mut starts_indented_line = paragraph.indent.first_line;
    while start < tokens.len() {
        let result = break_next_line(
            engine,
            tokens,
            metrics,
            paragraph,
            start,
            LineConstraints::new(paragraph, available_width, starts_indented_line),
            deferred.as_deref_mut(),
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
    lines
}

fn keep_with_previous_token(tokens: TokenSpan<'_>, metrics: &InlineTokenMetrics, i: usize) -> bool {
    if i == 0 {
        return false;
    }
    if !matches!(
        metrics.vertical_align,
        InlineVerticalAlign::Super(_) | InlineVerticalAlign::Sub(_)
    ) {
        return false;
    }
    tokens.metadata_at(i - 1).break_kind() == BreakKind::None
}
