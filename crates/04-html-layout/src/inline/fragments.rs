//! Shared line geometry and placement records used by composition and publication.
use html_engine_model::GlyphId;

use super::tokens::{InlineBidiSummary, InlineTokenKind, InlineTokenMetrics, TokenMetadata};
use crate::LayoutReader;
use kurbo::Vec2;
use std::iter::successors;
use std::ops::Range;

pub(super) fn inline_ancestors<'a>(
    reader: &'a LayoutReader<'_>,
    start: Option<usize>,
) -> impl Iterator<Item = usize> + 'a {
    successors(start, |&box_idx| reader.get_parent(box_idx))
        .take_while(|&box_idx| reader.is_inline_box(box_idx))
}

pub(super) fn parent_font_content_extents(
    reader: &LayoutReader<'_>,
    box_idx: usize,
) -> (f64, f64, f64) {
    let parent_idx = reader.get_parent(box_idx).unwrap_or(box_idx);
    let style = reader.style(parent_idx);
    let font_size = style.font_size() as f64;
    let metrics = reader.font_metrics(parent_idx);
    let (ascent, descent) = metrics.line_box_extents(font_size);
    (
        ascent,
        descent,
        f64::from(metrics.x_height_ratio) * font_size,
    )
}

pub(super) fn token_contributes_line_metrics(
    token: &TokenMetadata,
    metrics: &InlineTokenMetrics,
) -> bool {
    metrics.contributes_line_metrics
        && !matches!(
            token.kind(),
            InlineTokenKind::ZeroWidthBreak
                | InlineTokenKind::FloatAnchor { .. }
                | InlineTokenKind::AbsoluteAnchor { .. }
        )
}

#[derive(Clone, Copy)]
pub(super) struct TokenPlacement {
    pub(super) x: f64,
    pub(super) advance: f64,
    pub(super) y_offset: f32,
    pub(super) relative_offset: Vec2,
}

/// Full records are working storage for complex line measurement only. Before
/// publication they become streamed logical positions, run offsets, and an
/// optional visual x column. Paragraph caches never retain the full records.
pub(super) enum LinePlacements {
    Full(Box<[TokenPlacement]>),
    GlyphX(Box<[f64]>),
    Sequential { tab_origin: f64, publish: bool },
    Positioned(Box<PositionedLine>),
}

pub(super) struct PositionedLine {
    pub(super) xs: Box<[f64]>,
    pub(super) offsets: Box<[PlacementOffsetRun]>,
    pub(super) tab_origin: f64,
    pub(super) hyphen_index: Option<u32>,
}

pub(super) struct PlacementOffsetRun {
    pub(super) end: u32,
    pub(super) y: f32,
    pub(super) x: f64,
}

impl LinePlacements {
    pub(super) fn storage_bytes(&self) -> usize {
        match self {
            Self::Full(values) => values.len() * size_of::<TokenPlacement>(),
            Self::GlyphX(values) => values.len() * size_of::<f64>(),
            Self::Sequential { .. } => 0,
            Self::Positioned(line) => {
                size_of::<PositionedLine>()
                    + line.xs.len() * size_of::<f64>()
                    + line.offsets.len() * size_of::<PlacementOffsetRun>()
            }
        }
    }

    pub(super) fn compact(&mut self, tab_origin: f64, visual: bool, hyphen_index: Option<usize>) {
        let Self::Full(values) = self else {
            return;
        };
        let mut offsets: Vec<PlacementOffsetRun> = Vec::new();
        if values
            .iter()
            .any(|placement| placement.y_offset != 0.0 || placement.relative_offset.x != 0.0)
        {
            for (index, placement) in values.iter().enumerate() {
                let end = u32::try_from(index + 1).expect("line token count fits u32");
                if let Some(last) = offsets.last_mut().filter(|last| {
                    last.y.to_bits() == placement.y_offset.to_bits()
                        && last.x.to_bits() == placement.relative_offset.x.to_bits()
                }) {
                    last.end = end;
                } else {
                    offsets.push(PlacementOffsetRun {
                        end,
                        y: placement.y_offset,
                        x: placement.relative_offset.x,
                    });
                }
            }
        }
        if !visual && offsets.is_empty() {
            *self = Self::Sequential {
                tab_origin,
                publish: true,
            };
        } else {
            let xs = if visual {
                values.iter().map(|placement| placement.x).collect()
            } else {
                Box::default()
            };
            *self = Self::Positioned(Box::new(PositionedLine {
                xs,
                offsets: offsets.into_boxed_slice(),
                tab_origin,
                hyphen_index: hyphen_index
                    .map(|index| u32::try_from(index).expect("line token index fits u32")),
            }));
        }
    }
}

pub(super) struct LineLayout {
    pub(super) glyph_range: Range<u32>,
    /// Measurement proved the source glyphs cover this complete range.
    /// Position and bidi adjustments do not change that ownership proof.
    pub(super) owns_glyph_range: bool,
    pub(super) width: f64,
    pub(super) used_width: f64,
    pub(super) x_offset: f64,
    pub(super) line_height: f64,
    pub(super) baseline: f64,
    pub(super) word_spacing: f64,
    pub(super) letter_spacing: f64,
    /// Exact advances for justified spaces whose punctuation context gives
    /// them a different share of the line's bounded glue adjustment.
    pub(super) punctuation_space_advances: Vec<(u32, f32)>,
    pub(super) optical_offset_x: f64,
    pub(super) hyphen: Option<(GlyphId, f64)>,
    pub(super) publish_empty: bool,
    pub(super) publishes_line: bool,
    pub(super) has_tabs: bool,
    /// Logical text uses implicit or streamed positions. Reordered text retains
    /// x coordinates; complex content shares vertical and relative offset runs.
    pub(super) placements: Option<LinePlacements>,
}

/// Text presence and source ownership share one byte in the measured summary.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum TextCoverage {
    /// No source or synthetic text contributes to the line.
    None,
    /// Text is present, but its source-range coverage is unproven.
    Fragments,
    /// Glyph tokens cover every source index in the measured glyph range.
    WholeRange,
}

pub(super) struct TokenSpanSummary {
    pub(super) glyph_range: Range<u32>,
    pub(super) text_coverage: TextCoverage,
    pub(super) has_images: bool,
    pub(super) has_tabs: bool,
    pub(super) has_baseline_relative_image: bool,
    pub(super) plain_text: bool,
    pub(super) justifiable_text: bool,
    pub(super) preserves_newlines: bool,
    pub(super) bidi: InlineBidiSummary,
    pub(super) width: f64,
    pub(super) line_height: f64,
    pub(super) baseline: f64,
}

/// The vertical CSS contribution of one non-replaced inline box. These are
/// derived once for a selected line from retained box ownership; they do not
/// enlarge the dense token stream walked by line breaking.
#[derive(Clone, Copy)]
pub(super) struct InlineBoxStrut {
    pub(super) box_idx: usize,
    pub(super) ascent: f64,
    pub(super) descent: f64,
    pub(super) line_height: f64,
    pub(super) font_size: f64,
}

pub(super) fn inline_relative_position_offset(
    engine: &crate::LayoutEngine<'_, '_>,
    box_idx: usize,
    containing_width: f64,
) -> Vec2 {
    let mut offset = Vec2::ZERO;
    for current_idx in inline_ancestors(&engine.read.reader, Some(box_idx)) {
        let style = engine.read.reader.style(current_idx);
        if style.position() == html_style_model::PositionMode::Relative {
            offset += crate::boxes::relative_position_offset(&style, containing_width, None);
        }
    }
    offset
}
