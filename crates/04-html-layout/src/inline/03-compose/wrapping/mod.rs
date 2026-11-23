//! Line-selection algorithms over an immutable token stream.
//!
//! Results contain token ranges and line constraints; fragment placement is a
//! separate phase.
use crate::inline::boundaries::BoundaryReader;
use html_engine_model::GlyphId;

use crate::inline::justification::glue_capacities;
use crate::inline::{
    BreakKind, HYPHEN_PENALTY, InlineToken, InlineTokenKind, InlineTokenMetrics,
    InlineVerticalAlign, KP_TOLERANCE, MetricsView, ParagraphLayout, TextAlign, TextDirection,
    TokenMetadata, TokenSpan, WhiteSpace, indent_for_line, token_is_collapsible_line_edge_space,
};
use knuth_plass::BreakpointStorage;

pub(in crate::inline) mod greedy;
pub(in crate::inline) mod knuth_plass;
mod optimal;
mod tracking;
pub(in crate::inline) use greedy::break_next_line;
pub(in crate::inline) use optimal::break_lines_knuth;

pub(in crate::inline) struct KpPlan {
    segments: Box<[KpSegmentPlan]>,
}

struct KpSegmentPlan {
    token_start: usize,
    token_end: usize,
    breakpoints: BreakpointStorage,
}

impl KpPlan {
    pub(in crate::inline) fn build(
        tokens: TokenSpan<'_>,
        runs: &[InlineTokenMetrics],
        punctuation_aware: bool,
    ) -> Self {
        let metrics = tokens.metrics_view(runs);
        let mut segments = Vec::new();
        let mut token_start = 0usize;
        loop {
            let segment = KpSegmentPlan::build(tokens, metrics, token_start, punctuation_aware);
            let token_end = segment.token_end;
            segments.push(segment);
            if token_end == tokens.len() {
                break;
            }
            token_start = token_end + 1;
        }
        Self {
            segments: segments.into_boxed_slice(),
        }
    }

    pub(in crate::inline) fn memory_usage_bytes(&self) -> usize {
        self.segments.len() * std::mem::size_of::<KpSegmentPlan>()
            + self
                .segments
                .iter()
                .map(|segment| segment.breakpoints.memory_usage_bytes())
                .sum::<usize>()
    }
}

impl KpSegmentPlan {
    fn build(
        tokens: TokenSpan<'_>,
        metrics: MetricsView<'_>,
        token_start: usize,
        punctuation_aware: bool,
    ) -> Self {
        let mut token_end = tokens.len();
        let token_start = trim_leading_soft_tokens(tokens, metrics, token_start);
        use knuth_plass::CompactBreakpoint;

        let boundaries = BoundaryReader::new(tokens, metrics);
        let mut breakpoints = Vec::new();
        let (mut width, mut stretch, mut shrink) = (0.0, 0.0, 0.0);
        let mut word_open = false;
        let mut token_index = token_start;
        let mut content_end = token_start;
        while token_index < token_end {
            let token = tokens.metadata_at(token_index);
            if token_index > token_start && boundaries.before(token_index).normal {
                let boundary = u32::try_from(token_index)
                    .expect("inline token index exceeds compact Knuth plan capacity");
                breakpoints.push(CompactBreakpoint::soft(
                    width, stretch, shrink, 0.0, 0.0, 0.0, boundary, boundary,
                ));
                content_end = token_index;
                word_open = true;
            }
            // Discover segment boundaries while accumulating candidates,
            // without a preliminary scan over the paragraph.
            if boundaries.kind(token_index) == BreakKind::Hard {
                token_end = token_index;
                break;
            }
            if boundaries.kind(token_index) == BreakKind::Discretionary {
                token_index += 1;
                content_end = token_index;
                let breakpoint = if matches!(token.kind(), InlineTokenKind::ZeroWidthBreak) {
                    // Authored zero-width spaces have no hyphen penalty.
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
            if boundaries.kind(token_index) != BreakKind::Soft {
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
                    && (token_index == line_end || !boundaries.before(token_index).normal)
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
        // and discretionary breakpoints retain their source/control token. Only
        // the terminal soft-token suffix still needs its whitespace policy.
        let suffix_start = content_end;
        let mut cursor = metrics.cursor(tokens.len());
        for index in suffix_start..token_end {
            let token = tokens.metadata_at(index);
            if !token_is_collapsible_line_edge_space(token, &mut cursor) {
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

fn trim_leading_soft_tokens(
    tokens: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    mut start: usize,
) -> usize {
    let mut cursor = metrics.cursor(tokens.len());
    while start < tokens.len() {
        let token = tokens.metadata_at(start);
        // An authored zero-width break has no line content of its own.
        // Skip it at the start so it cannot create an empty wrapped line.
        if matches!(token.kind(), InlineTokenKind::ZeroWidthBreak) {
            start += 1;
            continue;
        }
        if !token_is_collapsible_line_edge_space(token, &mut cursor) {
            break;
        }
        start += 1;
    }
    start
}

impl LineConstraints {
    pub(in crate::inline) fn new(
        paragraph: ParagraphLayout,
        available_width: f64,
        starts_indented_line: bool,
    ) -> Self {
        let indent = indent_for_line(
            paragraph.indent.amount,
            paragraph.indent.hanging,
            starts_indented_line,
        );
        Self {
            indent,
            tab_origin: indent,
            width_limit: (available_width - indent).max(0.0),
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::inline) struct LineConstraints {
    pub(in crate::inline) indent: f64,
    pub(in crate::inline) tab_origin: f64,
    pub(in crate::inline) width_limit: f64,
}

pub(in crate::inline) struct LineBreakResult {
    pub(in crate::inline) line: Option<BrokenLine>,
    pub(in crate::inline) next_start: usize,
    pub(in crate::inline) ended_by_forced_break: bool,
}

#[derive(Clone)]
pub(in crate::inline) struct BrokenLine {
    pub(in crate::inline) start: usize,
    pub(in crate::inline) end: usize,
    pub(in crate::inline) hyphen: Option<(GlyphId, f32)>,
    /// Preserved source newline supplying the strut for an empty line.
    pub(in crate::inline) empty_segment_break: Option<usize>,
    pub(in crate::inline) indent: f64,
    pub(in crate::inline) tab_origin: f64,
    pub(in crate::inline) width_limit: f64,
    pub(in crate::inline) align: TextAlign,
    pub(in crate::inline) is_last_line: bool,
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
            false,
            true,
        );
        let mut clause_space = InlineToken::new(
            InlineTokenKind::Glyph { glyph_idx: 1 },
            10.0,
            BreakKind::Soft,
            false,
            true,
        );
        clause_space.set_space_glue_class(crate::inline::justification::SpaceGlueClass::Clause);
        let next_word = InlineToken::new(
            InlineTokenKind::Glyph { glyph_idx: 2 },
            10.0,
            BreakKind::None,
            false,
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
