//! Line-selection algorithms over an immutable token stream.
//!
//! Results contain token ranges and line constraints; fragment placement is a
//! separate phase.

use super::justification::glue_capacities;
use super::*;

mod greedy;
pub(super) mod knuth_plass;
mod optimal;
pub(super) const MICRO_TRACKING_FRACTION: f64 = knuth_plass::MICRO_TRACKING_FRACTION;
pub(super) use greedy::{break_lines, break_next_line};
pub(super) use optimal::break_lines_knuth;

pub(super) struct KpPlan {
    segments: Vec<KpSegmentPlan>,
}

struct KpSegmentPlan {
    token_start: usize,
    token_end: usize,
    breakpoints: Box<[knuth_plass::CompactBreakpoint]>,
}

impl KpPlan {
    pub(super) fn build(
        tokens: &[InlineToken],
        runs: &[InlineTokenMetrics],
        punctuation_aware: bool,
    ) -> Self {
        let mut segments = Vec::new();
        let mut token_start = 0usize;
        for (index, token) in tokens.iter().enumerate() {
            if token.break_kind() == BreakKind::Hard {
                segments.push(KpSegmentPlan::build(
                    tokens,
                    runs,
                    token_start,
                    index,
                    punctuation_aware,
                ));
                token_start = index + 1;
            }
        }
        segments.push(KpSegmentPlan::build(
            tokens,
            runs,
            token_start,
            tokens.len(),
            punctuation_aware,
        ));
        Self { segments }
    }

    pub(super) fn memory_usage_bytes(&self) -> usize {
        self.segments.capacity() * std::mem::size_of::<KpSegmentPlan>()
            + self
                .segments
                .iter()
                .map(|segment| {
                    segment.breakpoints.len()
                        * std::mem::size_of::<knuth_plass::CompactBreakpoint>()
                })
                .sum::<usize>()
    }
}

impl KpSegmentPlan {
    fn build(
        tokens: &[InlineToken],
        runs: &[InlineTokenMetrics],
        mut token_start: usize,
        token_end: usize,
        punctuation_aware: bool,
    ) -> Self {
        while token_start < token_end
            && token_is_collapsible_line_edge_space(&tokens[token_start], runs)
        {
            token_start += 1;
        }
        use knuth_plass::CompactBreakpoint;

        let mut breakpoints = Vec::new();
        let (mut width, mut stretch, mut shrink) = (0.0, 0.0, 0.0);
        let mut word_open = false;
        let mut token_index = token_start;
        while token_index < token_end {
            let token = &tokens[token_index];
            if token.break_kind() == BreakKind::Discretionary {
                token_index += 1;
                breakpoints.push(CompactBreakpoint::discretionary(
                    width,
                    stretch,
                    shrink,
                    token.discretionary_width(),
                    HYPHEN_PENALTY,
                    u32::try_from(token_index)
                        .expect("inline token index exceeds compact Knuth plan capacity"),
                    u32::try_from(token_index)
                        .expect("inline token index exceeds compact Knuth plan capacity"),
                ));
                word_open = true;
                continue;
            }
            if token.break_kind() != BreakKind::Soft {
                width += token.width();
                word_open = true;
                token_index += 1;
                continue;
            }

            if word_open {
                let line_end = token_index;
                let next_line_start = token_index + 1;
                let (mut discard_width, mut discard_stretch, mut discard_shrink) = (0.0, 0.0, 0.0);
                while token_index < token_end && tokens[token_index].break_kind() == BreakKind::Soft
                {
                    let space_token = &tokens[token_index];
                    let space_width = space_token.width();
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
                let space_width = token.width();
                let (space_stretch, space_shrink) =
                    glue_capacities(space_width, token.space_glue_class(), punctuation_aware);
                width += space_width;
                stretch += space_stretch;
                shrink += space_shrink;
                token_index += 1;
            }
        }
        breakpoints.push(CompactBreakpoint::final_(
            width,
            stretch,
            shrink,
            u32::try_from(token_end)
                .expect("inline token index exceeds compact Knuth plan capacity"),
        ));

        Self {
            token_start,
            token_end,
            breakpoints: breakpoints.into_boxed_slice(),
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

pub(super) struct BrokenLine {
    pub(super) start: usize,
    pub(super) end: usize,
    /// Preserved source newline supplying the strut for an empty line.
    pub(super) empty_segment_break: Option<usize>,
    pub(super) indent: f64,
    pub(super) tab_origin: f64,
    pub(super) width_limit: f64,
    pub(super) align: TextAlign,
    pub(super) is_last_line: bool,
}

fn empty_forced_break_index(token: &InlineToken, index: usize) -> Option<usize> {
    matches!(
        token.kind(),
        InlineTokenKind::Glyph { .. } | InlineTokenKind::Break { .. }
    )
    .then_some(index)
}

fn should_trim_leading_soft_token(token: &InlineToken, runs: &[InlineTokenMetrics]) -> bool {
    token_is_collapsible_line_edge_space(token, runs)
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

        let uniform = KpPlan::build(&tokens, &[], false);
        let punctuation_aware = KpPlan::build(&tokens, &[], true);
        let uniform_break = &uniform.segments[0].breakpoints[0];
        let punctuation_break = &punctuation_aware.segments[0].breakpoints[0];

        assert_eq!(uniform_break.discarded_stretch(), 5.0);
        assert_eq!(uniform_break.discarded_shrink(), 3.3f32 as f64);
        assert_eq!(punctuation_break.discarded_stretch(), 5.5);
        assert_eq!(punctuation_break.discarded_shrink(), 3.135f32 as f64);
    }
}

fn trim_trailing_line_whitespace(
    tokens: &[InlineToken],
    runs: &[InlineTokenMetrics],
    start: usize,
    end: usize,
) -> usize {
    let mut trimmed_end = end;
    while trimmed_end > start && should_trim_leading_soft_token(&tokens[trimmed_end - 1], runs) {
        trimmed_end -= 1;
    }
    trimmed_end
}
