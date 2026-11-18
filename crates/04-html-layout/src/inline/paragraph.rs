//! Paragraph-local geometry and separate reusable publication state.
//! Neither retains IDs into published document output.
use super::TokenSpan;
use super::fragments::LineLayout;
use super::s05_emit::LinePublication;
use super::storage::TokenData;
use super::tokens::{InlineTokenMetrics, InlineTokens};
use kurbo::{Point, Size};
use std::ops::Range;

/// Changed token spans own their data; metrics-only adjustments keep using the
/// canonical token plan by range. These adjustments have paragraph lifetime.
pub(super) struct LineAdjustment {
    pub(super) line: usize,
    pub(super) tokens: Option<TokenData<'static>>,
    pub(super) runs: Option<Box<[InlineTokenMetrics]>>,
}
impl LineAdjustment {
    pub(super) fn is_rewritten(&self) -> bool {
        self.tokens.is_some() || self.runs.is_some()
    }
}

/// Resolve the same original or rewritten token columns in geometry and emission.
pub(super) fn line_view<'a>(
    source: &'a InlineTokens<'_>,
    range: Range<usize>,
    adjustment: Option<&'a LineAdjustment>,
) -> (TokenSpan<'a>, &'a [InlineTokenMetrics]) {
    let tokens = adjustment
        .and_then(|entry| entry.tokens.as_ref())
        .map_or_else(|| source.span().slice(range), |tokens| tokens.span());
    let runs = adjustment
        .and_then(|entry| entry.runs.as_deref())
        .unwrap_or(&source.runs);
    (tokens, runs)
}

pub(super) struct PreparedLine {
    pub(super) tokens: Range<usize>,
    pub(super) origin: Point,
    pub(super) layout: LineLayout,
    pub(super) publication: LinePublication,
}
impl PreparedLine {
    pub(super) fn height(&self) -> f64 {
        if self.layout.publishes_line {
            self.layout.line_height
        } else {
            0.0
        }
    }
}
#[derive(Default)]
pub(super) struct ParagraphLayoutResult {
    pub(super) lines: Vec<PreparedLine>,
    pub(super) adjustments: Vec<LineAdjustment>,
    pub(super) size: Size,
}
impl ParagraphLayoutResult {
    pub(super) fn storage_bytes(&self) -> [usize; 5] {
        // Cache accounting runs on both insertion and removal. Read each
        // line once for its two owned columns instead of walking the retained
        // line records separately for placements and punctuation spacing.
        let mut placements = 0;
        let mut spacing = 0;
        for line in &self.lines {
            placements += line
                .layout
                .placements
                .as_ref()
                .map_or(0, |values| values.storage_bytes());
            placements += line.publication.storage_bytes();
            spacing += line.layout.punctuation_space_advances.capacity() * size_of::<(u32, f32)>();
        }
        [
            self.lines.capacity() * size_of::<PreparedLine>(),
            placements,
            self.adjustments.capacity() * size_of::<LineAdjustment>(),
            self.adjustments
                .iter()
                .map(|adjustment| {
                    adjustment
                        .tokens
                        .as_ref()
                        .map_or(0, |tokens| tokens.storage_bytes())
                        + adjustment
                            .runs
                            .as_ref()
                            .map_or(0, |runs| runs.len() * size_of::<InlineTokenMetrics>())
                })
                .sum(),
            spacing,
        ]
    }
}
