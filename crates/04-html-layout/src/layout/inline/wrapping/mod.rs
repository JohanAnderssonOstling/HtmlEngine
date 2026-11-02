//! Line-selection algorithms over an immutable token stream.
//!
//! Results contain token ranges and line constraints; fragment placement is a
//! separate phase.

use super::justification::glue_capacities;
use super::*;
use knuth_plass::BreakpointStorage;

pub(super) mod greedy;
pub(super) mod knuth_plass;
mod optimal;
pub(super) use greedy::break_next_line;
pub(super) use optimal::break_lines_knuth;

pub(super) struct KpPlan {
    segments: Box<[KpSegmentPlan]>,
}

struct KpSegmentPlan {
    token_start: usize,
    token_end: usize,
    breakpoints: BreakpointStorage,
}

impl KpPlan {
    pub(super) fn build(
        tokens: TokenSpan<'_>,
        runs: &[InlineTokenMetrics],
        punctuation_aware: bool,
    ) -> Self {
        let metrics = tokens.metrics_view(runs);
        match tokens {
            TokenSpan::Wide(tokens) => Self::build_with(tokens, metrics, punctuation_aware),
            TokenSpan::Compact { source, start, len } => Self::build_with(
                CompactTokens::new(source, start, len),
                metrics,
                punctuation_aware,
            ),
        }
    }

    fn build_with<S: TokenRead>(
        tokens: S,
        metrics: MetricsView<'_>,
        punctuation_aware: bool,
    ) -> Self {
        profile_scope!("lines.build_breakpoint_plan");
        let mut segments = Vec::new();
        let mut token_start = 0usize;
        for index in 0..tokens.len() {
            let token = tokens.metadata_at(index);
            if token.break_kind() == BreakKind::Hard {
                segments.push(KpSegmentPlan::build(
                    tokens,
                    metrics,
                    token_start,
                    index,
                    punctuation_aware,
                ));
                token_start = index + 1;
            }
        }
        segments.push(KpSegmentPlan::build(
            tokens,
            metrics,
            token_start,
            tokens.len(),
            punctuation_aware,
        ));
        Self {
            segments: segments.into_boxed_slice(),
        }
    }

    pub(super) fn memory_usage_bytes(&self) -> usize {
        self.segments.len() * std::mem::size_of::<KpSegmentPlan>()
            + self
                .segments
                .iter()
                .map(|segment| segment.breakpoints.memory_usage_bytes())
                .sum::<usize>()
    }
}

impl KpSegmentPlan {
    fn build<S: TokenRead>(
        tokens: S,
        metrics: MetricsView<'_>,
        mut token_start: usize,
        token_end: usize,
        punctuation_aware: bool,
    ) -> Self {
        while token_start < token_end
            && tokens.metadata_at(token_start).break_kind() == BreakKind::Soft
            && !metrics
                .at(tokens, token_start)
                .white_space
                .preserves_spaces()
        {
            token_start += 1;
        }
        use knuth_plass::CompactBreakpoint;

        let mut breakpoints = Vec::new();
        let (mut width, mut stretch, mut shrink) = (0.0, 0.0, 0.0);
        let mut word_open = false;
        let mut token_index = token_start;
        let mut content_end = token_start;
        while token_index < token_end {
            let token = tokens.metadata_at(token_index);
            if token.break_kind() == BreakKind::Discretionary {
                token_index += 1;
                content_end = token_index;
                let breakpoint = if matches!(token.kind(), InlineTokenKind::Opportunity) {
                    // CJK/atomic boundaries are ordinary opportunities and
                    // do not carry the per-hyphen break penalty.
                    CompactBreakpoint::soft(
                        width,
                        stretch,
                        shrink,
                        0.0,
                        0.0,
                        0.0,
                        u32::try_from(token_index)
                            .expect("inline token index exceeds compact Knuth plan capacity"),
                        u32::try_from(token_index)
                            .expect("inline token index exceeds compact Knuth plan capacity"),
                    )
                } else {
                    CompactBreakpoint::discretionary(
                        width,
                        stretch,
                        shrink,
                        token.discretionary_width(tokens.stored_width_at(token_index - 1)),
                        HYPHEN_PENALTY,
                        u32::try_from(token_index)
                            .expect("inline token index exceeds compact Knuth plan capacity"),
                        u32::try_from(token_index)
                            .expect("inline token index exceeds compact Knuth plan capacity"),
                    )
                };
                breakpoints.push(breakpoint);
                word_open = true;
                continue;
            }
            if token.break_kind() != BreakKind::Soft {
                width += token.width(tokens.stored_width_at(token_index));
                word_open = true;
                token_index += 1;
                content_end = token_index;
                continue;
            }

            if word_open {
                let line_end = token_index;
                let next_line_start = token_index + 1;
                let (mut discard_width, mut discard_stretch, mut discard_shrink) = (0.0, 0.0, 0.0);
                while token_index < token_end
                    && tokens.metadata_at(token_index).break_kind() == BreakKind::Soft
                {
                    let space_token = tokens.metadata_at(token_index);
                    let space_width = space_token.width(tokens.stored_width_at(token_index));
                    let (space_stretch, space_shrink) = glue_capacities(
                        space_width,
                        space_token.space_glue_class(),
                        punctuation_aware,
                    );
                    discard_width += space_width;
                    discard_stretch += space_stretch;
                    discard_shrink += space_shrink;
                    token_index += 1;
                }
                breakpoints.push(CompactBreakpoint::soft(
                    width,
                    stretch,
                    shrink,
                    discard_width,
                    discard_stretch,
                    discard_shrink,
                    u32::try_from(line_end)
                        .expect("inline token index exceeds compact Knuth plan capacity"),
                    u32::try_from(next_line_start)
                        .expect("inline token index exceeds compact Knuth plan capacity"),
                ));
                width += discard_width;
                stretch += discard_stretch;
                shrink += discard_shrink;
                word_open = false;
            } else {
                let space_width = token.width(tokens.stored_width_at(token_index));
                let (space_stretch, space_shrink) =
                    glue_capacities(space_width, token.space_glue_class(), punctuation_aware);
                width += space_width;
                stretch += space_stretch;
                shrink += space_shrink;
                token_index += 1;
            }
        }
        // Ordinary space breakpoints already end before their discarded glue,
        // and discretionary breakpoints retain their opportunity token. Only
        // the terminal soft-token suffix still needs its whitespace policy.
        let suffix_start = content_end;
        for index in suffix_start..token_end {
            let token = tokens.metadata_at(index);
            if token.break_kind() != BreakKind::Soft
                || metrics.at(tokens, index).white_space.preserves_spaces()
            {
                content_end = index + 1;
            }
        }
        breakpoints.push(CompactBreakpoint::final_(
            width,
            stretch,
            shrink,
            u32::try_from(content_end)
                .expect("inline token index exceeds compact Knuth plan capacity"),
        ));

        Self {
            token_start,
            token_end,
            breakpoints: BreakpointStorage::new(breakpoints),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct LineConstraints {
    pub(super) indent: f64,
    pub(super) tab_origin: f64,
    pub(super) width_limit: f64,
}

pub(super) struct LineBreakResult {
    pub(super) line: Option<BrokenLine>,
    pub(super) next_start: usize,
    pub(super) ended_by_forced_break: bool,
}

#[derive(Clone)]
pub(super) struct BrokenLine {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) hyphen: Option<(crate::GlyphId, f32)>,
    /// Preserved source newline supplying the strut for an empty line.
    pub(super) empty_segment_break: Option<usize>,
    pub(super) indent: f64,
    pub(super) tab_origin: f64,
    pub(super) width_limit: f64,
    pub(super) align: TextAlign,
    pub(super) is_last_line: bool,
}

fn empty_forced_break_index(token: &TokenMetadata, index: usize) -> Option<usize> {
    matches!(
        token.kind(),
        InlineTokenKind::Glyph { .. } | InlineTokenKind::Break { .. }
    )
    .then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kp_plan_consumes_punctuation_aware_space_capacities() {
        let word = InlineToken::new(
            InlineTokenKind::Glyph { glyph_idx: 0 },
            10.0,
            BreakKind::None,
            TokenWrap::Normal,
            true,
        );
        let mut clause_space = InlineToken::new(
            InlineTokenKind::Glyph { glyph_idx: 1 },
            10.0,
            BreakKind::Soft,
            TokenWrap::Normal,
            true,
        );
        clause_space.set_space_glue_class(super::super::justification::SpaceGlueClass::Clause);
        let next_word = InlineToken::new(
            InlineTokenKind::Glyph { glyph_idx: 2 },
            10.0,
            BreakKind::None,
            TokenWrap::Normal,
            true,
        );
        let tokens = [word, clause_space, next_word];

        let uniform = KpPlan::build(tokens.as_slice().into(), &[], false);
        let punctuation_aware = KpPlan::build(tokens.as_slice().into(), &[], true);
        let uniform_break = uniform.segments[0].breakpoints.get(0).unwrap();
        let punctuation_break = punctuation_aware.segments[0].breakpoints.get(0).unwrap();

        assert_eq!(uniform_break.discarded_stretch(), 5.0);
        assert_eq!(uniform_break.discarded_shrink(), 3.3f32 as f64);
        assert_eq!(punctuation_break.discarded_stretch(), 5.5);
        assert_eq!(punctuation_break.discarded_shrink(), 3.135f32 as f64);
    }
}
