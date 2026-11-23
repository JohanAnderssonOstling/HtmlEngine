//! Retry float-sensitive line selection before committing paragraph geometry.
use super::hyphenation::DeferredHyphenation;
use super::wrapping::{LineConstraints, break_next_line};
use crate::block::layout_float;
use crate::inline::paragraph::{ParagraphLayoutResult, PreparedLine};
use crate::inline::s04_lines::{LinePreparationContext, prepare_line};
use crate::inline::tokens::{InlineTokenKind, InlineTokens};
use crate::inline::{InlineFormattingInput, TextDirection};
use crate::{BoxLayoutRequest, LayoutEngine, LayoutReader};
use kurbo::{Insets, Point, Size};

/// Selects and measures lines whose available interval changes around floats.
pub(super) fn compose_float_paragraph<'input>(
    engine: &mut LayoutEngine<'input, '_>,
    context: &LinePreparationContext<'_>,
    mut deferred: Option<&mut DeferredHyphenation<'input, '_>>,
    paragraph: &mut ParagraphLayoutResult,
) {
    paragraph.lines.clear();
    paragraph.adjustments.clear();
    let mut y = 0.0;
    let mut max_width = 0.0f64;
    let mut start = 0usize;
    let mut is_first_line = context.input.starts_block;
    let mut starts_indented_line = context.input.paragraph.indent.first_line;
    let mut float_cursor = 0;
    let mut placed_boxes = Vec::new();
    let mut probe_adjustments = Vec::new();
    while start < context.tokens.len() {
        let mut required_line_height = 0.0_f64;
        let (result, prepared_line) = loop {
            let container_line_height = engine
                .read
                .reader
                .style(context.input.container_box_idx)
                .resolved_line_height();
            let probe_line_height =
                probe_line_height(context, start, is_first_line, container_line_height)
                    .max(required_line_height);

            fit_first_item(engine, context, start, &mut y);
            let (left, right, constraints) =
                line_constraints(engine, context, y, probe_line_height, starts_indented_line);
            let result = break_next_line(
                engine,
                context.tokens.span(),
                context.tokens.metrics_view(),
                context.input.paragraph,
                start,
                constraints,
                deferred.as_deref_mut(),
            );
            let mut prepared_line = None;
            probe_adjustments.clear();
            if let Some(line) = &result.line {
                let (prepared, width) = prepare_line(
                    engine,
                    context,
                    line,
                    is_first_line,
                    Point::new(left - context.input.area.origin.x, y),
                    paragraph.lines.len(),
                    &mut probe_adjustments,
                );
                let height = prepared.height();
                prepared_line = Some((prepared, width));

                // The actual row can be taller than its first token. Query
                // exclusions again before accepting breaks at that height.
                if height > probe_line_height + 0.001 {
                    required_line_height = height;
                    continue;
                }
            }
            // Inspect only the actual next line. A float changes that line's
            // available interval, so rebreak locally before publishing it.
            let anchor = next_float_anchor(
                context.tokens,
                start,
                result.next_start,
                float_cursor,
                &placed_boxes,
            );
            if let Some((index, box_idx)) = anchor {
                float_cursor = index + 1;
                placed_boxes.push(box_idx);
                let line_start = result.line.as_ref().map_or(start, |line| line.start);
                let preceding_line = preceding_line_extent(
                    &engine.read.reader,
                    context,
                    line_start,
                    index,
                    result.next_start,
                    constraints.tab_origin,
                    container_line_height,
                );
                let source_y = context.input.area.origin.y + y;
                let source_y = engine
                    .scratch
                    .floats
                    .current_float_context()
                    .clear_for(source_y, engine.read.reader.style(box_idx as usize).clear());
                place_float_anchor(
                    engine,
                    box_idx as usize,
                    Point::new(context.input.area.origin.x, source_y),
                    context.input.area.width,
                    context.input.area.percentage_height_basis,
                    preceding_line,
                );
                continue;
            }
            if fit_selected_line(
                engine,
                context,
                prepared_line.as_ref(),
                right - left,
                &mut y,
            ) {
                continue;
            }
            paragraph.adjustments.append(&mut probe_adjustments);
            break (result, prepared_line);
        };
        let previous_start = start;
        start = result.next_start;
        starts_indented_line =
            result.ended_by_forced_break && context.input.paragraph.indent.each_line;
        let forced_clear = result
            .ended_by_forced_break
            .then(|| {
                context
                    .tokens
                    .span()
                    .slice(previous_start..start)
                    .metadata_iter()
                    .rev()
                    .find_map(|token| match token.kind() {
                        InlineTokenKind::Break { clear } => Some(clear),
                        _ => None,
                    })
            })
            .flatten();

        if let Some((line, width)) = prepared_line {
            y += line.height();
            paragraph.lines.push(line);
            max_width = max_width.max(width);
            is_first_line = false;
        }
        if let Some(clear) = forced_clear {
            let current_y = context.input.area.origin.y + y;
            let cleared_y = engine
                .scratch
                .floats
                .current_float_context()
                .clear_for(current_y, clear);
            y = y.max(cleared_y - context.input.area.origin.y);
        }
        if start <= previous_start && start < context.tokens.len() {
            start = previous_start.saturating_add(1);
        }
    }

    // Float exclusions affect line placement, not the normal-flow height of
    // this inline formatting context. A formatting-context root that must
    // contain its own floats accounts for their bottom when its float context
    // is popped in `layout_box`; including the shared ancestor context here
    // incorrectly makes a paragraph as tall as a preceding sibling float.
    paragraph.size = Size::new(max_width, y);
}

fn probe_line_height(
    context: &LinePreparationContext<'_>,
    start: usize,
    is_first_line: bool,
    container_line_height: f64,
) -> f64 {
    context
        .tokens
        .metrics_view()
        .iter(context.tokens.span().slice(start..))
        .filter(|(token, _)| {
            !matches!(
                token.kind(),
                InlineTokenKind::FloatAnchor { .. }
                    | InlineTokenKind::AbsoluteAnchor { .. }
                    | InlineTokenKind::ZeroWidthBreak
            )
        })
        .map(|(_, metrics)| metrics.line_extent())
        .next()
        .unwrap_or(container_line_height)
        .max(if is_first_line {
            context
                .policy
                .first_line_height
                .unwrap_or(container_line_height)
        } else {
            container_line_height
        })
        .max(1.0)
}

fn fit_first_item(
    engine: &LayoutEngine<'_, '_>,
    context: &LinePreparationContext<'_>,
    start: usize,
    y: &mut f64,
) {
    // A zero-width opportunity before an atomic inline must not turn into
    // an empty line that leaves the atomic item overflowing beside a
    // float. If the item fits the containing block but not the current
    // exclusion lane, retry at the first vertical position where it does.
    // Percentages remain resolved against `context.input.area.width`; only line
    // placement is affected by the float lane.
    if let Some((offset, (item, metrics))) = context
        .tokens
        .metrics_view()
        .iter(context.tokens.span().slice(start..))
        .enumerate()
        .find(|(offset, (token, _))| {
            token.width(context.tokens.span().stored_width_at(start + offset)) > 0.0
        })
    {
        let item_width = item.width(context.tokens.span().stored_width_at(start + offset));
        let item_height = metrics.line_extent().max(1.0);
        if item_width <= context.input.area.width {
            if let Some(next_y) = fitting_y(
                engine,
                context.input,
                *y,
                Size::new(item_width, item_height),
                0.001,
            ) {
                *y = next_y;
            }
        }
    }
}

fn line_constraints(
    engine: &LayoutEngine<'_, '_>,
    context: &LinePreparationContext<'_>,
    y: f64,
    probe_line_height: f64,
    starts_indented_line: bool,
) -> (f64, f64, LineConstraints) {
    let (left, right) = engine.scratch.floats.current_float_context().available(
        context.input.area.origin.y + y,
        probe_line_height,
        context.input.area.origin.x..context.input.area.origin.x + context.input.area.width,
    );
    let mut constraints =
        LineConstraints::new(context.input.paragraph, right - left, starts_indented_line);
    constraints.tab_origin = if context.input.paragraph.direction == TextDirection::Rtl {
        context.input.area.origin.x + context.input.area.width - right + constraints.indent
    } else {
        (left - context.input.area.origin.x) + constraints.indent
    };
    (left, right, constraints)
}

fn next_float_anchor(
    tokens: &InlineTokens<'_>,
    start: usize,
    end: usize,
    float_cursor: usize,
    placed_boxes: &[u32],
) -> Option<(usize, u32)> {
    let scan_start = float_cursor.max(start).min(end);
    tokens
        .span()
        .slice(scan_start..end)
        .metadata_iter()
        .enumerate()
        .find_map(|(offset, token)| match token.kind() {
            InlineTokenKind::FloatAnchor { box_idx } if !placed_boxes.contains(&box_idx) => {
                Some((scan_start + offset, box_idx))
            }
            _ => None,
        })
}

fn preceding_line_extent(
    reader: &LayoutReader<'_>,
    context: &LinePreparationContext<'_>,
    line_start: usize,
    index: usize,
    line_end: usize,
    tab_origin: f64,
    container_line_height: f64,
) -> Size {
    let mut preceding_advance = 0.0;
    let mut preceding_height = container_line_height;
    for (offset, (token, metrics)) in context
        .tokens
        .metrics_view()
        .iter(context.tokens.span().slice(line_start..index))
        .enumerate()
    {
        if matches!(
            token.kind(),
            InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. }
        ) {
            continue;
        }
        preceding_advance += token.advance_with_metrics(
            context.tokens.span().stored_width_at(line_start + offset),
            reader,
            metrics,
            tab_origin + preceding_advance,
        );
        preceding_height = preceding_height.max(metrics.line_extent());
    }
    // Keep an unbreakable source span beside the anchor together.
    // The greedy breaker has already selected which line owns it.
    if !context
        .tokens
        .metrics_view()
        .at(context.tokens.span(), index)
        .white_space
        .allows_wrap()
        && preceding_advance > 0.001
    {
        for (offset, (token, metrics)) in context
            .tokens
            .metrics_view()
            .iter(context.tokens.span().slice(index + 1..line_end))
            .enumerate()
        {
            if matches!(
                token.kind(),
                InlineTokenKind::InlineBoundary { .. }
                    | InlineTokenKind::ZeroWidthBreak
                    | InlineTokenKind::AbsoluteAnchor { .. }
            ) {
                continue;
            }
            if matches!(token.kind(), InlineTokenKind::Break { .. })
                || metrics.white_space.allows_wrap()
            {
                break;
            }
            if !matches!(token.kind(), InlineTokenKind::FloatAnchor { .. }) {
                preceding_advance += token.advance_with_metrics(
                    context.tokens.span().stored_width_at(index + 1 + offset),
                    reader,
                    metrics,
                    tab_origin + preceding_advance,
                );
            }
        }
    }
    Size::new(preceding_advance, preceding_height)
}

fn fit_selected_line(
    engine: &LayoutEngine<'_, '_>,
    context: &LinePreparationContext<'_>,
    prepared_line: Option<&(PreparedLine, f64)>,
    lane_width: f64,
    y: &mut f64,
) -> bool {
    // A whole unbreakable text run can be wider than this float
    // lane even when its first glyph fits. Retry below the float if
    // that run fits the containing block. Shared line measurement
    // excludes hanging preserved spaces from this fit decision.
    if lane_width < context.input.area.width
        && engine
            .read
            .reader
            .style(context.input.container_box_idx)
            .white_space()
            .allows_wrap()
        && let Some((line, width)) = prepared_line
    {
        if *width > lane_width && *width <= context.input.area.width {
            if let Some(next_y) = fitting_y(
                engine,
                context.input,
                *y,
                Size::new(*width, line.height().max(1.0)),
                0.0,
            ) {
                *y = next_y;
                return true;
            }
        }
    }
    false
}

/// Return paragraph-local y only when the exclusion query advances far enough.
/// The first-item probe tolerates subpixel movement; whole-line retries do not.
fn fitting_y(
    engine: &LayoutEngine<'_, '_>,
    input: &InlineFormattingInput,
    y: f64,
    size: Size,
    minimum_advance: f64,
) -> Option<f64> {
    let current_y = input.area.origin.y + y;
    let next_y = engine
        .scratch
        .floats
        .current_float_context()
        .next_y_fitting(
            current_y,
            size.width,
            size.height,
            input.area.origin.x..input.area.origin.x + input.area.width,
        );
    (next_y > current_y + minimum_advance).then_some(next_y - input.area.origin.y)
}

fn place_float_anchor(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    position: Point,
    available_width: f64,
    containing_block_height: Option<f64>,
    preceding_line: Size,
) {
    let style = engine.read.reader.style(box_idx);
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
}
