//! Paragraph inputs and float-aware paragraph orchestration.

use super::*;

pub(in crate::layout) struct InlineFormattingInput {
    pub(in crate::layout) container_box_idx: usize,
    pub(in crate::layout) run_range: Range<u32>,
    pub(in crate::layout) area: InlineLayoutArea,
    pub(in crate::layout) paragraph: ParagraphLayout,
}

#[derive(Clone, Copy)]
pub(in crate::layout) struct InlineLayoutArea {
    pub(in crate::layout) origin: Point,
    pub(in crate::layout) width: f64,
    pub(in crate::layout) percentage_height_basis: Option<f64>,
}

#[derive(Clone, Copy)]
pub(in crate::layout) struct ParagraphLayout {
    pub(in crate::layout) indent: ResolvedTextIndent,
    pub(in crate::layout) text_align: TextAlign,
    pub(in crate::layout) text_align_last: TextAlign,
}

#[derive(Clone, Copy)]
pub(in crate::layout) struct ResolvedTextIndent {
    pub(in crate::layout) amount: f64,
    pub(in crate::layout) hanging: bool,
    pub(in crate::layout) each_line: bool,
}

/// Builds the paragraph token stream and places inline float anchors at their
/// source positions.
///
/// The boolean reports whether line widths must be queried from the active
/// float formatting context. Token preparation itself is shared by both paths.
pub(super) fn prepare_inline_tokens(engine: &mut crate::layout::LayoutEngine<'_, '_>, input: &InlineFormattingInput) -> (InlineTokens, bool) {
    let source_runs = engine.text.inline_items()[input.run_range.start as usize..input.run_range.end as usize].to_vec();
    let is_float_anchor = |run: &InlineItem| {
        let InlineItemKind::FloatAnchor { box_idx } = run.kind else { return false };
        matches!(engine.reader.style(box_idx as usize).float(), Float::Left | Float::Right)
    };
    let has_float_exclusions = source_runs.iter().any(is_float_anchor) || engine.floats.current_float_context().is_some_and(|context| !context.is_empty());

    let max_width = (input.area.width - input.paragraph.indent.amount.max(0.0)).max(0.0);
    // Keep zero-width float anchors in the canonical token sequence after
    // placement. Nested float contents are already excluded by
    // `run_belongs_to_inline_context`; deleting runs here also deleted the
    // source-order boundary between text before and after the float.
    let tokens = build_inline_tokens(engine, input.run_range.clone(), input.container_box_idx, max_width, input.area.percentage_height_basis);
    let mut floated_boxes = Vec::new();
    let mut line_y = input.area.origin.y;
    let mut line_advance = 0.0f64;
    let mut line_height = 0.0f64;
    let mut line_has_soft_break = false;
    let mut advance_after_soft_break = 0.0f64;
    let mut current_nowrap_advance = 0.0f64;
    let default_line_height = resolved_line_height(engine.reader.style(input.container_box_idx));
    for (token_index, token) in tokens.iter().enumerate() {
        let metrics = token.run_metrics(&tokens.runs);
        let token_height = (f64::from(metrics.ascent) + f64::from(metrics.descent)).max(metrics.line_height);
        match token.kind() {
            InlineTokenKind::FloatAnchor { box_idx } => {
                let box_idx = box_idx as usize;
                let style = engine.reader.style(box_idx);
                if matches!(style.float(), Float::Left | Float::Right) && !floated_boxes.contains(&box_idx) {
                    if let Some(cleared_y) = engine.floats.current_float_context().map(|context| context.clear_for(line_y, style.clear())) {
                        line_y = cleared_y;
                    }
                    floated_boxes.push(box_idx);
                    let following_nowrap_advance = if metrics.white_space.allows_wrap() || current_nowrap_advance <= 0.001 {
                        0.0
                    } else {
                        let mut advance = 0.0;
                        for following in &tokens[token_index + 1..] {
                            if matches!(following.kind(), InlineTokenKind::InlineBoundary { .. } | InlineTokenKind::AbsoluteAnchor { .. } | InlineTokenKind::Opportunity) {
                                continue;
                            }
                            if matches!(following.kind(), InlineTokenKind::Break { .. }) || following.run_metrics(&tokens.runs).white_space.allows_wrap() {
                                break;
                            }
                            if !matches!(following.kind(), InlineTokenKind::FloatAnchor { .. }) {
                                advance += following.advance_at(&tokens.runs, line_advance + advance);
                            }
                        }
                        advance
                    };
                    if !metrics.white_space.allows_wrap() && line_has_soft_break {
                        let available = engine
                            .floats
                            .current_float_context()
                            .map(|context| context.available(line_y, line_height.max(default_line_height).max(1.0), input.area.origin.x, input.area.origin.x + input.area.width))
                            .map_or(input.area.width, |(left, right)| (right - left).max(0.0));
                        if line_advance + following_nowrap_advance > available + 0.001 {
                            line_y += line_height.max(default_line_height);
                            line_advance = advance_after_soft_break;
                            line_has_soft_break = false;
                            advance_after_soft_break = 0.0;
                        }
                    }
                    place_float_anchor(engine, box_idx, Point::new(input.area.origin.x, line_y), input.area.width, input.area.percentage_height_basis, line_advance + following_nowrap_advance, line_height.max(default_line_height));
                }
            }
            InlineTokenKind::Break { clear } => {
                line_y += line_height.max(token_height).max(default_line_height);
                if let Some(cleared_y) = engine.floats.current_float_context().map(|context| context.clear_for(line_y, clear)) {
                    line_y = cleared_y;
                }
                line_advance = 0.0;
                line_height = 0.0;
                line_has_soft_break = false;
                advance_after_soft_break = 0.0;
                current_nowrap_advance = 0.0;
            }
            InlineTokenKind::Opportunity => {
                if matches!(token.break_kind(), BreakKind::Soft | BreakKind::Discretionary) {
                    line_has_soft_break = true;
                    advance_after_soft_break = 0.0;
                }
            }
            InlineTokenKind::AbsoluteAnchor { .. } | InlineTokenKind::InlineBoundary { .. } => {}
            _ => {
                let advance = token.advance_at(&tokens.runs, line_advance);
                let available = engine
                    .floats
                    .current_float_context()
                    .map(|context| context.available(line_y, token_height.max(1.0), input.area.origin.x, input.area.origin.x + input.area.width))
                    .map_or(input.area.width, |(left, right)| (right - left).max(0.0));
                if line_advance > 0.0 && line_has_soft_break && line_advance + advance > available + 0.001 {
                    line_y += line_height.max(default_line_height);
                    line_advance = 0.0;
                    line_height = 0.0;
                    line_has_soft_break = false;
                    advance_after_soft_break = 0.0;
                    current_nowrap_advance = 0.0;
                }
                line_advance += advance;
                line_height = line_height.max(token_height);
                if matches!(token.break_kind(), BreakKind::Soft | BreakKind::Discretionary) {
                    line_has_soft_break = true;
                    advance_after_soft_break = 0.0;
                } else if line_has_soft_break {
                    advance_after_soft_break += advance;
                }
                if metrics.white_space.allows_wrap() {
                    current_nowrap_advance = 0.0;
                } else {
                    current_nowrap_advance += advance;
                }
            }
        }
    }
    (tokens, has_float_exclusions)
}

/// Selects and emits lines whose available interval changes around floats.
pub(super) fn layout_inline_content_around_float_exclusions(
    engine: &mut crate::layout::LayoutEngine<'_, '_>, input: &InlineFormattingInput, tokens: &InlineTokens, overflow: InlineOverflow, first_line_height: Option<f64>, first_letter_height: Option<f64>,
) -> Size {
    let timing_started = Instant::now();
    let emission = LineEmissionContext::new(tokens, input, overflow, first_line_height, first_letter_height);
    let mut y = 0.0;
    let mut max_width = 0.0f64;
    let mut start = 0usize;
    let mut is_first_line = true;
    let mut starts_indented_line = true;
    while start < tokens.len() {
        let container_line_height = resolved_line_height(engine.reader.style(input.container_box_idx));
        let probe_line_height = tokens[start..]
            .iter()
            .filter(|token| !matches!(token.kind(), InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. } | InlineTokenKind::Opportunity))
            .map(|token| {
                let metrics = token.run_metrics(&tokens.runs);
                (f64::from(metrics.ascent) + f64::from(metrics.descent)).max(metrics.line_height)
            })
            .next()
            .unwrap_or(container_line_height)
            .max(if is_first_line { first_line_height.unwrap_or(container_line_height) } else { container_line_height })
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
                (f64::from(metrics.ascent) + f64::from(metrics.descent)).max(metrics.line_height).max(1.0)
            };
            if item_width <= input.area.width {
                let current_y = input.area.origin.y + y;
                if let Some(next_y) =
                    engine.floats.current_float_context().map(|context| context.next_y_fitting(current_y, item_width, item_height, input.area.origin.x, input.area.origin.x + input.area.width)).filter(|next_y| *next_y > current_y + 0.001)
                {
                    y = next_y - input.area.origin.y;
                }
            }
        }
        let (left, right) = engine
            .floats
            .current_float_context()
            .map(|context| context.available(input.area.origin.y + y, probe_line_height, input.area.origin.x, input.area.origin.x + input.area.width))
            .unwrap_or((input.area.origin.x, input.area.origin.x + input.area.width));
        let line_indent = indent_for_line(input.paragraph.indent.amount, input.paragraph.indent.hanging, starts_indented_line);
        let constraints = LineConstraints { indent: line_indent, tab_origin: (left - input.area.origin.x) + line_indent, width_limit: (right - left - line_indent).max(0.0) };
        let result = break_next_line(tokens, &tokens.runs, input.paragraph, start, constraints);
        let previous_start = start;
        start = result.next_start;
        starts_indented_line = result.ended_by_forced_break && input.paragraph.indent.each_line;
        let forced_clear = result
            .ended_by_forced_break
            .then(|| {
                tokens[previous_start..start].iter().rev().find_map(|token| match token.kind() {
                    InlineTokenKind::Break { clear } => Some(clear),
                    _ => None,
                })
            })
            .flatten();

        if let Some(line) = result.line {
            let line_origin = Point::new(left, input.area.origin.y + y);
            let (width, height) = emit_line(engine, &emission, &line, is_first_line, line_origin);
            max_width = max_width.max(width);
            y += height;
            is_first_line = false;
        }
        if let Some(clear) = forced_clear {
            let current_y = input.area.origin.y + y;
            if let Some(cleared_y) = engine.floats.current_float_context().map(|context| context.clear_for(current_y, clear)) {
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
    let size = Size::new(max_width, y);
    engine.record_timing(|timing| timing.layout_runs_around_float_exclusions += timing_started.elapsed());
    size
}

fn place_float_anchor(engine: &mut crate::layout::LayoutEngine<'_, '_>, box_idx: usize, position: Point, available_width: f64, containing_block_height: Option<f64>, preceding_line_advance: f64, preceding_line_height: f64) {
    let timing_started = Instant::now();
    let side = engine.reader.style(box_idx).float();
    let style = engine.reader.style(box_idx);
    let margin_left = style.margin_left().resolve(available_width);
    let margin_top = style.margin_top().resolve(available_width);
    let margin_right = style.margin_right().resolve(available_width);
    let margin_bottom = style.margin_bottom().resolve(available_width);
    let initial_point = Point::new(position.x + margin_left, position.y + margin_top);
    engine.geometry.set_point(box_idx, initial_point);
    let layout = engine.layout_box(crate::layout::BoxLayoutRequest::normal(box_idx, available_width, containing_block_height));
    let size = layout.size;
    let margin_width = size.width + margin_left + margin_right;
    let margin_height = size.height + margin_top + margin_bottom;
    let source_y = engine
        .floats
        .current_float_context()
        .map(|context| {
            let candidate_y = context.source_position_at_or_after(position.y);
            let (left, right) = context.available(candidate_y, margin_height.max(1.0), position.x, position.x + available_width);
            if preceding_line_advance > 0.001 && preceding_line_advance + margin_width > right - left + 0.001 { candidate_y + preceding_line_height } else { candidate_y }
        })
        .unwrap_or(position.y);
    let (placement_y, placement_left, placement_right) = engine
        .floats
        .current_float_context()
        .map(|context| {
            let placement = context.place_margin_box(source_y, margin_width, margin_height, position.x, position.x + available_width);
            (placement.y, placement.left, placement.right)
        })
        .unwrap_or((source_y, position.x, position.x + available_width));
    let final_x = if matches!(side, Float::Right) { placement_right - margin_width + margin_left } else { placement_left + margin_left };
    let final_point = Point::new(final_x, placement_y + margin_top);
    crate::layout::translate_laid_out_subtree_output(engine, box_idx, true, &layout.output, final_point - initial_point);
    // Lines are excluded from the float's *margin* box (CSS §9.5), not its
    // border box — drop caps rely on a negative margin-bottom to release
    // the last wrapped line early.
    let band = crate::layout::FloatBand {
        left: final_x - margin_left,
        right: final_x + size.width + margin_right,
        top: placement_y,
        bottom: final_point.y + size.height + margin_bottom,
        side: match side {
            Float::Right => FloatSide::Right,
            _ => FloatSide::Left,
        },
    };
    if let Some(ctx) = engine.floats.current_float_context_mut() {
        ctx.add_band(band);
    }
    engine.record_timing(|t| t.place_float_anchor += timing_started.elapsed());
}
