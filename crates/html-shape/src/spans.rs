//! Contextual shaping-span planning across inline content.

use super::*;
use html_style_model::{StyleView, UnicodeBidi};
use html_text::{joins_cursively, whitespace_context_root};

#[derive(Debug)]
struct ShapingPaintSpan {
    characters: Range<u32>,
    box_idx: usize,
    style_override: Option<StyleIndices>,
}

/// A contiguous unit sent to the font shaper. Source and paint ownership may
/// change inside a span, while font shaping remains contextual across those
/// boundaries.
#[derive(Debug)]
pub(super) struct ShapingSpan {
    pub(super) shaping_box: usize,
    pub(super) formatting_root: usize,
    pub(super) style_override: Option<StyleIndices>,
    paint_spans: Vec<ShapingPaintSpan>,
    pub(super) cursive_joining: CursiveJoining,
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

    fn shaping_style_matches(
        &self,
        styles: &ComputedStyles,
        layout_tree: &LayoutTree,
        font_metrics: &ShapedFontMetrics,
        other: &ShapingPaintSpan,
    ) -> bool {
        let left = self.style(styles, layout_tree);
        let right = other
            .style_override
            .and_then(|indices| styles.view(indices))
            .unwrap_or_else(|| get_style(styles, layout_tree, other.box_idx));

        if self.paint_spans.last().is_some_and(|previous| {
            shaping_boundary_breaks(styles, layout_tree, previous.box_idx, other.box_idx)
        }) {
            return false;
        }

        let left_size = font_metrics.resolved_font_size(left, self.shaping_box);
        let right_size = font_metrics.resolved_font_size(right, other.box_idx);

        left_size.to_bits() == right_size.to_bits()
            && left.font_weight() == right.font_weight()
            && left.font_style() == right.font_style()
            && left.font.font_stretch == right.font.font_stretch
            && left.font.font_oblique_angle == right.font.font_oblique_angle
            && left.font_family() == right.font_family()
            && left.cjk_font_family() == right.cjk_font_family()
            && left.open_type_features() == right.open_type_features()
            && left.font.font_variant_caps_features == right.font.font_variant_caps_features
            && left.font.font_synthesis_small_caps == right.font.font_synthesis_small_caps
            && left.language() == right.language()
            && left.direction() == right.direction()
    }

    pub(super) fn split_bidi(self, bidi: &ResolvedBidi) -> Vec<(Self, u8)> {
        profile_scope!("shape.split_bidi");
        let mut result: Vec<(Self, u8)> = Vec::new();
        for paint in self.paint_spans {
            for (characters, level) in bidi.level_runs(paint.characters) {
                let part = ShapingPaintSpan {
                    characters,
                    ..paint
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
                            cursive_joining: CursiveJoining::Isolated,
                        },
                        level,
                    ));
                }
            }
        }
        if let Some((first, _)) = result.first_mut() {
            first.cursive_joining = shaping::cursive_joining(
                self.cursive_joining.connects_to_previous(),
                first.cursive_joining.connects_to_next(),
            );
        }
        if let Some((last, _)) = result.last_mut() {
            last.cursive_joining = shaping::cursive_joining(
                last.cursive_joining.connects_to_previous(),
                self.cursive_joining.connects_to_next(),
            );
        }
        result
    }

    pub(super) fn character_count(&self) -> usize {
        self.paint_spans
            .iter()
            .map(|span| span.characters.len())
            .sum()
    }

    pub(super) fn character_indices(&self) -> impl DoubleEndedIterator<Item = u32> + Clone + '_ {
        self.paint_spans
            .iter()
            .flat_map(|span| span.characters.clone())
    }

    pub(super) fn contiguous_range(&self) -> Option<Range<u32>> {
        let mut range = self.paint_spans.first()?.characters.clone();
        for span in &self.paint_spans[1..] {
            if range.end != span.characters.start {
                return None;
            }
            range.end = span.characters.end;
        }
        Some(range)
    }

    /// Collect colors and the need for spacing-aware placement in one style scan.
    /// Omit per-character colors when the shaping style already describes them.
    pub(super) fn paint_and_placement(
        &self,
        styles: &ComputedStyles,
        layout_tree: &LayoutTree,
        font_metrics: &ShapedFontMetrics,
    ) -> (Option<Vec<u32>>, bool) {
        let default_color = self.style(styles, layout_tree).color();
        let mut colors = None;
        let mut placement_required = false;
        let mut prefix_len = 0;
        for span in &self.paint_spans {
            let style = span
                .style_override
                .and_then(|indices| styles.view(indices))
                .unwrap_or_else(|| get_style(styles, layout_tree, span.box_idx));
            let font_size = font_metrics.resolved_font_size(style, span.box_idx);
            placement_required |= font_metrics.resolve_text_spacing(styles, style.text.letter_spacing, font_size) != 0.0
                || font_metrics.resolve_text_spacing(styles, style.text.word_spacing, font_size) != 0.0;
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
        (colors, placement_required)
    }

    pub(super) fn has_tracking(&self, styles: &ComputedStyles, layout_tree: &LayoutTree, font_metrics: &ShapedFontMetrics) -> bool {
        self.paint_spans.iter().any(|span| {
            let style = span.style_override.and_then(|indices| styles.view(indices))
                .unwrap_or_else(|| get_style(styles, layout_tree, span.box_idx));
            font_metrics.resolve_text_spacing(styles, style.text.letter_spacing,
                font_metrics.resolved_font_size(style, span.box_idx)) != 0.0
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
    bidi: &ResolvedBidi,
) -> Vec<ShapingSpan> {
    #[derive(Clone, Copy, Default)]
    struct ContextState {
        previous_span: Option<usize>,
        may_join: bool,
    }

    let mut result = Vec::new();
    let mut contexts = FxHashMap::<usize, ContextState>::default();

    for (item_index, item) in inline_content.inline_items().iter().enumerate() {
        let range = match &item.kind {
            InlineItemKind::Text { glyphs } if glyphs.start < glyphs.end => Some(glyphs),
            InlineItemKind::Marker { glyphs } if glyphs.start < glyphs.end => {
                append_shaping_range(
                    styles,
                    layout_tree,
                    font_metrics,
                    inline_content,
                    &mut result,
                    item.box_idx as usize,
                    glyphs.clone(),
                    None,
                    None,
                );
                None
            }
            InlineItemKind::InlineBoundary {
                inline_start,
                inline_end,
            } => {
                let box_idx = item.box_idx as usize;
                let indices = layout_tree
                    .get_box_style_indices(box_idx)
                    .unwrap_or_else(|| styles.default_indices());
                let style = font_metrics
                    .used_style(styles, indices, box_idx)
                    .expect("validated inline-boundary style");
                let breaks_shaping = vertical_align_breaks_shaping(style.vertical_align())
                    || matches!(style.unicode_bidi(), UnicodeBidi::Isolate | UnicodeBidi::IsolateOverride | UnicodeBidi::Plaintext)
                    || {
                    let (left, right) = style.inline_edges();
                    let rtl = (left || right)
                        && boundary_text_is_rtl(inline_content, layout_tree, bidi, item_index, *inline_start)
                            .unwrap_or(style.direction() == html_style_model::TextDirection::Rtl);
                    let (start, end) = if rtl { (right, left) } else { (left, right) };
                    (*inline_start && start) || (*inline_end && end)
                };
                if !breaks_shaping {
                    continue;
                }
                None
            }
            InlineItemKind::FloatAnchor { .. } | InlineItemKind::AbsoluteAnchor { .. } => continue,
            InlineItemKind::Text { .. } | InlineItemKind::Marker { .. } => continue,
            InlineItemKind::Image { .. }
            | InlineItemKind::Break { .. }
            | InlineItemKind::AtomicBox { .. } => None,
        };
        let Some(range) = range else {
            contexts.insert(item.flow_root(layout_tree), ContextState::default());
            continue;
        };
        let formatting_root = item.flow_root(layout_tree);

        let mut start = range.start;
        while start < range.end {
            let (style_override, end) = style_overrides.segment_at(start, range.end);

            let mut part_start = start;
            let characters = &inline_content.characters()[start as usize..end as usize];
            for part in characters.split_inclusive(|&character| character == '\n') {
                let part_end = part_start + part.len() as u32;
                let has_break = part.last() == Some(&'\n');
                let text_end = part_end - u32::from(has_break);
                if part_start < text_end {
                    let state = contexts.get(&formatting_root).copied().unwrap_or_default();
                    let previous = state.may_join.then_some(state.previous_span).flatten();
                    let span = append_shaping_range(
                        styles,
                        layout_tree,
                        font_metrics,
                        inline_content,
                        &mut result,
                        item.box_idx as usize,
                        part_start..text_end,
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
                if has_break {
                    append_shaping_range(
                        styles,
                        layout_tree,
                        font_metrics,
                        inline_content,
                        &mut result,
                        item.box_idx as usize,
                        text_end..part_end,
                        style_override,
                        None,
                    );
                    contexts.insert(formatting_root, ContextState::default());
                }
                part_start = part_end;
            }
            start = end;
        }
    }
    result
}

/// Map a logical source edge to its physical side using the text actually
/// touching that edge. An Arabic run in an LTR paragraph still begins at its
/// physical right edge; a Latin run in an RTL paragraph has the converse case.
fn boundary_text_is_rtl(
    content: &InlineContent,
    tree: &LayoutTree,
    bidi: &ResolvedBidi,
    boundary: usize,
    start: bool,
) -> Option<bool> {
    let owner = content.inline_items()[boundary].box_idx as usize;
    let root = content.inline_items()[boundary].flow_root(tree);
    let mut index = boundary;
    loop {
        index = if start { index.checked_add(1)? } else { index.checked_sub(1)? };
        let item = content.inline_items().get(index)?;
        if item.flow_root(tree) != root { continue; }
        let Some(run) = item.text_run() else { continue; };
        let mut ancestor = run.box_idx as usize;
        while ancestor != owner {
            ancestor = tree.get_box_parent(ancestor)?;
        }
        let mut glyphs = run.glyphs.clone().filter(|&index| {
            !html_text::is_bidi_control(content.character_at(index as usize).unwrap_or_default())
        });
        let glyph = if start { glyphs.next() } else { glyphs.next_back() };
        if let Some(glyph) = glyph { return Some(bidi.level(glyph) & 1 != 0); }
    }
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

/// Undecorated inline ancestors still establish isolation and baseline
/// boundaries even when they have no physical edge token of their own.
fn shaping_boundary_breaks(
    styles: &ComputedStyles,
    tree: &LayoutTree,
    left: usize,
    right: usize,
) -> bool {
    let depth = |mut owner| {
        let mut depth = 0;
        while let Some(parent) = tree.get_box_parent(owner) {
            owner = parent;
            depth += 1;
        }
        depth
    };
    let mut left_depth = depth(left);
    let mut right_depth = depth(right);
    let (mut left, mut right) = (left, right);
    let barrier = |owner| {
        let style = get_style(styles, tree, owner);
        vertical_align_breaks_shaping(style.vertical_align())
            || matches!(
                style.unicode_bidi(),
                UnicodeBidi::Isolate | UnicodeBidi::IsolateOverride | UnicodeBidi::Plaintext
            )
    };
    while left != right {
        if left_depth >= right_depth {
            if barrier(left) {
                return true;
            }
            let Some(parent) = tree.get_box_parent(left) else {
                return true;
            };
            left = parent;
            left_depth -= 1;
        } else {
            if barrier(right) {
                return true;
            }
            let Some(parent) = tree.get_box_parent(right) else {
                return true;
            };
            right = parent;
            right_depth -= 1;
        }
    }
    false
}

fn append_shaping_range(
    styles: &ComputedStyles,
    layout_tree: &LayoutTree,
    font_metrics: &ShapedFontMetrics,
    inline_content: &InlineContent,
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
        characters,
        box_idx,
        style_override,
    };
    if let Some(previous_index) = previous_span
        && let Some(previous) = result.get_mut(previous_index)
        && previous.formatting_root == formatting_root
        && previous.shaping_style_matches(styles, layout_tree, font_metrics, &paint_span)
    {
        previous.paint_spans.push(paint_span);
        return previous_index;
    }
    let mut join_before = false;
    if let Some(previous) = previous_span.and_then(|index| result.get_mut(index))
        && previous.formatting_root == formatting_root
    {
        let left = previous.style(styles, layout_tree);
        let right = style_override.and_then(|indices| styles.view(indices))
            .unwrap_or_else(|| get_style(styles, layout_tree, box_idx));
        // A table cell's vertical-align positions the cell in its row; it
        // does not interrupt cursive joining between its inline descendants.
        // Inline owners and text pseudo styles can shift the text itself.
        let left_inline = previous.style_override.is_some()
            || matches!(layout_tree.get_box_layout_mode(previous.shaping_box), Some(LayoutMode::Inline(_)));
        let right_inline = style_override.is_some()
            || matches!(layout_tree.get_box_layout_mode(box_idx), Some(LayoutMode::Inline(_)));
        join_before = left.direction() == right.direction()
            && !shaping_boundary_breaks(
                styles, layout_tree,
                previous.paint_spans.last().expect("nonempty shaping span").box_idx,
                box_idx,
            )
            && !(left_inline && vertical_align_breaks_shaping(left.vertical_align()))
            && !(right_inline && vertical_align_breaks_shaping(right.vertical_align()))
            && joins_cursively(
                previous.character_indices().map(|index| inline_content.characters()[index as usize]),
                paint_span.characters.clone().map(|index| inline_content.characters()[index as usize]),
            );
        previous.cursive_joining =
            shaping::cursive_joining(previous.cursive_joining.connects_to_previous(), join_before);
    }
    let index = result.len();
    result.push(ShapingSpan {
        shaping_box: box_idx,
        formatting_root,
        style_override,
        paint_spans: vec![paint_span],
        cursive_joining: shaping::cursive_joining(join_before, false),
    });
    index
}
