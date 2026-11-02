//! Owned local geometry, independent of IDs and storage in published output.
use super::fragments::LineLayout;
use super::tokens::{InlineToken, InlineTokenMetrics};
use kurbo::{Point, Size};
use std::ops::Range;

/// Changed token spans own their data; metrics-only adjustments keep using the
/// canonical token plan by range. These adjustments have paragraph lifetime.
pub(super) struct LineAdjustment {
    pub(super) line: usize,
    pub(super) tokens: Option<Box<[InlineToken]>>,
    pub(super) runs: Option<Box<[InlineTokenMetrics]>>,
}
pub(super) struct PreparedLine {
    pub(super) tokens: Range<usize>,
    pub(super) origin: Point,
    pub(super) layout: LineLayout,
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
                        .map_or(0, |tokens| tokens.len() * size_of::<InlineToken>())
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
