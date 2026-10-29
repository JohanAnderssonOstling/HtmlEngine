//! Select and measure lines; float-sensitive feedback stays within composition.
use super::fragments::{LineEmissionContext, prepare_line, pseudo_line_heights};
use super::paragraph::ParagraphLayoutResult;
use super::publication::emit_prepared_paragraph;
use super::retained::{CachedParagraph, ParagraphDependencies, ParagraphLookup, ParagraphSlot};
use super::text_tokens::{InlineOverflow, inline_overflow};
use super::tokens::{INLINE_KP_CACHE_MIN_TOKENS, InlineTokenKind, InlineTokens, indent_for_line};
use super::wrapping::{self, LineConstraints, break_lines_knuth, break_next_line};
use super::{InlineFormattingInput, Size, TextAlign, TextDirection};
use crate::layout::block::layout_float;
use crate::layout::{BoxLayoutRequest, LayoutEngine};
use kurbo::{Insets, Point};
use std::mem::take;

/// Line algorithms read retained slices or work on an owned token buffer.
/// Derived line-break plans stay local until the borrowed token view ends.
pub(super) fn layout_prepared_tokens(
    engine: &mut LayoutEngine<'_, '_>,
    input: &InlineFormattingInput,
    tokens: &mut InlineTokens,
    has_float_exclusions: bool,
    immutable_text: bool,
) -> Size {
    profile_scope!("layout.paragraph");
    if tokens.is_empty() {
        return Size::ZERO;
    }
    let overflow = inline_overflow(engine, input.container_box_idx, tokens);
    let (first_line_height, first_letter_height) = if input.starts_block {
        pseudo_line_heights(engine, input.container_box_idx)
    } else {
        (None, None)
    };
    let cacheable = immutable_text && !has_float_exclusions && tokens.replaced.is_empty();
    let dependencies = ParagraphDependencies::new(
        engine,
        input,
        overflow,
        first_line_height,
        first_letter_height,
    );
    let slot = ParagraphSlot::new(engine, input);
    let mut paragraph = match engine
        .paragraph_layouts
        .take(slot, &dependencies, tokens, cacheable)
    {
        ParagraphLookup::Hit(cached) => {
            emit_prepared_paragraph(engine, input, tokens, &cached.paragraph);
            let size = cached.paragraph.size;
            engine.record_timing(|timings| timings.reused_lines += cached.paragraph.lines.len());
            engine.paragraph_layouts.insert(slot, cached);
            return size;
        }
        ParagraphLookup::Miss { paragraph } => paragraph,
    };
    if has_float_exclusions {
        prepare_lines_around_float_exclusions(
            engine,
            input,
            tokens,
            overflow,
            first_line_height,
            first_letter_height,
            &mut paragraph,
        );
    } else {
        let mut scratch = engine.paragraphs.pop().unwrap_or_default();
        let ragged_knuth = engine.config.book_optimized_text()
            && input.paragraph.text_align
                == (if input.paragraph.direction == TextDirection::Rtl {
                    TextAlign::Right
                } else {
                    TextAlign::Left
                })
            && !engine.config.force_justify()
            && !tokens.iter().any(|token| {
                token
                    .run_metrics(&tokens.runs)
                    .white_space
                    .preserves_spaces()
            });
        let use_knuth = !tokens.iter().any(|token| token.is_tab())
            && (matches!(input.paragraph.text_align, TextAlign::Justify)
                || engine.config.force_justify()
                || ragged_knuth);
        let break_storage = take(&mut scratch.lines);
        if use_knuth && immutable_text && tokens.len() >= INLINE_KP_CACHE_MIN_TOKENS {
            tokens.prepare_kp_plan(engine.config.book_optimized_text());
        }
        let lines = if use_knuth {
            break_lines_knuth(
                engine,
                tokens,
                &tokens.runs,
                input.paragraph,
                input.area.width,
                tokens.kp_plan.as_ref(),
                ragged_knuth,
                break_storage,
                &mut scratch.solver,
            )
        } else {
            wrapping::greedy::break_lines_into(
                engine,
                tokens,
                &tokens.runs,
                input.paragraph,
                input.area.width,
                break_storage,
            )
        };
        let context = LineEmissionContext::new(
            tokens,
            input,
            overflow,
            first_line_height,
            first_letter_height,
        );
        prepare_selected_paragraph(engine, &context, &lines, &mut paragraph);
        scratch.lines = lines;
        scratch.lines.clear();
        // Publication can recurse into atomic content; line breaking is done,
        // so nested paragraphs can reuse its scratch immediately.
        engine.paragraphs.push(scratch);
    }
    emit_prepared_paragraph(engine, input, tokens, &paragraph);
    let size = paragraph.size;
    if cacheable {
        engine.paragraph_layouts.insert(
            slot,
            CachedParagraph::new(dependencies, tokens.len(), paragraph),
        );
    }
    size
}

/// Build a position-independent result without touching published output.
fn prepare_selected_paragraph(
    engine: &LayoutEngine<'_, '_>,
    context: &LineEmissionContext<'_>,
    lines: &[wrapping::BrokenLine],
    paragraph: &mut ParagraphLayoutResult,
) {
    paragraph.lines.clear();
    paragraph.adjustments.clear();
    let mut y = 0.0;
    let mut width = 0.0f64;
    for (index, selected) in lines.iter().enumerate() {
        let (line, measured_width, height) = prepare_line(
            engine,
            context,
            selected,
            index == 0,
            Point::new(0.0, y),
            index,
            &mut paragraph.adjustments,
        );
        paragraph.lines.push(line);
        y += height;
        width = width.max(measured_width);
    }
    paragraph.size = Size::new(width, y);
}

/// Selects and measures lines whose available interval changes around floats.
fn prepare_lines_around_float_exclusions(
    engine: &mut LayoutEngine<'_, '_>,
    input: &InlineFormattingInput,
    tokens: &InlineTokens,
    overflow: InlineOverflow,
    first_line_height: Option<f64>,
    first_letter_height: Option<f64>,
    paragraph: &mut ParagraphLayoutResult,
) {
    let timing_started = engine.start_timing();
    let emission = LineEmissionContext::new(
        tokens,
        input,
        overflow,
        first_line_height,
        first_letter_height,
    );
    paragraph.lines.clear();
    paragraph.adjustments.clear();
    let mut y = 0.0;
    let mut max_width = 0.0f64;
    let mut start = 0usize;
    let mut is_first_line = input.starts_block;
    let mut starts_indented_line = input.paragraph.indent.first_line;
    let mut float_cursor = 0;
    let mut placed_boxes = Vec::new();
    while start < tokens.len() {
        let (result, left) = loop {
            let container_line_height =
                engine.reader.style(input.container_box_idx).resolved_line_height();
            let probe_line_height = tokens[start..]
                .iter()
                .filter(|token| {
                    !matches!(
                        token.kind(),
                        InlineTokenKind::FloatAnchor { .. }
                            | InlineTokenKind::AbsoluteAnchor { .. }
                            | InlineTokenKind::Opportunity
                    )
                })
                .map(|token| {
                    let metrics = token.run_metrics(&tokens.runs);
                    (f64::from(metrics.ascent) + f64::from(metrics.descent))
                        .max(f64::from(metrics.line_height))
                })
                .next()
                .unwrap_or(container_line_height)
                .max(if is_first_line {
                    first_line_height.unwrap_or(container_line_height)
                } else {
                    container_line_height
                })
                .max(1.0);
            // A zero-width opportunity before an atomic inline must not turn into
            // an empty line that leaves the atomic item overflowing beside a
            // float. If the item fits the containing block but not the current
            // exclusion lane, retry at the first vertical position where it does.
            // Percentages remain resolved against `input.area.width`; only line
            // placement is affected by the float lane.
            if let Some(item) = tokens[start..].iter().find(|token| token.width() > 0.0) {
                let item_width = item.width();
                let item_height = {
                    let metrics = item.run_metrics(&tokens.runs);
                    (f64::from(metrics.ascent) + f64::from(metrics.descent))
                        .max(f64::from(metrics.line_height))
                        .max(1.0)
                };
                if item_width <= input.area.width {
                    let current_y = input.area.origin.y + y;
                    if let Some(next_y) = engine
                        .floats
                        .current_float_context()
                        .map(|context| {
                            context.next_y_fitting(
                                current_y,
                                item_width,
                                item_height,
                                input.area.origin.x,
                                input.area.origin.x + input.area.width,
                            )
                        })
                        .filter(|next_y| *next_y > current_y + 0.001)
                    {
                        y = next_y - input.area.origin.y;
                    }
                }
            }
            let (left, right) = engine
                .floats
                .current_float_context()
                .map(|context| {
                    context.available(
                        input.area.origin.y + y,
                        probe_line_height,
                        input.area.origin.x,
                        input.area.origin.x + input.area.width,
                    )
                })
                .unwrap_or((input.area.origin.x, input.area.origin.x + input.area.width));
            let line_indent = indent_for_line(
                input.paragraph.indent.amount,
                input.paragraph.indent.hanging,
                starts_indented_line,
            );
            let constraints = LineConstraints {
                indent: line_indent,
                tab_origin: if input.paragraph.direction == TextDirection::Rtl {
                    input.area.origin.x + input.area.width - right + line_indent
                } else {
                    (left - input.area.origin.x) + line_indent
                },
                width_limit: (right - left - line_indent).max(0.0),
            };
            let result = break_next_line(
                &engine.reader,
                tokens,
                &tokens.runs,
                input.paragraph,
                start,
                constraints,
            );
            // Inspect only the actual next line. A float changes that line's
            // available interval, so rebreak locally before publishing it.
            let scan_start = float_cursor.max(start).min(result.next_start);
            let anchor = tokens[scan_start..result.next_start]
                .iter()
                .enumerate()
                .find_map(|(offset, token)| match token.kind() {
                    InlineTokenKind::FloatAnchor { box_idx }
                        if !placed_boxes.contains(&box_idx) =>
                    {
                        Some((scan_start + offset, box_idx))
                    }
                    _ => None,
                });
            if let Some((index, box_idx)) = anchor {
                float_cursor = index + 1;
                placed_boxes.push(box_idx);
                let line_start = result.line.as_ref().map_or(start, |line| line.start);
                let mut preceding_advance = 0.0;
                let mut preceding_height = container_line_height;
                for token in &tokens[line_start..index] {
                    if matches!(
                        token.kind(),
                        InlineTokenKind::FloatAnchor { .. }
                            | InlineTokenKind::AbsoluteAnchor { .. }
                    ) {
                        continue;
                    }
                    preceding_advance += token.advance_at(
                        &engine.reader,
                        &tokens.runs,
                        constraints.tab_origin + preceding_advance,
                    );
                    let metrics = token.run_metrics(&tokens.runs);
                    preceding_height = preceding_height.max(
                        (f64::from(metrics.ascent) + f64::from(metrics.descent))
                            .max(f64::from(metrics.line_height)),
                    );
                }
                // Keep an unbreakable source span beside the anchor together.
                // The greedy breaker has already selected which line owns it.
                if !tokens[index]
                    .run_metrics(&tokens.runs)
                    .white_space
                    .allows_wrap()
                    && preceding_advance > 0.001
                {
                    for token in &tokens[index + 1..result.next_start] {
                        if matches!(
                            token.kind(),
                            InlineTokenKind::InlineBoundary { .. }
                                | InlineTokenKind::Opportunity
                                | InlineTokenKind::AbsoluteAnchor { .. }
                        ) {
                            continue;
                        }
                        if matches!(token.kind(), InlineTokenKind::Break { .. })
                            || token.run_metrics(&tokens.runs).white_space.allows_wrap()
                        {
                            break;
                        }
                        if !matches!(token.kind(), InlineTokenKind::FloatAnchor { .. }) {
                            preceding_advance += token.advance_at(
                                &engine.reader,
                                &tokens.runs,
                                constraints.tab_origin + preceding_advance,
                            );
                        }
                    }
                }
                let source_y = input.area.origin.y + y;
                let source_y = engine
                    .floats
                    .current_float_context()
                    .map_or(source_y, |context| {
                        context.clear_for(source_y, engine.reader.style(box_idx as usize).clear())
                    });
                place_float_anchor(
                    engine,
                    box_idx as usize,
                    Point::new(input.area.origin.x, source_y),
                    input.area.width,
                    input.area.percentage_height_basis,
                    Size::new(preceding_advance, preceding_height),
                );
                continue;
            }
            break (result, left);
        };
        let previous_start = start;
        start = result.next_start;
        starts_indented_line = result.ended_by_forced_break && input.paragraph.indent.each_line;
        let forced_clear = result
            .ended_by_forced_break
            .then(|| {
                tokens[previous_start..start]
                    .iter()
                    .rev()
                    .find_map(|token| match token.kind() {
                        InlineTokenKind::Break { clear } => Some(clear),
                        _ => None,
                    })
            })
            .flatten();

        if let Some(line) = result.line {
            let line_origin = Point::new(left - input.area.origin.x, y);
            let (line, width, height) = prepare_line(
                engine,
                &emission,
                &line,
                is_first_line,
                line_origin,
                paragraph.lines.len(),
                &mut paragraph.adjustments,
            );
            paragraph.lines.push(line);
            max_width = max_width.max(width);
            y += height;
            is_first_line = false;
        }
        if let Some(clear) = forced_clear {
            let current_y = input.area.origin.y + y;
            if let Some(cleared_y) = engine
                .floats
                .current_float_context()
                .map(|context| context.clear_for(current_y, clear))
            {
                y = y.max(cleared_y - input.area.origin.y);
            }
        }
        if start <= previous_start && start < tokens.len() {
            start = previous_start.saturating_add(1);
        }
    }

    // Float exclusions affect line placement, not the normal-flow height of
    // this inline formatting context. A formatting-context root that must
    // contain its own floats accounts for their bottom when its float context
    // is popped in `layout_box`; including the shared ancestor context here
    // incorrectly makes a paragraph as tall as a preceding sibling float.
    paragraph.size = Size::new(max_width, y);
    engine.record_timing(|timing| {
        timing.layout_runs_around_float_exclusions += timing_started.elapsed()
    });
}

fn place_float_anchor(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    position: Point,
    available_width: f64,
    containing_block_height: Option<f64>,
    preceding_line: Size,
) {
    let timing_started = engine.start_timing();
    let style = engine.reader.style(box_idx);
    let margins = Insets::new(
        style.margin_left().resolve(available_width),
        style.margin_top().resolve(available_width),
        style.margin_right().resolve(available_width),
        style.margin_bottom().resolve(available_width),
    );
    let resolved = engine.resolve_box_sizing(BoxLayoutRequest::normal(
        box_idx,
        available_width,
        containing_block_height,
    ));
    layout_float(engine, resolved, position, margins, preceding_line);
    engine.record_timing(|t| t.place_float_anchor += timing_started.elapsed());
}
