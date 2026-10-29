//! Owned local geometry, independent of IDs and storage in published output.
use super::fragments::{LineLayout, TokenPlacement};
use super::tokens::{InlineToken, InlineTokenMetrics};
use kurbo::{Point, Size};
use std::ops::Range;

/// Only changed overflow/pseudo spans own tokens; ordinary lines refer to the
/// canonical token plan by range. These adjustments have paragraph lifetime.
pub(super) struct LineAdjustment {
    pub(super) line: usize,
    pub(super) tokens: Box<[InlineToken]>,
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
        [
            self.lines.capacity() * size_of::<PreparedLine>(),
            self.lines
                .iter()
                .map(|line| {
                    line.layout.placements.as_ref().map_or(0, |placements| {
                        placements.capacity() * size_of::<TokenPlacement>()
                    })
                })
                .sum(),
            self.adjustments.capacity() * size_of::<LineAdjustment>(),
            self.adjustments
                .iter()
                .map(|adjustment| {
                    adjustment.tokens.len() * size_of::<InlineToken>()
                        + adjustment
                            .runs
                            .as_ref()
                            .map_or(0, |runs| runs.len() * size_of::<InlineTokenMetrics>())
                })
                .sum(),
            self.lines
                .iter()
                .map(|line| {
                    line.layout.punctuation_space_advances.capacity() * size_of::<(u32, f32)>()
                })
                .sum(),
        ]
    }
}
