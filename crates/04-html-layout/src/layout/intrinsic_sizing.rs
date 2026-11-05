use super::box_model::ResolvedBoxModel;
use super::inline::{BreakKind, tokens::indent_for_line};
use crate::flex_grid::TaffyContainerKind;
use crate::layout::LayoutEngine;
use crate::layout_model::{Children, InlineItemKind, LayoutMode};
use html_style_model::{Float, OverflowWrap, UsedPreferredSize as PreferredSize, WhiteSpace, WordBreak};
use html_text::is_other_space_separator;
use std::ops::Range;

/// Shared intrinsic inline-size calculation for every formatting context.
/// Container-specific algorithms contribute through the dispatch in
/// `box_content_intrinsic_widths`; table track solving remains table-owned.
pub fn box_intrinsic_widths(engine: &LayoutEngine<'_, '_>, box_idx: usize) -> (f64, f64) {
    box_intrinsic_widths_with_available(engine, box_idx, None)
}

/// Width transferred from direct percentage-height image children when a
/// container receives a definite block size during shrink-to-fit measurement.
pub fn percentage_height_image_width(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    content_height: f64,
) -> Option<f64> {
    let element = engine
        .reader
        .layout_tree()
        .element_for_box(box_idx, engine.reader.document())?;
    let mut width: Option<f64> = None;
    for child_id in element.children() {
        let child_raw = child_id.raw();
        let Some(child_box_idx) = (0..engine.reader.box_count())
            .find(|&candidate| engine.reader.box_dom_element(candidate) == Some(child_raw))
        else {
            continue;
        };
        let Some(intrinsic) = engine.reader.image_intrinsic(child_box_idx) else {
            continue;
        };
        let child_style = engine.reader.style(child_box_idx);
        let PreferredSize::Percent(percent) = child_style.height() else {
            continue;
        };
        if intrinsic.size.height <= 0.0 {
            continue;
        }
        let used_height = content_height * percent.max(0.0) as f64;
        let content_width = used_height * intrinsic.size.width / intrinsic.size.height;
        let outer_width = content_width
            + child_style.get_horizontal_margin_padding(0.0)
            + child_style.border_left_width() as f64
            + child_style.border_right_width() as f64;
        width = Some(width.map_or(outer_width, |current| current.max(outer_width)));
    }
    width
}

fn box_intrinsic_widths_with_available(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    available_width: Option<f64>,
) -> (f64, f64) {
    // Intrinsic sizing: percentage padding/margin count as zero.
    let style = engine.reader.style(box_idx);
    let mut box_model = ResolvedBoxModel::new(style, 0.0);
    if engine.reader.is_table_box(box_idx)
        && matches!(
            style.border_collapse(),
            html_style_model::BorderCollapseMode::Collapse
        )
    {
        // Match final table layout: padding has zero used value and authored
        // borders are replaced by the collapsed edge grid. Otherwise an
        // auto/shrink-to-fit wrapper measures a wider box than it lays out.
        box_model = box_model
            .without_padding()
            .with_used_borders(crate::layout::UsedBorderInsets::default());
    }
    let margins = if engine.reader.is_table_cell_box(box_idx) {
        0.0
    } else {
        box_model.horizontal_margin()
    };
    let padding_border = box_model.horizontal_padding_border();
    let extras = margins + padding_border;
    let available_content = available_width.map(|available| (available - extras).max(0.0));
    let (content_min, content_max) =
        box_content_intrinsic_widths(engine, box_idx, available_content);
    let outer_min = content_min + extras;
    let outer_max = content_max + extras;
    let intrinsic_value = |size: PreferredSize, auto: f64| match size {
        PreferredSize::Auto | PreferredSize::Content | PreferredSize::Percent(_) | PreferredSize::Stretch => auto,
        PreferredSize::MinContent => outer_min,
        PreferredSize::MaxContent => outer_max,
        PreferredSize::FitContent => available_width.map_or(outer_max, |available| {
            outer_max.min(available.max(outer_min))
        }),
        // Intrinsic sizing has no percentage basis. The shared resolver
        // accepts numeric sizes only when they are independent of that basis.
        PreferredSize::Px(_) | PreferredSize::Calc { .. } | PreferredSize::Comparison { .. } => {
            super::resolve_definite_outer_inline_size(
                size,
                style.box_sizing(),
                padding_border,
                margins,
            )
            .unwrap_or(auto)
        }
    };

    let mut preferred_min = intrinsic_value(style.width(), outer_min);
    let mut preferred_max = intrinsic_value(style.width(), outer_max);
    let minimum = intrinsic_value(style.min_width(), extras);
    let maximum = intrinsic_value(style.max_width(), f64::INFINITY).max(minimum);
    preferred_min = preferred_min.clamp(minimum, maximum);
    preferred_max = preferred_max.clamp(minimum, maximum).max(preferred_min);
    if engine.reader.is_table_box(box_idx) {
        // Table tracks retain their min-content floor even when an authored
        // width or max-width is smaller. Wrappers must measure that used size.
        preferred_min = preferred_min.max(outer_min);
        preferred_max = preferred_max.max(preferred_min);
    }
    (preferred_min, preferred_max)
}

/// Intrinsic content contributions. A definite available inline size supplies
/// the stretch-fit limit needed by `fit-content`; `None` keeps it indefinite.
pub fn box_content_intrinsic_widths(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    available_width: Option<f64>,
) -> (f64, f64) {
    let available_width = available_width.map(|width| width.max(0.0));
    if let Some(intrinsic) = engine.reader.image_intrinsic(box_idx) {
        return (intrinsic.size.width, intrinsic.size.width);
    }
    match engine.reader.box_layout_mode(box_idx) {
        Some(LayoutMode::Block(block)) => {
            children_intrinsic_widths(engine, &block.children, box_idx, available_width)
        }
        Some(LayoutMode::TableCell(cell)) => {
            children_intrinsic_widths(engine, &cell.children, box_idx, available_width)
        }
        Some(LayoutMode::Inline(range)) | Some(LayoutMode::Anonymous(range)) => {
            runs_intrinsic_widths(engine, range.clone(), box_idx)
        }
        Some(LayoutMode::Table(_)) => crate::table::table_intrinsic_widths(engine, box_idx),
        Some(LayoutMode::TableRow(row)) => {
            let mut min_w = 0.0f64;
            let mut max_w = 0.0f64;
            for &cell_idx in &row.cells {
                let (cell_min, cell_max) = box_intrinsic_widths(engine, cell_idx as usize);
                min_w += cell_min;
                max_w += cell_max;
            }
            (min_w, max_w)
        }
        Some(LayoutMode::Flex(container)) => crate::flex_grid::intrinsic_widths(
            engine,
            box_idx,
            &container.children,
            TaffyContainerKind::Flex,
            None,
            None,
        ),
        Some(LayoutMode::Grid(container)) => crate::flex_grid::intrinsic_widths(
            engine,
            box_idx,
            &container.children,
            TaffyContainerKind::Grid,
            available_width,
            None,
        ),
        None => (0.0, 0.0),
    }
}

/// A full-width percentage table has an indefinite max-content
/// contribution until a containing width is chosen. Preserve that fact
/// through auto-table shrink-to-fit sizing without feeding infinities into
/// the concrete column-width solver.
pub fn contains_full_width_percentage_table(engine: &LayoutEngine<'_, '_>, box_idx: usize) -> bool {
    if engine.reader.is_table_box(box_idx)
        && matches!(engine.reader.style(box_idx).width(), PreferredSize::Percent(value) if value >= 1.0)
    {
        return true;
    }

    let contains_box = |&child: &u32| contains_full_width_percentage_table(engine, child as usize);
    let inline_contains = |mut range: Range<u32>| {
        range.any(|run_idx| {
            engine
                .text
                .inline_item(run_idx as usize)
                .is_some_and(|run| match &run.kind {
                    InlineItemKind::AtomicBox { box_idx }
                    | InlineItemKind::FloatAnchor { box_idx } => {
                        contains_full_width_percentage_table(engine, *box_idx as usize)
                    }
                    _ => false,
                })
        })
    };
    let children_contain = |children: &Children| match children {
        Children::Blocks(children) => children.iter().any(contains_box),
        Children::InlineItems(range) => inline_contains(range.clone()),
        Children::Empty => false,
    };

    match engine.reader.box_layout_mode(box_idx) {
        Some(LayoutMode::Block(block)) => children_contain(&block.children),
        Some(LayoutMode::Table(table)) => table
            .rows
            .iter()
            .chain(&table.captions_top)
            .chain(&table.captions_bottom)
            .any(contains_box),
        Some(LayoutMode::TableRow(row)) => row.cells.iter().any(contains_box),
        Some(LayoutMode::TableCell(cell)) => children_contain(&cell.children),
        Some(LayoutMode::Flex(container)) => container.children.iter().any(contains_box),
        Some(LayoutMode::Grid(container)) => container.children.iter().any(contains_box),
        Some(LayoutMode::Inline(range)) | Some(LayoutMode::Anonymous(range)) => {
            inline_contains(range.clone())
        }
        None => false,
    }
}

fn children_intrinsic_widths(
    engine: &LayoutEngine<'_, '_>,
    children: &Children,
    container_box_idx: usize,
    available_width: Option<f64>,
) -> (f64, f64) {
    match children {
        Children::InlineItems(range) => {
            runs_intrinsic_widths(engine, range.clone(), container_box_idx)
        }
        Children::Blocks(indices) => {
            let mut min_w = 0.0f64;
            let mut max_w = 0.0f64;
            let mut float_line_max = 0.0f64;
            for &child_idx in indices {
                // Blockification makes an absolutely positioned inline child
                // a direct block-tree child, but it remains out of flow and
                // contributes nothing to its containing block's intrinsic
                // width. Including it here lets a late image/SVG intrinsic
                // size enlarge an auto-width absolute containing block.
                if engine.reader.style(child_idx as usize).position()
                    == html_style_model::PositionMode::Absolute
                {
                    continue;
                }
                let (child_min, child_max) = box_intrinsic_widths_with_available(
                    engine,
                    child_idx as usize,
                    available_width,
                );
                min_w = min_w.max(child_min);
                if matches!(
                    engine.reader.style(child_idx as usize).float(),
                    Float::Left | Float::Right
                ) {
                    float_line_max += child_max;
                } else {
                    max_w = max_w.max(float_line_max).max(child_max);
                    float_line_max = 0.0;
                }
            }
            max_w = max_w.max(float_line_max);
            (min_w, max_w)
        }
        Children::Empty => (0.0, 0.0),
    }
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
        self.widest.max((self.current - self.trailing_removable).max(0.0))
    }
}

fn runs_intrinsic_widths(
    engine: &LayoutEngine<'_, '_>,
    run_range: Range<u32>,
    container_box_idx: usize,
) -> (f64, f64) {
    let container_style = engine.reader.style(container_box_idx);
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
    let mut pending_collapsible_space = 0.0f64;
    let mut preserved_space_break = false;
    let mut text_cursor = engine.text.cursor();

    for run in &engine.text.inline_items()[run_range.start as usize..run_range.end as usize] {
        if !engine
            .reader
            .inline_item_belongs_to_context(run, container_box_idx)
        {
            continue;
        }
        let style = engine.reader.style(run.box_idx as usize);
        match &run.kind {
            InlineItemKind::Text { glyphs } => {
                for glyph_idx in glyphs.clone() {
                    let white_space = style.white_space();
                    let (unit, cluster_boundary) = super::inline::canonical_text_unit(
                        engine,
                        &mut text_cursor,
                        glyph_idx,
                        run.box_idx as usize,
                        glyphs.end,
                        style,
                        white_space,
                    );
                    let c = unit.character;
                    let break_kind = unit.break_kind;
                    if preserved_space_break && !matches!(c, ' ' | '\t') {
                        minimum.finish(soft_indent);
                        preserved_space_break = false;
                    }
                    if break_kind == BreakKind::Hard {
                        if matches!(c, '\u{000b}' | '\u{000c}' | '\u{0085}') {
                            minimum.add(unit.natural_advance);
                            maximum.add(pending_collapsible_space + unit.natural_advance);
                        }
                        minimum.finish(hard_indent);
                        maximum.finish(hard_indent);
                        pending_collapsible_space = 0.0;
                        preserved_space_break = false;
                        continue;
                    }

                    // Emergency opportunities affect min-content for `anywhere`
                    // (including legacy word-break), but not overflow-wrap's
                    // `break-word`. Keep shaped clusters intact and honor nowrap.
                    if white_space.allows_wrap()
                        && !(white_space == WhiteSpace::PreWrap && matches!(c, ' ' | '\t'))
                        && (style.overflow_wrap() == OverflowWrap::Anywhere
                            || style.word_break() == WordBreak::BreakWord)
                        && cluster_boundary
                        && minimum.started()
                    {
                        minimum.finish(soft_indent);
                    }

                    let advance = if c == '\u{00ad}' {
                        0.0
                    } else if c == '\t' && white_space.preserves_spaces() {
                        let (interval, minimum_advance) = super::inline::preserved_tab_metrics(
                            &engine.reader,
                            run.box_idx as usize,
                        );
                        if interval <= 0.0 {
                            0.0
                        } else {
                            let remainder = maximum.current.rem_euclid(interval);
                            let mut to_stop = if remainder <= f64::EPSILON {
                                interval
                            } else {
                                interval - remainder
                            };
                            if to_stop < minimum_advance {
                                to_stop += interval;
                            }
                            to_stop
                        }
                    } else {
                        unit.natural_advance
                    };
                    if break_kind == BreakKind::Discretionary
                        && cluster_boundary
                    {
                        minimum.finish(soft_indent);
                        if c == '\u{200b}' {
                            continue;
                        }
                    }
                    if is_other_space_separator(c) && white_space.collapses_spaces() {
                        // Other separators hang at an intrinsic line end;
                        // Ogham is trimmed instead. Both leave internal spaces
                        // in the width once following content commits the run.
                        // Collapsed spaces between separators belong to that
                        // same hanging suffix, rather than committing it.
                        minimum.add_removable(advance);
                        maximum.add_removable(pending_collapsible_space + advance);
                        pending_collapsible_space = 0.0;
                        continue;
                    }
                    if white_space.collapses_spaces() && matches!(c, ' ' | '\t' | '\n') {
                        if break_kind == BreakKind::Soft {
                            minimum.finish(soft_indent);
                        } else {
                            // A suppressed wrap (notably nowrap) still retains
                            // internal spaces while excluding a hanging suffix.
                            minimum.add_removable(advance);
                        }
                        if maximum.current > 0.0 { pending_collapsible_space = advance; }
                        continue;
                    }
                    if break_kind == BreakKind::Soft {
                        minimum.finish(soft_indent);
                    } else {
                        minimum.apply_indent();
                        // Pre-wrap spaces and tabs hang at a hypothetical
                        // line end, so they do not enlarge min-content.
                        if white_space != WhiteSpace::PreWrap || !matches!(c, ' ' | '\t') {
                            minimum.add(advance);
                        }
                        if matches!(c, ' ' | '\t') {
                            if white_space == WhiteSpace::BreakSpaces {
                                minimum.finish(soft_indent);
                            } else if white_space == WhiteSpace::PreWrap {
                                preserved_space_break = true;
                            }
                        }
                    }
                    maximum.add(pending_collapsible_space + advance);
                    pending_collapsible_space = 0.0;
                }
            }
            InlineItemKind::Image { image_idx } => {
                let (mut width, _) = engine.reader.image_display_size(*image_idx);
                if width <= 1.0 {
                    // A resource without decoded intrinsic metrics can still
                    // have a definite authored inline size (for example an
                    // HTML `width` attribute). Preserve that contribution in
                    // shrink-to-fit and table intrinsic sizing.
                    width = super::resolve_definite_size_value(
                        engine.reader.style(run.box_idx as usize).width(),
                        None,
                    )
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
                pending_collapsible_space = 0.0;
            }
            InlineItemKind::FloatAnchor { box_idx } => {
                let (float_min, float_max) = box_intrinsic_widths(engine, *box_idx as usize);
                // Consecutive floats may share a line at max-content size,
                // while the min-content contribution only needs room for
                // the widest individual float.
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
            // Markers are positioned outside the containing inline flow.
            InlineItemKind::Marker { .. } => {}
        }
    }

    let min_content = minimum.value();
    (min_content, maximum.value().max(min_content))
}
