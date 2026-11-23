//! Token headers index paragraph-local metric records. Uniform plans load one
//! record; chunk readers group adjacent equal indices without a stored range map.

use super::TokenSpan;
use super::tokens::{InlineTokenMetrics, TokenMetadata};

#[derive(Clone, Copy)]
pub(super) enum MetricsView<'a> {
    Uniform(&'a InlineTokenMetrics),
    Indexed(&'a [InlineTokenMetrics]),
}

impl<'a> MetricsView<'a> {
    /// A single record covers every token, including metrics-only line overrides.
    pub(super) fn direct(records: &'a [InlineTokenMetrics]) -> Self {
        match records {
            [metrics] => Self::Uniform(metrics),
            _ => Self::Indexed(records),
        }
    }

    pub(super) fn at(self, tokens: TokenSpan<'_>, index: usize) -> &'a InlineTokenMetrics {
        match self {
            Self::Uniform(metrics) => metrics,
            Self::Indexed(records) => tokens.metadata_at(index).run_metrics(records),
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
            Self::Indexed(records) => tokens
                .metadata_iter()
                .any(|token| predicate(token, token.run_metrics(records))),
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
            Self::Indexed(records) => tokens.metadata_iter().fold(
                initial,
                #[inline(always)]
                |state, token| f(state, token, token.run_metrics(records)),
            ),
        }
    }

    pub(super) fn chunks<'t>(self, tokens: TokenSpan<'t>) -> MetricsChunks<'a, 't> {
        MetricsChunks {
            view: self,
            tokens,
            position: 0,
        }
    }

    pub(super) fn cursor(self, token_len: usize) -> MetricsCursor<'a> {
        MetricsCursor {
            view: self,
            token_len,
        }
    }
}

pub(super) struct MetricsChunks<'a, 't> {
    view: MetricsView<'a>,
    tokens: TokenSpan<'t>,
    position: usize,
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
            MetricsView::Indexed(records) => {
                let index = self.tokens.metadata_at(start).run_idx();
                let mut end = start + 1;
                while end < self.tokens.len() && self.tokens.metadata_at(end).run_idx() == index {
                    end += 1;
                }
                (end, &records[index as usize])
            }
        };
        self.position = end;
        Some((self.tokens.slice(start..end), metrics))
    }
}

/// Reuse already-loaded headers for metric lookup in forward scans.
pub(super) struct MetricsCursor<'a> {
    view: MetricsView<'a>,
    token_len: usize,
}

impl<'a> MetricsCursor<'a> {
    /// Only uniform associations prove a range without inspecting headers.
    #[inline]
    pub(super) fn shared_range(&mut self) -> Option<(usize, &'a InlineTokenMetrics)> {
        match self.view {
            MetricsView::Uniform(metrics) => Some((self.token_len, metrics)),
            MetricsView::Indexed(_) => None,
        }
    }

    #[inline]
    pub(super) fn at(&mut self, token: TokenMetadata) -> &'a InlineTokenMetrics {
        match self.view {
            MetricsView::Uniform(metrics) => metrics,
            MetricsView::Indexed(records) => token.run_metrics(records),
        }
    }
}
