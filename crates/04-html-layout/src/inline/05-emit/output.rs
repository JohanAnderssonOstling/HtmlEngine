//! Publish a line, then execute its placements in source paint order.
use super::positions::{resolved_positions, sequential_text_positions};
use crate::LayoutEngine;
use crate::boxes::absolute_positioning;
use crate::inline::TokenSpan;
use crate::inline::fragments::{LineLayout, LinePlacements, TokenPlacement};
use crate::inline::metrics::MetricsView;
use crate::inline::tokens::{InlineTokenKind, InlineTokenMetrics, ReplacedToken, TokenMetadata};
use crate::resolve_vertical_size;
use html_engine_model::{
    EllipsisFragment, GlyphAdvanceRun, GlyphOffsetRun, HyphenFragment, ImageFragment, Line,
    LineInlineBoxFragment,
};
use html_engine_model::{GroupedArena, LineId, OwnedTextFragments};
use kurbo::{Point, Vec2};

pub(super) fn publish_line(
    engine: &mut LayoutEngine<'_, '_>,
    container_box_idx: usize,
    span: TokenSpan<'_>,
    metrics_view: MetricsView<'_>,
    point: Point,
    line_idx: usize,
    line: &LineLayout,
    paint_color: Option<u32>,
    text_fragments: Option<OwnedTextFragments>,
    inline_box_fragments: &[LineInlineBoxFragment],
    containing_width: f64,
) {
    let plain = line.placements.is_none() && !line.glyph_range.is_empty();
    // Absolute/float anchors are executable source-position markers, not
    // line-box content. Keep processing them below, but do not publish the
    // zero-height carrier line that exists solely to trigger that work.
    if line.publishes_line {
        let inline_box_fragments = engine.fragments.push_inline_box_fragments(
            inline_box_fragments.iter().copied(),
            engine.read.reader.box_count(),
        );
        let layout = engine.fragments.state_mut();
        let details = layout.line_output.push_details(
            line.optical_offset_x,
            paint_color,
            text_fragments,
            inline_box_fragments,
        );
        if let Some(details) = details {
            layout.line_output.line_details[details.get() as usize - 1].inline_containing_width =
                containing_width;
        }
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
}

pub(super) fn publish_placements(
    engine: &mut LayoutEngine<'_, '_>,
    span: TokenSpan<'_>,
    metrics_view: MetricsView<'_>,
    replaced: &[ReplacedToken],
    point: Point,
    line_idx: usize,
    line: &LineLayout,
    static_inline_start: f64,
) {
    let reader = engine.read.reader;
    let text = engine.read.text;
    let needs_publication = match line.placements.as_ref() {
        Some(LinePlacements::Sequential { publish, .. }) => {
            *publish || !line.punctuation_space_advances.is_empty()
        }
        Some(LinePlacements::Positioned(plan)) => {
            !line.punctuation_space_advances.is_empty()
                || plan.offsets.iter().any(|run| run.y != 0.0)
                || line.has_tabs
                || span.metadata_iter().any(|token| {
                    matches!(
                        token.kind(),
                        InlineTokenKind::Image { .. }
                            | InlineTokenKind::AtomicBox { .. }
                            | InlineTokenKind::Ellipsis { .. }
                            | InlineTokenKind::AbsoluteAnchor { .. }
                    )
                })
                || metrics_view.chunks(span).any(|(_, metrics)| {
                    metrics.owner_box_idx != u32::MAX && {
                        let style = engine.read.reader.style(metrics.owner_box_idx as usize);
                        style.word_spacing() != 0.0 || style.word_spacing_factor() != 1.0
                    }
                })
        }
        Some(LinePlacements::Full(_)) => true,
        _ => false,
    };
    if !needs_publication {
        return;
    }
    let context = PlacementContext {
        replaced,
        point,
        line_idx,
        line,
        static_inline_start,
        publish_fragments: engine.placement.retains_output(),
    };
    let mut publish = |paint_order, token, metrics: &InlineTokenMetrics, placement| {
        publish_placement(engine, &context, paint_order, token, metrics, placement);
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
}

struct PlacementContext<'a> {
    replaced: &'a [ReplacedToken],
    point: Point,
    line_idx: usize,
    line: &'a LineLayout,
    static_inline_start: f64,
    publish_fragments: bool,
}

fn publish_placement(
    engine: &mut LayoutEngine<'_, '_>,
    context: &PlacementContext<'_>,
    paint_order: usize,
    token: TokenMetadata,
    metrics: &InlineTokenMetrics,
    placement: TokenPlacement,
) {
    let paint_order = paint_order as u32;
    let offset = placement.y_offset as f64;
    let ascent = metrics.ascent as f64;
    if let InlineTokenKind::Image { payload_idx } = token.kind() {
        debug_assert!(context.line.publishes_line);
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
        } = context
            .replaced
            .get(payload_idx as usize)
            .expect("image token payload index in bounds")
        else {
            unreachable!("image token must reference an image payload")
        };
        let border_offset = Point::new(
            placement.x + placement.relative_offset.x + *margin_left,
            context.line.baseline - ascent - offset + *margin_top,
        ) + *position_offset;
        if *set_box_geometry {
            engine
                .geometry
                .set_point(*box_idx as usize, context.point + border_offset.to_vec2());
            engine.geometry.set_size(*box_idx as usize, *border_size);
        }
        let content_offset = border_offset + content_inset.to_vec2();
        let intrinsic = engine
            .read
            .reader
            .image_intrinsic(*box_idx as usize)
            .expect("image fragment owner must retain intrinsic geometry")
            .size;
        let style = engine.read.reader.style(*box_idx as usize);
        if style.visibility() == html_style_model::Visibility::Hidden {
            return;
        }
        let object = crate::boxes::replaced::replaced_object_geometry(
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
                line_idx: context.line_idx,
                image_idx: *image_idx,
                box_idx: *box_idx,
                offset: content_offset + object.rect.origin().to_vec2(),
                size: object.rect.size(),
                clip: object.clip + content_offset.to_vec2(),
                paint_order,
            });
    } else if let InlineTokenKind::AtomicBox { payload_idx } = token.kind() {
        debug_assert!(context.line.publishes_line);
        let ReplacedToken::AtomicBox {
            box_idx,
            border_size,
            containing_width,
            containing_height,
            margin_left,
            margin_top,
        } = context
            .replaced
            .get(payload_idx as usize)
            .expect("atomic token payload index in bounds")
        else {
            unreachable!("atomic token must reference an atomic payload")
        };
        let border_point = Point::new(
            context.point.x + placement.x + placement.relative_offset.x + *margin_left,
            context.point.y + context.line.baseline - ascent - offset + *margin_top,
        );
        engine.geometry.set_point(*box_idx as usize, border_point);
        let paint_start = engine.fragments.state().paint_program.commands.len();
        let height = engine.read.reader.style(*box_idx as usize).height();
        let authored_height_is_definite =
            resolve_vertical_size(height, *containing_height, 0.0).is_some();
        let request = crate::BoxLayoutRequest::assigned(
            *box_idx as usize,
            *containing_width,
            *containing_height,
            *border_size,
            authored_height_is_definite,
        );
        let children_start = engine.fragments.line_len();
        // The outer line owns fragmentation. Its nested formatting context
        // must not insert page gaps into the measured atomic item.
        let _ = engine.layout_box(request);
        let output = &mut engine.fragments.state_mut().line_output;
        let children_end = output.lines.len();
        if children_start < children_end {
            output
                .atomic_inline_lines
                .push((LineId::new(context.line_idx), children_start..children_end));
        }
        engine
            .fragments
            .state_mut()
            .paint_program
            .record_line_since(paint_start, context.line_idx, paint_order);
    } else if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
        if !context.publish_fragments {
            return;
        }
        debug_assert!(context.line.publishes_line);
        let adjusted_space = matches!(engine.read.text.character(glyph_idx), ' ' | '\u{00a0}') && {
            let style = engine.read.reader.style(metrics.owner_box_idx as usize);
            style.word_spacing_factor() != 1.0 || style.word_spacing() != 0.0
        };
        let justified_space = context
            .line
            .punctuation_space_advances
            .iter()
            .find(|&&(index, _)| index == glyph_idx)
            .map(|&(_, advance)| advance);
        let layout = engine.fragments.state_mut();
        if offset != 0.0 {
            push_glyph_offset_run(
                &mut layout.line_output.line_glyph_offsets,
                context.line_idx,
                glyph_idx,
                offset as f32,
            );
        }
        if token.is_tab() || adjusted_space || justified_space.is_some() {
            layout.line_output.line_glyph_advances.push_to(
                context.line_idx,
                GlyphAdvanceRun {
                    range: glyph_idx..glyph_idx + 1,
                    advance: justified_space.unwrap_or(placement.advance as f32),
                },
            );
        }
    } else if let InlineTokenKind::Ellipsis { glyph } = token.kind() {
        debug_assert!(context.line.publishes_line);
        engine
            .fragments
            .state_mut()
            .line_output
            .ellipsis_fragments
            .push(EllipsisFragment {
                line_idx: context.line_idx,
                glyph,
                offset: Point::new(placement.x, offset),
                visible: metrics.visible,
            });
    } else if let InlineTokenKind::AbsoluteAnchor { box_idx } = token.kind() {
        let static_y = engine
            .scratch
            .floats
            .current_float_context()
            .source_position_at_or_after(context.point.y);
        absolute_positioning::defer(
            engine,
            box_idx as usize,
            Point::new(
                if engine
                    .read
                    .reader
                    .layout_style(box_idx as usize)
                    .static_position_inline
                {
                    context.point.x + placement.x
                } else {
                    context.static_inline_start
                },
                static_y,
            ),
        );
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
