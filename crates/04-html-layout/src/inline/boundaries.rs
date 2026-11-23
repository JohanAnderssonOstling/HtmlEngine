//! Break eligibility at content boundaries, independent of token storage.
use super::metrics::MetricsView;
use super::token_span::TokenSpan;
use super::tokens::{BreakKind, InlineTokenKind};

pub(super) const EMERGENCY: u8 = 1;
pub(super) const NORMAL_BEFORE: u8 = 2;
pub(super) const NORMAL_AFTER: u8 = 4;

#[derive(Clone, Copy, Default)]
pub(super) struct BreakBoundary {
    pub(super) normal: bool,
    pub(super) emergency: bool,
    pub(super) emergency_from_current: bool,
}

/// Ordinary opportunities are prepared independently of line width. Emergency
/// boundaries are considered only after ordinary opportunities fail to fit.
#[derive(Clone, Copy)]
pub(super) struct BoundaryReader<'a> {
    tokens: TokenSpan<'a>,
    metrics: MetricsView<'a>,
}

impl<'a> BoundaryReader<'a> {
    pub(super) fn new(tokens: TokenSpan<'a>, metrics: MetricsView<'a>) -> Self {
        Self { tokens, metrics }
    }

    pub(super) fn before(self, index: usize) -> BreakBoundary {
        let previous = if index == 0 {
            0
        } else {
            self.tokens.boundary_flags_at(index - 1)
        };
        if index == self.tokens.len() {
            return BreakBoundary {
                normal: previous & NORMAL_AFTER != 0,
                emergency: false,
                emergency_from_current: false,
            };
        }
        let flags = self.tokens.boundary_flags_at(index);
        let token = self.tokens.metadata_at(index);
        BreakBoundary {
            emergency_from_current: flags & EMERGENCY != 0,
            normal: flags & NORMAL_BEFORE != 0 || previous & NORMAL_AFTER != 0,
            emergency: index != 0
                && (flags | previous) & EMERGENCY != 0
                && matches!(token.kind(), InlineTokenKind::Glyph { .. })
                && token.is_cluster_boundary()
                && self
                    .metrics
                    .at(self.tokens, index)
                    .white_space
                    .allows_wrap(),
        }
    }

    /// Forced breaks, authored zero-width breaks and discretionary hyphens
    /// retain source/control records; both breakers classify them identically.
    pub(super) fn kind(self, index: usize) -> BreakKind {
        self.tokens.metadata_at(index).break_kind()
    }
}
