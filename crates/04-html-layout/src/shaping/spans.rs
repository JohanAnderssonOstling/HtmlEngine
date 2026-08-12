//! Contextual shaping-span planning across inline content.

use super::*;

#[derive(Clone, Debug)]
struct ShapingPaintSpan {
    characters: Range<u32>,
    box_idx: usize,
    style_override: Option<StyleIndices>,
}

/// A contiguous unit sent to the font shaper. Source and paint ownership may
/// change inside a span, while font shaping remains contextual across those
/// boundaries.
#[derive(Clone, Debug)]
pub(super) struct ShapingSpan {
    pub(super) shaping_box: usize,
    pub(super) formatting_root: usize,
    pub(super) style_override: Option<StyleIndices>,
    paint_spans: Vec<ShapingPaintSpan>,
}

impl ShapingSpan {
    pub(super) fn character_count(&self) -> usize {
        self.paint_spans
            .iter()
            .map(|span| span.characters.len())
            .sum()
    }

    pub(super) fn character_indices(&self) -> impl Iterator<Item = u32> + '_ {
        self.paint_spans
            .iter()
            .flat_map(|span| span.characters.clone())
    }

    pub(super) fn contiguous_range(&self) -> Option<Range<u32>> {
        let first = self.paint_spans.first()?.characters.clone();
        let mut range = first;
        for span in &self.paint_spans[1..] {
            if range.end != span.characters.start {
                return None;
            }
            range.end = span.characters.end;
        }
        Some(range)
    }

    pub(super) fn paint_colors(
        &self,
        styles: &ComputedStyles,
        layout_tree: &LayoutTree,
    ) -> Vec<u32> {
        let mut colors = Vec::with_capacity(self.character_count());
        for span in &self.paint_spans {
            let style = span
                .style_override
                .and_then(|indices| styles.view(indices))
                .unwrap_or_else(|| get_style(styles, layout_tree, span.box_idx));
            colors.extend(std::iter::repeat_n(style.color(), span.characters.len()));
        }
        debug_assert_eq!(colors.len(), self.character_count());
        colors
    }

    pub(super) fn placement_required(
        &self,
        styles: &ComputedStyles,
        layout_tree: &LayoutTree,
    ) -> bool {
        self.paint_spans.iter().any(|span| {
            let style = span
                .style_override
                .and_then(|indices| styles.view(indices))
                .unwrap_or_else(|| get_style(styles, layout_tree, span.box_idx));
            style.letter_spacing() != 0.0 || style.word_spacing() != 0.0
        })
    }
}

/// Derive typographic spans from the complete inline item stream. This must
/// observe non-text items: numerically adjacent glyph ranges are not
/// necessarily typographically adjacent when an empty atomic item, image, or
/// forced break occurs between them.
pub(super) fn plan_shaping_spans(
    styles: &ComputedStyles,
    layout_tree: &LayoutTree,
    inline_content: &InlineContent,
    style_overrides: &[Option<StyleIndices>],
    font_metrics: &ShapedFontMetrics,
) -> Vec<ShapingSpan> {
    #[derive(Clone, Copy, Default)]
    struct ContextState {
        previous_span: Option<usize>,
        may_join: bool,
    }

    let mut result = Vec::new();
    let mut contexts = FxHashMap::<usize, ContextState>::default();

    for item in inline_content.inline_items() {
        let range = match &item.kind {
            InlineItemKind::Text { glyphs } if glyphs.start < glyphs.end => glyphs.clone(),
            InlineItemKind::Marker { glyphs } if glyphs.start < glyphs.end => {
                let root = whitespace_context_root(layout_tree, item.box_idx as usize);
                append_shaping_range(
                    styles,
                    layout_tree,
                    font_metrics,
                    &mut result,
                    item.box_idx as usize,
                    glyphs.clone(),
                    None,
                    None,
                );
                contexts.insert(root, ContextState::default());
                continue;
            }
            InlineItemKind::InlineBoundary {
                inline_start,
                inline_end,
            } => {
                if inline_boundary_breaks_shaping(
                    styles,
                    layout_tree,
                    font_metrics,
                    item.box_idx as usize,
                    *inline_start,
                    *inline_end,
                ) {
                    contexts.insert(
                        whitespace_context_root(layout_tree, item.box_idx as usize),
                        ContextState::default(),
                    );
                }
                continue;
            }
            InlineItemKind::FloatAnchor { .. } | InlineItemKind::AbsoluteAnchor { .. } => continue,
            InlineItemKind::Text { .. } | InlineItemKind::Marker { .. } => continue,
            InlineItemKind::Image { .. }
            | InlineItemKind::Break { .. }
            | InlineItemKind::AtomicBox { .. } => {
                let context_box = match item.kind {
                    InlineItemKind::Image { .. } | InlineItemKind::AtomicBox { .. } => layout_tree
                        .get_box_parent(item.box_idx as usize)
                        .unwrap_or(item.box_idx as usize),
                    _ => item.box_idx as usize,
                };
                contexts.insert(
                    whitespace_context_root(layout_tree, context_box),
                    ContextState::default(),
                );
                continue;
            }
        };
        let formatting_root = whitespace_context_root(layout_tree, item.box_idx as usize);

        let mut start = range.start;
        while start < range.end {
            let style_override = style_overrides[start as usize];
            let mut end = start + 1;
            while end < range.end && style_overrides[end as usize] == style_override {
                end += 1;
            }

            let mut part_start = start;
            for index in start..end {
                if inline_content
                    .glyph_at(index as usize)
                    .and_then(char::from_u32)
                    != Some('\n')
                {
                    continue;
                }
                if part_start < index {
                    let state = contexts.get(&formatting_root).copied().unwrap_or_default();
                    let previous = state.may_join.then_some(state.previous_span).flatten();
                    let span = append_shaping_range(
                        styles,
                        layout_tree,
                        font_metrics,
                        &mut result,
                        item.box_idx as usize,
                        part_start..index,
                        style_override,
                        previous,
                    );
                    contexts.insert(
                        formatting_root,
                        ContextState {
                            previous_span: Some(span),
                            may_join: true,
                        },
                    );
                }
                append_shaping_range(
                    styles,
                    layout_tree,
                    font_metrics,
                    &mut result,
                    item.box_idx as usize,
                    index..index + 1,
                    style_override,
                    None,
                );
                contexts.insert(formatting_root, ContextState::default());
                part_start = index + 1;
            }
            if part_start < end {
                let state = contexts.get(&formatting_root).copied().unwrap_or_default();
                let previous = state.may_join.then_some(state.previous_span).flatten();
                let span = append_shaping_range(
                    styles,
                    layout_tree,
                    font_metrics,
                    &mut result,
                    item.box_idx as usize,
                    part_start..end,
                    style_override,
                    previous,
                );
                contexts.insert(
                    formatting_root,
                    ContextState {
                        previous_span: Some(span),
                        may_join: true,
                    },
                );
            }
            start = end;
        }
    }
    result
}

fn inline_boundary_breaks_shaping(
    styles: &ComputedStyles,
    layout_tree: &LayoutTree,
    font_metrics: &ShapedFontMetrics,
    box_idx: usize,
    inline_start: bool,
    inline_end: bool,
) -> bool {
    let indices = layout_tree
        .get_box_style_indices(box_idx)
        .unwrap_or_else(|| styles.default_indices());
    let style = font_metrics
        .used_style(styles, indices, box_idx)
        .expect("validated inline-boundary style");
    if vertical_align_breaks_shaping(style.vertical_align()) {
        return true;
    }
    let start_breaks = inline_start
        && (!style.margin_left().is_zero()
            || !style.padding_left().is_zero()
            || style.border_left_width() != 0.0);
    let end_breaks = inline_end
        && (!style.margin_right().is_zero()
            || !style.padding_right().is_zero()
            || style.border_right_width() != 0.0);
    start_breaks || end_breaks
}

fn vertical_align_breaks_shaping(value: VerticalAlignValue) -> bool {
    match value {
        VerticalAlignValue::Baseline
        | VerticalAlignValue::Length(0.0)
        | VerticalAlignValue::Percent(0.0) => false,
        VerticalAlignValue::Calc {
            absolute_px,
            line_height_fraction,
            x_height_px,
        } => absolute_px != 0.0 || line_height_fraction != 0.0 || x_height_px != 0.0,
        _ => true,
    }
}

fn append_shaping_range(
    styles: &ComputedStyles,
    layout_tree: &LayoutTree,
    font_metrics: &ShapedFontMetrics,
    result: &mut Vec<ShapingSpan>,
    box_idx: usize,
    characters: Range<u32>,
    style_override: Option<StyleIndices>,
    previous_span: Option<usize>,
) -> usize {
    if characters.start >= characters.end {
        return previous_span.unwrap_or(result.len());
    }
    let formatting_root = whitespace_context_root(layout_tree, box_idx);
    let paint_span = ShapingPaintSpan {
        characters: characters.clone(),
        box_idx,
        style_override,
    };
    if let Some(previous_index) = previous_span
        && let Some(previous) = result.get_mut(previous_index)
        && previous.formatting_root == formatting_root
        && shaping_styles_are_equivalent(
            styles,
            layout_tree,
            font_metrics,
            previous.shaping_box,
            previous.style_override,
            box_idx,
            style_override,
        )
    {
        previous.paint_spans.push(paint_span);
        return previous_index;
    }
    let index = result.len();
    result.push(ShapingSpan {
        shaping_box: box_idx,
        formatting_root,
        style_override,
        paint_spans: vec![paint_span],
    });
    index
}

fn shaping_styles_are_equivalent(
    styles: &ComputedStyles,
    layout_tree: &LayoutTree,
    font_metrics: &ShapedFontMetrics,
    left_box: usize,
    left_override: Option<StyleIndices>,
    right_box: usize,
    right_override: Option<StyleIndices>,
) -> bool {
    let left = left_override
        .and_then(|indices| styles.view(indices))
        .unwrap_or_else(|| get_style(styles, layout_tree, left_box));
    let right = right_override
        .and_then(|indices| styles.view(indices))
        .unwrap_or_else(|| get_style(styles, layout_tree, right_box));

    if left_box != right_box
        && (vertical_align_breaks_shaping(left.vertical_align())
            || vertical_align_breaks_shaping(right.vertical_align()))
    {
        return false;
    }

    let left_size = font_metrics.resolved_font_size(left, left_box);
    let right_size = font_metrics.resolved_font_size(right, right_box);

    left_size.to_bits() == right_size.to_bits()
        && left.font_weight() == right.font_weight()
        && left.font_style() == right.font_style()
        && left.font_family() == right.font_family()
        && left.open_type_features() == right.open_type_features()
        && left.language() == right.language()
        && left.direction() == right.direction()
}

pub(super) fn text_runs(inline_content: &InlineContent) -> Vec<(usize, std::ops::Range<u32>)> {
    inline_content
        .inline_items()
        .iter()
        .filter_map(|run| match &run.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs }
                if glyphs.start < glyphs.end =>
            {
                Some((run.box_idx as usize, glyphs.clone()))
            }
            _ => None,
        })
        .collect()
}
