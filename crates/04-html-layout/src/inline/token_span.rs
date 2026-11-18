//! Parallel token headers and direct f32 widths.
use super::metrics::MetricsView;
use super::storage::TokenData;
use super::tokens::{InlineToken, InlineTokenMetrics, TokenMetadata};
use std::ops::{Bound, Range, RangeBounds};
use std::slice::Iter;

#[derive(Clone, Copy)]
pub(super) struct TokenSpan<'a> {
    pub(super) metadata: &'a [u64],
    pub(super) widths: &'a [f32],
}
impl<'a> TokenSpan<'a> {
    #[inline]
    pub(super) fn len(self) -> usize {
        self.metadata.len()
    }
    #[inline]
    pub(super) fn is_empty(self) -> bool {
        self.metadata.is_empty()
    }
    #[inline]
    pub(super) fn widths(self) -> &'a [f32] {
        self.widths
    }
    #[inline]
    pub(super) fn metadata_at(self, index: usize) -> TokenMetadata {
        TokenMetadata::from_header(self.metadata[index])
    }
    #[inline]
    pub(super) fn stored_width_at(self, index: usize) -> f32 {
        self.widths[index]
    }
    #[inline]
    pub(super) fn metadata_iter(self) -> MetadataIter<'a> {
        MetadataIter(self.metadata.iter())
    }
    #[inline]
    pub(super) fn boundary_flags_at(self, index: usize) -> u8 {
        self.metadata_at(index).boundary_flags()
    }
    #[inline]
    pub(super) fn token_at(self, index: usize) -> InlineToken {
        InlineToken {
            header: self.metadata[index],
            width: self.stored_width_at(index),
        }
    }
    pub(super) fn iter(
        self,
    ) -> impl DoubleEndedIterator<Item = InlineToken> + ExactSizeIterator + 'a {
        (0..self.len()).map(move |index| self.token_at(index))
    }
    #[inline]
    pub(super) fn slice(self, range: impl RangeBounds<usize>) -> Self {
        let start = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&n) => n + 1,
            Bound::Excluded(&n) => n,
            Bound::Unbounded => self.len(),
        };
        Self {
            metadata: &self.metadata[start..end],
            widths: &self.widths[start..end],
        }
    }
    pub(super) fn append_to(self, tokens: &mut TokenData<'_>) {
        tokens.metadata.make_mut().extend_from_slice(self.metadata);
        tokens.widths.make_mut().extend_from_slice(self.widths);
    }
    pub(super) fn metrics_view(self, records: &'a [InlineTokenMetrics]) -> MetricsView<'a> {
        MetricsView::direct(records)
    }
    #[inline]
    pub(super) fn add_stored_widths(self, range: Range<usize>, initial: f64) -> f64 {
        self.widths[range]
            .iter()
            .fold(initial, |sum, &width| sum + f64::from(width))
    }
}
pub(super) struct MetadataIter<'a>(Iter<'a, u64>);
impl Iterator for MetadataIter<'_> {
    type Item = TokenMetadata;
    fn next(&mut self) -> Option<Self::Item> {
        self.0
            .next()
            .map(|&header| TokenMetadata::from_header(header))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}
impl DoubleEndedIterator for MetadataIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0
            .next_back()
            .map(|&header| TokenMetadata::from_header(header))
    }
}
impl ExactSizeIterator for MetadataIter<'_> {}
