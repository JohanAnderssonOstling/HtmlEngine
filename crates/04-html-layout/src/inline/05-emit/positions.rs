//! Read prepared line positions for fragment construction and source-order publication.
use crate::LayoutReader;
use crate::inline::TokenSpan;
use crate::inline::fragments::{LineLayout, LinePlacements, TokenPlacement};
use crate::inline::metrics::MetricsView;
use crate::inline::reader::InlineReader;
use crate::inline::tokens::{InlineTokenKind, InlineTokenMetrics, TokenMetadata};
use html_text::spaced_advance;
use kurbo::Vec2;

/// Keep the full path's two accumulators and arithmetic order: natural
/// positions are measured first, then justification adds a separate offset.
pub(super) fn sequential_text_positions<'a>(
    text: &'a InlineReader<'_>,
    span: TokenSpan<'a>,
    metrics: MetricsView<'a>,
    line: &'a LineLayout,
) -> impl Iterator<Item = (TokenMetadata, &'a InlineTokenMetrics, f64, f64)> + 'a {
    let widths = span.widths();
    let mut natural_x = 0.0;
    let mut adjustment = 0.0;
    let adjusted = line.word_spacing != 0.0
        || line.letter_spacing != 0.0
        || !line.punctuation_space_advances.is_empty();
    metrics
        .iter(span)
        .enumerate()
        .map(move |(index, (token, metrics))| {
            let natural = token.width(widths[index]);
            let x = natural_x + adjustment;
            let advance = if adjusted && let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
                line.punctuation_space_advances
                    .iter()
                    .find(|&&(index, _)| index == glyph_idx)
                    .map(|&(_, advance)| f64::from(advance))
                    .unwrap_or_else(|| {
                        spaced_advance(
                            natural,
                            text.character(glyph_idx),
                            line.letter_spacing != 0.0
                                && text
                                    .has_tracking_boundary_after(glyph_idx, line.glyph_range.end),
                            line.word_spacing,
                            line.letter_spacing,
                        )
                    })
            } else {
                natural
            };
            natural_x += natural;
            adjustment += advance - natural;
            (token, metrics, x, advance)
        })
}

/// Preserve the working path's arithmetic order: tabs use natural logical
/// positions, while justification and bidi supply separate final positions.
pub(super) fn resolved_positions<'a, 'input: 'a>(
    reader: LayoutReader<'input>,
    text: InlineReader<'input>,
    span: TokenSpan<'a>,
    metrics: MetricsView<'a>,
    line: &'a LineLayout,
) -> impl Iterator<Item = (TokenMetadata, &'a InlineTokenMetrics, TokenPlacement)> + 'a {
    let (tab_origin, xs, offsets, hyphen_index) = match line.placements.as_ref() {
        Some(LinePlacements::Sequential { tab_origin, .. }) => {
            (*tab_origin, &[][..], &[][..], None)
        }
        Some(LinePlacements::Positioned(plan)) => (
            plan.tab_origin,
            plan.xs.as_ref(),
            plan.offsets.as_ref(),
            plan.hyphen_index,
        ),
        _ => unreachable!("resolved positions require a compact placement plan"),
    };
    let widths = span.widths();
    let mut natural_x = 0.0;
    let mut adjustment = 0.0;
    let mut offset_index = 0;
    let adjusted = line.word_spacing != 0.0
        || line.letter_spacing != 0.0
        || !line.punctuation_space_advances.is_empty();
    metrics
        .iter(span)
        .enumerate()
        .map(move |(index, (token, metrics))| {
            let natural =
                token.advance_with_metrics(widths[index], &reader, metrics, tab_origin + natural_x);
            let x = if xs.is_empty() {
                natural_x + adjustment
            } else {
                xs[index]
            };
            let mut advance = natural;
            if Some(index as u32) == hyphen_index {
                advance += token.discretionary_width(widths[index]);
            }
            if adjusted && let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
                advance = line
                    .punctuation_space_advances
                    .iter()
                    .find(|&&(index, _)| index == glyph_idx)
                    .map(|&(_, advance)| f64::from(advance))
                    .unwrap_or_else(|| {
                        spaced_advance(
                            advance,
                            text.character(glyph_idx),
                            line.letter_spacing != 0.0
                                && text
                                    .has_tracking_boundary_after(glyph_idx, line.glyph_range.end),
                            line.word_spacing,
                            line.letter_spacing,
                        )
                    });
            }
            natural_x += natural;
            adjustment += advance - natural;
            while offsets
                .get(offset_index)
                .is_some_and(|run| index >= run.end as usize)
            {
                offset_index += 1;
            }
            let (y_offset, relative_x) = offsets
                .get(offset_index)
                .map_or((0.0, 0.0), |run| (run.y, run.x));
            (
                token,
                metrics,
                TokenPlacement {
                    x,
                    advance,
                    y_offset,
                    relative_offset: Vec2::new(relative_x, 0.0),
                },
            )
        })
}

/// Column access is selected once per line. Readers never materialize a full
/// placement record for glyphs whose offsets are implicitly zero.
pub(super) trait PlacementRead: Copy {
    fn x(self, index: usize) -> f64;
    fn advance(self, index: usize) -> f64;
    fn y_offset(self, _index: usize) -> f32 {
        0.0
    }
    fn relative_x(self, _index: usize) -> f64 {
        0.0
    }
}

impl PlacementRead for &[TokenPlacement] {
    fn x(self, index: usize) -> f64 {
        self[index].x
    }
    fn advance(self, index: usize) -> f64 {
        self[index].advance
    }
    fn y_offset(self, index: usize) -> f32 {
        self[index].y_offset
    }
    fn relative_x(self, index: usize) -> f64 {
        self[index].relative_offset.x
    }
}

#[derive(Clone, Copy)]
pub(super) struct GlyphPositions<'a> {
    pub(super) xs: &'a [f64],
    pub(super) tokens: TokenSpan<'a>,
}

impl PlacementRead for GlyphPositions<'_> {
    fn x(self, index: usize) -> f64 {
        self.xs[index]
    }
    fn advance(self, index: usize) -> f64 {
        f64::from(self.tokens.stored_width_at(index))
    }
}

/// Reordered text without offsets or tabs can read its x column directly.
/// Compute advances only when a consumer actually needs them.
#[derive(Clone, Copy)]
pub(super) struct VisualPositions<'a> {
    pub(super) xs: &'a [f64],
    pub(super) tokens: TokenSpan<'a>,
    pub(super) text: InlineReader<'a>,
    pub(super) line: &'a LineLayout,
    pub(super) hyphen_index: Option<u32>,
}

impl PlacementRead for VisualPositions<'_> {
    fn x(self, index: usize) -> f64 {
        self.xs[index]
    }
    fn advance(self, index: usize) -> f64 {
        let token = self.tokens.metadata_at(index);
        let width = self.tokens.stored_width_at(index);
        let mut advance = token.width(width);
        if Some(index as u32) == self.hyphen_index {
            advance += token.discretionary_width(width);
        }
        if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
            self.line
                .punctuation_space_advances
                .iter()
                .find(|&&(index, _)| index == glyph_idx)
                .map(|&(_, advance)| f64::from(advance))
                .unwrap_or_else(|| {
                    spaced_advance(
                        advance,
                        self.text.character(glyph_idx),
                        self.line.letter_spacing != 0.0
                            && self
                                .text
                                .has_tracking_boundary_after(glyph_idx, self.line.glyph_range.end),
                        self.line.word_spacing,
                        self.line.letter_spacing,
                    )
                })
        } else {
            advance
        }
    }
}
