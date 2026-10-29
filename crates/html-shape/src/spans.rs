//! Contextual shaping-span planning across inline content.

use super::*;
use html_style_model::StyleView;
use html_text::whitespace_context_root;

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
    pub(super) fn style<'a>(
        &self,
        styles: &'a ComputedStyles,
        layout_tree: &LayoutTree,
    ) -> StyleView<'a> {
        self.style_override
            .and_then(|indices| styles.view(indices))
            .unwrap_or_else(|| get_style(styles, layout_tree, self.shaping_box))
    }

    pub(super) fn split_bidi(self, bidi: &ResolvedBidi) -> Vec<(Self, u8)> {
        profile_scope!("shape.split_bidi");
        let mut result: Vec<(Self, u8)> = Vec::new();
        for paint in self.paint_spans {
            for (characters, level) in bidi.level_runs(paint.characters.clone()) {
                let part = ShapingPaintSpan {
                    characters,
                    ..paint.clone()
                };
                if let Some((span, _)) =
                    result.last_mut().filter(|(_, previous)| *previous == level)
                {
                    span.paint_spans.push(part);
                } else {
                    result.push((
                        Self {
                            shaping_box: self.shaping_box,
                            formatting_root: self.formatting_root,
                            style_override: self.style_override,
                            paint_spans: vec![part],
                        },
                        level,
                    ));
                }
            }
        }
        result
    }

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

    /// Omit per-character colors when the shaping style already describes them.
    pub(super) fn paint_colors(
        &self,
        styles: &ComputedStyles,
        layout_tree: &LayoutTree,
    ) -> Option<Vec<u32>> {
        let default_color = self.style(styles, layout_tree).color();
        let mut colors = None;
        let mut prefix_len = 0;
        for span in &self.paint_spans {
            let style = span
                .style_override
                .and_then(|indices| styles.view(indices))
                .unwrap_or_else(|| get_style(styles, layout_tree, span.box_idx));
            let color = style.color();
            if colors.is_none() && color != default_color {
                let mut values = Vec::with_capacity(self.character_count());
                values.resize(prefix_len, default_color);
                colors = Some(values);
            }
            if let Some(colors) = &mut colors {
                colors.extend(std::iter::repeat_n(color, span.characters.len()));
            }
            prefix_len += span.characters.len();
        }
        debug_assert!(
            colors
                .as_ref()
                .is_none_or(|colors| colors.len() == self.character_count())
        );
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
pub(super) fn plan_shaping_spans<Overrides: StyleOverrides + ?Sized>(
    styles: &ComputedStyles,
    layout_tree: &LayoutTree,
    inline_content: &InlineContent,
    style_overrides: &Overrides,
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
                let root = item.flow_root(layout_tree);
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
                        item.flow_root(layout_tree),
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
                contexts.insert(
                    item.flow_root(layout_tree),
                    ContextState::default(),
                );
                continue;
            }
        };
        let formatting_root = item.flow_root(layout_tree);

        let mut start = range.start;
        while start < range.end {
            let (style_override, end) = style_overrides.segment_at(start, range.end);

            let mut part_start = start;
            for index in start..end {
                if inline_content.character_at(index as usize) != Some('\n') {
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
    let (start, end) = style.inline_edges();
    (inline_start && start) || (inline_end && end)
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
        && left.cjk_font_family() == right.cjk_font_family()
        && left.open_type_features() == right.open_type_features()
        && left.language() == right.language()
        && left.direction() == right.direction()
}
