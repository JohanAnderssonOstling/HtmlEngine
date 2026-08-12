//! Durable text, box, image, and glyph-fragment publication.

use super::*;

/// Records independently positioned text fragments only for lines where
/// replaced inline content interrupts the source text. Text-only lines keep
/// the compact implicit representation (`None`, whole range at x=0).
pub(super) fn positioned_text_fragments(
    span: &[InlineToken],
    placements: &[TokenPlacement],
    runs: &[InlineTokenMetrics],
) -> Option<Box<[LineTextFragment]>> {
    let mut expected_glyph = None;
    let split = span.iter().any(|token| {
        let discontinuous = match token.kind() {
            InlineTokenKind::Glyph { glyph_idx } => {
                let discontinuous = expected_glyph.is_some_and(|expected| expected != glyph_idx);
                expected_glyph = Some(glyph_idx + 1);
                discontinuous
            }
            _ => false,
        };
        discontinuous
            || matches!(
                token.kind(),
                InlineTokenKind::Image { .. }
                    | InlineTokenKind::AtomicBox { .. }
                    | InlineTokenKind::InlineBoundary { .. }
            )
            || token.run_metrics(runs).placement_required
            || !token.run_metrics(runs).visible
    });
    if !split {
        return None;
    }

    let mut fragments = Vec::<LineTextFragment>::new();
    let mut current_fragment: Option<usize> = None;
    let mut current_run_idx = None;
    for (paint_order, (token, placement)) in span.iter().zip(placements).enumerate() {
        if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
            if let Some(index) = current_fragment
                && fragments[index].glyphs.end == glyph_idx
                && current_run_idx == Some(token.run_idx)
                && fragments[index].visible == token.run_metrics(runs).visible
                && !token.run_metrics(runs).placement_required
            {
                fragments[index].glyphs.end += 1;
                continue;
            }
            fragments.push(LineTextFragment {
                glyphs: glyph_idx..glyph_idx + 1,
                offset_x: placement.x + placement.relative_offset.x,
                paint_order: paint_order as u32,
                visible: token.run_metrics(runs).visible,
            });
            current_fragment = Some(fragments.len() - 1);
            current_run_idx = Some(token.run_idx);
        } else {
            current_fragment = None;
            current_run_idx = None;
        }
    }
    Some(fragments.into_boxed_slice())
}

pub(super) fn positioned_inline_box_fragments(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    container_box_idx: usize,
    span: &[InlineToken],
    placements: &[TokenPlacement],
    runs: &[InlineTokenMetrics],
    replaced: &[ReplacedToken],
    line_height: f64,
    baseline: f64,
    containing_width: f64,
) -> Vec<LineInlineBoxFragment> {
    positioned_inline_box_fragments_from(
        engine,
        container_box_idx,
        span.iter().zip(placements.iter().copied()),
        runs,
        replaced,
        line_height,
        baseline,
        containing_width,
    )
}

fn positioned_plain_inline_box_fragments(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    container_box_idx: usize,
    span: &[InlineToken],
    runs: &[InlineTokenMetrics],
    replaced: &[ReplacedToken],
    line_height: f64,
    baseline: f64,
    containing_width: f64,
) -> Vec<LineInlineBoxFragment> {
    let mut x = 0.0;
    let placements = span.iter().map(|token| {
        let advance = token.advance_at(runs, x);
        let placement = TokenPlacement {
            x,
            advance,
            y_offset: 0.0,
            relative_offset: Vec2::ZERO,
        };
        x += advance;
        (token, placement)
    });
    positioned_inline_box_fragments_from(
        engine,
        container_box_idx,
        placements,
        runs,
        replaced,
        line_height,
        baseline,
        containing_width,
    )
}

#[allow(clippy::too_many_arguments)]
fn positioned_inline_box_fragments_from<'a>(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    container_box_idx: usize,
    placements: impl IntoIterator<Item = (&'a InlineToken, TokenPlacement)>,
    runs: &[InlineTokenMetrics],
    replaced: &[ReplacedToken],
    line_height: f64,
    baseline: f64,
    containing_width: f64,
) -> Vec<LineInlineBoxFragment> {
    // A line normally intersects only a handful of inline boxes. A compact
    // linear accumulator beats allocating a box-count-sized side table for
    // every line while still publishing one durable fragment per owner.
    let mut bounds = Vec::<LineInlineBoxFragment>::new();
    for (paint_order, (token, placement)) in placements.into_iter().enumerate() {
        let paint_order = paint_order as u32;
        if placement.advance <= 0.0
            && !matches!(token.kind(), InlineTokenKind::InlineBoundary { .. })
        {
            continue;
        }
        let metrics = token.run_metrics(runs);
        let token_top = (baseline - metrics.ascent as f64 - placement.y_offset as f64) as f32;
        let token_bottom = (baseline + metrics.descent as f64 - placement.y_offset as f64) as f32;
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
                        placement.x + margin_left + left_inset,
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
                    let border_left = placement.x + margin_left;
                    let border_top =
                        baseline - metrics.ascent as f64 - placement.y_offset as f64 + margin_top;
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
        let mut owner = match token.kind() {
            // Replaced and atomic boxes paint themselves. Their occupied span
            // contributes to enclosing inline fragments, starting at parent.
            InlineTokenKind::Image { payload_idx } => match replaced.get(payload_idx as usize) {
                Some(ReplacedToken::Image { box_idx, .. }) => {
                    engine.reader.get_parent(*box_idx as usize)
                }
                _ => None,
            },
            InlineTokenKind::AtomicBox { payload_idx } => {
                match replaced.get(payload_idx as usize) {
                    Some(ReplacedToken::AtomicBox { box_idx, .. }) => {
                        engine.reader.get_parent(*box_idx as usize)
                    }
                    _ => None,
                }
            }
            InlineTokenKind::InlineBoundary { .. } => boundary.map(|boundary| boundary.0),
            _ => (token.run_metrics(runs).owner_box_idx != u32::MAX)
                .then_some(token.run_metrics(runs).owner_box_idx as usize),
        };
        let mut outside_formatting_context = false;
        while let Some(box_idx) = owner {
            let is_inline = matches!(
                engine.reader.box_layout_mode(box_idx),
                Some(LayoutMode::Inline(_))
            );
            let has_propagated_decoration = !engine
                .reader
                .style(box_idx)
                .text_decoration_lines()
                .is_empty();
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
                let (fragment_top, fragment_bottom) = if is_inline {
                    let style = engine.reader.style(box_idx);
                    let font_size = style.font_size() as f64;
                    let font_metrics = engine.reader.font_metrics(box_idx);
                    let (ascent, descent) = font_metrics
                        .line_box_ratios()
                        .map_or((font_size * 0.8, font_size * 0.2), |(ascent, descent)| {
                            (font_size * ascent as f64, font_size * descent as f64)
                        });
                    let mut top = fragment_baseline - ascent as f32;
                    let mut bottom = fragment_baseline + descent as f32;
                    let trim = style.text_box_trim();
                    let edge = style.text_box_edge();
                    if matches!(trim, TextBoxTrim::Start | TextBoxTrim::Both) {
                        top = match edge.over {
                            TextBoxOverEdge::Text => top,
                            TextBoxOverEdge::Cap => {
                                fragment_baseline
                                    - font_size as f32 * font_metrics.cap_height_ratio()
                            }
                            TextBoxOverEdge::Ex => {
                                fragment_baseline - font_size as f32 * font_metrics.x_height_ratio()
                            }
                        };
                    }
                    if matches!(trim, TextBoxTrim::End | TextBoxTrim::Both)
                        && matches!(edge.under, TextBoxUnderEdge::Alphabetic)
                    {
                        bottom = fragment_baseline;
                    }
                    (top, bottom)
                } else {
                    (token_top, token_bottom)
                };
                let own_boundary = boundary.filter(|boundary| boundary.0 == box_idx);
                let fragment_start = (own_boundary.map_or(placement.x, |boundary| boundary.1)
                    + relative_offset.x) as f32;
                let fragment_end = (own_boundary
                    .map_or(placement.x + placement.advance, |boundary| boundary.1)
                    + relative_offset.x) as f32;
                if let Some(fragment) = bounds
                    .iter_mut()
                    .find(|fragment| fragment.box_idx as usize == box_idx)
                {
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
                        || engine.reader.inline_fragment_edges(box_idx),
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
            if stops_text_decoration_propagation(engine, box_idx) {
                break;
            }
            // Inline fragments belong to this formatting context only. A
            // nested table/block context can share flattened run storage,
            // but must not recreate decorations established outside its
            // container on the nested line.
            if box_idx == container_box_idx {
                outside_formatting_context = true;
            }
            owner = engine.reader.get_parent(box_idx);
        }
        // Enclosing inline backgrounds must precede the replaced element's
        // own background and border in paint order, so append this fragment
        // after accumulating its ancestor chain.
        if let Some(fragment) = replaced_border_fragment {
            bounds.push(fragment);
        }
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
    engine: &crate::layout::LayoutEngine<'_, '_>,
    box_idx: usize,
    containing_width: f64,
) -> Vec2 {
    let mut offset = Vec2::ZERO;
    let mut current = Some(box_idx);
    while let Some(current_idx) = current {
        if !matches!(
            engine.reader.box_layout_mode(current_idx),
            Some(LayoutMode::Inline(_))
        ) {
            break;
        }
        let style = engine.reader.style(current_idx);
        if style.position() == html_style_model::PositionMode::Relative {
            offset += crate::layout::box_positioning::relative_position_offset(
                &style,
                containing_width,
                None,
            );
        }
        current = engine.reader.get_parent(current_idx);
    }
    offset
}

/// Returns the alignment of the fragment owner itself and its inline
/// ancestors, excluding shifts authored on descendants. Decoration geometry
/// is tied to the decorating box's baseline, not whichever descendant token
/// happened to create its accumulated fragment first.
fn inline_fragment_vertical_align_offset(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    box_idx: usize,
    line_height: f64,
    baseline: f64,
) -> f64 {
    let mut offset = 0.0;
    let mut current = Some(box_idx);
    while let Some(current_idx) = current {
        if !matches!(
            engine.reader.box_layout_mode(current_idx),
            Some(LayoutMode::Inline(_))
        ) {
            break;
        }
        let style = engine.reader.style(current_idx);
        let value = style.vertical_align();
        if !value.is_initial() {
            let own_line_height = resolved_line_height(style);
            let font_size = style.font_size() as f64;
            let (ascent, descent) = engine
                .reader
                .font_metrics(current_idx)
                .line_box_ratios()
                .map_or((font_size * 0.8, font_size * 0.2), |(ascent, descent)| {
                    (font_size * ascent as f64, font_size * descent as f64)
                });
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
        current = engine.reader.get_parent(current_idx);
    }
    offset
}

fn has_inline_fragment_owners(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    span: &[InlineToken],
    runs: &[InlineTokenMetrics],
    replaced: &[ReplacedToken],
) -> bool {
    span.iter().any(|token| {
        let mut owner = match token.kind() {
            InlineTokenKind::Image { payload_idx } => match replaced.get(payload_idx as usize) {
                Some(ReplacedToken::Image { box_idx, .. }) => {
                    engine.reader.get_parent(*box_idx as usize)
                }
                _ => None,
            },
            InlineTokenKind::AtomicBox { payload_idx } => {
                match replaced.get(payload_idx as usize) {
                    Some(ReplacedToken::AtomicBox { box_idx, .. }) => {
                        engine.reader.get_parent(*box_idx as usize)
                    }
                    _ => None,
                }
            }
            InlineTokenKind::InlineBoundary { payload_idx } => {
                match replaced.get(payload_idx as usize) {
                    Some(ReplacedToken::InlineBoundary { box_idx, .. }) => Some(*box_idx as usize),
                    _ => None,
                }
            }
            _ => (token.run_metrics(runs).owner_box_idx != u32::MAX)
                .then_some(token.run_metrics(runs).owner_box_idx as usize),
        };
        while let Some(box_idx) = owner {
            let is_inline = matches!(
                engine.reader.box_layout_mode(box_idx),
                Some(LayoutMode::Inline(_))
            );
            if is_inline
                || !engine
                    .reader
                    .style(box_idx)
                    .text_decoration_lines()
                    .is_empty()
            {
                return true;
            }
            if stops_text_decoration_propagation(engine, box_idx) {
                break;
            }
            owner = engine.reader.get_parent(box_idx);
        }
        false
    })
}

/// Text decorations propagate through ordinary in-flow block and table boxes;
/// they are not inherited, but each ancestor that establishes a decoration
/// paints it across descendant inline content. Atomic inline formatting
/// contexts and out-of-flow descendants form the CSS propagation boundary.
fn stops_text_decoration_propagation(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    box_idx: usize,
) -> bool {
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

/// Writes fragments for one broken line using a precomputed line layout.
///
/// The write phase only mutates the document; all layout decisions have
/// already been made by `break_lines` and `measure_line`.
pub(super) fn write_line_fragments(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    container_box_idx: usize,
    span: &[InlineToken],
    runs: &[InlineTokenMetrics],
    replaced: &[ReplacedToken],
    origin: Point,
    line: &LineLayout,
    paint_color: Option<u32>,
    containing_width: f64,
) -> f64 {
    let timing_started = engine.start_timing();
    let point = Point::new(origin.x + line.x_offset, origin.y);
    let line_idx = engine.fragments.state().line_output.lines.len();
    if line.placements.is_none() && !line.glyph_range.is_empty() {
        let inline_box_fragments = positioned_plain_inline_box_fragments(
            engine,
            container_box_idx,
            span,
            runs,
            replaced,
            line.line_height,
            line.baseline,
            containing_width,
        );
        let inline_box_fragments = engine
            .fragments
            .push_inline_box_fragments(inline_box_fragments, engine.reader.box_count());
        let layout = engine.fragments.state_mut();
        layout.line_output.lines.push(Line {
            owner_box_idx: container_box_idx as u32,
            glyphs: line.glyph_range.clone(),
            point,
            height: line.line_height,
            baseline: line.baseline,
            word_spacing: line.word_spacing,
            letter_spacing: line.letter_spacing,
            optical_offset_x: line.optical_offset_x,
            paint_color,
            text_fragments: None,
            prepared_text_runs: 0..0,
            native_text_runs_complete: false,
            inline_box_fragments,
        });
        layout.line_output.line_glyph_offsets.push(Vec::new());
        layout.line_output.line_glyph_advances.push(
            line.punctuation_space_advances
                .iter()
                .map(|&(glyph_idx, advance)| GlyphAdvanceRun {
                    range: glyph_idx..glyph_idx + 1,
                    advance,
                })
                .collect(),
        );
        if let Some((glyph, x)) = line.hyphen {
            layout.line_output.hyphen_fragments.push(HyphenFragment {
                line_idx,
                glyph,
                offset: Point::new(x, 0.0),
                visible: true,
            });
        }
        engine.push_line_owner(container_box_idx as u32);
        engine.record_timing(|timings| {
            timings.write_line_fragments += timing_started.elapsed();
        });
        return line.line_height;
    }
    let has_fragment_owners = has_inline_fragment_owners(engine, span, runs, replaced);
    let needs_ordered_fragments = has_fragment_owners
        || span.iter().any(|token| {
            matches!(
                token.kind(),
                InlineTokenKind::Image { .. }
                    | InlineTokenKind::AtomicBox { .. }
                    | InlineTokenKind::InlineBoundary { .. }
            ) || !token.run_metrics(runs).visible
        });
    let fallback_placements;
    let placements = if let Some(placements) = line.placements.as_deref() {
        Some(placements)
    } else if needs_ordered_fragments {
        fallback_placements = build_token_placements(
            engine,
            span,
            runs,
            0.0,
            line.line_height,
            line.baseline,
            containing_width,
        );
        Some(fallback_placements.as_slice())
    } else {
        None
    };
    let text_fragments =
        placements.and_then(|placements| positioned_text_fragments(span, placements, runs));
    let inline_box_fragments = if has_fragment_owners {
        let placements = placements.expect("inline fragment owners require token placements");
        positioned_inline_box_fragments(
            engine,
            container_box_idx,
            span,
            placements,
            runs,
            replaced,
            line.line_height,
            line.baseline,
            containing_width,
        )
    } else {
        Vec::new()
    };
    // Absolute/float anchors are executable source-position markers, not
    // line-box content. Keep processing them below, but do not publish the
    // zero-height carrier line that exists solely to trigger that work.
    let publishes_line = line.publish_empty
        || span.iter().any(|token| match token.kind() {
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
                        token.width != 0.0
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
                let glyph = engine.text.glyph_at(glyph_idx as usize).unwrap_or_default();
                let character = engine.text.glyph_metric(glyph).ch();
                token.width != 0.0
                    || !character.is_whitespace()
                    || token.run_metrics(runs).white_space.preserves_spaces()
            }
            _ => true,
        });
    if publishes_line {
        let inline_box_fragments = engine
            .fragments
            .push_inline_box_fragments(inline_box_fragments, engine.reader.box_count());
        let layout = engine.fragments.state_mut();
        layout.line_output.lines.push(Line {
            owner_box_idx: container_box_idx as u32,
            glyphs: line.glyph_range.clone(),
            point,
            height: line.line_height,
            baseline: line.baseline,
            word_spacing: line.word_spacing,
            letter_spacing: line.letter_spacing,
            optical_offset_x: line.optical_offset_x,
            paint_color,
            text_fragments,
            prepared_text_runs: 0..0,
            native_text_runs_complete: false,
            inline_box_fragments,
        });
        layout.line_output.line_glyph_offsets.push(Vec::new());
        layout.line_output.line_glyph_advances.push(Vec::new());
        layout.line_output.line_glyph_advances[line_idx].extend(
            line.punctuation_space_advances
                .iter()
                .map(|&(glyph_idx, advance)| GlyphAdvanceRun {
                    range: glyph_idx..glyph_idx + 1,
                    advance,
                }),
        );
        if let Some((glyph, x)) = line.hyphen {
            let visible = span
                .iter()
                .rev()
                .find(|token| matches!(token.kind(), InlineTokenKind::Glyph { .. }))
                .is_none_or(|token| token.run_metrics(runs).visible);
            layout.line_output.hyphen_fragments.push(HyphenFragment {
                line_idx,
                glyph,
                offset: Point::new(x, 0.0),
                visible,
            });
        }
        // Ownership is also the transient stacking-order key. Finalization
        // drops it after sorting when overflow clips are not needed.
        engine.push_line_owner(container_box_idx as u32);
    }

    let Some(placements) = line.placements.as_deref() else {
        engine.record_timing(|t| t.write_line_fragments += timing_started.elapsed());
        return if publishes_line {
            line.line_height
        } else {
            0.0
        };
    };

    for (paint_order, (token, placement)) in span.iter().zip(placements).enumerate() {
        let paint_order = paint_order as u32;
        let offset = placement.y_offset as f64;
        let ascent = token.run_metrics(runs).ascent as f64;
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
                continue;
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
            let height = engine.reader.style(*box_idx as usize).height();
            let authored_height_is_definite = match height {
                html_style_model::UsedPreferredSize::Px(_) => true,
                html_style_model::UsedPreferredSize::Percent(_)
                | html_style_model::UsedPreferredSize::Stretch => containing_height.is_some(),
                html_style_model::UsedPreferredSize::Calc { .. }
                | html_style_model::UsedPreferredSize::Comparison { .. } => {
                    !height.percentage_dependent() || containing_height.is_some()
                }
                html_style_model::UsedPreferredSize::Auto
                | html_style_model::UsedPreferredSize::MinContent
                | html_style_model::UsedPreferredSize::MaxContent
                | html_style_model::UsedPreferredSize::FitContent => false,
            };
            let request = crate::layout::BoxLayoutRequest::atomic_assigned(
                *box_idx as usize,
                *containing_width,
                *containing_height,
                *border_size,
                authored_height_is_definite,
            );
            let _ = engine.layout_box(request);
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
            let layout = engine.fragments.state_mut();
            if offset != 0.0 {
                push_glyph_offset_run(
                    &mut layout.line_output.line_glyph_offsets[line_idx],
                    glyph_idx,
                    offset as f32,
                );
            }
            if token.is_tab(runs) {
                layout.line_output.line_glyph_advances[line_idx].push(GlyphAdvanceRun {
                    range: glyph_idx..glyph_idx + 1,
                    advance: placement.advance as f32,
                });
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
                    visible: token.run_metrics(runs).visible,
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
                Point::new(point.x + placement.x, static_y),
            );
        }
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
fn push_glyph_offset_run(offsets: &mut Vec<GlyphOffsetRun>, glyph_idx: u32, offset: f32) {
    if let Some(last) = offsets
        .last_mut()
        .filter(|last| last.range.end == glyph_idx && last.offset == offset)
    {
        last.range.end = glyph_idx + 1;
    } else {
        offsets.push(GlyphOffsetRun {
            range: glyph_idx..glyph_idx + 1,
            offset,
        });
    }
}
