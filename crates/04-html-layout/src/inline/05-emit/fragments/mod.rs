//! Durable text, box, image, and glyph-fragment publication.

use super::positions::{
    GlyphPositions, PlacementRead, VisualPositions, resolved_positions, sequential_text_positions,
};
mod boxes;
mod text;
use crate::LayoutEngine;
use crate::inline::TokenSpan;
use crate::inline::fragments::{LineLayout, LinePlacements, TokenPlacement};
use crate::inline::metrics::MetricsView;
use crate::inline::reader::InlineReader;
use crate::inline::tokens::{InlineTokenKind, ReplacedToken};
use boxes::{
    has_inline_fragment_owners, owner_has_inline_fragments, positioned_inline_box_fragments_from,
};
use html_engine_model::{CompactTextFragments, GroupedArena, OwnedTextFragments};
use html_engine_model::{LineInlineBoxFragment, LineTextFragment};
use html_text::spaced_advance;
use std::borrow::Cow;
use text::{
    TextFragmentBuilder, TextFragmentRecord, needs_positioned_text_fragments,
    positioned_text_fragments,
};

/// Obtain retained fragments, or prepare them only for a final publication pass.
pub(super) fn prepare_fragments<'a>(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    span: TokenSpan<'_>,
    metrics_view: MetricsView<'_>,
    replaced: &[ReplacedToken],
    line: &LineLayout,
    publication: &'a mut LinePublication,
    retain_fragments: bool,
    containing_width: f64,
) -> (Option<OwnedTextFragments>, Cow<'a, [LineInlineBoxFragment]>) {
    // Probes consume line geometry and baselines, but never painted fragment
    // structure. Preserve the line and source-position execution below.
    let publish_fragments = engine.placement.retains_output();
    if publish_fragments && retain_fragments && publication.fragments.is_none() {
        publication.fragments = Some(Box::new(prepare_line_fragment_plan(
            engine,
            container_box_idx,
            span,
            metrics_view,
            replaced,
            line,
            containing_width,
        )));
    }
    if !publish_fragments {
        (None, Cow::Borrowed(&[][..]))
    } else if let Some(plan) = &publication.fragments {
        (plan.text.clone(), Cow::Borrowed(plan.boxes.as_slice()))
    } else {
        let plan = prepare_line_fragment_plan(
            engine,
            container_box_idx,
            span,
            metrics_view,
            replaced,
            line,
            containing_width,
        );
        (plan.text, Cow::Owned(plan.boxes))
    }
}

fn prepare_line_fragment_plan(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    span: TokenSpan<'_>,
    metrics_view: MetricsView<'_>,
    replaced: &[ReplacedToken],
    line: &LineLayout,
    containing_width: f64,
) -> LineFragmentPlan {
    // Measurement omits placements only for plain text with a glyph range.
    // Bidi may replace that representation, but never removes prepared positions.
    let (text, boxes) = match line.placements.as_ref() {
        None => {
            debug_assert!(!line.glyph_range.is_empty());
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
        }
        Some(LinePlacements::Sequential { publish, .. }) if !*publish || !line.has_tabs => {
            positioned_sequential_text(
                engine,
                container_box_idx,
                span,
                metrics_view,
                replaced,
                line,
                containing_width,
            )
        }
        Some(LinePlacements::Sequential { .. } | LinePlacements::Positioned(_)) => {
            positioned_resolved_fragments(
                engine,
                container_box_idx,
                span,
                metrics_view,
                replaced,
                line,
                containing_width,
            )
        }
        Some(LinePlacements::GlyphX(xs)) => positioned_glyph_fragments(
            engine,
            container_box_idx,
            span,
            GlyphPositions { xs, tokens: span },
            metrics_view,
            replaced,
            line,
            containing_width,
        ),
        Some(LinePlacements::Full(placements)) => {
            let has_fragment_owners =
                has_inline_fragment_owners(engine, span, metrics_view, replaced);
            let text_fragments = positioned_text_fragments(
                &engine.read.text,
                replaced,
                span,
                placements.as_ref(),
                metrics_view,
                false,
            );
            let inline_box_fragments = if has_fragment_owners {
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
        }
    };
    LineFragmentPlan { text, boxes }
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
        builder.append(text, index, token, metrics, x, 0.0, || advance);
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
            && text.bidi_level(expected) == 0
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
            x += f64::from(widths[index]);
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
    let needs_text =
        needs_positioned_text_fragments(&engine.read.text, replaced, span, metrics, false);
    let has_owners = metrics.chunks(span).any(|(_, metrics)| {
        let owner = metrics.owner_box_idx;
        owner_has_inline_fragments(engine, (owner != u32::MAX).then_some(owner as usize))
    });
    if needs_text && has_owners {
        return if span.len() <= 1 << 14 {
            sequential_text_and_box_fragments::<CompactTextFragments>(
                engine,
                container_box_idx,
                span,
                metrics,
                replaced,
                line,
                containing_width,
            )
        } else {
            sequential_text_and_box_fragments::<LineTextFragment>(
                engine,
                container_box_idx,
                span,
                metrics,
                replaced,
                line,
                containing_width,
            )
        };
    }
    let text = needs_text.then(|| {
        if span.len() <= 1 << 14 {
            regular_sequential_fragments(&engine.read.text, span, metrics, line).unwrap_or_else(
                || {
                    sequential_text_fragments::<CompactTextFragments>(
                        &engine.read.text,
                        span,
                        metrics,
                        line,
                    )
                },
            )
        } else {
            sequential_text_fragments::<LineTextFragment>(&engine.read.text, span, metrics, line)
        }
    });
    let boxes = if has_owners {
        positioned_inline_box_fragments_from(
            engine,
            container_box_idx,
            sequential_text_positions(&engine.read.text, span, metrics, line)
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

/// Resolve source positions once and feed both publication consumers from
/// that stream. Neither consumer materializes an intermediate placement array.
fn sequential_text_and_box_fragments<R: TextFragmentRecord>(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    span: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    replaced: &[ReplacedToken],
    line: &LineLayout,
    containing_width: f64,
) -> (Option<OwnedTextFragments>, Vec<LineInlineBoxFragment>) {
    let mut text = TextFragmentBuilder::<R> {
        fragments: Default::default(),
        has_fragment: false,
        current_metrics: None,
        current_level: None,
        previous_end: 0.0,
    };
    let positions = sequential_text_positions(&engine.read.text, span, metrics, line)
        .enumerate()
        .map(|(index, (token, metrics, x, advance))| {
            text.append(&engine.read.text, index, token, metrics, x, 0.0, || advance);
            ((token, metrics), (x, advance, 0.0))
        });
    let boxes = positioned_inline_box_fragments_from(
        engine,
        container_box_idx,
        positions,
        false,
        replaced,
        line.line_height,
        line.baseline,
        containing_width,
    );
    (Some(R::finish(text.fragments)), boxes)
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
        resolved_positions(engine.read.reader, engine.read.text, span, metrics, line).enumerate()
    {
        builder.append(
            &engine.read.text,
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
                        | InlineTokenKind::ZeroWidthBreak
                        | InlineTokenKind::Break { .. }
                )
        })
        && needs_positioned_text_fragments(&engine.read.text, replaced, span, metrics, false)
    {
        {
            return positioned_glyph_fragments(
                engine,
                container_box_idx,
                span,
                VisualPositions {
                    xs: &plan.xs,
                    tokens: span,
                    text: engine.read.text,
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
    let text = needs_positioned_text_fragments(&engine.read.text, replaced, span, metrics, false)
        .then(|| {
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
            resolved_positions(engine.read.reader, engine.read.text, span, metrics, line).map(
                |(token, metrics, placement)| {
                    (
                        (token, metrics),
                        (placement.x, placement.advance, placement.y_offset),
                    )
                },
            ),
            metrics.iter(span).any(|(token, metrics)| {
                engine.read.text.token_bidi_level(token, metrics, replaced) % 2 == 1
            }),
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

fn positioned_inline_box_fragments(
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
        metrics.iter(span).any(|(token, metrics)| {
            engine.read.text.token_bidi_level(token, metrics, replaced) % 2 == 1
        }),
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
            let natural =
                token.advance_with_metrics(widths[index], &engine.read.reader, metrics, x);
            let advance = if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
                line.punctuation_space_advances
                    .iter()
                    .find(|&&(index, _)| index == glyph_idx)
                    .map(|&(_, advance)| f64::from(advance))
                    .unwrap_or_else(|| {
                        spaced_advance(
                            natural,
                            engine.read.text.character(glyph_idx),
                            line.letter_spacing != 0.0
                                && engine
                                    .read
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
    let text =
        positioned_text_fragments(&engine.read.text, replaced, span, positions, metrics, true);
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

/// Publication-owned fragment cache, separate from composed line geometry.
#[derive(Default)]
pub(in crate::inline) struct LinePublication {
    fragments: Option<Box<LineFragmentPlan>>,
}

impl LinePublication {
    pub(in crate::inline) fn storage_bytes(&self) -> usize {
        self.fragments
            .as_ref()
            .map_or(0, |plan| plan.storage_bytes())
    }
}

/// Position-independent fragments retained with an immutable paragraph result.
struct LineFragmentPlan {
    text: Option<OwnedTextFragments>,
    boxes: Vec<LineInlineBoxFragment>,
}

impl LineFragmentPlan {
    fn storage_bytes(&self) -> usize {
        size_of::<Self>()
            + self
                .text
                .as_ref()
                .map_or(0, OwnedTextFragments::storage_bytes)
            + self.boxes.capacity() * size_of::<LineInlineBoxFragment>()
    }
}
