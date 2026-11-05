//! Durable text, box, image, and glyph-fragment publication.

use html_engine_model::shaping_contracts;
use html_style_model::computed_style;
use super::*;
use crate::layout::inline_reader::InlineReader;
use crate::layout::{LayoutEngine, resolve_vertical_size};
use crate::layout_model::{CompactTextFragments, GroupedArena, OwnedTextFragments};
use html_text::spaced_advance;
use std::ptr;

/// Records independently positioned text fragments only for lines where
/// replaced inline content interrupts the source text. Text-only lines keep
/// the compact implicit representation (`None`, whole range at x=0).
/// Keep the streaming loops outside the ordinary writer's stack frame.
#[inline(never)]
pub(super) fn positioned_text_fragments(
    span: TokenSpan<'_>,
    placements: impl PlacementRead,
    metrics_view: MetricsView<'_>,
    plain_reordered: bool,
) -> Option<OwnedTextFragments> {
    profile_scope!("fragments.positioned_text");
    if !needs_positioned_text_fragments(span, metrics_view, plain_reordered) {
        return None;
    }

    // Every fragment contains at most one glyph per token, and paint order
    // is a token index. This bound proves the packed fields fit for the line.
    Some(if span.len() <= 1 << 14 {
        build_text_fragments::<CompactTextFragments>(span, placements, metrics_view)
    } else {
        build_text_fragments::<LineTextFragment>(span, placements, metrics_view)
    })
}

fn needs_positioned_text_fragments(
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
                || token.bidi_level() != 0
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

trait TextFragmentRecord {
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
struct TextFragmentBuilder<'m, R: TextFragmentRecord> {
    fragments: R::Buffer,
    has_fragment: bool,
    current_metrics: Option<&'m InlineTokenMetrics>,
    current_level: Option<u8>,
    previous_end: f64,
}

impl<'m, R: TextFragmentRecord> TextFragmentBuilder<'m, R> {
    // Inline the per-glyph update into each concrete metadata loop.
    #[inline(always)]
    fn append(
        &mut self,
        paint_order: usize,
        token: TokenMetadata,
        metrics: &'m InlineTokenMetrics,
        x: f64,
        relative_x: f64,
        advance: impl FnOnce() -> f64,
    ) {
        if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
            let level = token.bidi_level();
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

/// Keep the full path's two accumulators and arithmetic order: natural
/// positions are measured first, then justification adds a separate offset.
fn sequential_text_positions<'a>(
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
            let natural = token.width(widths.at(index));
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

fn sequential_text_fragments<R: TextFragmentRecord>(
    text: &InlineReader<'_>,
    span: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    line: &LineLayout,
) -> OwnedTextFragments {
    let mut builder = TextFragmentBuilder::<R> {
        fragments: Default::default(),
        has_fragment: false,
        current_metrics: None,
        current_level: None,
        previous_end: 0.0,
    };
    for (index, (token, metrics, x, advance)) in
        sequential_text_positions(text, span, metrics, line).enumerate()
    {
        builder.append(index, token, metrics, x, 0.0, || advance);
    }
    R::finish(builder.fragments)
}

/// Positioned spacing often produces consecutive one-glyph fragments. Prove
/// that shape once, then write coordinates and one shared metadata run.
fn regular_sequential_fragments(
    text: &InlineReader<'_>,
    span: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    line: &LineLayout,
) -> Option<OwnedTextFragments> {
    let first = span.metadata_iter().next()?;
    let InlineTokenKind::Glyph { glyph_idx: start } = first.kind() else {
        return None;
    };
    let visible = metrics.at(span, 0).visible;
    if !metrics
        .chunks(span)
        .all(|(_, metrics)| metrics.placement_required && metrics.visible == visible)
    {
        return None;
    }
    let mut expected = start;
    if !span.metadata_iter().all(|token| {
        let matches = token.kind()
            == InlineTokenKind::Glyph {
                glyph_idx: expected,
            }
            && token.is_cluster_boundary()
            && token.bidi_level() == 0
            && !token.is_tab();
        expected += 1;
        matches
    }) {
        return None;
    }
    let fragments = if line.word_spacing == 0.0
        && line.letter_spacing == 0.0
        && line.punctuation_space_advances.is_empty()
    {
        let widths = span.widths();
        let mut x = 0.0;
        let offsets = (0..span.len()).map(|index| {
            let position = x;
            x += f64::from(widths.at(index));
            position
        });
        CompactTextFragments::from_glyph_offsets(start..expected, 0, visible, offsets)
    } else {
        CompactTextFragments::from_glyph_offsets(
            start..expected,
            0,
            visible,
            sequential_text_positions(text, span, metrics, line).map(|(_, _, x, _)| x),
        )
    };
    Some(OwnedTextFragments::Compact(fragments))
}

#[inline(never)]
fn positioned_sequential_text(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    span: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    replaced: &[ReplacedToken],
    line: &LineLayout,
    containing_width: f64,
) -> (Option<OwnedTextFragments>, Vec<LineInlineBoxFragment>) {
    let text = needs_positioned_text_fragments(span, metrics, false).then(|| {
        if span.len() <= 1 << 14 {
            regular_sequential_fragments(&engine.text, span, metrics, line).unwrap_or_else(|| {
                sequential_text_fragments::<CompactTextFragments>(&engine.text, span, metrics, line)
            })
        } else {
            sequential_text_fragments::<LineTextFragment>(&engine.text, span, metrics, line)
        }
    });
    let has_owners = metrics.chunks(span).any(|(_, metrics)| {
        let owner = metrics.owner_box_idx;
        owner_has_inline_fragments(engine, (owner != u32::MAX).then_some(owner as usize))
    });
    let boxes = if has_owners {
        positioned_inline_box_fragments_from(
            engine,
            container_box_idx,
            sequential_text_positions(&engine.text, span, metrics, line)
                .map(|(token, metrics, x, advance)| ((token, metrics), (x, advance, 0.0))),
            false,
            replaced,
            line.line_height,
            line.baseline,
            containing_width,
        )
    } else {
        Vec::new()
    };
    (text, boxes)
}

/// Reordered text without offsets or tabs can read its x column directly.
/// Compute advances only when a consumer actually needs them.
#[derive(Clone, Copy)]
struct VisualPositions<'a, S> {
    xs: &'a [f64],
    tokens: S,
    text: InlineReader<'a>,
    line: &'a LineLayout,
    hyphen_index: Option<u32>,
}

impl<S: TokenRead> PlacementRead for VisualPositions<'_, S> {
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

/// Preserve the working path's arithmetic order: tabs use natural logical
/// positions, while justification and bidi supply separate final positions.
fn resolved_positions<'a, 'input: 'a>(
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
            let natural = token.advance_with_metrics(
                widths.at(index),
                &reader,
                metrics,
                tab_origin + natural_x,
            );
            let x = if xs.is_empty() {
                natural_x + adjustment
            } else {
                xs[index]
            };
            let mut advance = natural;
            if Some(index as u32) == hyphen_index {
                advance += token.discretionary_width(widths.at(index));
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

fn resolved_text_fragments<R: TextFragmentRecord>(
    engine: &LayoutEngine<'_, '_>,
    span: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    line: &LineLayout,
) -> OwnedTextFragments {
    let mut builder = TextFragmentBuilder::<R> {
        fragments: Default::default(),
        has_fragment: false,
        current_metrics: None,
        current_level: None,
        previous_end: 0.0,
    };
    for (index, (token, metrics, placement)) in
        resolved_positions(engine.reader, engine.text, span, metrics, line).enumerate()
    {
        builder.append(
            index,
            token,
            metrics,
            placement.x,
            placement.relative_offset.x,
            || placement.advance,
        );
    }
    R::finish(builder.fragments)
}

#[inline(never)]
fn positioned_resolved_fragments(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    span: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    replaced: &[ReplacedToken],
    line: &LineLayout,
    containing_width: f64,
) -> (Option<OwnedTextFragments>, Vec<LineInlineBoxFragment>) {
    if let Some(LinePlacements::Positioned(plan)) = line.placements.as_ref()
        && !plan.xs.is_empty()
        && plan.offsets.is_empty()
        && span.metadata_iter().all(|token| {
            !token.is_tab()
                && matches!(
                    token.kind(),
                    InlineTokenKind::Glyph { .. }
                        | InlineTokenKind::Discretionary { .. }
                        | InlineTokenKind::Opportunity
                        | InlineTokenKind::Break { .. }
                )
        })
        && needs_positioned_text_fragments(span, metrics, false)
    {
        match span {
            TokenSpan::Wide(tokens) => {
                return positioned_glyph_fragments(
                    engine,
                    container_box_idx,
                    span,
                    VisualPositions {
                        xs: &plan.xs,
                        tokens,
                        text: engine.text,
                        line,
                        hyphen_index: plan.hyphen_index,
                    },
                    metrics,
                    replaced,
                    line,
                    containing_width,
                );
            }
            TokenSpan::Compact { source, start, len } => {
                return positioned_glyph_fragments(
                    engine,
                    container_box_idx,
                    span,
                    VisualPositions {
                        xs: &plan.xs,
                        tokens: CompactTokens::new(source, start, len),
                        text: engine.text,
                        line,
                        hyphen_index: plan.hyphen_index,
                    },
                    metrics,
                    replaced,
                    line,
                    containing_width,
                );
            }
        }
    }
    let text = needs_positioned_text_fragments(span, metrics, false).then(|| {
        if span.len() <= 1 << 14 {
            resolved_text_fragments::<CompactTextFragments>(engine, span, metrics, line)
        } else {
            resolved_text_fragments::<LineTextFragment>(engine, span, metrics, line)
        }
    });
    let has_owners = has_inline_fragment_owners(engine, span, metrics, replaced);
    let boxes = if has_owners {
        positioned_inline_box_fragments_from(
            engine,
            container_box_idx,
            resolved_positions(engine.reader, engine.text, span, metrics, line).map(
                |(token, metrics, placement)| {
                    (
                        (token, metrics),
                        (placement.x, placement.advance, placement.y_offset),
                    )
                },
            ),
            span.metadata_iter()
                .any(|token| token.bidi_level() % 2 == 1),
            replaced,
            line.line_height,
            line.baseline,
            containing_width,
        )
    } else {
        Vec::new()
    };
    (text, boxes)
}

pub(super) fn positioned_inline_box_fragments(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    span: TokenSpan<'_>,
    placements: &[TokenPlacement],
    metrics: MetricsView<'_>,
    replaced: &[ReplacedToken],
    line_height: f64,
    baseline: f64,
    containing_width: f64,
) -> Vec<LineInlineBoxFragment> {
    positioned_inline_box_fragments_from(
        engine,
        container_box_idx,
        metrics.iter(span).zip(
            placements
                .iter()
                .map(|placement| (placement.x, placement.advance, placement.y_offset)),
        ),
        span.metadata_iter()
            .any(|token| token.bidi_level() % 2 == 1),
        replaced,
        line_height,
        baseline,
        containing_width,
    )
}

fn positioned_plain_inline_box_fragments(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    span: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    replaced: &[ReplacedToken],
    line: &LineLayout,
    containing_width: f64,
) -> Vec<LineInlineBoxFragment> {
    profile_scope!("fragments.plain_inline_boxes");
    // Plain lines contain only glyphs, whose fragment owner is the metrics
    // owner. Uniform/ranged metrics can reject an undecorated line without
    // loading token metadata, widths, or source glyphs.
    if !metrics.chunks(span).any(|(_, metrics)| {
        let owner = metrics.owner_box_idx;
        owner_has_inline_fragments(engine, (owner != u32::MAX).then_some(owner as usize))
    }) {
        return Vec::new();
    }
    let mut x = 0.0;
    let widths = span.widths();
    let placements = metrics
        .iter(span)
        .enumerate()
        .map(|(index, (token, metrics))| {
            let natural = token.advance_with_metrics(widths.at(index), &engine.reader, metrics, x);
            let advance = if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
                line.punctuation_space_advances
                    .iter()
                    .find(|&&(index, _)| index == glyph_idx)
                    .map(|&(_, advance)| f64::from(advance))
                    .unwrap_or_else(|| {
                        spaced_advance(
                            natural,
                            engine.text.character(glyph_idx),
                            line.letter_spacing != 0.0
                                && engine
                                    .text
                                    .has_tracking_boundary_after(glyph_idx, line.glyph_range.end),
                            line.word_spacing,
                            line.letter_spacing,
                        )
                    })
            } else {
                natural
            };
            let position = x;
            x += advance;
            ((token, metrics), (position, advance, 0.0))
        });
    positioned_inline_box_fragments_from(
        engine,
        container_box_idx,
        placements,
        false,
        replaced,
        line.line_height,
        line.baseline,
        containing_width,
    )
}

#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn positioned_glyph_fragments(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    span: TokenSpan<'_>,
    positions: impl PlacementRead,
    metrics: MetricsView<'_>,
    replaced: &[ReplacedToken],
    line: &LineLayout,
    containing_width: f64,
) -> (Option<OwnedTextFragments>, Vec<LineInlineBoxFragment>) {
    let text = positioned_text_fragments(span, positions, metrics, true);
    let has_owners = metrics.chunks(span).any(|(_, metrics)| {
        let owner = metrics.owner_box_idx;
        owner_has_inline_fragments(engine, (owner != u32::MAX).then_some(owner as usize))
    });
    let boxes = if has_owners {
        positioned_inline_box_fragments_from(
            engine,
            container_box_idx,
            metrics
                .iter(span)
                .enumerate()
                .map(|(index, token_metrics)| {
                    (
                        token_metrics,
                        (
                            positions.x(index),
                            positions.advance(index),
                            positions.y_offset(index),
                        ),
                    )
                }),
            true,
            replaced,
            line.line_height,
            line.baseline,
            containing_width,
        )
    } else {
        Vec::new()
    };
    (text, boxes)
}

#[derive(Clone, Copy)]
struct InlineFragmentAncestor {
    box_idx: usize,
    relative_offset: Vec2,
    baseline: f32,
    content_bounds: Option<(f32, f32)>,
}

#[allow(clippy::too_many_arguments)]
fn resolve_inline_fragment_ancestors(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    mut owner: Option<usize>,
    line_height: f64,
    baseline: f64,
    containing_width: f64,
    ancestors: &mut Vec<InlineFragmentAncestor>,
) {
    profile_scope!("fragments.resolve_ancestors");
    ancestors.clear();
    let mut outside_formatting_context = false;
    while let Some(box_idx) = owner {
        let is_inline = engine.reader.is_inline_box(box_idx);
        let has_propagated_decoration = !computed_style::text_decoration_lines_is_empty(engine
            .reader
            .style(box_idx)
            .text_decoration_lines());
        if (!outside_formatting_context && is_inline) || has_propagated_decoration {
            let relative_offset =
                inline_relative_position_offset(engine, box_idx, containing_width);
            let fragment_baseline = (baseline
                - inline_fragment_vertical_align_offset(engine, box_idx, line_height, baseline)
                + relative_offset.y) as f32;
            // CSS 2.1 defines a non-replaced inline's vertical padding,
            // border, and margin edges from its content box. Half-leading
            // participates in line-box height, but is not part of that
            // content box. Use the decorating inline's own font metrics so
            // descendants with a different font do not resize its border.
            let content_bounds = if is_inline {
                let style = engine.reader.style(box_idx);
                let font_size = style.font_size() as f64;
                let font_metrics = engine.reader.font_metrics(box_idx);
                let (ascent, descent) = shaping_contracts::font_relative_metrics_line_box_extents(font_metrics, font_size);
                let mut top = fragment_baseline - ascent as f32;
                let mut bottom = fragment_baseline + descent as f32;
                let trim = style.text_box_trim();
                let edge = style.text_box_edge();
                if matches!(trim, TextBoxTrim::Start | TextBoxTrim::Both) {
                    top = match edge.over {
                        TextBoxOverEdge::Text => top,
                        TextBoxOverEdge::Cap => {
                            fragment_baseline - font_size as f32 * font_metrics.cap_height_ratio
                        }
                        TextBoxOverEdge::Ex => {
                            fragment_baseline - font_size as f32 * font_metrics.x_height_ratio
                        }
                    };
                }
                if matches!(trim, TextBoxTrim::End | TextBoxTrim::Both)
                    && matches!(edge.under, TextBoxUnderEdge::Alphabetic)
                {
                    bottom = fragment_baseline;
                }
                Some((top, bottom))
            } else {
                None
            };
            ancestors.push(InlineFragmentAncestor {
                box_idx,
                relative_offset,
                baseline: fragment_baseline,
                content_bounds,
            });
        }
        if stops_text_decoration_propagation(engine, box_idx) {
            break;
        }
        if box_idx == container_box_idx {
            outside_formatting_context = true;
        }
        owner = engine.reader.get_parent(box_idx);
    }
}

#[allow(clippy::too_many_arguments)]
fn positioned_inline_box_fragments_from<'m>(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    placements: impl IntoIterator<Item = ((TokenMetadata, &'m InlineTokenMetrics), (f64, f64, f32))>,
    bidi: bool,
    replaced: &[ReplacedToken],
    line_height: f64,
    baseline: f64,
    containing_width: f64,
) -> Vec<LineInlineBoxFragment> {
    profile_scope!("fragments.positioned_inline_boxes");
    // A line normally intersects only a handful of inline boxes. A compact
    // linear accumulator beats allocating a box-count-sized side table for
    // every line while still publishing one durable fragment per owner.
    let mut bounds = Vec::<LineInlineBoxFragment>::new();
    let mut cached_owner = None;
    let mut ancestors = Vec::new();
    for (paint_order, ((token, metrics), (x, advance, y_offset))) in
        placements.into_iter().enumerate()
    {
        let paint_order = paint_order as u32;
        if advance <= 0.0 && !matches!(token.kind(), InlineTokenKind::InlineBoundary { .. }) {
            continue;
        }
        let token_top = (baseline - metrics.ascent as f64 - y_offset as f64) as f32;
        let token_bottom = (baseline + metrics.descent as f64 - y_offset as f64) as f32;
        let boundary = match token.kind() {
            InlineTokenKind::InlineBoundary { payload_idx } => {
                match replaced.get(payload_idx as usize) {
                    Some(ReplacedToken::InlineBoundary {
                        box_idx,
                        margin_left,
                        left_inset,
                        inline_start,
                        inline_end,
                    }) => Some((
                        *box_idx as usize,
                        x + margin_left + left_inset,
                        *inline_start,
                        *inline_end,
                    )),
                    _ => None,
                }
            }
            _ => None,
        };
        let replaced_border_fragment = match token.kind() {
            InlineTokenKind::Image { payload_idx } => match replaced.get(payload_idx as usize) {
                Some(ReplacedToken::Image {
                    box_idx,
                    border_size,
                    margin_left,
                    margin_top,
                    ..
                }) => {
                    let border_left = x + margin_left;
                    let border_top =
                        baseline - metrics.ascent as f64 - y_offset as f64 + margin_top;
                    Some(LineInlineBoxFragment {
                        box_idx: *box_idx,
                        paint_order,
                        start_x: border_left as f32,
                        end_x: (border_left + border_size.width) as f32,
                        top: border_top as f32,
                        bottom: (border_top + border_size.height) as f32,
                        baseline: baseline as f32,
                        flags: LineInlineBoxFragment::INLINE_START
                            | LineInlineBoxFragment::INLINE_END
                            | LineInlineBoxFragment::BORDER_BOX_BOUNDS,
                    })
                }
                _ => None,
            },
            _ => None,
        };
        let owner = inline_fragment_owner(engine, &token, metrics, replaced);
        // Consecutive tokens usually share an owner. Resolve its ancestry and
        // line-relative inline geometry once; block decoration extents still
        // use each token's own ascent, descent, and placement below.
        if cached_owner != Some(owner) {
            resolve_inline_fragment_ancestors(
                engine,
                container_box_idx,
                owner,
                line_height,
                baseline,
                containing_width,
                &mut ancestors,
            );
            cached_owner = Some(owner);
        }
        for ancestor in &ancestors {
            let box_idx = ancestor.box_idx;
            let relative_offset = ancestor.relative_offset;
            let fragment_baseline = ancestor.baseline;
            let (fragment_top, fragment_bottom) =
                ancestor.content_bounds.unwrap_or((token_top, token_bottom));
            let own_boundary = boundary.filter(|boundary| boundary.0 == box_idx);
            let fragment_start =
                (own_boundary.map_or(x, |boundary| boundary.1) + relative_offset.x) as f32;
            let fragment_end = (own_boundary.map_or(x + advance, |boundary| boundary.1)
                + relative_offset.x) as f32;
            if let Some(fragment) = bounds.iter_mut().find(|fragment| {
                fragment.box_idx as usize == box_idx
                    && (!bidi
                        || (fragment.start_x <= fragment_end + 0.001
                            && fragment.end_x >= fragment_start - 0.001))
            }) {
                fragment.paint_order = fragment.paint_order.min(paint_order);
                fragment.start_x = fragment.start_x.min(fragment_start);
                fragment.end_x = fragment.end_x.max(fragment_end);
                fragment.top = fragment.top.min(fragment_top);
                fragment.bottom = fragment.bottom.max(fragment_bottom);
                if let Some((_, _, inline_start, inline_end)) = own_boundary {
                    fragment.flags |= if inline_start {
                        LineInlineBoxFragment::INLINE_START
                    } else {
                        0
                    } | if inline_end {
                        LineInlineBoxFragment::INLINE_END
                    } else {
                        0
                    };
                }
            } else {
                let (inline_start, inline_end) = own_boundary.map_or_else(
                    || {
                        if bidi && replaced.iter().any(|replaced| matches!(replaced, ReplacedToken::InlineBoundary { box_idx: owner, .. } if *owner as usize == box_idx)) {
                            return (false, false);
                        }
                        let edges = engine.reader.inline_fragment_edges(box_idx);
                        if engine.reader.style(box_idx).direction() == TextDirection::Rtl { (edges.1, edges.0) } else { edges }
                    },
                    |boundary| (boundary.2, boundary.3),
                );
                bounds.push(LineInlineBoxFragment {
                    box_idx: box_idx as u32,
                    paint_order,
                    start_x: fragment_start,
                    end_x: fragment_end,
                    top: fragment_top,
                    bottom: fragment_bottom,
                    baseline: fragment_baseline,
                    flags: if inline_start {
                        LineInlineBoxFragment::INLINE_START
                    } else {
                        0
                    } | if inline_end {
                        LineInlineBoxFragment::INLINE_END
                    } else {
                        0
                    },
                });
            }
        }
        // Enclosing inline backgrounds must precede the replaced element's
        // own background and border in paint order, so append this fragment
        // after accumulating its ancestor chain.
        if let Some(fragment) = replaced_border_fragment {
            bounds.push(fragment);
        }
    }
    if bidi {
        bounds.sort_by(|a, b| {
            a.box_idx
                .cmp(&b.box_idx)
                .then(a.start_x.total_cmp(&b.start_x))
        });
        let mut merged: Vec<LineInlineBoxFragment> = Vec::with_capacity(bounds.len());
        for fragment in bounds {
            if let Some(previous) = merged.last_mut().filter(|previous| {
                previous.box_idx == fragment.box_idx
                    && (previous.flags ^ fragment.flags) & LineInlineBoxFragment::BORDER_BOX_BOUNDS
                        == 0
                    && previous.end_x + 0.001 >= fragment.start_x
            }) {
                previous.end_x = previous.end_x.max(fragment.end_x);
                previous.top = previous.top.min(fragment.top);
                previous.bottom = previous.bottom.max(fragment.bottom);
                previous.paint_order = previous.paint_order.min(fragment.paint_order);
                previous.flags |= fragment.flags;
            } else {
                merged.push(fragment);
            }
        }
        bounds = merged;
    }
    // For the same source position, ancestor backgrounds/borders paint before
    // descendant boxes. Token order remains the primary key across siblings.
    bounds.sort_by_key(|fragment| {
        let mut depth = 0u32;
        let mut parent = engine.reader.get_parent(fragment.box_idx as usize);
        while let Some(parent_idx) = parent {
            depth += 1;
            parent = engine.reader.get_parent(parent_idx);
        }
        (fragment.paint_order, depth)
    });
    bounds
}

pub(super) fn inline_relative_position_offset(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    containing_width: f64,
) -> Vec2 {
    let mut offset = Vec2::ZERO;
    for current_idx in inline_ancestors(&engine.reader, Some(box_idx)) {
        let style = engine.reader.style(current_idx);
        if style.position() == html_style_model::PositionMode::Relative {
            offset += crate::layout::box_positioning::relative_position_offset(
                &style,
                containing_width,
                None,
            );
        }
    }
    offset
}

/// Returns the alignment of the fragment owner itself and its inline
/// ancestors, excluding shifts authored on descendants. Decoration geometry
/// is tied to the decorating box's baseline, not whichever descendant token
/// happened to create its accumulated fragment first.
fn inline_fragment_vertical_align_offset(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    line_height: f64,
    baseline: f64,
) -> f64 {
    let mut offset = 0.0;
    for current_idx in inline_ancestors(&engine.reader, Some(box_idx)) {
        let style = engine.reader.style(current_idx);
        let value = style.vertical_align();
        if !value.is_initial() {
            let own_line_height = style.resolved_line_height();
            let font_size = style.font_size() as f64;
            let (ascent, descent) = shaping_contracts::font_relative_metrics_line_box_extents(engine
                .reader
                .font_metrics(current_idx), font_size);
            let half_leading = (own_line_height - ascent - descent) * 0.5;
            let (parent_ascent, parent_descent) = parent_font_content_extents(engine, current_idx);
            offset += vertical_align_metrics_offset(
                value,
                ascent,
                descent,
                own_line_height,
                font_size,
                half_leading,
                line_height,
                baseline,
                parent_ascent,
                parent_descent,
            );
        }
    }
    offset
}

fn inline_fragment_owner(
    engine: &LayoutEngine<'_, '_>,
    token: &TokenMetadata,
    metrics: &InlineTokenMetrics,
    replaced: &[ReplacedToken],
) -> Option<usize> {
    match token.kind() {
        // Replaced boxes paint themselves; their parent owns enclosing fragments.
        InlineTokenKind::Image { payload_idx } => match replaced.get(payload_idx as usize) {
            Some(ReplacedToken::Image { box_idx, .. }) => {
                engine.reader.get_parent(*box_idx as usize)
            }
            _ => None,
        },
        InlineTokenKind::AtomicBox { payload_idx } => match replaced.get(payload_idx as usize) {
            Some(ReplacedToken::AtomicBox { box_idx, .. }) => {
                engine.reader.get_parent(*box_idx as usize)
            }
            _ => None,
        },
        InlineTokenKind::InlineBoundary { payload_idx } => {
            match replaced.get(payload_idx as usize) {
                Some(ReplacedToken::InlineBoundary { box_idx, .. }) => Some(*box_idx as usize),
                _ => None,
            }
        }
        _ => {
            let owner = metrics.owner_box_idx;
            (owner != u32::MAX).then_some(owner as usize)
        }
    }
}

fn has_inline_fragment_owners(
    engine: &LayoutEngine<'_, '_>,
    span: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    replaced: &[ReplacedToken],
) -> bool {
    let mut checked_owner = None;
    metrics.any(span, move |token, metrics| {
        // Images publish their own border-box fragment even when none of
        // their ancestors contributes an inline background.
        if matches!(token.kind(), InlineTokenKind::Image { .. }) {
            return true;
        }
        let owner = inline_fragment_owner(engine, &token, metrics, replaced);
        // A positive result ends the scan. Consecutive tokens with the same
        // negative owner can reuse its ancestry check without style reads.
        if checked_owner == Some(owner) {
            return false;
        }
        checked_owner = Some(owner);
        owner_has_inline_fragments(engine, owner)
    })
}

fn owner_has_inline_fragments(engine: &LayoutEngine<'_, '_>, mut owner: Option<usize>) -> bool {
    while let Some(box_idx) = owner {
        if engine.reader.is_inline_box(box_idx)
            || !computed_style::text_decoration_lines_is_empty(engine
                .reader
                .style(box_idx)
                .text_decoration_lines())
        {
            return true;
        }
        if stops_text_decoration_propagation(engine, box_idx) {
            break;
        }
        owner = engine.reader.get_parent(box_idx);
    }
    false
}

/// Text decorations propagate through ordinary in-flow block and table boxes;
/// they are not inherited, but each ancestor that establishes a decoration
/// paints it across descendant inline content. Atomic inline formatting
/// contexts and out-of-flow descendants form the CSS propagation boundary.
fn stops_text_decoration_propagation(engine: &LayoutEngine<'_, '_>, box_idx: usize) -> bool {
    let style = engine.reader.style(box_idx);
    matches!(
        style.display(),
        html_style_model::Display::InlineBlock
            | html_style_model::Display::InlineTable
            | html_style_model::Display::InlineFlex
            | html_style_model::Display::InlineGrid
    ) || matches!(
        style.float(),
        html_style_model::Float::Left | html_style_model::Float::Right
    ) || style.position() == html_style_model::PositionMode::Absolute
}

pub(super) fn line_publishes(
    engine: &LayoutEngine<'_, '_>,
    span: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    replaced: &[ReplacedToken],
    line: &LineLayout,
) -> bool {
    if line.placements.is_none() && !line.glyph_range.is_empty() {
        return true;
    }
    let widths = span.widths();
    line.publish_empty
        || metrics
            .iter(span)
            .enumerate()
            .any(|(index, (token, metrics))| match token.kind() {
                InlineTokenKind::FloatAnchor { .. }
                | InlineTokenKind::AbsoluteAnchor { .. }
                | InlineTokenKind::Opportunity => false,
                InlineTokenKind::InlineBoundary { payload_idx } => {
                    match replaced.get(payload_idx as usize) {
                        Some(ReplacedToken::InlineBoundary {
                            box_idx,
                            inline_start,
                            inline_end,
                            ..
                        }) => {
                            let style = engine.reader.style(*box_idx as usize);
                            widths.at(index) != 0.0
                                || (*inline_start
                                    && (!style.padding_left().is_zero()
                                        || style.border_left_width() != 0.0))
                                || (*inline_end
                                    && (!style.padding_right().is_zero()
                                        || style.border_right_width() != 0.0))
                                || !style.margin_top().is_zero()
                                || !style.margin_bottom().is_zero()
                                || !style.padding_top().is_zero()
                                || !style.padding_bottom().is_zero()
                                || style.border_top_width() != 0.0
                                || style.border_bottom_width() != 0.0
                        }
                        _ => false,
                    }
                }
                InlineTokenKind::Glyph { glyph_idx } => {
                    let character = engine.text.character(glyph_idx);
                    widths.at(index) != 0.0
                        || !character.is_whitespace()
                        || metrics.white_space.preserves_spaces()
                }
                _ => true,
            })
}

/// Writes fragments for one broken line using a precomputed line layout.
///
/// The write phase only mutates the document; all layout decisions have
/// already been made by `break_lines` and `measure_line`.
pub(in crate::layout::inline) fn write_line_fragments(
    engine: &mut LayoutEngine<'_, '_>,
    container_box_idx: usize,
    span: TokenSpan<'_>,
    metrics_view: MetricsView<'_>,
    replaced: &[ReplacedToken],
    origin: Point,
    line: &LineLayout,
    paint_color: Option<u32>,
    containing_width: f64,
    static_inline_start: f64,
) -> f64 {
    profile_scope!("fragments.write");
    let timing_started = engine.start_timing();
    let point = Point::new(origin.x + line.x_offset, origin.y);
    let line_idx = engine.fragments.state().line_output.lines.len();
    let plain = line.placements.is_none() && !line.glyph_range.is_empty();
    let (text_fragments, inline_box_fragments) = if plain {
        let inline_box_fragments = positioned_plain_inline_box_fragments(
            engine,
            container_box_idx,
            span,
            metrics_view,
            replaced,
            line,
            containing_width,
        );
        (None, inline_box_fragments)
    } else if matches!(
        line.placements,
        Some(LinePlacements::Sequential { publish: false, .. })
    ) || (matches!(line.placements, Some(LinePlacements::Sequential { .. }))
        && !span.metadata_iter().any(|token| token.is_tab()))
    {
        positioned_sequential_text(
            engine,
            container_box_idx,
            span,
            metrics_view,
            replaced,
            line,
            containing_width,
        )
    } else if matches!(
        line.placements,
        Some(LinePlacements::Sequential { .. } | LinePlacements::Positioned(_))
    ) {
        positioned_resolved_fragments(
            engine,
            container_box_idx,
            span,
            metrics_view,
            replaced,
            line,
            containing_width,
        )
    } else if let Some(LinePlacements::GlyphX(xs)) = line.placements.as_ref() {
        match span {
            TokenSpan::Wide(tokens) => positioned_glyph_fragments(
                engine,
                container_box_idx,
                span,
                GlyphPositions { xs, tokens },
                metrics_view,
                replaced,
                line,
                containing_width,
            ),
            TokenSpan::Compact { source, start, len } => positioned_glyph_fragments(
                engine,
                container_box_idx,
                span,
                GlyphPositions {
                    xs,
                    tokens: CompactTokens::new(source, start, len),
                },
                metrics_view,
                replaced,
                line,
                containing_width,
            ),
        }
    } else {
        let has_fragment_owners = has_inline_fragment_owners(engine, span, metrics_view, replaced);
        let needs_ordered_fragments = has_fragment_owners
            || metrics_view.any(span, |token, metrics| {
                matches!(
                    token.kind(),
                    InlineTokenKind::Image { .. }
                        | InlineTokenKind::AtomicBox { .. }
                        | InlineTokenKind::InlineBoundary { .. }
                ) || !metrics.visible
            });
        let fallback_placements;
        let placements = if let Some(LinePlacements::Full(placements)) = line.placements.as_ref() {
            Some(placements.as_ref())
        } else if needs_ordered_fragments {
            fallback_placements = build_token_placements(
                engine,
                span,
                metrics_view,
                0.0,
                line.line_height,
                line.baseline,
                containing_width,
            );
            Some(fallback_placements.as_slice())
        } else {
            None
        };
        let text_fragments = placements.and_then(|placements| {
            positioned_text_fragments(span, placements, metrics_view, false)
        });
        let inline_box_fragments = if has_fragment_owners {
            let placements = placements.expect("inline fragment owners require token placements");
            positioned_inline_box_fragments(
                engine,
                container_box_idx,
                span,
                placements,
                metrics_view,
                replaced,
                line.line_height,
                line.baseline,
                containing_width,
            )
        } else {
            Vec::new()
        };
        (text_fragments, inline_box_fragments)
    };
    // Absolute/float anchors are executable source-position markers, not
    // line-box content. Keep processing them below, but do not publish the
    // zero-height carrier line that exists solely to trigger that work.
    let publishes_line = plain || line_publishes(engine, span, metrics_view, replaced, line);
    if publishes_line {
        let inline_box_fragments = engine
            .fragments
            .push_inline_box_fragments(inline_box_fragments, engine.reader.box_count());
        let layout = engine.fragments.state_mut();
        let details = layout.line_output.push_details(
            line.optical_offset_x,
            paint_color,
            text_fragments,
            inline_box_fragments,
        );
        layout.line_output.lines.push(Line {
            owner_box_idx: container_box_idx as u32,
            glyphs: line.glyph_range.clone(),
            owns_glyph_range: line.owns_glyph_range,
            point,
            height: line.line_height,
            baseline: line.baseline,
            word_spacing: line.word_spacing,
            letter_spacing: line.letter_spacing,
            details,
            prepared_text_runs: 0..0,
            native_text_runs_complete: false,
        });
        layout.line_output.line_glyph_offsets.push(Vec::new());
        let punctuation_advances = if plain {
            line.punctuation_space_advances.as_slice()
        } else {
            &[]
        };
        layout
            .line_output
            .line_glyph_advances
            .push(
                punctuation_advances
                    .iter()
                    .map(|&(glyph_idx, advance)| GlyphAdvanceRun {
                        range: glyph_idx..glyph_idx + 1,
                        advance,
                    }),
            );
        if let Some((glyph, x)) = line.hyphen {
            let visible = plain
                || span
                    .metadata_iter()
                    .rposition(|token| matches!(token.kind(), InlineTokenKind::Glyph { .. }))
                    .is_none_or(|index| metrics_view.at(span, index).visible);
            layout.line_output.hyphen_fragments.push(HyphenFragment {
                line_idx,
                glyph,
                offset: Point::new(x, 0.0),
                visible,
            });
        }
        engine.placement.record_line();
    }

    let reader = engine.reader;
    let text = engine.text;
    let needs_publication = match line.placements.as_ref() {
        Some(LinePlacements::Sequential { publish, .. }) => {
            *publish || !line.punctuation_space_advances.is_empty()
        }
        Some(LinePlacements::Positioned(plan)) => {
            !line.punctuation_space_advances.is_empty()
                || plan.offsets.iter().any(|run| run.y != 0.0)
                || span.metadata_iter().any(|token| {
                    token.is_tab()
                        || matches!(
                            token.kind(),
                            InlineTokenKind::Image { .. }
                                | InlineTokenKind::AtomicBox { .. }
                                | InlineTokenKind::Ellipsis { .. }
                                | InlineTokenKind::AbsoluteAnchor { .. }
                        )
                })
                || metrics_view.chunks(span).any(|(_, metrics)| {
                    metrics.owner_box_idx != u32::MAX && {
                        let style = engine.reader.style(metrics.owner_box_idx as usize);
                        style.word_spacing() != 0.0 || style.word_spacing_factor() != 1.0
                    }
                })
        }
        Some(LinePlacements::Full(_)) => true,
        _ => false,
    };
    if !needs_publication {
        engine.record_timing(|t| t.write_line_fragments += timing_started.elapsed());
        return if publishes_line {
            line.line_height
        } else {
            0.0
        };
    }
    let mut publish = |paint_order: usize,
                       token: TokenMetadata,
                       metrics: &InlineTokenMetrics,
                       placement: TokenPlacement| {
        let paint_order = paint_order as u32;
        let offset = placement.y_offset as f64;
        let ascent = metrics.ascent as f64;
        if let InlineTokenKind::Image { payload_idx } = token.kind() {
            debug_assert!(publishes_line);
            let ReplacedToken::Image {
                image_idx,
                box_idx,
                content_size,
                border_size,
                content_inset,
                margin_left,
                margin_top,
                position_offset,
                set_box_geometry,
            } = replaced
                .get(payload_idx as usize)
                .expect("image token payload index in bounds")
            else {
                unreachable!("image token must reference an image payload")
            };
            let border_offset = Point::new(
                placement.x + placement.relative_offset.x + *margin_left,
                line.baseline - ascent - offset + *margin_top,
            ) + *position_offset;
            if *set_box_geometry {
                engine
                    .geometry
                    .set_point(*box_idx as usize, point + border_offset.to_vec2());
                engine.geometry.set_size(*box_idx as usize, *border_size);
            }
            let content_offset = border_offset + content_inset.to_vec2();
            let intrinsic = engine
                .reader
                .image_intrinsic(*box_idx as usize)
                .expect("image fragment owner must retain intrinsic geometry")
                .size;
            let style = engine.reader.style(*box_idx as usize);
            if style.visibility() == html_style_model::Visibility::Hidden {
                return;
            }
            let object = crate::layout::replaced::replaced_object_geometry(
                *content_size,
                intrinsic,
                style.object_fit(),
                style.object_position(),
            );
            engine
                .fragments
                .state_mut()
                .fragment_output
                .image_fragments
                .push(ImageFragment {
                    line_idx,
                    image_idx: *image_idx,
                    box_idx: *box_idx,
                    offset: content_offset + object.rect.origin().to_vec2(),
                    size: object.rect.size(),
                    clip: object.clip + content_offset.to_vec2(),
                    paint_order,
                });
        } else if let InlineTokenKind::AtomicBox { payload_idx } = token.kind() {
            debug_assert!(publishes_line);
            let ReplacedToken::AtomicBox {
                box_idx,
                border_size,
                containing_width,
                containing_height,
                margin_left,
                margin_top,
            } = replaced
                .get(payload_idx as usize)
                .expect("atomic token payload index in bounds")
            else {
                unreachable!("atomic token must reference an atomic payload")
            };
            let border_point = Point::new(
                point.x + placement.x + placement.relative_offset.x + *margin_left,
                point.y + line.baseline - ascent - offset + *margin_top,
            );
            engine.geometry.set_point(*box_idx as usize, border_point);
            let decoration_start = engine.fragments.decoration_len();
            let paint_start = engine.fragments.state().paint_program.commands.len();
            let height = engine.reader.style(*box_idx as usize).height();
            let authored_height_is_definite =
                resolve_vertical_size(height, *containing_height, 0.0).is_some();
            let request = crate::layout::BoxLayoutRequest::assigned(
                *box_idx as usize,
                *containing_width,
                *containing_height,
                *border_size,
                authored_height_is_definite,
            );
            let _ = engine.layout_box(request);
            engine
                .fragments
                .state_mut()
                .paint_program
                .record_line_since(paint_start, line_idx, paint_order);
            // CSS paints an inline-block (and the non-positioned contents it
            // establishes) atomically at this point in the owning line. Keep
            // its block decorations with that line instead of replaying them
            // in the document-wide block-background phase.
            engine
                .fragments
                .record_decoration_line_since(line_idx, decoration_start);
            engine
                .fragments
                .record_decoration_paint_order_since(paint_order, decoration_start);
        } else if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
            debug_assert!(publishes_line);
            let adjusted_space = matches!(engine.text.character(glyph_idx), ' ' | '\u{00a0}') && {
                let style = engine.reader.style(metrics.owner_box_idx as usize);
                style.word_spacing_factor() != 1.0 || style.word_spacing() != 0.0
            };
            let justified_space = line
                .punctuation_space_advances
                .iter()
                .find(|&&(index, _)| index == glyph_idx)
                .map(|&(_, advance)| advance);
            let layout = engine.fragments.state_mut();
            if offset != 0.0 {
                push_glyph_offset_run(
                    &mut layout.line_output.line_glyph_offsets,
                    line_idx,
                    glyph_idx,
                    offset as f32,
                );
            }
            if token.is_tab() || adjusted_space || justified_space.is_some() {
                layout.line_output.line_glyph_advances.push_to(
                    line_idx,
                    GlyphAdvanceRun {
                        range: glyph_idx..glyph_idx + 1,
                        advance: justified_space.unwrap_or(placement.advance as f32),
                    },
                );
            }
        } else if let InlineTokenKind::Ellipsis { glyph } = token.kind() {
            debug_assert!(publishes_line);
            engine
                .fragments
                .state_mut()
                .line_output
                .ellipsis_fragments
                .push(EllipsisFragment {
                    line_idx,
                    glyph,
                    offset: Point::new(placement.x, offset),
                    visible: metrics.visible,
                });
        } else if let InlineTokenKind::AbsoluteAnchor { box_idx } = token.kind() {
            let static_y = engine
                .floats
                .current_float_context()
                .map_or(point.y, |context| {
                    context.source_position_at_or_after(point.y)
                });
            engine.defer_absolute_box(
                box_idx as usize,
                Point::new(
                    if engine
                        .reader
                        .layout_style(box_idx as usize)
                        .static_position_inline
                    {
                        point.x + placement.x
                    } else {
                        static_inline_start
                    },
                    static_y,
                ),
            );
        }
    };
    match line.placements.as_ref() {
        Some(LinePlacements::Full(placements)) => {
            for (index, ((token, metrics), placement)) in
                metrics_view.iter(span).zip(placements.iter()).enumerate()
            {
                publish(index, token, metrics, *placement);
            }
        }
        Some(LinePlacements::Sequential { .. })
            if !span.metadata_iter().any(|token| token.is_tab()) =>
        {
            for (index, (token, metrics, x, advance)) in
                sequential_text_positions(&text, span, metrics_view, line).enumerate()
            {
                publish(
                    index,
                    token,
                    metrics,
                    TokenPlacement {
                        x,
                        advance,
                        y_offset: 0.0,
                        relative_offset: Vec2::ZERO,
                    },
                );
            }
        }
        Some(LinePlacements::Sequential { .. } | LinePlacements::Positioned(_)) => {
            for (index, (token, metrics, placement)) in
                resolved_positions(reader, text, span, metrics_view, line).enumerate()
            {
                publish(index, token, metrics, placement);
            }
        }
        _ => {}
    }
    engine.record_timing(|t| t.write_line_fragments += timing_started.elapsed());
    if publishes_line {
        line.line_height
    } else {
        0.0
    }
}

/// Appends or extends a glyph offset run.
///
/// Adjacent glyphs with the same vertical offset are merged so the document
/// stores compact offset ranges instead of one entry per glyph.
fn push_glyph_offset_run(
    offsets: &mut GroupedArena<GlyphOffsetRun>,
    line_idx: usize,
    glyph_idx: u32,
    offset: f32,
) {
    if let Some(last) = offsets
        .last_mut(line_idx)
        .filter(|last| last.range.end == glyph_idx && last.offset == offset)
    {
        last.range.end = glyph_idx + 1;
    } else {
        offsets.push_to(
            line_idx,
            GlyphOffsetRun {
                range: glyph_idx..glyph_idx + 1,
                offset,
            },
        );
    }
}
