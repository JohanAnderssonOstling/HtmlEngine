//! Shared token storage; encoding, metrics, summaries and text rules have private owners.
mod encoding;
mod metrics;
mod summary;
mod text;

use crate::inline::TokenSpan;
use crate::inline::metrics::MetricsView;
use crate::inline::storage::{TokenBuffer, TokenData};
pub(super) use encoding::{InlineToken, InlineTokenKind, TokenMetadata};
use html_engine_model::TextCompositionPolicy;
use kurbo::{Point, Size, Vec2};
pub(crate) use metrics::indent_for_line;
#[cfg(any(test, feature = "testing"))]
pub(super) use metrics::vertical_align_offset;
pub(super) use metrics::{
    InlineTokenMetrics, InlineVerticalAlign, compute_baseline, has_aligned_inline_ancestor,
    text_vertical_align, vertical_align_metrics_offset,
};
use std::ops::Range;
pub(super) use summary::{
    InlineBidiSummary, InlineSummaryBlock, PreparedTokenFeatures, may_be_bidi_separator,
};
pub(crate) use text::{BreakKind, break_kind, preserved_tab_metrics};
pub(super) use text::{canonical_text_unit_from_facts, token_is_collapsible_line_edge_space};

/// Normal justification consumes at most the glue declared by the paragraph:
/// 50% space expansion or 33% contraction, followed by bounded cluster tracking.
/// A separate emergency pass may choose an underfull line; rendering still
/// observes the same space and tracking limits.
pub(super) const KP_TOLERANCE: f64 = 1.0;
pub(super) const HYPHEN_PENALTY: f64 = 50.0;

/// Keep line-breaking plans only when the token stream is substantial enough
/// to amortize their retained storage across later layouts.
pub(super) const INLINE_KP_CACHE_MIN_TOKENS: usize = 64;
pub(super) const INLINE_SUMMARY_BLOCK_TOKENS: usize = 16;

/// Prepared token data. Construction and prose segmentation may mutate storage;
/// line composition and publication borrow it read-only.
#[derive(Default)]
pub(super) struct InlineTokens<'a> {
    pub(super) data: TokenData<'a>,
    pub(super) runs: TokenBuffer<'a, InlineTokenMetrics>,
    pub(super) summary_blocks: TokenBuffer<'a, InlineSummaryBlock>,
    pub(super) replaced: TokenBuffer<'a, ReplacedToken>,
    pub(super) features: PreparedTokenFeatures,
}

impl<'a> InlineTokens<'a> {
    pub(super) fn span(&self) -> TokenSpan<'_> {
        self.data.span()
    }
    pub(super) fn len(&self) -> usize {
        self.span().len()
    }
    pub(super) fn metadata_iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = TokenMetadata> + ExactSizeIterator {
        self.span().metadata_iter()
    }
    pub(super) fn metrics_view(&self) -> MetricsView<'_> {
        MetricsView::direct(&self.runs)
    }

    /// Copy one paragraph's token columns while moving its borrowed
    /// metric and payload views temporarily. Restore those views after emission.
    pub(super) fn take_paragraph(
        &mut self,
        range: Range<usize>,
        breaks: Option<(usize, usize)>,
    ) -> Self {
        let start = range.start;
        let data = self
            .span()
            .slice(range)
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                !breaks.is_some_and(|(a, b)| start + *index == a || start + *index == b)
            })
            .map(|(_, token)| token)
            .collect();
        let mut paragraph = Self {
            data,
            runs: std::mem::take(&mut self.runs),
            replaced: std::mem::take(&mut self.replaced),
            features: PreparedTokenFeatures {
                flags: self.features.flags
                    & (PreparedTokenFeatures::NO_BIDI_GLYPH_SEPARATORS
                        | PreparedTokenFeatures::DEFERRED_HYPHENATION),
                ..PreparedTokenFeatures::default()
            },
            ..Self::default()
        };
        paragraph.rebuild_summary_blocks();
        paragraph
    }

    pub(super) fn restore_paragraph(&mut self, paragraph: Self) {
        self.runs = paragraph.runs;
        self.replaced = paragraph.replaced;
    }

    #[cfg(any(test, feature = "testing"))]
    pub(super) fn with_capacity(capacity: usize) -> Self {
        let mut tokens = Self::default();
        tokens.data.reserve(capacity);
        tokens
            .summary_blocks
            .reserve(capacity.div_ceil(INLINE_SUMMARY_BLOCK_TOKENS));
        tokens
    }

    pub(super) fn bind_metrics_in(
        runs: &mut TokenBuffer<'_, InlineTokenMetrics>,
        previous_run_idx: Option<u32>,
        token: &mut InlineToken,
        metrics: InlineTokenMetrics,
    ) -> u32 {
        if let Some(run_idx) = previous_run_idx
            && runs
                .get(run_idx as usize)
                .is_some_and(|previous| *previous == metrics)
        {
            token.set_run_idx(run_idx);
            return run_idx;
        }

        let run_idx = runs
            .iter()
            .position(|candidate| *candidate == metrics)
            .unwrap_or_else(|| {
                runs.push(metrics);
                runs.len() - 1
            });
        let run_idx = u32::try_from(run_idx).expect("inline run-metrics arena capacity exhausted");
        token.set_run_idx(run_idx);
        run_idx
    }

    pub(super) fn bind_metrics(
        &mut self,
        mut token: InlineToken,
        metrics: InlineTokenMetrics,
    ) -> InlineToken {
        let previous_run_idx = self
            .data
            .metadata
            .last()
            .map(|&header| TokenMetadata::from_header(header).run_idx());
        Self::bind_metrics_in(&mut self.runs, previous_run_idx, &mut token, metrics);
        token
    }

    pub(super) fn push(&mut self, token: InlineToken, metrics: InlineTokenMetrics) {
        // Generic pushes do not supply source characters. Preserve the proof
        // only for synthetic/replaced tokens; text emission tracks glyphs.
        match token.kind() {
            InlineTokenKind::Glyph { .. } => {
                self.features.flags &= !PreparedTokenFeatures::NO_BIDI_GLYPH_SEPARATORS;
            }
            InlineTokenKind::Break { .. } => {
                self.features.flags |= PreparedTokenFeatures::HAS_BREAKS;
            }
            _ => {}
        }
        let token = self.bind_metrics(token, metrics);
        self.data.push(token);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.span().is_empty()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PreparedInlinePlanKey {
    pub(super) run_start: u32,
    pub(super) run_end: u32,
    pub(super) container_box_idx: u32,
    pub(super) composition: TextCompositionPolicy,
}

/// Large, uncommon replaced-element geometry lives outside the token
/// columns. Most documents scan only headers and widths for every text
/// pass instead of dragging image-sized enum storage through cache lines.
#[derive(Clone, PartialEq)]
pub(super) enum ReplacedToken {
    Image {
        image_idx: u32,
        box_idx: u32,
        content_size: Size,
        border_size: Size,
        content_inset: Point,
        margin_left: f64,
        margin_top: f64,
        position_offset: Vec2,
        set_box_geometry: bool,
    },
    AtomicBox {
        box_idx: u32,
        border_size: Size,
        containing_width: f64,
        containing_height: Option<f64>,
        margin_left: f64,
        margin_top: f64,
    },
    InlineBoundary {
        box_idx: u32,
        margin_left: f64,
        left_inset: f64,
        inline_start: bool,
        inline_end: bool,
    },
}
