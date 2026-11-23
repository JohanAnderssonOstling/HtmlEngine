//! Token-block geometry and paragraph features collected during preparation.
use super::{InlineToken, InlineTokenKind, InlineTokenMetrics, InlineVerticalAlign, TokenMetadata};
use crate::inline::TokenSpan;
use unicode_bidi::UNICODE_VERSION;

/// Bidi facts resolved from source geometry for a selected line.
#[derive(Clone, Copy, Default)]
pub(in crate::inline) struct InlineBidiSummary {
    pub(in crate::inline) has_odd_level: bool,
    // 255 is outside the valid bidi-level range and denotes mixed levels.
    uniform_level: u8,
}

impl InlineBidiSummary {
    #[inline]
    pub(in crate::inline) fn from_level(level: u8) -> Self {
        Self {
            has_odd_level: level % 2 == 1,
            uniform_level: level,
        }
    }

    #[inline]
    pub(in crate::inline) fn merge(&mut self, other: Self) {
        self.has_odd_level |= other.has_odd_level;
        if self.uniform_level != other.uniform_level {
            self.uniform_level = u8::MAX;
        }
    }

    #[inline]
    pub(in crate::inline) fn uniform_level(self) -> Option<u8> {
        (self.uniform_level != u8::MAX).then_some(self.uniform_level)
    }

    #[inline]
    pub(in crate::inline) fn is_uniform_at(self, level: u8) -> bool {
        self.uniform_level == level
    }
}

#[derive(Clone, Copy, Default)]
pub(in crate::inline) struct InlineSummaryBlock {
    // A plain block covers consecutive glyphs, so its end follows from the
    // fixed block size rather than occupying another field and padding.
    pub(in crate::inline) glyph_start: u32,
    pub(in crate::inline) preserves_newlines: bool,
    pub(in crate::inline) plain_text: bool,
    pub(in crate::inline) width: f64,
    pub(in crate::inline) max_ascent: f64,
    pub(in crate::inline) max_descent: f64,
}

// Summary geometry remains compact without retaining bidi levels.
const _: () = assert!(size_of::<InlineSummaryBlock>() == 32);

impl InlineSummaryBlock {
    pub(in crate::inline) fn build(
        tokens: TokenSpan<'_>,
        runs: &[InlineTokenMetrics],
        mut observe: impl FnMut(&InlineToken),
    ) -> Self {
        let facts = tokens.iter().map(|token| {
            observe(&token);
            (token.metadata(), token.width, token.run_idx())
        });
        Self::from_facts(facts, runs)
    }

    fn include_metrics(&mut self, metrics: &InlineTokenMetrics) {
        self.plain_text &= metrics.contributes_line_metrics
            && !metrics.placement_required
            && metrics.visible
            && matches!(metrics.vertical_align, InlineVerticalAlign::Baseline);
        self.preserves_newlines |= metrics.white_space.preserves_newlines();
        let ascent = metrics.ascent as f64;
        let descent = metrics.descent as f64;
        let half_leading = (f64::from(metrics.line_height) - (ascent + descent)) / 2.0;
        self.max_ascent = self.max_ascent.max(ascent + half_leading);
        self.max_descent = self.max_descent.max(descent + half_leading);
    }

    fn finish(mut self) -> Self {
        if !self.max_ascent.is_finite() {
            self.max_ascent = 0.0;
        }
        if !self.max_descent.is_finite() {
            self.max_descent = 0.0;
        }
        self
    }

    pub(in crate::inline) fn from_facts(
        tokens: impl ExactSizeIterator<Item = (TokenMetadata, f32, u32)>,
        runs: &[InlineTokenMetrics],
    ) -> Self {
        let mut summary = Self {
            glyph_start: u32::MAX,
            max_ascent: f64::NEG_INFINITY,
            max_descent: f64::NEG_INFINITY,
            plain_text: tokens.len() != 0,
            ..Self::default()
        };
        let mut expected_glyph = None;
        let mut previous_metrics = None;
        for (token, stored_width, index) in tokens {
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
                summary.plain_text = false;
                continue;
            };
            // Consecutive glyphs usually share metrics. Their eligibility and
            // extents contribute only once until the metrics index changes.
            if previous_metrics != Some(index) {
                summary.include_metrics(&runs[index as usize]);
                previous_metrics = Some(index);
            }
            summary.glyph_start = summary.glyph_start.min(glyph_idx);
            summary.plain_text &=
                expected_glyph.is_none_or(|expected| expected == glyph_idx) && !token.is_tab();
            expected_glyph = Some(glyph_idx + 1);
            summary.width += token.width(stored_width);
        }
        summary.finish()
    }
}

/// Paragraph properties collected with the summaries and retained with the
/// token plan. Reading them during composition never revisits token storage.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::inline) struct PreparedTokenFeatures {
    pub(in crate::inline) has_tabs: bool,
    pub(in crate::inline) preserves_spaces: bool,
    pub(in crate::inline) has_float_anchors: bool,
    pub(in crate::inline) flags: u8,
}

const _: () = assert!(size_of::<PreparedTokenFeatures>() == 4);

impl PreparedTokenFeatures {
    pub(in crate::inline) const NO_BIDI_GLYPH_SEPARATORS: u8 = 1 << 1;
    pub(in crate::inline) const HAS_BREAKS: u8 = 1 << 2;
    pub(in crate::inline) const DEFERRED_HYPHENATION: u8 = 1 << 3;
    // Construction-only fact; summary rebuilding clears it after expansion.
    const HAS_UNICODE_BREAKS: u8 = 1 << 4;

    pub(in crate::inline) fn has_unicode_breaks(self) -> bool {
        self.flags & Self::HAS_UNICODE_BREAKS != 0
    }

    pub(in crate::inline) fn record_unicode_break(&mut self) {
        self.flags |= Self::HAS_UNICODE_BREAKS;
    }

    pub(in crate::inline) fn deferred_hyphenation(self) -> bool {
        self.flags & Self::DEFERRED_HYPHENATION != 0
    }

    pub(in crate::inline) fn set_deferred_hyphenation(&mut self, deferred: bool) {
        self.flags = (self.flags & !Self::DEFERRED_HYPHENATION)
            | (u8::from(deferred) * Self::DEFERRED_HYPHENATION);
    }

    /// False is unknown. Text emission proves this from source characters;
    /// metadata-only summary rebuilds cannot establish it on their own.
    #[inline]
    pub(in crate::inline) fn no_bidi_glyph_separators(self) -> bool {
        self.flags & Self::NO_BIDI_GLYPH_SEPARATORS != 0
    }

    #[inline]
    pub(in crate::inline) fn set_no_bidi_glyph_separators(&mut self, proven: bool) {
        self.flags = (self.flags & !Self::NO_BIDI_GLYPH_SEPARATORS)
            | (u8::from(proven) * Self::NO_BIDI_GLYPH_SEPARATORS);
    }

    #[inline]
    pub(in crate::inline) fn no_bidi_separators(self) -> bool {
        self.flags & (Self::NO_BIDI_GLYPH_SEPARATORS | Self::HAS_BREAKS)
            == Self::NO_BIDI_GLYPH_SEPARATORS
    }
}

/// Unicode 16 B/S characters occur only among C0 controls, NEL and PARAGRAPH
/// SEPARATOR. Other data versions keep full classification until verified.
#[inline]
pub(in crate::inline) fn may_be_bidi_separator(character: char) -> bool {
    !matches!(UNICODE_VERSION, (16, 0, 0))
        || character <= '\u{001f}'
        || matches!(character, '\u{0085}' | '\u{2029}')
}
