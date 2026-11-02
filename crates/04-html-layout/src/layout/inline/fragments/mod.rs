//! Line measurement, token placement, and fragment publication.
//!
//! This module consumes already-selected line ranges and never decides where
//! the paragraph should wrap.

use super::*;
use crate::layout::LayoutReader;
use crate::layout_model::{LineInlineBoxFragment, LineTextFragment, paragraph_unicode_bidi};
use std::iter::successors;

fn inline_ancestors<'a>(
    reader: &'a LayoutReader<'_>,
    start: Option<usize>,
) -> impl Iterator<Item = usize> + 'a {
    successors(start, |&box_idx| reader.get_parent(box_idx))
        .take_while(|&box_idx| reader.is_inline_box(box_idx))
}

fn token_contributes_line_metrics(token: &TokenMetadata, metrics: &InlineTokenMetrics) -> bool {
    metrics.contributes_line_metrics
        && !matches!(
            token.kind(),
            InlineTokenKind::Opportunity
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
    xs: Box<[f64]>,
    offsets: Box<[PlacementOffsetRun]>,
    tab_origin: f64,
    hyphen_index: Option<u32>,
}

struct PlacementOffsetRun {
    end: u32,
    y: f32,
    x: f64,
}

// Keep positioned and implicit line descriptors as small as the old Vec.
const _: () = assert!(size_of::<Option<LinePlacements>>() == 3 * size_of::<usize>());

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

    fn compact(&mut self, tab_origin: f64, visual: bool, hyphen_index: Option<usize>) {
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

/// Column access is selected once per line. Readers never materialize a full
/// placement record for glyphs whose offsets are implicitly zero.
trait PlacementRead: Copy {
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
struct GlyphPositions<'a, S> {
    xs: &'a [f64],
    tokens: S,
}

impl<S: TokenRead> PlacementRead for GlyphPositions<'_, S> {
    fn x(self, index: usize) -> f64 {
        self.xs[index]
    }
    fn advance(self, index: usize) -> f64 {
        f64::from(self.tokens.stored_width_at(index))
    }
}

mod alignment;
mod bidi;
mod emission;
mod measurement;
mod preparation;
mod tracking;

use alignment::{adjusted_line_metrics, build_token_placements, can_stream_text_placements};
pub(super) use emission::write_line_fragments;
use emission::{inline_relative_position_offset, line_publishes};
#[cfg(any(test, feature = "testing"))]
pub(super) use measurement::measured_tracking_boundary_capacity;
use measurement::{measure_line, summarize_token_span};
pub(super) use preparation::first_line_color;
pub(super) use preparation::pseudo_line_heights;
use preparation::{
    apply_first_line_pseudo_styles, apply_inline_overflow, include_container_strut,
    inline_box_struts, parent_font_content_extents, strut_for_inline_box,
};
pub(super) use tracking::TrackingCapacityIndex;

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
    pub(super) hyphen: Option<(crate::GlyphId, f64)>,
    pub(super) publish_empty: bool,
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
struct InlineBoxStrut {
    box_idx: usize,
    ascent: f64,
    descent: f64,
    line_height: f64,
    font_size: f64,
}

/// Paragraph-wide inputs shared by every line emitted from one token plan.
pub(super) struct LineEmissionContext<'a> {
    tokens: &'a InlineTokens<'a>,
    input: &'a InlineFormattingInput,
    overflow: InlineOverflow,
    first_line_height: Option<f64>,
    first_letter_height: Option<f64>,
    rewrite_first_line_metrics: bool,
}

impl<'a> LineEmissionContext<'a> {
    pub(super) fn new(
        tokens: &'a InlineTokens<'a>,
        input: &'a InlineFormattingInput,
        overflow: InlineOverflow,
        first_line_height: Option<f64>,
        first_letter_height: Option<f64>,
    ) -> Self {
        // Text metrics resolve baseline alignment at token construction;
        // it cannot change with height. Prove unchanged metrics once for
        // this paragraph, without reading token headers or owner styles.
        let rewrite_first_line_metrics = first_letter_height.is_some()
            || first_line_height.is_some_and(|height| {
                tokens.runs.iter().any(|metrics| {
                    metrics.line_height.to_bits() != (height as f32).to_bits()
                        || (metrics.owner_box_idx != u32::MAX
                            && !matches!(metrics.vertical_align, InlineVerticalAlign::Baseline))
                })
            });
        Self {
            tokens,
            input,
            overflow,
            first_line_height,
            first_letter_height,
            rewrite_first_line_metrics,
        }
    }
}

pub(super) fn prepare_line(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    context: &LineEmissionContext<'_>,
    line: &BrokenLine,
    is_first_line: bool,
    origin: Point,
    line_index: usize,
    adjustments: &mut Vec<LineAdjustment>,
) -> (PreparedLine, f64, f64) {
    let tokens = context.tokens;
    let original_span = tokens.span().slice(line.start..line.end);
    let style = engine.reader.style(context.input.container_box_idx);
    let plaintext = paragraph_unicode_bidi(
        engine.reader.styles(),
        engine.reader.layout_tree(),
        context.input.container_box_idx,
    ) == UnicodeBidi::Plaintext;
    let direction = if plaintext {
        original_span
            .metadata_iter()
            .find_map(|token| match token.kind() {
                InlineTokenKind::Glyph { glyph_idx } => {
                    Some(engine.text.paragraph_direction(glyph_idx))
                }
                _ => None,
            })
            .unwrap_or(context.input.paragraph.direction)
    } else {
        context.input.paragraph.direction
    };
    let mut resolved_line = line.clone();
    if plaintext {
        use html_style_model::{LogicalTextAlign, TextDirection};
        let logical = if line.is_last_line {
            style.text_align_last_logical()
        } else {
            style.text_align_logical()
        };
        resolved_line.align = match (logical, direction) {
            (LogicalTextAlign::Start, TextDirection::Rtl)
            | (LogicalTextAlign::End, TextDirection::Ltr) => TextAlign::Right,
            (LogicalTextAlign::Start, TextDirection::Ltr)
            | (LogicalTextAlign::End, TextDirection::Rtl) => TextAlign::Left,
            _ => line.align,
        };
    }
    let line = &resolved_line;
    let mut overflow_adjusted = Vec::new();
    let span = apply_inline_overflow(
        &engine.reader,
        original_span,
        &tokens.runs,
        line.tab_origin,
        line.width_limit,
        context.overflow,
        &mut overflow_adjusted,
    );
    let mut pseudo_adjusted = Vec::new();
    let mut pseudo_runs = Vec::new();
    let is_first_line = is_first_line && context.input.starts_block;
    let (span, line_runs) = if is_first_line && context.rewrite_first_line_metrics {
        apply_first_line_pseudo_styles(
            engine,
            span,
            &tokens.runs,
            context.first_line_height,
            context.first_letter_height,
            &mut pseudo_adjusted,
            &mut pseudo_runs,
        )
    } else {
        (span, tokens.runs.as_ref())
    };
    let summary_offset = (span.same_storage(original_span)
        && line_runs.as_ptr() == tokens.runs.as_ptr())
    .then_some(line.start);
    let metrics = summary_offset.map_or_else(
        || MetricsView::direct(line_runs),
        |offset| tokens.metrics_view().shifted(offset),
    );
    let inline_struts = inline_box_struts(engine, context.input.container_box_idx, span, metrics);
    let mut summary = summarize_token_span(
        &engine.reader,
        span,
        metrics,
        &tokens.summary_blocks,
        summary_offset,
        line.tab_origin,
        &inline_struts,
    );
    include_container_strut(
        engine,
        context.input.container_box_idx,
        is_first_line.then_some(context.first_line_height).flatten(),
        &mut summary,
    );
    let empty_break_metrics = line
        .empty_segment_break
        .filter(|&index| index < tokens.len())
        .map(|index| *tokens.metrics_view().at(tokens.span(), index));
    let bidi = summary.bidi;
    let mut layout = measure_line(
        engine,
        span,
        metrics,
        summary,
        line,
        empty_break_metrics,
        &inline_struts,
        context.input.container_box_idx,
        context.input.area.width,
        direction,
    );
    bidi::place_visual_line(
        engine,
        span,
        line_runs,
        &tokens.replaced,
        line,
        direction,
        bidi,
        tokens.features.no_bidi_separators(),
        &mut layout,
    );
    let height = if line_publishes(engine, span, metrics, &tokens.replaced, &layout) {
        layout.line_height
    } else {
        0.0
    };
    let width = layout.width + line.indent;
    let adjusted = !span.same_storage(original_span) || span.len() != original_span.len();
    let pseudo = line_runs.as_ptr() != tokens.runs.as_ptr();
    if adjusted || pseudo {
        adjustments.push(LineAdjustment {
            line: line_index,
            tokens: adjusted.then(|| {
                if pseudo_adjusted.is_empty() {
                    overflow_adjusted.into_boxed_slice()
                } else {
                    pseudo_adjusted.into_boxed_slice()
                }
            }),
            runs: pseudo.then(|| pseudo_runs.into_boxed_slice()),
        });
    }
    let indent = if direction == TextDirection::Rtl {
        0.0
    } else {
        line.indent
    };
    (
        PreparedLine {
            tokens: line.start..line.end,
            origin: Point::new(origin.x + indent, origin.y),
            layout,
        },
        width,
        height,
    )
}
