//! Line measurement, token placement, and fragment publication.
//!
//! This module consumes already-selected line ranges and never decides where
//! the paragraph should wrap.

use super::*;
use crate::layout_model::{LayoutMode, LineInlineBoxFragment, LineTextFragment};
use unicode_categories::UnicodeCategories;

pub(super) struct TokenPlacement {
    pub(super) x: f64,
    pub(super) advance: f64,
    pub(super) y_offset: f32,
    pub(super) relative_offset: Vec2,
}

/// Records independently positioned text fragments only for lines where
/// replaced inline content interrupts the source text. Text-only lines keep
/// the compact implicit representation (`None`, whole range at x=0).
pub(super) fn positioned_text_fragments(span: &[InlineToken], placements: &[TokenPlacement], runs: &[InlineTokenMetrics]) -> Option<Box<[LineTextFragment]>> {
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
        discontinuous || matches!(token.kind(), InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. } | InlineTokenKind::InlineBoundary { .. }) || token.run_metrics(runs).placement_required
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
                && !token.run_metrics(runs).placement_required
            {
                fragments[index].glyphs.end += 1;
                continue;
            }
            fragments.push(LineTextFragment { glyphs: glyph_idx..glyph_idx + 1, offset_x: placement.x + placement.relative_offset.x, paint_order: paint_order as u32 });
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
    engine: &crate::layout::LayoutEngine<'_, '_>, container_box_idx: usize, span: &[InlineToken], placements: &[TokenPlacement], runs: &[InlineTokenMetrics], replaced: &[ReplacedToken], line_height: f64, baseline: f64,
    containing_width: f64,
) -> Vec<LineInlineBoxFragment> {
    // A line normally intersects only a handful of inline boxes. A compact
    // linear accumulator beats allocating a box-count-sized side table for
    // every line while still publishing one durable fragment per owner.
    let mut bounds = Vec::<LineInlineBoxFragment>::new();
    for (paint_order, (token, placement)) in span.iter().zip(placements).enumerate() {
        let paint_order = paint_order as u32;
        if placement.advance <= 0.0 && !matches!(token.kind(), InlineTokenKind::InlineBoundary { .. }) {
            continue;
        }
        let metrics = token.run_metrics(runs);
        let token_top = (baseline - metrics.ascent as f64 - placement.y_offset as f64) as f32;
        let token_bottom = (baseline + metrics.descent as f64 - placement.y_offset as f64) as f32;
        let boundary = match token.kind() {
            InlineTokenKind::InlineBoundary { payload_idx } => match replaced.get(payload_idx as usize) {
                Some(ReplacedToken::InlineBoundary { box_idx, margin_left, left_inset, inline_start, inline_end }) => Some((*box_idx as usize, placement.x + margin_left + left_inset, *inline_start, *inline_end)),
                _ => None,
            },
            _ => None,
        };
        let replaced_border_fragment = match token.kind() {
            InlineTokenKind::Image { payload_idx } => match replaced.get(payload_idx as usize) {
                Some(ReplacedToken::Image { box_idx, border_size, margin_left, margin_top, .. }) => {
                    let border_left = placement.x + margin_left;
                    let border_top = baseline - metrics.ascent as f64 - placement.y_offset as f64 + margin_top;
                    Some(LineInlineBoxFragment {
                        box_idx: *box_idx,
                        paint_order,
                        start_x: border_left as f32,
                        end_x: (border_left + border_size.width) as f32,
                        top: border_top as f32,
                        bottom: (border_top + border_size.height) as f32,
                        baseline: baseline as f32,
                        flags: LineInlineBoxFragment::INLINE_START | LineInlineBoxFragment::INLINE_END | LineInlineBoxFragment::BORDER_BOX_BOUNDS,
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
                Some(ReplacedToken::Image { box_idx, .. }) => engine.reader.get_parent(*box_idx as usize),
                _ => None,
            },
            InlineTokenKind::AtomicBox { payload_idx } => match replaced.get(payload_idx as usize) {
                Some(ReplacedToken::AtomicBox { box_idx, .. }) => engine.reader.get_parent(*box_idx as usize),
                _ => None,
            },
            InlineTokenKind::InlineBoundary { .. } => boundary.map(|boundary| boundary.0),
            _ => (token.run_metrics(runs).owner_box_idx != u32::MAX).then_some(token.run_metrics(runs).owner_box_idx as usize),
        };
        let mut outside_formatting_context = false;
        while let Some(box_idx) = owner {
            let is_inline = matches!(engine.reader.box_layout_mode(box_idx), Some(LayoutMode::Inline(_)));
            let has_propagated_decoration = !engine.reader.style(box_idx).text_decoration().lines.is_empty();
            if (!outside_formatting_context && is_inline) || has_propagated_decoration {
                let relative_offset = inline_relative_position_offset(engine, box_idx, containing_width);
                let fragment_baseline = (baseline - inline_fragment_vertical_align_offset(engine, box_idx, line_height, baseline) + relative_offset.y) as f32;
                // CSS 2.1 defines a non-replaced inline's vertical padding,
                // border, and margin edges from its content box. Half-leading
                // participates in line-box height, but is not part of that
                // content box. Use the decorating inline's own font metrics so
                // descendants with a different font do not resize its border.
                let (fragment_top, fragment_bottom) = if is_inline {
                    let style = engine.reader.style(box_idx);
                    let font_size = style.font_size() as f64;
                    let font_metrics = engine.reader.font_metrics(box_idx);
                    let (ascent, descent) = font_metrics.line_box_ratios().map_or((font_size * 0.8, font_size * 0.2), |(ascent, descent)| (font_size * ascent as f64, font_size * descent as f64));
                    let mut top = fragment_baseline - ascent as f32;
                    let mut bottom = fragment_baseline + descent as f32;
                    let trim = style.text_box_trim();
                    let edge = style.text_box_edge();
                    if matches!(trim, TextBoxTrim::Start | TextBoxTrim::Both) {
                        top = match edge.over {
                            TextBoxOverEdge::Text => top,
                            TextBoxOverEdge::Cap => fragment_baseline - font_size as f32 * font_metrics.cap_height_ratio(),
                            TextBoxOverEdge::Ex => fragment_baseline - font_size as f32 * font_metrics.x_height_ratio(),
                        };
                    }
                    if matches!(trim, TextBoxTrim::End | TextBoxTrim::Both) && matches!(edge.under, TextBoxUnderEdge::Alphabetic) {
                        bottom = fragment_baseline;
                    }
                    (top, bottom)
                } else {
                    (token_top, token_bottom)
                };
                let own_boundary = boundary.filter(|boundary| boundary.0 == box_idx);
                let fragment_start = (own_boundary.map_or(placement.x, |boundary| boundary.1) + relative_offset.x) as f32;
                let fragment_end = (own_boundary.map_or(placement.x + placement.advance, |boundary| boundary.1) + relative_offset.x) as f32;
                if let Some(fragment) = bounds.iter_mut().find(|fragment| fragment.box_idx as usize == box_idx) {
                    fragment.paint_order = fragment.paint_order.min(paint_order);
                    fragment.start_x = fragment.start_x.min(fragment_start);
                    fragment.end_x = fragment.end_x.max(fragment_end);
                    fragment.top = fragment.top.min(fragment_top);
                    fragment.bottom = fragment.bottom.max(fragment_bottom);
                    if let Some((_, _, inline_start, inline_end)) = own_boundary {
                        fragment.flags |= if inline_start { LineInlineBoxFragment::INLINE_START } else { 0 } | if inline_end { LineInlineBoxFragment::INLINE_END } else { 0 };
                    }
                } else {
                    let (inline_start, inline_end) = own_boundary.map_or_else(|| engine.reader.inline_fragment_edges(box_idx), |boundary| (boundary.2, boundary.3));
                    bounds.push(LineInlineBoxFragment {
                        box_idx: box_idx as u32,
                        paint_order,
                        start_x: fragment_start,
                        end_x: fragment_end,
                        top: fragment_top,
                        bottom: fragment_bottom,
                        baseline: fragment_baseline,
                        flags: if inline_start { LineInlineBoxFragment::INLINE_START } else { 0 } | if inline_end { LineInlineBoxFragment::INLINE_END } else { 0 },
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

fn inline_relative_position_offset(engine: &crate::layout::LayoutEngine<'_, '_>, box_idx: usize, containing_width: f64) -> Vec2 {
    let mut offset = Vec2::ZERO;
    let mut current = Some(box_idx);
    while let Some(current_idx) = current {
        if !matches!(engine.reader.box_layout_mode(current_idx), Some(LayoutMode::Inline(_))) {
            break;
        }
        let style = engine.reader.style(current_idx);
        if style.position() == html_style_model::PositionMode::Relative {
            offset += crate::layout::box_positioning::relative_position_offset(&style, containing_width, None);
        }
        current = engine.reader.get_parent(current_idx);
    }
    offset
}

/// Returns the alignment of the fragment owner itself and its inline
/// ancestors, excluding shifts authored on descendants. Decoration geometry
/// is tied to the decorating box's baseline, not whichever descendant token
/// happened to create its accumulated fragment first.
fn inline_fragment_vertical_align_offset(engine: &crate::layout::LayoutEngine<'_, '_>, box_idx: usize, line_height: f64, baseline: f64) -> f64 {
    let mut offset = 0.0;
    let mut current = Some(box_idx);
    while let Some(current_idx) = current {
        if !matches!(engine.reader.box_layout_mode(current_idx), Some(LayoutMode::Inline(_))) {
            break;
        }
        let style = engine.reader.style(current_idx);
        let value = style.vertical_align();
        if !value.is_initial() {
            let own_line_height = resolved_line_height(style);
            let font_size = style.font_size() as f64;
            let (ascent, descent) = engine.reader.font_metrics(current_idx).line_box_ratios().map_or((font_size * 0.8, font_size * 0.2), |(ascent, descent)| (font_size * ascent as f64, font_size * descent as f64));
            let half_leading = (own_line_height - ascent - descent) * 0.5;
            let (parent_ascent, parent_descent) = parent_font_content_extents(engine, current_idx);
            offset += vertical_align_metrics_offset(value, ascent, descent, own_line_height, font_size, half_leading, line_height, baseline, parent_ascent, parent_descent);
        }
        current = engine.reader.get_parent(current_idx);
    }
    offset
}

fn has_inline_fragment_owners(engine: &crate::layout::LayoutEngine<'_, '_>, span: &[InlineToken], runs: &[InlineTokenMetrics], replaced: &[ReplacedToken]) -> bool {
    span.iter().any(|token| {
        let mut owner = match token.kind() {
            InlineTokenKind::Image { payload_idx } => match replaced.get(payload_idx as usize) {
                Some(ReplacedToken::Image { box_idx, .. }) => engine.reader.get_parent(*box_idx as usize),
                _ => None,
            },
            InlineTokenKind::AtomicBox { payload_idx } => match replaced.get(payload_idx as usize) {
                Some(ReplacedToken::AtomicBox { box_idx, .. }) => engine.reader.get_parent(*box_idx as usize),
                _ => None,
            },
            InlineTokenKind::InlineBoundary { payload_idx } => match replaced.get(payload_idx as usize) {
                Some(ReplacedToken::InlineBoundary { box_idx, .. }) => Some(*box_idx as usize),
                _ => None,
            },
            _ => (token.run_metrics(runs).owner_box_idx != u32::MAX).then_some(token.run_metrics(runs).owner_box_idx as usize),
        };
        while let Some(box_idx) = owner {
            let is_inline = matches!(engine.reader.box_layout_mode(box_idx), Some(LayoutMode::Inline(_)));
            if is_inline || !engine.reader.style(box_idx).text_decoration().lines.is_empty() {
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
fn stops_text_decoration_propagation(engine: &crate::layout::LayoutEngine<'_, '_>, box_idx: usize) -> bool {
    let style = engine.reader.style(box_idx);
    matches!(style.display(), html_style_model::Display::InlineBlock | html_style_model::Display::InlineTable | html_style_model::Display::InlineFlex | html_style_model::Display::InlineGrid)
        || matches!(style.float(), html_style_model::Float::Left | html_style_model::Float::Right)
        || style.position() == html_style_model::PositionMode::Absolute
}

pub(super) struct LineLayout {
    pub(super) glyph_range: Range<u32>,
    pub(super) width: f64,
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
    /// Plain, baseline-aligned text needs no per-glyph placement storage. Its
    /// glyph range is contiguous and every offset is zero.
    pub(super) placements: Option<Vec<TokenPlacement>>,
}

pub(super) struct TokenSpanSummary {
    pub(super) glyph_range: Range<u32>,
    pub(super) has_text: bool,
    pub(super) has_images: bool,
    pub(super) has_baseline_relative_image: bool,
    pub(super) plain_text: bool,
    pub(super) preserves_newlines: bool,
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
    tokens: &'a InlineTokens,
    input: &'a InlineFormattingInput,
    overflow: InlineOverflow,
    first_line_height: Option<f64>,
    first_letter_height: Option<f64>,
}

impl<'a> LineEmissionContext<'a> {
    pub(super) fn new(tokens: &'a InlineTokens, input: &'a InlineFormattingInput, overflow: InlineOverflow, first_line_height: Option<f64>, first_letter_height: Option<f64>) -> Self {
        Self { tokens, input, overflow, first_line_height, first_letter_height }
    }
}

/// Measures and emits broken lines, returning the final laid-out size.
///
/// This is the write-side pass of the pipeline. It turns `BrokenLine`
/// records into measured line layouts and then writes document fragments.
pub(super) fn emit_lines(
    engine: &mut crate::layout::LayoutEngine<'_, '_>, tokens: &InlineTokens, input: &InlineFormattingInput, overflow: InlineOverflow, first_line_height: Option<f64>, first_letter_height: Option<f64>, lines: &[BrokenLine],
) -> Size {
    let timing_started = Instant::now();
    let context = LineEmissionContext::new(tokens, input, overflow, first_line_height, first_letter_height);
    let mut y = 0.0;
    let mut max_width = 0.0f64;
    for (line_index, line) in lines.iter().enumerate() {
        let origin = Point::new(input.area.origin.x, input.area.origin.y + y);
        let (width, height) = emit_line(engine, &context, line, line_index == 0, origin);
        max_width = max_width.max(width);
        y += height;
    }
    let size = Size::new(max_width, y);
    engine.record_timing(|t| t.emit_lines += timing_started.elapsed());
    size
}

/// Measures and publishes one selected line. Ordinary and float-constrained
/// paragraphs share this write path; only their selected line geometry differs.
pub(super) fn emit_line(engine: &mut crate::layout::LayoutEngine<'_, '_>, context: &LineEmissionContext<'_>, line: &BrokenLine, is_first_line: bool, origin: Point) -> (f64, f64) {
    let tokens = context.tokens;
    let original_span = &tokens[line.start..line.end];
    let mut overflow_adjusted = Vec::new();
    let span = apply_inline_overflow(original_span, &tokens.runs, line.tab_origin, line.width_limit, context.overflow, &mut overflow_adjusted);
    let mut pseudo_adjusted = Vec::new();
    let mut pseudo_runs = Vec::new();
    let (span, line_runs) =
        if is_first_line { apply_first_line_pseudo_styles(engine, span, &tokens.runs, context.first_line_height, context.first_letter_height, &mut pseudo_adjusted, &mut pseudo_runs) } else { (span, tokens.runs.as_ref()) };
    let summary_offset = (span.as_ptr() == original_span.as_ptr() && line_runs.as_ptr() == tokens.runs.as_ptr()).then_some(line.start);
    let inline_struts = inline_box_struts(engine, context.input.container_box_idx, span, line_runs);
    let mut summary = summarize_token_span(span, line_runs, &tokens.summary_blocks, summary_offset, line.tab_origin, &inline_struts);
    include_container_strut(engine, context.input.container_box_idx, is_first_line.then_some(context.first_line_height).flatten(), &mut summary);
    let empty_break_metrics = line.empty_segment_break.and_then(|index| tokens.get(index)).map(|token| *token.run_metrics(&tokens.runs));
    let layout = measure_line(engine, span, line_runs, summary, line, empty_break_metrics, &inline_struts, context.input.container_box_idx, context.input.area.width);
    let fragment_origin = Point::new(origin.x + line.indent, origin.y);
    let paint_color = is_first_line.then(|| first_line_color(engine, context.input.container_box_idx)).flatten();
    let height = write_line_fragments(engine, context.input.container_box_idx, span, line_runs, &tokens.replaced, fragment_origin, &layout, paint_color, context.input.area.width);
    (layout.width + line.indent, height)
}

fn strut_for_inline_box(engine: &crate::layout::LayoutEngine<'_, '_>, box_idx: usize) -> InlineBoxStrut {
    let style = engine.reader.style(box_idx);
    let font_size = style.font_size() as f64;
    let line_height = if style.line_height_is_normal() && engine.reader.box_uses_ahem(box_idx) { font_size } else { resolved_line_height(style) };
    let (font_ascent, font_descent) = engine.reader.font_metrics(box_idx).line_box_ratios().map_or((font_size * 0.8, font_size * 0.2), |(ascent, descent)| (font_size * ascent as f64, font_size * descent as f64));
    let half_leading = (line_height - font_ascent - font_descent) * 0.5;
    InlineBoxStrut { box_idx, ascent: font_ascent + half_leading, descent: font_descent + half_leading, line_height, font_size }
}

fn parent_font_content_extents(engine: &crate::layout::LayoutEngine<'_, '_>, box_idx: usize) -> (f64, f64) {
    let parent_idx = engine.reader.get_parent(box_idx).unwrap_or(box_idx);
    let style = engine.reader.style(parent_idx);
    let font_size = style.font_size() as f64;
    engine.reader.font_metrics(parent_idx).line_box_ratios().map_or((font_size * 0.8, font_size * 0.2), |(ascent, descent)| (font_size * ascent as f64, font_size * descent as f64))
}

fn inline_box_struts(engine: &crate::layout::LayoutEngine<'_, '_>, container_box_idx: usize, tokens: &[InlineToken], runs: &[InlineTokenMetrics]) -> Vec<InlineBoxStrut> {
    let mut struts = Vec::new();
    for token in tokens {
        if matches!(token.kind(), InlineTokenKind::Opportunity | InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. }) {
            continue;
        }
        let owner = token.run_metrics(runs).owner_box_idx;
        if owner == u32::MAX {
            continue;
        }
        // Text owned by the formatting-context root has no inline ancestors
        // inside this context.  Walking from its parent would cross the
        // atomic/block boundary and incorrectly import an outer inline
        // strut into every line of an inline-block.
        if owner as usize == container_box_idx {
            continue;
        }
        // The token metrics are the direct owner's strut (or the replaced
        // element's own geometry). Explicit struts fill only the otherwise
        // missing non-replaced inline ancestors.
        let mut current = engine.reader.get_parent(owner as usize);
        while let Some(box_idx) = current {
            if box_idx == container_box_idx {
                break;
            }
            if !matches!(engine.reader.box_layout_mode(box_idx), Some(LayoutMode::Inline(_))) {
                break;
            }
            if !struts.iter().any(|strut: &InlineBoxStrut| strut.box_idx == box_idx) {
                struts.push(strut_for_inline_box(engine, box_idx));
            }
            current = engine.reader.get_parent(box_idx);
        }
    }
    struts
}

/// Every CSS line box starts with the inline formatting root's zero-width
/// font strut. Descendant text can enlarge it, but a descendant with
/// `font-size: 0` cannot remove the parent's normal line height.
fn include_container_strut(engine: &crate::layout::LayoutEngine<'_, '_>, container_box_idx: usize, first_line_height: Option<f64>, summary: &mut TokenSpanSummary) {
    let style = engine.reader.style(container_box_idx);
    let font_size = style.font_size() as f64;
    let line_height = first_line_height.unwrap_or_else(|| if style.line_height_is_normal() && engine.reader.box_uses_ahem(container_box_idx) { font_size } else { resolved_line_height(style) });
    if !summary.has_baseline_relative_image && summary.line_height + f64::EPSILON >= line_height {
        return;
    }
    let ascent = engine.reader.font_metrics(container_box_idx).ascent_ratio().map_or(font_size * 0.8, |ratio| font_size * ratio as f64);
    let descent = font_size - ascent;
    let half_leading = (line_height - font_size) / 2.0;
    let max_ascent = summary.baseline.max(ascent + half_leading);
    let max_descent = (summary.line_height - summary.baseline).max(descent + half_leading);
    (summary.line_height, summary.baseline) = compute_baseline(max_ascent, max_descent, 0.0);
}

fn first_line_color(engine: &crate::layout::LayoutEngine<'_, '_>, container_box_idx: usize) -> Option<u32> {
    let styles = engine.reader.styles();
    let tree = engine.reader.layout_tree();
    let root = crate::shaping::whitespace_context_root(tree, container_box_idx);
    crate::shaping::first_line_style_for_inline_root(engine.replaced.document(), styles, tree, engine.text.content(), root).and_then(|style| styles.view(style)).map(|style| style.color())
}

/// Returns pseudo-element line heights for the element that establishes
/// this inline formatting context. Anonymous flex/grid items deliberately
/// have no DOM node, so pseudo-elements on their container cannot leak into
/// the anonymous item's text.
pub(super) fn pseudo_line_heights(engine: &crate::layout::LayoutEngine<'_, '_>, container_box_idx: usize) -> (Option<f64>, Option<f64>) {
    let styles = engine.reader.styles();
    let tree = engine.reader.layout_tree();
    let root = crate::shaping::whitespace_context_root(tree, container_box_idx);
    let resolve = |style: html_style_model::StyleView<'_>| {
        if style.line_height_is_normal() { style.font_size() as f64 * 1.2 } else { style.line_height().max(0.0) as f64 }
    };
    let line = crate::shaping::first_line_style_for_inline_root(engine.replaced.document(), styles, tree, engine.text.content(), root).and_then(|style| styles.view(style)).map(resolve);
    let letter = crate::shaping::first_letter_style_for_inline_root(engine.replaced.document(), styles, tree, engine.text.content(), root).and_then(|style| styles.view(style)).map(resolve);
    (line, letter)
}

/// Applies pseudo-element line-height after line breaking. The shaper-aware
/// stage separately refines width-affecting `::first-line` font and spacing
/// properties, while this pass establishes the final line box strut.
pub(super) fn apply_first_line_pseudo_styles<'b>(
    engine: &crate::layout::LayoutEngine<'_, '_>, span: &'b [InlineToken], runs: &'b [InlineTokenMetrics], first_line_height: Option<f64>, first_letter_height: Option<f64>, storage: &'b mut Vec<InlineToken>,
    run_storage: &'b mut Vec<InlineTokenMetrics>,
) -> (&'b [InlineToken], &'b [InlineTokenMetrics]) {
    if first_line_height.is_none() && first_letter_height.is_none() {
        return (span, runs);
    }

    storage.extend_from_slice(span);
    run_storage.extend_from_slice(runs);
    if let Some(height) = first_line_height {
        for token in storage.iter_mut().filter(|token| matches!(token.kind(), InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. })) {
            set_token_line_height(token, run_storage, height);
        }
    }
    if let Some(height) = first_letter_height {
        let mut selected_core = false;
        let mut selection_started = false;
        for token in storage.iter_mut() {
            let character = match token.kind() {
                InlineTokenKind::Glyph { glyph_idx } => {
                    let glyph = engine.text.glyph_at(glyph_idx as usize).unwrap_or_default();
                    engine.text.glyph_metric(glyph).ch()
                }
                InlineTokenKind::Ellipsis { .. } => '\u{2026}',
                InlineTokenKind::InlineBoundary { .. } | InlineTokenKind::Opportunity => continue,
                _ if !selection_started => continue,
                _ => break,
            };
            if !selection_started && character.is_whitespace() {
                continue;
            }
            let qualifying_punctuation = is_first_letter_punctuation(character);
            let selected = if !selection_started {
                selection_started = true;
                if !qualifying_punctuation {
                    selected_core = true;
                }
                true
            } else if !selected_core && qualifying_punctuation {
                true
            } else if !selected_core {
                selected_core = true;
                true
            } else {
                qualifying_punctuation || character.is_mark()
            };
            if !selected {
                break;
            }
            set_token_line_height(token, run_storage, height);
        }
    }
    (storage.as_slice(), run_storage.as_slice())
}

fn is_first_letter_punctuation(character: char) -> bool {
    character.is_punctuation_open() || character.is_punctuation_close() || character.is_punctuation_initial_quote() || character.is_punctuation_final_quote() || character.is_punctuation_other()
}

fn set_token_line_height(token: &mut InlineToken, runs: &mut Vec<InlineTokenMetrics>, height: f64) {
    let mut metrics = *token.run_metrics(runs);
    metrics.line_height = height;
    let run_idx = u32::try_from(runs.len()).expect("inline pseudo run-metrics arena capacity exhausted");
    runs.push(metrics);
    token.run_idx = run_idx;
}

/// Applies the block container's inline overflow policy to one already
/// broken line. The borrowed source span is returned unchanged on the hot
/// path; storage is populated only when a line actually overflows.
pub(super) fn apply_inline_overflow<'b>(span: &'b [InlineToken], runs: &[InlineTokenMetrics], tab_origin: f64, line_width_limit: f64, overflow: InlineOverflow, storage: &'b mut Vec<InlineToken>) -> &'b [InlineToken] {
    if !overflow.clips {
        return span;
    }

    let mut full_width = 0.0;
    for token in span {
        full_width += token.advance_at(runs, tab_origin + full_width);
    }
    if full_width <= line_width_limit {
        return span;
    }

    // `text-overflow: clip` clips paint at the content edge; it does not
    // remove inline-level boxes from layout. Keeping the original span is
    // especially important for an oversized atomic inline, whose subtree
    // must still be laid out before the ancestor overflow clip is applied.
    let Some(marker) = overflow.ellipsis else {
        return span;
    };

    let marker_width = marker.advance_at(runs, tab_origin);
    let show_marker = marker_width <= line_width_limit;
    let prefix_limit = if show_marker { (line_width_limit - marker_width).max(0.0) } else { line_width_limit };
    let mut width = 0.0;
    let mut end = 0usize;
    for (idx, token) in span.iter().enumerate() {
        let advance = token.advance_at(runs, tab_origin + width);
        if width + advance > prefix_limit {
            break;
        }
        width += advance;
        end = idx + 1;
    }

    storage.extend_from_slice(&span[..end]);
    if show_marker {
        storage.push(marker);
    }
    storage.as_slice()
}

fn measure_line(
    engine: &crate::layout::LayoutEngine<'_, '_>, tokens: &[InlineToken], runs: &[InlineTokenMetrics], mut summary: TokenSpanSummary, line: &BrokenLine, empty_break_metrics: Option<InlineTokenMetrics>, inline_struts: &[InlineBoxStrut],
    container_box_idx: usize, containing_width: f64,
) -> LineLayout {
    let timing_started = Instant::now();
    let hyphen = tokens.last().and_then(|token| match token.kind() {
        InlineTokenKind::Discretionary { glyph } => Some((glyph, token.discretionary_width())),
        _ => None,
    });
    if let Some((_, width)) = hyphen {
        summary.width += width;
    }
    if let Some(metrics) = empty_break_metrics {
        let ascent = f64::from(metrics.ascent);
        let descent = f64::from(metrics.descent);
        let half_leading = (metrics.line_height - (ascent + descent)) / 2.0;
        summary.line_height = metrics.line_height;
        summary.baseline = ascent + half_leading;
    }
    let (optical_start, optical_end) = optical_margin_protrusion(engine, &summary, line.align);
    let justification_width = if line.align == TextAlign::Justify { line.width_limit + optical_start + optical_end } else { line.width_limit };
    let (line_height, baseline, spacing) = base_line_metrics(engine, tokens, &summary, justification_width, line.is_last_line, line.align);
    let JustificationSpacing { word_spacing, letter_spacing, word_adjustment, punctuation_space_advances } = spacing;
    let (line_height, baseline) = if summary.plain_text { (line_height, baseline) } else { adjusted_line_metrics(engine, tokens, runs, inline_struts, container_box_idx, line_height, baseline) };
    let x_offset = line_x_offset(line.width_limit, summary.width, line.align);
    let optical_offset_x = match line.align {
        TextAlign::Left | TextAlign::Justify => -optical_start,
        TextAlign::Right => optical_end,
        TextAlign::Center => 0.0,
    };
    let placements = (!summary.plain_text).then(|| build_token_placements(engine, tokens, runs, line.tab_origin, line_height, baseline, containing_width));

    let adjusted_width = summary.width + word_adjustment + letter_spacing * tracking_boundary_count(engine, summary.glyph_range.clone()) as f64;
    let hyphen = hyphen.map(|(glyph, width)| (glyph, (adjusted_width - width).max(0.0)));
    let layout = LineLayout {
        glyph_range: summary.glyph_range,
        width: summary.width,
        x_offset,
        line_height,
        baseline,
        word_spacing,
        letter_spacing,
        punctuation_space_advances,
        optical_offset_x,
        placements,
        hyphen,
        publish_empty: empty_break_metrics.is_some(),
    };
    engine.record_timing(|t| t.measure_line += timing_started.elapsed());
    layout
}

/// Returns bounded start/end punctuation protrusion without changing the
/// breaker's width model. Only plain text in book mode participates.
fn optical_margin_protrusion(engine: &crate::layout::LayoutEngine<'_, '_>, summary: &TokenSpanSummary, align: TextAlign) -> (f64, f64) {
    if !engine.config.book_optimized_text() || !summary.plain_text || summary.glyph_range.is_empty() || align == TextAlign::Center {
        return (0.0, 0.0);
    }
    let visible = |index: u32| {
        let metric = engine.text.glyph_metric(engine.text.glyph_at(index as usize).unwrap_or_default());
        (!metric.ch().is_whitespace() && metric.ch() != '\u{00ad}').then_some((index, metric))
    };
    let first = summary.glyph_range.clone().find_map(visible);
    let last = summary.glyph_range.clone().rev().find_map(visible);
    let protrusion = |entry: Option<(u32, crate::layout_model::GlyphMetric)>, start: bool| {
        let Some((index, metric)) = entry else { return 0.0 };
        let fraction = match (start, metric.ch()) {
            (true, '“' | '‘' | '"' | '\'') | (false, '”' | '’' | '"' | '\'') => 0.65,
            (true, '«' | '‹') | (false, '»' | '›') => 0.5,
            (false, '.' | ',') => 0.5,
            (false, '…') => 0.4,
            (false, ':' | ';' | '!' | '?') => 0.3,
            (false, '-' | '‐' | '‑' | '–') => 0.25,
            (false, '—' | ')' | ']' | '}') => 0.2,
            _ => 0.0,
        };
        let advance = engine.text.text_advance(index as usize, metric.advance()) as f64;
        (advance * fraction).min((metric.ascent() + metric.descent()) as f64 * 0.35).max(0.0)
    };
    (protrusion(first, true), protrusion(last, false))
}

/// Computes the horizontal start offset for non-justified aligned lines.
///
/// Inline placement uses this to support `left`, `center`, and `right`
/// alignment instead of always starting each line at x=0.
fn line_x_offset(line_width_limit: f64, line_width: f64, text_align: TextAlign) -> f64 {
    let remaining = (line_width_limit - line_width).max(0.0);
    match text_align {
        TextAlign::Center => remaining / 2.0,
        TextAlign::Right => remaining,
        _ => 0.0,
    }
}

/// Summarizes the content mix and glyph coverage of a token span.
fn summarize_token_span(tokens: &[InlineToken], runs: &[InlineTokenMetrics], summary_blocks: &[InlineSummaryBlock], summary_offset: Option<usize>, tab_origin: f64, inline_struts: &[InlineBoxStrut]) -> TokenSpanSummary {
    let mut glyph_start = u32::MAX;
    let mut glyph_end = 0;
    let mut expected_glyph = None;
    let mut has_text = false;
    let mut has_images = false;
    let mut has_baseline_relative_image = false;
    let mut plain_text = true;
    let mut preserves_newlines = false;
    let mut width = 0.0;
    let mut max_ascent = f64::NEG_INFINITY;
    let mut max_descent = f64::NEG_INFINITY;
    let mut max_line_height = 0.0f64;

    let mut token_index = 0usize;
    while token_index < tokens.len() {
        if let Some(global_index) = summary_offset.map(|offset| offset + token_index)
            && global_index % INLINE_SUMMARY_BLOCK_TOKENS == 0
            && tokens.len() - token_index >= INLINE_SUMMARY_BLOCK_TOKENS
            && let Some(block) = summary_blocks.get(global_index / INLINE_SUMMARY_BLOCK_TOKENS)
            && block.plain_text
        {
            glyph_start = glyph_start.min(block.glyph_start);
            glyph_end = glyph_end.max(block.glyph_end);
            has_text = true;
            plain_text &= expected_glyph.is_none_or(|expected| expected == block.glyph_start);
            expected_glyph = Some(block.glyph_end);
            preserves_newlines |= block.preserves_newlines;
            width += block.width;
            max_ascent = max_ascent.max(block.max_ascent);
            max_descent = max_descent.max(block.max_descent);
            token_index += INLINE_SUMMARY_BLOCK_TOKENS;
            continue;
        }

        let token = &tokens[token_index];
        let kind = token.kind();
        let metrics = token.run_metrics(runs);
        let ascent = metrics.ascent as f64;
        let descent = metrics.descent as f64;
        let token_line_height = metrics.line_height;
        preserves_newlines |= metrics.white_space.preserves_newlines();

        match kind {
            InlineTokenKind::Glyph { glyph_idx } => {
                glyph_start = glyph_start.min(glyph_idx);
                glyph_end = glyph_end.max(glyph_idx + 1);
                has_text = true;
                plain_text &= expected_glyph.is_none_or(|expected| expected == glyph_idx) && metrics.tab_interval < 0.0 && !metrics.placement_required && matches!(metrics.vertical_align, VerticalAlignValue::Baseline);
                expected_glyph = Some(glyph_idx + 1);
                let half_leading = (token_line_height - (ascent + descent)) / 2.0;
                max_ascent = max_ascent.max(ascent + half_leading);
                max_descent = max_descent.max(descent + half_leading);
            }
            InlineTokenKind::Ellipsis { .. } => {
                has_text = true;
                plain_text = false;
                let half_leading = (token_line_height - (ascent + descent)) / 2.0;
                max_ascent = max_ascent.max(ascent + half_leading);
                max_descent = max_descent.max(descent + half_leading);
            }
            InlineTokenKind::Discretionary { .. } | InlineTokenKind::Opportunity => {}
            InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. } | InlineTokenKind::InlineBoundary { .. } => {
                has_images = true;
                has_baseline_relative_image |= matches!(kind, InlineTokenKind::Image { .. }) && !matches!(metrics.vertical_align, VerticalAlignValue::Top | VerticalAlignValue::Bottom);
                plain_text = false;
                // Line-relative top/bottom replaced boxes constrain the line's
                // total height, but do not participate in choosing its
                // baseline. Including a tall top-aligned image's ascent here
                // adds the font strut's descent a second time.
                let line_relative_replaced = matches!(kind, InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. }) && matches!(metrics.vertical_align, VerticalAlignValue::Top | VerticalAlignValue::Bottom);
                if !line_relative_replaced {
                    max_ascent = max_ascent.max(ascent);
                    max_descent = max_descent.max(descent);
                }
                // An image retains `line-height` for percentage
                // `vertical-align`, but that value does not size the replaced
                // box and must not become a minimum height for the line.
                if !matches!(kind, InlineTokenKind::Image { .. }) {
                    max_line_height = max_line_height.max(token_line_height);
                }
            }
            InlineTokenKind::Break { .. } => {
                plain_text = false;
                max_ascent = max_ascent.max(ascent);
                max_descent = max_descent.max(descent);
                max_line_height = max_line_height.max(token_line_height);
            }
            InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. } => {
                plain_text = false;
            }
        }
        width += if metrics.tab_interval < 0.0 { token.width() } else { token.advance_at(runs, tab_origin + width) };
        token_index += 1;
    }

    if !max_ascent.is_finite() {
        max_ascent = 0.0;
    }
    if !max_descent.is_finite() {
        max_descent = 0.0;
    }
    for strut in inline_struts {
        max_ascent = max_ascent.max(strut.ascent);
        max_descent = max_descent.max(strut.descent);
        max_line_height = max_line_height.max(strut.line_height);
    }

    // Each text metric already contains its computed CSS line-height. The
    // caller-provided fallback is deliberately not applied as a minimum.
    let (line_height, baseline) = compute_baseline(max_ascent, max_descent, max_line_height);
    TokenSpanSummary { glyph_range: if has_text { glyph_start..glyph_end } else { 0..0 }, has_text, has_images, has_baseline_relative_image, plain_text: plain_text && has_text, preserves_newlines, width, line_height, baseline }
}

fn tracking_boundary_count(engine: &crate::layout::LayoutEngine<'_, '_>, glyphs: Range<u32>) -> usize {
    (glyphs.start..glyphs.end.saturating_sub(1)).filter(|&glyph_idx| has_tracking_boundary_after(engine, glyph_idx, glyphs.end)).count()
}

fn has_tracking_boundary_after(engine: &crate::layout::LayoutEngine<'_, '_>, glyph_idx: u32, line_end: u32) -> bool {
    if glyph_idx + 1 >= line_end || !engine.text.is_cluster_boundary(glyph_idx as usize + 1) {
        return false;
    }
    let current = engine.text.glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default()).ch();
    let next = engine.text.glyph_metric(engine.text.glyph_at(glyph_idx as usize + 1).unwrap_or_default()).ch();
    current != '\u{00ad}' && next != '\u{00ad}'
}

#[derive(Default)]
struct JustificationSpacing {
    word_spacing: f64,
    letter_spacing: f64,
    word_adjustment: f64,
    punctuation_space_advances: Vec<(u32, f32)>,
}

/// Computes bounded space adjustment followed by very small tracking at
/// shaped-cluster boundaries. Any remaining positive slack is deliberately
/// left on the right by the emergency underfull-line pass.
fn justification_spacing(engine: &crate::layout::LayoutEngine<'_, '_>, tokens: &[InlineToken], summary: &TokenSpanSummary, available_width: f64, is_last_line: bool, text_align: TextAlign) -> JustificationSpacing {
    let should_justify = text_align == TextAlign::Justify || (engine.config.force_justify() && text_align == TextAlign::Left);
    if is_last_line || summary.preserves_newlines || !should_justify || !summary.plain_text {
        return JustificationSpacing::default();
    }

    // `summary.width` includes authored letter and word spacing, so this
    // slack matches the width that line placement will actually consume.
    let extra_space = available_width - summary.width;
    let punctuation_aware = engine.config.book_optimized_text();
    let spaces = tokens
        .iter()
        .filter_map(|token| {
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else { return None };
            let metric = engine.text.glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default());
            (metric.ch() == ' ' && summary.glyph_range.contains(&glyph_idx)).then(|| {
                // Web-compatible placement retains the historical natural
                // glyph-space basis. Book composition uses the complete token
                // width, matching the optimal line-breaking plan.
                let glue_width = if punctuation_aware { token.width() } else { engine.text.text_advance(glyph_idx as usize, metric.advance()) as f64 };
                let (stretch, shrink) = super::justification::glue_capacities(glue_width, token.space_glue_class(), punctuation_aware);
                (glyph_idx, token.width(), token.space_glue_class(), stretch, shrink)
            })
        })
        .collect::<Vec<_>>();
    let space_count = spaces.len();
    let (expansion_limit, shrink_capacity) = spaces.iter().fold((0.0, 0.0), |(stretch, shrink), &(_, _, _, space_stretch, space_shrink)| (stretch + space_stretch, shrink + space_shrink));
    // Use the same per-space glue capacities as Knuth--Plass. This also
    // protects greedy fallback lines with unbreakable content from producing
    // zero or negative space advances.
    let shrink_limit = -shrink_capacity;
    let word_adjustment = extra_space.clamp(shrink_limit, expansion_limit);
    let word_spacing = if space_count == 0 { 0.0 } else { word_adjustment / space_count as f64 };
    let tracking_count = tracking_boundary_count(engine, summary.glyph_range.clone());
    let tracking_capacity = summary.width.max(0.0) * super::wrapping::MICRO_TRACKING_FRACTION;
    let tracking_adjustment = (extra_space - word_adjustment).clamp(-tracking_capacity, tracking_capacity);
    let per_boundary_limit = summary.line_height.max(0.0) * super::wrapping::MICRO_TRACKING_FRACTION;
    let letter_spacing = if tracking_count > 0 { (tracking_adjustment / tracking_count as f64).clamp(-per_boundary_limit, per_boundary_limit) } else { 0.0 };
    let mut spacing = JustificationSpacing { word_spacing, letter_spacing, word_adjustment, punctuation_space_advances: Vec::new() };

    if punctuation_aware && space_count > 1 && word_adjustment != 0.0 {
        let expanding = word_adjustment > 0.0;
        let capacity = if expanding { expansion_limit } else { shrink_capacity };
        let first_capacity = spaces.first().map(|space| if expanding { space.3 } else { space.4 }).unwrap_or_default();
        let requires_exact_advances = spaces.iter().any(|space| space.2 != super::justification::SpaceGlueClass::Ordinary || ((if expanding { space.3 } else { space.4 }) - first_capacity).abs() > 1.0e-9);
        if requires_exact_advances && capacity > 0.0 {
            let ratio = word_adjustment.abs() / capacity;
            let mut advances = Vec::with_capacity(space_count);
            for &(glyph_idx, width, _, stretch, shrink) in &spaces {
                let glue_adjustment = ratio * if expanding { stretch } else { -shrink };
                let tracking = if has_tracking_boundary_after(engine, glyph_idx, summary.glyph_range.end) { letter_spacing } else { 0.0 };
                let advance = width + glue_adjustment + tracking;
                if !advance.is_finite() || advance < 0.0 {
                    advances.clear();
                    break;
                }
                advances.push((glyph_idx, advance as f32));
            }
            if advances.len() == space_count {
                spacing.word_spacing = 0.0;
                spacing.punctuation_space_advances = advances;
            }
        }
    }
    spacing
}

/// Chooses the initial line metrics before vertical-align adjustments are applied.
fn base_line_metrics(engine: &crate::layout::LayoutEngine<'_, '_>, tokens: &[InlineToken], summary: &TokenSpanSummary, line_width_limit: f64, is_last_line: bool, text_align: TextAlign) -> (f64, f64, JustificationSpacing) {
    let (line_height, baseline) = (summary.line_height, summary.baseline);
    if summary.has_text && !summary.has_images {
        let spacing = justification_spacing(engine, tokens, summary, line_width_limit, is_last_line, text_align);
        (line_height, baseline, spacing)
    } else {
        (line_height, baseline, JustificationSpacing::default())
    }
}

/// Applies a token's own alignment and then the movement of each enclosing
/// inline box. The property value itself is never inherited: an aligned parent
/// moves its complete fragment, including descendants, as a unit.
fn hierarchical_vertical_align_offset(engine: &crate::layout::LayoutEngine<'_, '_>, token: &InlineToken, runs: &[InlineTokenMetrics], line_height: f64, baseline: f64) -> f64 {
    let metrics = token.run_metrics(runs);
    let own_is_group_alignment =
        matches!(token.kind(), InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. } | InlineTokenKind::Discretionary { .. }) && matches!(metrics.vertical_align, VerticalAlignValue::Top | VerticalAlignValue::Bottom);
    let mut offset = if own_is_group_alignment {
        0.0
    } else {
        let ascent = metrics.ascent as f64;
        let descent = metrics.descent as f64;
        let half_leading = match token.kind() {
            InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. } | InlineTokenKind::Discretionary { .. } => (token.line_height(runs) - ascent - descent) / 2.0,
            _ => 0.0,
        };
        let (parent_ascent, parent_descent) = if metrics.owner_box_idx == u32::MAX { (ascent, descent) } else { parent_font_content_extents(engine, metrics.owner_box_idx as usize) };
        vertical_align_metrics_offset(metrics.vertical_align, ascent, descent, token.line_height(runs), metrics.font_size as f64, half_leading, line_height, baseline, parent_ascent, parent_descent)
    };
    if metrics.owner_box_idx == u32::MAX {
        return offset;
    }
    // Glyph metrics describe the conceptual anonymous inline surrounding text,
    // whose vertical-align is baseline. The retained owner is its element
    // parent, so include that owner when accumulating enclosing-box movement.
    let mut current = engine.reader.get_parent(metrics.owner_box_idx as usize);
    while let Some(box_idx) = current {
        if !matches!(engine.reader.box_layout_mode(box_idx), Some(LayoutMode::Inline(_))) {
            break;
        }
        let style = engine.reader.style(box_idx);
        let value = style.vertical_align();
        // `top` and `bottom` align the complete aligned subtree, not this
        // ancestor's font strut independently. Their shared group correction
        // is resolved after the final line box is known.
        if !value.is_initial() && !matches!(value, VerticalAlignValue::Top | VerticalAlignValue::Bottom) {
            let own_line_height = resolved_line_height(style);
            let ascent = own_line_height * 0.8;
            let descent = own_line_height - ascent;
            let (parent_ascent, parent_descent) = parent_font_content_extents(engine, box_idx);
            offset += vertical_align_metrics_offset(value, ascent, descent, own_line_height, style.font_size() as f64, 0.0, line_height, baseline, parent_ascent, parent_descent);
        }
        current = engine.reader.get_parent(box_idx);
    }
    offset
}

#[derive(Clone, Copy)]
struct LineRelativeGroupMetrics {
    box_idx: usize,
    alignment: VerticalAlignValue,
    top: f64,
    bottom: f64,
}

fn line_relative_alignment_for_box(engine: &crate::layout::LayoutEngine<'_, '_>, mut box_idx: usize) -> Option<(usize, VerticalAlignValue)> {
    loop {
        if !matches!(engine.reader.box_layout_mode(box_idx), Some(LayoutMode::Inline(_))) {
            return None;
        }
        let alignment = engine.reader.style(box_idx).vertical_align();
        if matches!(alignment, VerticalAlignValue::Top | VerticalAlignValue::Bottom) {
            return Some((box_idx, alignment));
        }
        box_idx = engine.reader.get_parent(box_idx)?;
    }
}

fn line_relative_alignment_for_token(engine: &crate::layout::LayoutEngine<'_, '_>, token: &InlineToken, runs: &[InlineTokenMetrics]) -> Option<(usize, VerticalAlignValue)> {
    let metrics = token.run_metrics(runs);
    if metrics.owner_box_idx == u32::MAX {
        return None;
    }
    if matches!(token.kind(), InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. }) && matches!(metrics.vertical_align, VerticalAlignValue::Top | VerticalAlignValue::Bottom) {
        return Some((metrics.owner_box_idx as usize, metrics.vertical_align));
    }
    let start = if matches!(token.kind(), InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. } | InlineTokenKind::Discretionary { .. }) {
        metrics.owner_box_idx as usize
    } else {
        engine.reader.get_parent(metrics.owner_box_idx as usize)?
    };
    line_relative_alignment_for_box(engine, start)
}

fn line_relative_groups(engine: &crate::layout::LayoutEngine<'_, '_>, span: &[InlineToken], runs: &[InlineTokenMetrics], inline_struts: &[InlineBoxStrut], line_height: f64, baseline: f64) -> Vec<LineRelativeGroupMetrics> {
    let mut groups = Vec::<LineRelativeGroupMetrics>::new();
    let mut add_bounds = |box_idx, alignment, top: f64, bottom: f64| {
        if let Some(group) = groups.iter_mut().find(|group| group.box_idx == box_idx) {
            group.top = group.top.min(top);
            group.bottom = group.bottom.max(bottom);
        } else {
            groups.push(LineRelativeGroupMetrics { box_idx, alignment, top, bottom });
        }
    };
    for token in span {
        if matches!(token.kind(), InlineTokenKind::Opportunity | InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. }) {
            continue;
        }
        let Some((box_idx, alignment)) = line_relative_alignment_for_token(engine, token, runs) else { continue };
        let metrics = token.run_metrics(runs);
        let ascent = metrics.ascent as f64;
        let descent = metrics.descent as f64;
        let half_leading = if matches!(token.kind(), InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. } | InlineTokenKind::Discretionary { .. }) { (metrics.line_height - ascent - descent) * 0.5 } else { 0.0 };
        let offset = hierarchical_vertical_align_offset(engine, token, runs, line_height, baseline);
        add_bounds(box_idx, alignment, baseline - ascent - half_leading - offset, baseline + descent + half_leading - offset);
    }
    for strut in inline_struts {
        let Some((box_idx, alignment)) = line_relative_alignment_for_box(engine, strut.box_idx) else { continue };
        let offset = inline_strut_vertical_align_offset(engine, *strut, line_height, baseline);
        add_bounds(box_idx, alignment, baseline - strut.ascent - offset, baseline + strut.descent - offset);
    }
    groups
}

/// Recomputes the line box after per-token vertical alignment offsets are applied.
fn adjusted_line_metrics(engine: &crate::layout::LayoutEngine<'_, '_>, span: &[InlineToken], runs: &[InlineTokenMetrics], inline_struts: &[InlineBoxStrut], container_box_idx: usize, line_height: f64, baseline: f64) -> (f64, f64) {
    let line_relative_groups = line_relative_groups(engine, span, runs, inline_struts, line_height, baseline);
    if !line_relative_groups.is_empty() {
        let container_strut = strut_for_inline_box(engine, container_box_idx);
        let mut adjusted_ascent = container_strut.ascent;
        let mut adjusted_descent = container_strut.descent;
        for token in span {
            if matches!(token.kind(), InlineTokenKind::Opportunity | InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. }) || line_relative_alignment_for_token(engine, token, runs).is_some() {
                continue;
            }
            let metrics = token.run_metrics(runs);
            let ascent = metrics.ascent as f64;
            let descent = metrics.descent as f64;
            let offset = hierarchical_vertical_align_offset(engine, token, runs, line_height, baseline);
            let half_leading = match token.kind() {
                InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. } => (metrics.line_height - ascent - descent) * 0.5,
                _ => 0.0,
            };
            adjusted_ascent = adjusted_ascent.max(ascent + half_leading + offset);
            adjusted_descent = adjusted_descent.max(descent + half_leading - offset);
        }
        for strut in inline_struts {
            if line_relative_alignment_for_box(engine, strut.box_idx).is_some() {
                continue;
            }
            let offset = inline_strut_vertical_align_offset(engine, *strut, line_height, baseline);
            adjusted_ascent = adjusted_ascent.max(strut.ascent + offset);
            adjusted_descent = adjusted_descent.max(strut.descent - offset);
        }
        let (mut resolved_height, mut resolved_baseline) = compute_baseline(adjusted_ascent, adjusted_descent, 0.0);
        let tallest_top = line_relative_groups.iter().filter(|group| group.alignment == VerticalAlignValue::Top).map(|group| group.bottom - group.top).fold(0.0, f64::max);
        resolved_height = resolved_height.max(tallest_top);
        let tallest_bottom = line_relative_groups.iter().filter(|group| group.alignment == VerticalAlignValue::Bottom).map(|group| group.bottom - group.top).fold(0.0, f64::max);
        if tallest_bottom > resolved_height {
            resolved_baseline += tallest_bottom - resolved_height;
            resolved_height = tallest_bottom;
        }
        return (resolved_height, resolved_baseline);
    }

    // Baseline images and genuinely aligned inline content need the
    // formatting root's ascent and descent as independent constraints. Plain
    // baseline text already carries that strut in its token metrics.
    let needs_container_strut = span.iter().any(|token| {
        let metrics = token.run_metrics(runs);
        matches!(token.kind(), InlineTokenKind::Image { .. })
            || (!matches!(metrics.vertical_align, VerticalAlignValue::Baseline) && metrics.owner_box_idx != u32::MAX && matches!(engine.reader.box_layout_mode(metrics.owner_box_idx as usize), Some(LayoutMode::Inline(_))))
    });
    let container_strut = strut_for_inline_box(engine, container_box_idx);
    let mut adjusted_ascent = if needs_container_strut { container_strut.ascent } else { f64::NEG_INFINITY };
    let mut adjusted_descent = if needs_container_strut { container_strut.descent } else { f64::NEG_INFINITY };
    for token in span {
        if matches!(token.kind(), InlineTokenKind::Opportunity | InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. }) {
            continue;
        }
        let metrics = token.run_metrics(runs);
        let mut ascent = metrics.ascent as f64;
        let mut descent = metrics.descent as f64;
        // A baseline-aligned atomic inline has its baseline at its bottom
        // margin edge, while the parent line still contains a font strut.
        // Put the strut's descent below that baseline instead of splitting
        // spare line-height above and below the atomic box.
        if matches!(token.kind(), InlineTokenKind::AtomicBox { .. }) && matches!(metrics.vertical_align, VerticalAlignValue::Baseline) && descent <= f64::EPSILON {
            ascent = ascent.max(token.line_height(runs) * 0.8);
            descent = descent.max(token.line_height(runs) * 0.2);
        }
        let offset = hierarchical_vertical_align_offset(engine, token, runs, line_height, baseline);
        let half_leading = match token.kind() {
            InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. } => (token.line_height(runs) - (ascent + descent)) / 2.0,
            _ => 0.0,
        };
        adjusted_ascent = adjusted_ascent.max(ascent + half_leading + offset);
        adjusted_descent = adjusted_descent.max(descent + half_leading - offset);
    }
    for strut in inline_struts {
        let offset = inline_strut_vertical_align_offset(engine, *strut, line_height, baseline);
        adjusted_ascent = adjusted_ascent.max(strut.ascent + offset);
        adjusted_descent = adjusted_descent.max(strut.descent - offset);
    }
    if !adjusted_ascent.is_finite() {
        adjusted_ascent = 0.0;
    }
    if !adjusted_descent.is_finite() {
        adjusted_descent = 0.0;
    }
    let (resolved_height, resolved_baseline) = compute_baseline(adjusted_ascent, adjusted_descent, line_height);
    if resolved_height.abs() < 0.001 { (0.0, resolved_baseline) } else { (resolved_height, resolved_baseline) }
}

fn inline_strut_vertical_align_offset(engine: &crate::layout::LayoutEngine<'_, '_>, strut: InlineBoxStrut, line_height: f64, baseline: f64) -> f64 {
    let mut offset = 0.0;
    let mut current = Some(strut.box_idx);
    let mut own = true;
    while let Some(box_idx) = current {
        if !matches!(engine.reader.box_layout_mode(box_idx), Some(LayoutMode::Inline(_))) {
            break;
        }
        let style = engine.reader.style(box_idx);
        let value = style.vertical_align();
        if !value.is_initial() && !matches!(value, VerticalAlignValue::Top | VerticalAlignValue::Bottom) {
            let metrics = if own { strut } else { strut_for_inline_box(engine, box_idx) };
            let (parent_ascent, parent_descent) = parent_font_content_extents(engine, box_idx);
            offset += vertical_align_metrics_offset(value, metrics.ascent, metrics.descent, metrics.line_height, metrics.font_size, 0.0, line_height, baseline, parent_ascent, parent_descent);
        }
        own = false;
        current = engine.reader.get_parent(box_idx);
    }
    offset
}

/// Builds final per-token placements for a measured line box.
fn build_token_placements(engine: &crate::layout::LayoutEngine<'_, '_>, span: &[InlineToken], runs: &[InlineTokenMetrics], tab_origin: f64, line_height: f64, baseline: f64, containing_width: f64) -> Vec<TokenPlacement> {
    #[derive(Clone, Copy)]
    struct LineRelativeGroup {
        box_idx: usize,
        alignment: VerticalAlignValue,
        top: f64,
        bottom: f64,
    }

    let aligned_ancestor = |token: &InlineToken| {
        let metrics = token.run_metrics(runs);
        if metrics.owner_box_idx == u32::MAX {
            return None;
        }
        // A replaced/atomic token carries its own top/bottom alignment in its
        // token metrics. This grouping is for non-replaced inline ancestors.
        let mut current = if matches!(token.kind(), InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. } | InlineTokenKind::Discretionary { .. }) {
            Some(metrics.owner_box_idx as usize)
        } else {
            engine.reader.get_parent(metrics.owner_box_idx as usize)
        };
        while let Some(box_idx) = current {
            if !matches!(engine.reader.box_layout_mode(box_idx), Some(LayoutMode::Inline(_))) {
                break;
            }
            let alignment = engine.reader.style(box_idx).vertical_align();
            if matches!(alignment, VerticalAlignValue::Top | VerticalAlignValue::Bottom) {
                return Some((box_idx, alignment));
            }
            current = engine.reader.get_parent(box_idx);
        }
        None
    };

    // Measure every top/bottom-aligned subtree with its ordinary descendant
    // offsets first. One shared correction then moves the subtree as a unit.
    let mut groups = Vec::<LineRelativeGroup>::new();
    for token in span {
        let Some((box_idx, alignment)) = aligned_ancestor(token) else { continue };
        if matches!(token.kind(), InlineTokenKind::Opportunity | InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. }) {
            continue;
        }
        let metrics = token.run_metrics(runs);
        let ascent = metrics.ascent as f64;
        let descent = metrics.descent as f64;
        let half_leading = if matches!(token.kind(), InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. }) { (token.line_height(runs) - ascent - descent) * 0.5 } else { 0.0 };
        let offset = hierarchical_vertical_align_offset(engine, token, runs, line_height, baseline);
        let top = baseline - ascent - half_leading - offset;
        let bottom = baseline + descent + half_leading - offset;
        if let Some(group) = groups.iter_mut().find(|group| group.box_idx == box_idx) {
            group.top = group.top.min(top);
            group.bottom = group.bottom.max(bottom);
        } else {
            groups.push(LineRelativeGroup { box_idx, alignment, top, bottom });
        }
    }

    let mut curr_x = 0.0;
    span.iter()
        .map(|token| {
            let advance = token.advance_at(runs, tab_origin + curr_x);
            let owner = token.run_metrics(runs).owner_box_idx;
            let relative_offset = if owner == u32::MAX {
                Vec2::ZERO
            } else {
                let start = match token.kind() {
                    InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. } => engine.reader.get_parent(owner as usize),
                    _ => Some(owner as usize),
                };
                start.map_or(Vec2::ZERO, |box_idx| inline_relative_position_offset(engine, box_idx, containing_width))
            };
            let group_offset = aligned_ancestor(token).and_then(|(box_idx, _)| groups.iter().find(|group| group.box_idx == box_idx)).map_or(0.0, |group| match group.alignment {
                VerticalAlignValue::Top => group.top,
                VerticalAlignValue::Bottom => group.bottom - line_height,
                _ => 0.0,
            });
            let placement = TokenPlacement { x: curr_x, advance, y_offset: (hierarchical_vertical_align_offset(engine, token, runs, line_height, baseline) + group_offset - relative_offset.y) as f32, relative_offset };
            curr_x += advance;
            placement
        })
        .collect()
}

/// Writes fragments for one broken line using a precomputed line layout.
///
/// The write phase only mutates the document; all layout decisions have
/// already been made by `break_lines` and `measure_line`.
pub(super) fn write_line_fragments(
    engine: &mut crate::layout::LayoutEngine<'_, '_>, container_box_idx: usize, span: &[InlineToken], runs: &[InlineTokenMetrics], replaced: &[ReplacedToken], origin: Point, line: &LineLayout, paint_color: Option<u32>,
    containing_width: f64,
) -> f64 {
    let timing_started = Instant::now();
    let point = Point::new(origin.x + line.x_offset, origin.y);
    let line_idx = engine.fragments.state().line_output.lines.len();
    let has_fragment_owners = has_inline_fragment_owners(engine, span, runs, replaced);
    let needs_ordered_fragments = has_fragment_owners || span.iter().any(|token| matches!(token.kind(), InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. } | InlineTokenKind::InlineBoundary { .. }));
    let fallback_placements;
    let placements = if let Some(placements) = line.placements.as_deref() {
        Some(placements)
    } else if needs_ordered_fragments {
        fallback_placements = build_token_placements(engine, span, runs, 0.0, line.line_height, line.baseline, containing_width);
        Some(fallback_placements.as_slice())
    } else {
        None
    };
    let text_fragments = placements.and_then(|placements| positioned_text_fragments(span, placements, runs));
    let inline_box_fragments = if has_fragment_owners {
        let placements = placements.expect("inline fragment owners require token placements");
        positioned_inline_box_fragments(engine, container_box_idx, span, placements, runs, replaced, line.line_height, line.baseline, containing_width)
    } else {
        Vec::new()
    };
    // Absolute/float anchors are executable source-position markers, not
    // line-box content. Keep processing them below, but do not publish the
    // zero-height carrier line that exists solely to trigger that work.
    let publishes_line = line.publish_empty
        || span.iter().any(|token| match token.kind() {
            InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. } | InlineTokenKind::Opportunity => false,
            InlineTokenKind::InlineBoundary { payload_idx } => match replaced.get(payload_idx as usize) {
                Some(ReplacedToken::InlineBoundary { box_idx, inline_start, inline_end, .. }) => {
                    let style = engine.reader.style(*box_idx as usize);
                    token.width != 0.0
                        || (*inline_start && (!style.padding_left().is_zero() || style.border_left_width() != 0.0))
                        || (*inline_end && (!style.padding_right().is_zero() || style.border_right_width() != 0.0))
                        || !style.margin_top().is_zero()
                        || !style.margin_bottom().is_zero()
                        || !style.padding_top().is_zero()
                        || !style.padding_bottom().is_zero()
                        || style.border_top_width() != 0.0
                        || style.border_bottom_width() != 0.0
                }
                _ => false,
            },
            InlineTokenKind::Glyph { glyph_idx } => {
                let glyph = engine.text.glyph_at(glyph_idx as usize).unwrap_or_default();
                let character = engine.text.glyph_metric(glyph).ch();
                token.width != 0.0 || !character.is_whitespace() || token.run_metrics(runs).white_space.preserves_spaces()
            }
            _ => true,
        });
    if publishes_line {
        let inline_box_fragments = engine.fragments.push_inline_box_fragments(inline_box_fragments, engine.reader.box_count());
        let layout = engine.fragments.state_mut();
        layout.line_output.lines.push(Line {
            glyphs: line.glyph_range.clone(),
            point,
            height: line.line_height,
            baseline: line.baseline,
            word_spacing: line.word_spacing,
            letter_spacing: line.letter_spacing,
            optical_offset_x: line.optical_offset_x,
            paint_color,
            text_fragments,
            inline_box_fragments,
        });
        layout.line_output.line_glyph_offsets.push(Vec::new());
        layout.line_output.line_glyph_advances.push(Vec::new());
        layout.line_output.line_glyph_advances[line_idx].extend(line.punctuation_space_advances.iter().map(|&(glyph_idx, advance)| GlyphAdvanceRun { range: glyph_idx..glyph_idx + 1, advance }));
        if let Some((glyph, x)) = line.hyphen {
            layout.line_output.hyphen_fragments.push(HyphenFragment { line_idx, glyph, offset: Point::new(x, 0.0) });
        }
        // Ownership is also the transient stacking-order key. Finalization
        // drops it after sorting when overflow clips are not needed.
        engine.fragments.push_line_owner(container_box_idx as u32);
    }

    let Some(placements) = line.placements.as_deref() else {
        if publishes_line && line.glyph_range.start < line.glyph_range.end {
            engine.fragments.state_mut().line_output.line_glyph_offsets[line_idx].push(GlyphOffsetRun { range: line.glyph_range.clone(), offset: 0.0 });
        }
        engine.record_timing(|t| t.write_line_fragments += timing_started.elapsed());
        return if publishes_line { line.line_height } else { 0.0 };
    };

    for (paint_order, (token, placement)) in span.iter().zip(placements).enumerate() {
        let paint_order = paint_order as u32;
        let offset = placement.y_offset as f64;
        let ascent = token.run_metrics(runs).ascent as f64;
        if let InlineTokenKind::Image { payload_idx } = token.kind() {
            debug_assert!(publishes_line);
            let ReplacedToken::Image { image_idx, box_idx, content_size, border_size, content_inset, margin_left, margin_top, position_offset, set_box_geometry } =
                replaced.get(payload_idx as usize).expect("image token payload index in bounds")
            else {
                unreachable!("image token must reference an image payload")
            };
            let border_offset = Point::new(placement.x + placement.relative_offset.x + *margin_left, line.baseline - ascent - offset + *margin_top) + *position_offset;
            if *set_box_geometry {
                engine.geometry.set_point(*box_idx as usize, point + border_offset.to_vec2());
                engine.geometry.set_size(*box_idx as usize, *border_size);
            }
            engine.fragments.state_mut().fragment_output.image_fragments.push(ImageFragment { line_idx, image_idx: *image_idx, offset: border_offset + content_inset.to_vec2(), size: *content_size, paint_order });
        } else if let InlineTokenKind::AtomicBox { payload_idx } = token.kind() {
            debug_assert!(publishes_line);
            let ReplacedToken::AtomicBox { box_idx, border_size, containing_width, containing_height, margin_left, margin_top } = replaced.get(payload_idx as usize).expect("atomic token payload index in bounds") else {
                unreachable!("atomic token must reference an atomic payload")
            };
            let border_point = Point::new(point.x + placement.x + placement.relative_offset.x + *margin_left, point.y + line.baseline - ascent - offset + *margin_top);
            engine.geometry.set_point(*box_idx as usize, border_point);
            let decoration_start = engine.fragments.decoration_len();
            let height = engine.reader.style(*box_idx as usize).height();
            let authored_height_is_definite = match height {
                html_style_model::UsedPreferredSize::Px(_) => true,
                html_style_model::UsedPreferredSize::Percent(_) | html_style_model::UsedPreferredSize::Stretch => containing_height.is_some(),
                html_style_model::UsedPreferredSize::Calc { .. } | html_style_model::UsedPreferredSize::Comparison { .. } => !height.percentage_dependent() || containing_height.is_some(),
                html_style_model::UsedPreferredSize::Auto
                | html_style_model::UsedPreferredSize::MinContent
                | html_style_model::UsedPreferredSize::MaxContent
                | html_style_model::UsedPreferredSize::FitContent => false,
            };
            let request = crate::layout::BoxLayoutRequest::atomic_assigned(*box_idx as usize, *containing_width, *containing_height, *border_size, authored_height_is_definite);
            let _ = engine.layout_box(request);
            // CSS paints an inline-block (and the non-positioned contents it
            // establishes) atomically at this point in the owning line. Keep
            // its block decorations with that line instead of replaying them
            // in the document-wide block-background phase.
            engine.fragments.record_decoration_line_since(line_idx, decoration_start);
            engine.fragments.record_decoration_paint_order_since(paint_order, decoration_start);
        } else if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
            debug_assert!(publishes_line);
            let layout = engine.fragments.state_mut();
            push_glyph_offset_run(&mut layout.line_output.line_glyph_offsets[line_idx], glyph_idx, offset as f32);
            if token.is_tab(runs) {
                layout.line_output.line_glyph_advances[line_idx].push(GlyphAdvanceRun { range: glyph_idx..glyph_idx + 1, advance: placement.advance as f32 });
            }
        } else if let InlineTokenKind::Ellipsis { glyph } = token.kind() {
            debug_assert!(publishes_line);
            engine.fragments.state_mut().line_output.ellipsis_fragments.push(EllipsisFragment { line_idx, glyph, offset: Point::new(placement.x, offset) });
        } else if let InlineTokenKind::AbsoluteAnchor { box_idx } = token.kind() {
            let static_y = engine.floats.current_float_context().map_or(point.y, |context| context.source_position_at_or_after(point.y));
            engine.defer_absolute_box(box_idx as usize, Point::new(point.x + placement.x, static_y));
        }
    }
    engine.record_timing(|t| t.write_line_fragments += timing_started.elapsed());
    if publishes_line { line.line_height } else { 0.0 }
}

/// Appends or extends a glyph offset run.
///
/// Adjacent glyphs with the same vertical offset are merged so the document
/// stores compact offset ranges instead of one entry per glyph.
fn push_glyph_offset_run(offsets: &mut Vec<GlyphOffsetRun>, glyph_idx: u32, offset: f32) {
    if let Some(last) = offsets.last_mut().filter(|last| last.range.end == glyph_idx && last.offset == offset) {
        last.range.end = glyph_idx + 1;
    } else {
        offsets.push(GlyphOffsetRun { range: glyph_idx..glyph_idx + 1, offset });
    }
}
