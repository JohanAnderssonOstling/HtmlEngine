//! Construct compact or wide text fragments with shared merge rules.
use super::super::positions::PlacementRead;
use crate::inline::TokenSpan;
use crate::inline::metrics::MetricsView;
use crate::inline::reader::InlineReader;
use crate::inline::tokens::{InlineTokenKind, InlineTokenMetrics, ReplacedToken, TokenMetadata};
use html_engine_model::{CompactTextFragments, LineTextFragment, OwnedTextFragments};
use std::ptr;

/// Records independently positioned text fragments only for lines where
/// replaced inline content interrupts the source text. Text-only lines keep
/// the compact implicit representation (`None`, whole range at x=0).
/// Keep the streaming loops outside the ordinary writer's stack frame.
#[inline(never)]
pub(super) fn positioned_text_fragments(
    text: &InlineReader<'_>,
    replaced: &[ReplacedToken],
    span: TokenSpan<'_>,
    placements: impl PlacementRead,
    metrics_view: MetricsView<'_>,
    plain_reordered: bool,
) -> Option<OwnedTextFragments> {
    if !needs_positioned_text_fragments(text, replaced, span, metrics_view, plain_reordered) {
        return None;
    }

    // Every fragment contains at most one glyph per token, and paint order
    // is a token index. This bound proves the packed fields fit for the line.
    Some(if span.len() <= 1 << 14 {
        build_text_fragments::<CompactTextFragments>(text, span, placements, metrics_view)
    } else {
        build_text_fragments::<LineTextFragment>(text, span, placements, metrics_view)
    })
}

pub(super) fn needs_positioned_text_fragments(
    text: &InlineReader<'_>,
    replaced: &[ReplacedToken],
    span: TokenSpan<'_>,
    metrics_view: MetricsView<'_>,
    plain_reordered: bool,
) -> bool {
    let mut expected_glyph = None;
    plain_reordered
        || metrics_view.any(span, |token, metrics| {
            let discontinuous = match token.kind() {
                InlineTokenKind::Glyph { glyph_idx } => {
                    let discontinuous =
                        expected_glyph.is_some_and(|expected| expected != glyph_idx);
                    expected_glyph = Some(glyph_idx + 1);
                    discontinuous
                }
                _ => false,
            };
            // L1 can move trailing spaces out of an even-level embedded
            // run in an RTL paragraph. Retain its measured positions too.
            discontinuous
                || text.token_bidi_level(token, metrics, replaced) != 0
                || matches!(
                    token.kind(),
                    InlineTokenKind::Image { .. }
                        | InlineTokenKind::AtomicBox { .. }
                        | InlineTokenKind::InlineBoundary { .. }
                )
                || metrics.placement_required
                || !metrics.visible
        })
}

#[inline(never)]
fn build_text_fragments<R: TextFragmentRecord>(
    text: &InlineReader<'_>,
    span: TokenSpan<'_>,
    placements: impl PlacementRead,
    metrics_view: MetricsView<'_>,
) -> OwnedTextFragments {
    let mut builder = TextFragmentBuilder::<R> {
        fragments: Default::default(),
        has_fragment: false,
        current_metrics: None,
        current_level: None,
        previous_end: 0.0,
    };
    let mut paint_order = 0;
    metrics_view.fold(
        span,
        (),
        // Keep the update inside the concrete token loop.
        #[inline(always)]
        |(), token, metrics| {
            builder.append(
                text,
                paint_order,
                token,
                metrics,
                placements.x(paint_order),
                placements.relative_x(paint_order),
                || placements.advance(paint_order),
            );
            paint_order += 1;
        },
    );
    R::finish(builder.fragments)
}

pub(super) trait TextFragmentRecord {
    type Buffer: Default;
    fn push(buffer: &mut Self::Buffer, fragment: LineTextFragment);
    fn last_glyphs_end(buffer: &Self::Buffer) -> u32;
    fn extend_last_to(buffer: &mut Self::Buffer, end: u32, offset_x: f64);
    fn finish(buffer: Self::Buffer) -> OwnedTextFragments;
}

impl TextFragmentRecord for CompactTextFragments {
    type Buffer = Self;
    #[inline(always)]
    fn push(buffer: &mut Self, fragment: LineTextFragment) {
        buffer.push(fragment);
    }
    #[inline(always)]
    fn last_glyphs_end(buffer: &Self) -> u32 {
        buffer.last_glyphs_end()
    }
    #[inline(always)]
    fn extend_last_to(buffer: &mut Self, end: u32, offset_x: f64) {
        buffer.extend_last_to(end, offset_x);
    }
    fn finish(buffer: Self) -> OwnedTextFragments {
        OwnedTextFragments::Compact(buffer)
    }
}

impl TextFragmentRecord for LineTextFragment {
    type Buffer = Vec<Self>;
    #[inline(always)]
    fn push(buffer: &mut Vec<Self>, fragment: Self) {
        buffer.push(fragment);
    }
    #[inline(always)]
    fn last_glyphs_end(buffer: &Vec<Self>) -> u32 {
        buffer.last().expect("current text fragment").glyphs.end
    }
    #[inline(always)]
    fn extend_last_to(buffer: &mut Vec<Self>, end: u32, offset_x: f64) {
        let fragment = buffer.last_mut().expect("current text fragment");
        fragment.glyphs.end = end;
        fragment.offset_x = fragment.offset_x.min(offset_x);
    }
    fn finish(buffer: Vec<Self>) -> OwnedTextFragments {
        OwnedTextFragments::Wide(buffer.into_boxed_slice())
    }
}

/// Metrics chunks share one accumulator, updated in place by their concrete
/// glyph loops without moving its vector and cursor state on every token.
pub(super) struct TextFragmentBuilder<'m, R: TextFragmentRecord> {
    pub(super) fragments: R::Buffer,
    pub(super) has_fragment: bool,
    pub(super) current_metrics: Option<&'m InlineTokenMetrics>,
    pub(super) current_level: Option<u8>,
    pub(super) previous_end: f64,
}

impl<'m, R: TextFragmentRecord> TextFragmentBuilder<'m, R> {
    // Inline the per-glyph update into each concrete metadata loop.
    #[inline(always)]
    pub(super) fn append(
        &mut self,
        text: &InlineReader<'_>,
        paint_order: usize,
        token: TokenMetadata,
        metrics: &'m InlineTokenMetrics,
        x: f64,
        relative_x: f64,
        advance: impl FnOnce() -> f64,
    ) {
        if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
            let level = text.bidi_level(glyph_idx);
            let rtl = level % 2 == 1;
            let offset_x = x + relative_x;
            let boundary = token.is_cluster_boundary();
            let merge = self.has_fragment && {
                // Reject unmergeable glyphs using their existing flags before
                // reading the preceding fragment or its cached association.
                (!boundary || (!rtl && !metrics.placement_required))
                    && self.current_metrics.is_some_and(|previous| {
                        ptr::eq(previous, metrics)
                            || (!boundary
                                && *previous
                                    == InlineTokenMetrics {
                                        owner_box_idx: previous.owner_box_idx,
                                        ..*metrics
                                    })
                    })
                    && self.current_level == Some(level)
                    && R::last_glyphs_end(&self.fragments) == glyph_idx
                    && (!boundary || (x - self.previous_end).abs() < 0.001)
            };
            // Continuity uses advances only for matching LTR levels and
            // metrics without placement overrides. Other glyphs need no
            // width load or conversion during text-fragment publication.
            if !rtl && !metrics.placement_required {
                self.previous_end = x + advance();
            }
            if merge {
                R::extend_last_to(&mut self.fragments, glyph_idx + 1, offset_x);
            } else {
                R::push(
                    &mut self.fragments,
                    LineTextFragment {
                        glyphs: glyph_idx..glyph_idx + 1,
                        offset_x,
                        rtl,
                        paint_order: paint_order as u32,
                        visible: metrics.visible,
                    },
                );
                self.has_fragment = true;
                self.current_metrics = Some(metrics);
                self.current_level = Some(level);
            }
        } else {
            self.has_fragment = false;
            self.current_metrics = None;
        }
    }
}
