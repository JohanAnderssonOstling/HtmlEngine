//! Paragraph-local metrics associations. Uniform spans load one record; mixed
//! spans advance through boundaries. Fragmented and rewritten tokens retain
//! direct indexing as a fallback.

use super::tokens::{InlineTokenMetrics, InlineTokens, TokenMetadata};
use super::{TokenRead, TokenSpan};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum MetricsMode {
    #[default]
    Unprepared,
    Uniform(u32),
    Ranges,
    Dense(u32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct MetricsRange {
    pub(super) end: u32,
    pub(super) metrics: u32,
}

#[derive(Clone, Copy)]
pub(super) enum MetricsView<'a> {
    Uniform(&'a InlineTokenMetrics),
    Ranges {
        source: &'a InlineTokens<'a>,
        offset: usize,
    },
    Dense(&'a [InlineTokenMetrics]),
}

impl<'a> MetricsView<'a> {
    /// Rewritten lines have no retained range map. A single record is still
    /// uniform; other rewritten content uses its token-local associations.
    pub(super) fn direct(records: &'a [InlineTokenMetrics]) -> Self {
        match records {
            [metrics] => Self::Uniform(metrics),
            _ => Self::Dense(records),
        }
    }

    pub(super) fn shifted(self, start: usize) -> Self {
        match self {
            Self::Ranges { source, offset } => Self::Ranges {
                source,
                offset: offset + start,
            },
            _ => self,
        }
    }

    pub(super) fn at(self, tokens: impl TokenRead, index: usize) -> &'a InlineTokenMetrics {
        match self {
            Self::Uniform(metrics) => metrics,
            Self::Ranges { source, offset } => {
                let ranges = &source.metrics_ranges;
                let records = &source.runs;
                let range = ranges.partition_point(|range| range.end as usize <= offset + index);
                &records[ranges[range].metrics as usize]
            }
            Self::Dense(records) => tokens.metadata_at(index).run_metrics(records),
        }
    }

    /// Select metrics once per chunk, then reuse the reference across its tokens.
    #[inline]
    pub(super) fn iter<'t>(
        self,
        tokens: TokenSpan<'t>,
    ) -> impl Iterator<Item = (TokenMetadata, &'a InlineTokenMetrics)> {
        self.chunks(tokens)
            .flat_map(|(tokens, metrics)| tokens.metadata_iter().map(move |token| (token, metrics)))
    }

    /// Early-exit scans select the association format outside their token
    /// loop, just like full folds. Dense scans load each header only once.
    #[inline]
    pub(super) fn any<F>(self, tokens: TokenSpan<'_>, mut predicate: F) -> bool
    where
        F: FnMut(TokenMetadata, &'a InlineTokenMetrics) -> bool,
    {
        match self {
            Self::Uniform(metrics) => tokens
                .metadata_iter()
                .any(|token| predicate(token, metrics)),
            Self::Dense(records) => tokens
                .metadata_iter()
                .any(|token| predicate(token, token.run_metrics(records))),
            Self::Ranges { .. } => self.chunks(tokens).any(|(tokens, metrics)| {
                tokens
                    .metadata_iter()
                    .any(|token| predicate(token, metrics))
            }),
        }
    }

    /// Streaming consumers select the association format outside their token
    /// loop. Dense associations reuse each header for both metrics and content.
    #[inline(always)]
    pub(super) fn fold<B, F>(self, tokens: TokenSpan<'_>, initial: B, mut f: F) -> B
    where
        F: FnMut(B, TokenMetadata, &'a InlineTokenMetrics) -> B,
    {
        match self {
            Self::Uniform(metrics) => tokens.metadata_iter().fold(
                initial,
                #[inline(always)]
                |state, token| f(state, token, metrics),
            ),
            Self::Dense(records) => tokens.metadata_iter().fold(
                initial,
                #[inline(always)]
                |state, token| f(state, token, token.run_metrics(records)),
            ),
            Self::Ranges { .. } => self.chunks(tokens).fold(
                initial,
                #[inline(always)]
                |state, (tokens, metrics)| {
                    tokens.metadata_iter().fold(
                        state,
                        #[inline(always)]
                        |state, token| f(state, token, metrics),
                    )
                },
            ),
        }
    }

    pub(super) fn chunks<'t>(self, tokens: TokenSpan<'t>) -> MetricsChunks<'a, 't> {
        let range = match self {
            Self::Ranges { source, offset } => source
                .metrics_ranges
                .partition_point(|range| range.end as usize <= offset),
            _ => 0,
        };
        MetricsChunks {
            view: self,
            tokens,
            position: 0,
            range,
        }
    }

    pub(super) fn cursor(self, token_len: usize) -> MetricsCursor<'a> {
        MetricsCursor {
            view: self,
            token_len,
            current: None,
            start: 0,
            end: 0,
            range: 0,
        }
    }
}

pub(super) struct MetricsChunks<'a, 't> {
    view: MetricsView<'a>,
    tokens: TokenSpan<'t>,
    position: usize,
    range: usize,
}

impl<'a, 't> Iterator for MetricsChunks<'a, 't> {
    type Item = (TokenSpan<'t>, &'a InlineTokenMetrics);

    fn next(&mut self) -> Option<Self::Item> {
        if self.position == self.tokens.len() {
            return None;
        }
        let start = self.position;
        let (end, metrics) = match self.view {
            MetricsView::Uniform(metrics) => (self.tokens.len(), metrics),
            MetricsView::Ranges { source, offset } => {
                let ranges = &source.metrics_ranges;
                let records = &source.runs;
                let range = ranges[self.range];
                self.range += 1;
                (
                    (range.end as usize - offset).min(self.tokens.len()),
                    &records[range.metrics as usize],
                )
            }
            MetricsView::Dense(records) => (
                start + 1,
                self.tokens.metadata_at(start).run_metrics(records),
            ),
        };
        self.position = end;
        Some((self.tokens.slice(start..end), metrics))
    }
}

/// Forward readers may skip whole summary blocks. They seek only when the
/// requested token leaves the current metrics range, not on every character.
/// Reuse the caller's already-loaded metadata for direct metrics associations.
pub(super) struct MetricsCursor<'a> {
    view: MetricsView<'a>,
    token_len: usize,
    current: Option<&'a InlineTokenMetrics>,
    start: usize,
    end: usize,
    range: usize,
}

impl<'a> MetricsCursor<'a> {
    /// Uniform/ranged associations share one record up to this exclusive end.
    /// Dense associations cannot prove a range without inspecting its tokens.
    #[inline]
    pub(super) fn shared_range(
        &mut self,
        index: usize,
    ) -> Option<(usize, &'a InlineTokenMetrics)> {
        match self.view {
            MetricsView::Uniform(metrics) => Some((self.token_len, metrics)),
            MetricsView::Ranges { source, offset } => {
                let metrics = self.range_at(index, source, offset);
                Some((self.end, metrics))
            }
            MetricsView::Dense(_) => None,
        }
    }

    #[inline]
    pub(super) fn at(&mut self, index: usize, token: TokenMetadata) -> &'a InlineTokenMetrics {
        match self.view {
            MetricsView::Uniform(metrics) => metrics,
            MetricsView::Ranges { source, offset } => self.range_at(index, source, offset),
            MetricsView::Dense(records) => token.run_metrics(records),
        }
    }

    #[inline]
    fn range_at(
        &mut self,
        index: usize,
        source: &'a InlineTokens<'a>,
        offset: usize,
    ) -> &'a InlineTokenMetrics {
        if index >= self.start && index < self.end {
            return self.current.expect("metrics cover the requested token");
        }
        self.seek_range(index, source, offset)
    }

    // Keep searches and cursor updates out of the per-token accessors.
    #[inline(never)]
    fn seek_range(
        &mut self,
        index: usize,
        source: &'a InlineTokens<'a>,
        offset: usize,
    ) -> &'a InlineTokenMetrics {
        let ranges = &source.metrics_ranges;
        let position = offset + index;
        if self.current.is_none() || index < self.start {
            self.range = ranges.partition_point(|range| range.end as usize <= position);
        } else {
            while ranges[self.range].end as usize <= position {
                self.range += 1;
            }
        }
        self.start = self.range.checked_sub(1).map_or(0, |previous| {
            (ranges[previous].end as usize).saturating_sub(offset)
        });
        self.end = (ranges[self.range].end as usize - offset).min(self.token_len);
        let metrics = &source.runs[ranges[self.range].metrics as usize];
        self.current = Some(metrics);
        metrics
    }
}
