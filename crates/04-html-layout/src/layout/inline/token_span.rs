//! Read tokens directly from compact structure-of-arrays storage. Wide tokens
//! remain useful during construction and for paragraphs with exceptional state.
use super::metrics::{MetricsMode, MetricsView};
use super::storage::WidthTable;
use super::tokens::{InlineToken, InlineTokenMetrics, InlineTokens, TokenMetadata, TokenWrap};
use std::ops::{Bound, Range, RangeBounds};
use std::slice::Iter;

#[derive(Clone, Copy)]
pub(super) enum TokenSpan<'a> {
    Wide(&'a [InlineToken]),
    Compact {
        source: &'a InlineTokens<'a>,
        start: u32,
        len: u32,
    },
}

impl<'a> TokenSpan<'a> {
    #[inline]
    pub(super) fn len(self) -> usize {
        match self {
            Self::Wide(tokens) => tokens.len(),
            Self::Compact { len, .. } => len as usize,
        }
    }
    pub(super) fn is_empty(self) -> bool {
        self.len() == 0
    }
    /// Select the header storage once; scanning it never loads widths.
    #[inline]
    pub(super) fn metadata(self) -> MetadataView<'a> {
        match self {
            Self::Wide(tokens) => MetadataView::Wide(tokens),
            Self::Compact { source, start, len } => MetadataView::Compact(
                &source.metadata[start as usize..(start + len) as usize],
                source.compact_bidi_level(),
            ),
        }
    }
    #[inline]
    pub(super) fn widths(self) -> WidthsView<'a> {
        match self {
            Self::Wide(tokens) => WidthsView::Wide(tokens),
            Self::Compact { source, start, len } => {
                source.widths.view(start as usize..(start + len) as usize)
            }
        }
    }
    #[inline]
    pub(super) fn metadata_at(self, index: usize) -> TokenMetadata {
        self.metadata().at(index)
    }
    #[inline]
    pub(super) fn stored_width_at(self, index: usize) -> f32 {
        self.widths().at(index)
    }
    #[inline]
    pub(super) fn metadata_iter(self) -> MetadataIter<'a> {
        self.metadata().iter()
    }
    #[inline]
    pub(super) fn wrap_at(self, index: usize) -> TokenWrap {
        match self {
            Self::Wide(tokens) => tokens[index].wrap(),
            Self::Compact { source, len, .. } => {
                assert!(index < len as usize);
                TokenMetadata::from_defaults(source.compact_defaults.unwrap()).wrap()
            }
        }
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
        assert!(start <= end && end <= self.len());
        match self {
            Self::Wide(tokens) => Self::Wide(&tokens[start..end]),
            Self::Compact {
                source,
                start: offset,
                ..
            } => Self::Compact {
                source,
                start: offset + start as u32,
                len: (end - start) as u32,
            },
        }
    }
    /// Rewrites need token-local metrics indices. Resolve those associations
    /// only while copying the affected line, never while scanning normal text.
    pub(super) fn to_vec(self) -> Vec<InlineToken> {
        let mut tokens = Vec::with_capacity(self.len());
        self.append_to(&mut tokens);
        tokens
    }

    /// Materialize directly into rewrite storage, preserving its prefix and
    /// capacity without allocating and copying an intermediate token vector.
    pub(super) fn append_to(self, tokens: &mut Vec<InlineToken>) {
        match self {
            Self::Wide(source) => tokens.extend_from_slice(source),
            Self::Compact { source, start, .. } => {
                let mut range = source
                    .metrics_ranges
                    .partition_point(|range| range.end as usize <= start as usize);
                tokens.extend((0..self.len()).map(|offset| {
                    let mut token = InlineToken::from_compact(
                        source.metadata[start as usize + offset],
                        source.widths.view(0..source.widths.len()).at(start as usize + offset),
                        source.compact_defaults.unwrap(),
                    );
                    if source.metrics_mode == MetricsMode::Ranges {
                        while source.metrics_ranges[range].end as usize <= start as usize + offset {
                            range += 1;
                        }
                        token.set_run_idx(source.metrics_ranges[range].metrics);
                    }
                    token
                }))
            }
        }
    }
    pub(super) fn metrics_view(self, records: &'a [InlineTokenMetrics]) -> MetricsView<'a> {
        match self {
            Self::Wide(_) => MetricsView::direct(records),
            Self::Compact { source, start, .. } => source.metrics_view().shifted(start as usize),
        }
    }
    pub(super) fn same_storage(self, other: Self) -> bool {
        match (self, other) {
            (Self::Wide(a), Self::Wide(b)) => a.as_ptr() == b.as_ptr(),
            (
                Self::Compact {
                    source: a,
                    start: x,
                    ..
                },
                Self::Compact {
                    source: b,
                    start: y,
                    ..
                },
            ) => std::ptr::eq(a, b) && x == y,
            _ => false,
        }
    }
}
impl<'a> From<&'a [InlineToken]> for TokenSpan<'a> {
    fn from(tokens: &'a [InlineToken]) -> Self {
        Self::Wide(tokens)
    }
}

#[derive(Clone, Copy)]
pub(super) enum MetadataView<'a> {
    Wide(&'a [InlineToken]),
    Compact(&'a [u32], u8),
}
impl<'a> MetadataView<'a> {
    #[inline]
    pub(super) fn at(self, index: usize) -> TokenMetadata {
        match self {
            Self::Wide(tokens) => tokens[index].metadata(),
            Self::Compact(headers, level) => {
                TokenMetadata::from_compact_with_bidi(headers[index], level)
            }
        }
    }
    #[inline]
    pub(super) fn iter(self) -> MetadataIter<'a> {
        match self {
            Self::Wide(tokens) => MetadataIter::Wide(tokens.iter()),
            Self::Compact(headers, level) => MetadataIter::Compact(headers.iter(), level),
        }
    }
}
pub(super) enum MetadataIter<'a> {
    Wide(Iter<'a, InlineToken>),
    Compact(Iter<'a, u32>, u8),
}
impl Iterator for MetadataIter<'_> {
    type Item = TokenMetadata;
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Wide(tokens) => tokens.next().map(InlineToken::metadata),
            Self::Compact(headers, level) => headers
                .next()
                .map(|&header| TokenMetadata::from_compact_with_bidi(header, *level)),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Wide(tokens) => tokens.size_hint(),
            Self::Compact(headers, _) => headers.size_hint(),
        }
    }

    /// Keep the storage choice outside a short-circuiting header scan.
    #[inline]
    fn any<F>(&mut self, mut predicate: F) -> bool
    where
        F: FnMut(Self::Item) -> bool,
    {
        match self {
            Self::Wide(tokens) => tokens.any(|token| predicate(token.metadata())),
            Self::Compact(headers, level) => {
                let level = *level;
                headers
                    .any(|&header| predicate(TokenMetadata::from_compact_with_bidi(header, level)))
            }
        }
    }

    /// Fold-based consumers choose storage once for the whole metrics chunk.
    #[inline(always)]
    fn fold<B, F>(self, initial: B, mut f: F) -> B
    where
        F: FnMut(B, Self::Item) -> B,
    {
        match self {
            Self::Wide(tokens) => tokens.fold(
                initial,
                #[inline(always)]
                |state, token| f(state, token.metadata()),
            ),
            Self::Compact(headers, level) => headers.fold(
                initial,
                #[inline(always)]
                |state, &header| f(state, TokenMetadata::from_compact_with_bidi(header, level)),
            ),
        }
    }
}
impl DoubleEndedIterator for MetadataIter<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        match self {
            Self::Wide(tokens) => tokens.next_back().map(InlineToken::metadata),
            Self::Compact(headers, level) => headers
                .next_back()
                .map(|&header| TokenMetadata::from_compact_with_bidi(header, *level)),
        }
    }
}
impl ExactSizeIterator for MetadataIter<'_> {}
#[derive(Clone, Copy)]
pub(super) enum WidthsView<'a> {
    Wide(&'a [InlineToken]),
    Compact(&'a [f32]),
    Indexed { indexes: &'a [u8], values: &'a WidthTable },
}
impl<'a> WidthsView<'a> {
    #[inline]
    pub(super) fn at(self, index: usize) -> f32 {
        match self {
            Self::Wide(tokens) => tokens[index].width,
            Self::Compact(widths) => widths[index],
            Self::Indexed { indexes, values } => values[indexes[index] as usize],
        }
    }
    fn slice(self, range: Range<usize>) -> Self {
        match self {
            Self::Wide(tokens) => Self::Wide(&tokens[range]),
            Self::Compact(widths) => Self::Compact(&widths[range]),
            Self::Indexed { indexes, values } => Self::Indexed { indexes: &indexes[range], values },
        }
    }
    /// Add raw stored widths in logical order. The caller establishes that
    /// discretionary and tab adjustments are unnecessary for this span.
    #[inline]
    pub(super) fn add_to(self, initial: f64) -> f64 {
        match self {
            Self::Wide(tokens) => tokens
                .iter()
                .fold(initial, |sum, token| sum + f64::from(token.width)),
            Self::Compact(widths) => widths
                .iter()
                .fold(initial, |sum, &width| sum + f64::from(width)),
            Self::Indexed { indexes, values } => indexes.iter().fold(initial, |sum, &index| sum + f64::from(values[index as usize])),
        }
    }
}

/// Concrete readers let hot passes select storage once and specialize their
/// loops. Generic calls are monomorphized; this trait is never a trait object.
pub(super) trait TokenRead: Copy {
    /// Uniform readers seed bidi facts once, without inspecting later levels.
    const MAY_HAVE_MIXED_BIDI_LEVELS: bool = true;

    fn len(self) -> usize;
    fn metadata_at(self, index: usize) -> TokenMetadata;
    fn stored_width_at(self, index: usize) -> f32;
    /// Check the range once, then add its raw widths without header reads.
    fn add_stored_widths(self, range: Range<usize>, initial: f64) -> f64;
    fn wrap_at(self, index: usize) -> TokenWrap;
}

impl TokenRead for TokenSpan<'_> {
    #[inline]
    fn len(self) -> usize {
        TokenSpan::len(self)
    }
    #[inline]
    fn metadata_at(self, index: usize) -> TokenMetadata {
        TokenSpan::metadata_at(self, index)
    }
    #[inline]
    fn stored_width_at(self, index: usize) -> f32 {
        TokenSpan::stored_width_at(self, index)
    }
    #[inline]
    fn add_stored_widths(self, range: Range<usize>, initial: f64) -> f64 {
        self.slice(range).widths().add_to(initial)
    }
    #[inline]
    fn wrap_at(self, index: usize) -> TokenWrap {
        TokenSpan::wrap_at(self, index)
    }
}

impl TokenRead for &[InlineToken] {
    #[inline]
    fn len(self) -> usize {
        <[InlineToken]>::len(self)
    }
    #[inline]
    fn metadata_at(self, index: usize) -> TokenMetadata {
        self[index].metadata()
    }
    #[inline]
    fn stored_width_at(self, index: usize) -> f32 {
        self[index].width
    }
    #[inline]
    fn add_stored_widths(self, range: Range<usize>, initial: f64) -> f64 {
        WidthsView::Wide(&self[range]).add_to(initial)
    }
    #[inline]
    fn wrap_at(self, index: usize) -> TokenWrap {
        self[index].wrap()
    }
}

#[derive(Clone, Copy)]
pub(super) struct CompactTokens<'a> {
    metadata: &'a [u32],
    widths: WidthsView<'a>,
    wrap: TokenWrap,
    bidi_level: u8,
}

// The shared level occupies padding in this per-traversal reader.
const _: () = assert!(size_of::<CompactTokens<'static>>() <= 8 * size_of::<usize>());

impl<'a> CompactTokens<'a> {
    #[inline]
    pub(super) fn new(source: &'a InlineTokens<'a>, start: u32, len: u32) -> Self {
        let defaults =
            TokenMetadata::from_defaults(source.compact_defaults.expect("compact token defaults"));
        let range = start as usize..(start + len) as usize;
        Self {
            metadata: &source.metadata[range.clone()],
            widths: source.widths.view(range),
            wrap: defaults.wrap(),
            bidi_level: defaults.bidi_level(),
        }
    }
}
impl TokenRead for CompactTokens<'_> {
    const MAY_HAVE_MIXED_BIDI_LEVELS: bool = false;

    #[inline]
    fn len(self) -> usize {
        self.metadata.len()
    }
    #[inline]
    fn metadata_at(self, index: usize) -> TokenMetadata {
        TokenMetadata::from_compact_with_bidi(self.metadata[index], self.bidi_level)
    }
    #[inline]
    fn stored_width_at(self, index: usize) -> f32 {
        self.widths.at(index)
    }
    #[inline]
    fn add_stored_widths(self, range: Range<usize>, initial: f64) -> f64 {
        self.widths.slice(range).add_to(initial)
    }
    #[inline]
    fn wrap_at(self, index: usize) -> TokenWrap {
        debug_assert!(index < self.metadata.len());
        self.wrap
    }
}
