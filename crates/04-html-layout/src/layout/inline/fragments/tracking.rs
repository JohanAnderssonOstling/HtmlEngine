//! Width-independent tracking limits for near-fitting line candidates.

use super::*;
use crate::layout::inline::justification::{tracking_boundary_allowed, tracking_boundary_limit};

#[derive(Clone, Copy, Default)]
struct Prefix {
    boundaries: u32,
    unsupported: u32,
    gaps: u32,
}

struct HeightRun {
    start: usize,
    end: usize,
    ascent: f64,
    descent: f64,
}

/// Built lazily for a segment that needs tracking admission. Prefix counts
/// avoid rescanning glyphs for each candidate; height runs keep range queries
/// cheap for ordinary paragraphs with uniform metrics. This is temporary
/// solver storage, rather than part of the retained token/breakpoint plan.
pub(in crate::layout::inline) struct TrackingCapacityIndex<'a> {
    tokens: &'a [InlineToken],
    prefix: Vec<Prefix>,
    heights: Vec<HeightRun>,
}

impl<'a> TrackingCapacityIndex<'a> {
    pub(in crate::layout::inline) fn new(
        engine: &crate::layout::LayoutEngine<'_, '_>,
        tokens: &'a [InlineToken],
        runs: &[InlineTokenMetrics],
    ) -> Self {
        let mut prefix = Vec::with_capacity(tokens.len() + 1);
        let mut counts = Prefix::default();
        prefix.push(counts);
        let mut heights: Vec<HeightRun> = Vec::new();
        let mut previous_glyph = None;
        let mut text_cursor = engine.text.cursor();
        for (index, token) in tokens.iter().enumerate() {
            let metrics = token.run_metrics(runs);
            let mut unsupported = metrics.white_space.preserves_newlines();
            match token.kind() {
                InlineTokenKind::Glyph { glyph_idx } => {
                    unsupported |= token.is_tab()
                        || metrics.placement_required
                        || !metrics.visible
                        || !matches!(metrics.vertical_align, InlineVerticalAlign::Baseline);
                    let glyph = engine.text.glyph_at(glyph_idx as usize).unwrap_or_default();
                    let character = text_cursor
                        .glyph_metric(glyph)
                        .expect("registered text metric")
                        .ch();
                    if let Some((previous, previous_character)) = previous_glyph {
                        if previous + 1 != glyph_idx {
                            counts.gaps += 1;
                        } else if token.is_cluster_boundary()
                            && tracking_boundary_allowed(previous_character, character)
                        {
                            // Cluster eligibility is already cached on the
                            // token from authoritative character geometry.
                            counts.boundaries += 1;
                        }
                    }
                    previous_glyph = Some((glyph_idx, character));
                    // Match summarize_token_span's f64 addition order.
                    let half_leading = (f64::from(metrics.line_height)
                        - (f64::from(metrics.ascent) + f64::from(metrics.descent)))
                        / 2.0;
                    let ascent = f64::from(metrics.ascent) + half_leading;
                    let descent = f64::from(metrics.descent) + half_leading;
                    match heights.last_mut() {
                        Some(run) if run.ascent == ascent && run.descent == descent => {
                            run.end = index + 1
                        }
                        _ => heights.push(HeightRun {
                            start: index,
                            end: index + 1,
                            ascent,
                            descent,
                        }),
                    }
                }
                InlineTokenKind::Discretionary { .. } | InlineTokenKind::Opportunity => {}
                _ => unsupported = true,
            }
            counts.unsupported += u32::from(unsupported);
            prefix.push(counts);
        }
        Self {
            tokens,
            prefix,
            heights,
        }
    }

    pub(in crate::layout::inline) fn boundary_capacity(&self, start: usize, end: usize) -> f64 {
        if self.prefix[end].unsupported != self.prefix[start].unsupported {
            return 0.0;
        }
        let Some(first) = self.tokens[start..end]
            .iter()
            .position(|token| matches!(token.kind(), InlineTokenKind::Glyph { .. }))
        else {
            return 0.0;
        };
        let first = start + first;
        // Exclude the edge before the first glyph: it belongs to the previous
        // line, including any gap in global glyph indices at that edge.
        let before = self.prefix[first + 1];
        let after = self.prefix[end];
        if after.gaps != before.gaps || after.boundaries == before.boundaries {
            return 0.0;
        }
        let mut ascent = f64::NEG_INFINITY;
        let mut descent = f64::NEG_INFINITY;
        let first_run = self.heights.partition_point(|run| run.end <= first);
        for run in &self.heights[first_run..] {
            if run.start >= end {
                break;
            }
            ascent = ascent.max(run.ascent);
            descent = descent.max(run.descent);
        }
        if !ascent.is_finite() {
            ascent = 0.0;
        }
        if !descent.is_finite() {
            descent = 0.0;
        }
        // Inline struts omitted here can only increase final line height, so
        // this is conservative when a surrounding inline adds a larger strut.
        tracking_boundary_limit((ascent + descent).max(0.0))
            * f64::from(after.boundaries - before.boundaries)
    }
}
