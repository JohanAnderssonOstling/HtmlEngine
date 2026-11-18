//! Intrinsic inline widths consume the same prepared text as line layout.
use super::boundaries::BoundaryReader;
use super::tokens::{BreakKind, InlineTokenKind, InlineTokens, indent_for_line};
use crate::LayoutEngine;
use crate::measurement::{IntrinsicFloats, box_intrinsic_widths};
use html_engine_model::InlineItemKind;
use html_style_model::{OverflowWrap, WhiteSpace, WordBreak};
use html_text::is_other_space_separator;

pub(super) fn measure_tokens(
    engine: &mut LayoutEngine<'_, '_>,
    container_box_idx: usize,
    tokens: &InlineTokens<'_>,
) -> (f64, f64) {
    let container_style = engine.read.reader.style(container_box_idx);
    // Percentages are cyclic while determining an intrinsic inline size, so,
    // like percentage padding and margins above, they resolve against zero.
    let text_indent = container_style.text_indent().resolve(0.0);
    let hanging_indent = container_style.text_indent_hanging();
    let each_line_indent = container_style.text_indent_each_line();
    let first_indent = indent_for_line(text_indent, hanging_indent, true);
    let hard_indent = indent_for_line(text_indent, hanging_indent, each_line_indent);
    let soft_indent = indent_for_line(text_indent, hanging_indent, false);
    // Max-content breaks only at forced breaks; min-content also breaks at
    // soft opportunities. Each accumulator consumes its own pending indent.
    let mut minimum = IntrinsicWidth::new(first_indent);
    let mut maximum = IntrinsicWidth::new(first_indent);
    let mut floats = IntrinsicFloats::default();
    let mut pending_collapsible_space = 0.0f64;
    let inputs = engine.read;
    let span = tokens.span();
    let boundaries = BoundaryReader::new(span, tokens.metrics_view());
    for (index, token) in span.iter().enumerate() {
        if let InlineTokenKind::SourceItem { item_idx } = token.kind() {
            if boundaries.before(index).normal {
                minimum.finish(soft_indent);
            }
            let run = &inputs.text.inline_items()[item_idx as usize];
            let style = inputs.reader.style(run.box_idx as usize);
            match &run.kind {
                InlineItemKind::Image { image_idx } => {
                    let (mut width, _) = engine.read.reader.image_display_size(*image_idx);
                    if width <= 1.0 {
                        // A resource without decoded intrinsic metrics can still
                        // have a definite authored inline size (for example an
                        // HTML `width` attribute). Preserve that contribution in
                        // shrink-to-fit and table intrinsic sizing.
                        width = engine
                            .read
                            .reader
                            .style(run.box_idx as usize)
                            .width()
                            .resolve_definite(None)
                            .unwrap_or(1.0)
                            .max(0.0);
                    }
                    maximum.add(pending_collapsible_space + width);
                    pending_collapsible_space = 0.0;
                    minimum.add(width);
                }
                InlineItemKind::AtomicBox { box_idx } => {
                    let (atomic_min, atomic_max) = box_intrinsic_widths(engine, *box_idx as usize);
                    // Atomic inline boxes admit a soft wrap on either side. They
                    // remain adjacent for max-content sizing, but each atomic box
                    // is its own unbreakable segment for min-content sizing.
                    if minimum.started() {
                        minimum.finish(soft_indent);
                    }
                    maximum.add(pending_collapsible_space + atomic_max);
                    pending_collapsible_space = 0.0;
                    minimum.add(atomic_min);
                    minimum.finish(soft_indent);
                }
                InlineItemKind::Break { .. } => {
                    minimum.finish(hard_indent);
                    maximum.finish(hard_indent);
                    floats = IntrinsicFloats::default();
                    pending_collapsible_space = 0.0;
                }
                InlineItemKind::FloatAnchor { box_idx } => {
                    let (float_min, float_max) = box_intrinsic_widths(engine, *box_idx as usize);
                    let float_style = engine.read.reader.style(*box_idx as usize);
                    // Preserve the earlier row's width before removing floats
                    // that this anchor clears. Uncleared floats can share a row.
                    maximum.widest = maximum.value();
                    maximum.current -=
                        floats.add(float_style.float(), float_style.clear(), float_max);
                    minimum.widest = minimum.widest.max(float_min);
                    maximum.current += pending_collapsible_space + float_max;
                    pending_collapsible_space = 0.0;
                }
                InlineItemKind::AbsoluteAnchor { .. } => {}
                InlineItemKind::InlineBoundary {
                    inline_start,
                    inline_end,
                } => {
                    let width = if *inline_start {
                        style.margin_left().resolve(0.0)
                            + style.padding_left().resolve(0.0)
                            + style.border_left_width() as f64
                    } else {
                        0.0
                    } + if *inline_end {
                        style.margin_right().resolve(0.0)
                            + style.padding_right().resolve(0.0)
                            + style.border_right_width() as f64
                    } else {
                        0.0
                    };
                    if width != 0.0 {
                        maximum.add(pending_collapsible_space + width);
                        pending_collapsible_space = 0.0;
                        minimum.add(width);
                    }
                }
                InlineItemKind::Text { .. } | InlineItemKind::Marker { .. } => {
                    unreachable!("text is already prepared")
                }
            }
            continue;
        }
        let metrics = token.run_metrics(&tokens.runs);
        let white_space = metrics.white_space;
        let character = match token.kind() {
            InlineTokenKind::Glyph { glyph_idx } => Some(inputs.text.character(glyph_idx)),
            _ => None,
        };
        if token.break_kind() == BreakKind::Hard {
            minimum.finish(hard_indent);
            maximum.finish(hard_indent);
            floats = IntrinsicFloats::default();
            pending_collapsible_space = 0.0;
            continue;
        }
        // Intrinsic sizing retains its authored-break policy. Dictionary hyphens
        // and their inserted glyph widths belong to line selection, not this sum.
        if matches!(token.kind(), InlineTokenKind::Discretionary { .. }) {
            continue;
        }
        let style = inputs.reader.style(metrics.owner_box_idx as usize);
        let boundary = boundaries.before(index);
        if boundary.normal
            || (minimum.started()
                && boundary.emergency
                && (style.overflow_wrap() == OverflowWrap::Anywhere
                    || style.word_break() == WordBreak::BreakWord))
        {
            minimum.finish(soft_indent);
        }
        if matches!(token.kind(), InlineTokenKind::ZeroWidthBreak) {
            minimum.finish(soft_indent);
            continue;
        }
        let Some(c) = character else {
            continue;
        };
        let advance = token.advance_at(&inputs.reader, &tokens.runs, maximum.current);
        if token.break_kind() == BreakKind::Discretionary && token.is_cluster_boundary() {
            minimum.finish(soft_indent);
        }
        if is_other_space_separator(c) && white_space.collapses_spaces() {
            minimum.add_removable(advance);
            maximum.add_removable(pending_collapsible_space + advance);
            pending_collapsible_space = 0.0;
            continue;
        }
        if white_space.collapses_spaces() && matches!(c, ' ' | '\t' | '\n') {
            if token.break_kind() == BreakKind::Soft {
                minimum.finish(soft_indent);
            } else {
                minimum.add_removable(advance);
            }
            if maximum.current > 0.0 {
                pending_collapsible_space = advance;
            }
            continue;
        }
        if token.break_kind() == BreakKind::Soft {
            minimum.finish(soft_indent);
        } else {
            minimum.apply_indent();
            if white_space != WhiteSpace::PreWrap || !matches!(c, ' ' | '\t') {
                minimum.add(advance);
            }
        }
        maximum.add(pending_collapsible_space + advance);
        pending_collapsible_space = 0.0;
    }
    let min_content = minimum.value();
    (min_content, maximum.value().max(min_content))
}

/// Tracks either min-content segments or max-content lines. The caller owns
/// break policy; this state owns width accumulation and one indent per line.
struct IntrinsicWidth {
    current: f64,
    trailing_removable: f64,
    widest: f64,
    pending_indent: Option<f64>,
}

impl IntrinsicWidth {
    fn new(indent: f64) -> Self {
        Self {
            current: 0.0,
            trailing_removable: 0.0,
            widest: 0.0,
            pending_indent: Some(indent),
        }
    }

    fn apply_indent(&mut self) {
        if let Some(indent) = self.pending_indent.take() {
            self.current += indent;
        }
    }

    fn add(&mut self, width: f64) {
        self.apply_indent();
        if width != 0.0 {
            self.trailing_removable = 0.0;
        }
        self.current += width;
    }

    fn add_removable(&mut self, width: f64) {
        self.apply_indent();
        self.current += width;
        self.trailing_removable += width;
    }

    fn started(&self) -> bool {
        self.current != 0.0 || self.pending_indent.is_none()
    }

    fn finish(&mut self, next_indent: f64) {
        self.widest = self.value();
        self.current = 0.0;
        self.trailing_removable = 0.0;
        self.pending_indent = Some(next_indent);
    }

    fn value(&self) -> f64 {
        self.widest
            .max((self.current - self.trailing_removable).max(0.0))
    }
}
