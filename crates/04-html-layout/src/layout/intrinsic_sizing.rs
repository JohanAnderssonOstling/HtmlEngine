use super::box_model::ResolvedBoxModel;
use crate::flex_grid::TaffyContainerKind;
use crate::layout::LayoutEngine;
use crate::layout_model::{Children, InlineItemKind, LayoutMode};
use html_style_model::{Float, UsedPreferredSize as PreferredSize};
use std::ops::Range;

/// Shared intrinsic inline-size calculation for every formatting context.
/// Container-specific algorithms contribute through the dispatch in
/// `box_content_intrinsic_widths`; table track solving remains table-owned.
pub(crate) fn box_intrinsic_widths(engine: &LayoutEngine<'_, '_>, box_idx: usize) -> (f64, f64) {
    box_intrinsic_widths_with_available(engine, box_idx, None)
}

/// Width transferred from direct percentage-height image children when a
/// container receives a definite block size during shrink-to-fit measurement.
pub(crate) fn percentage_height_image_width(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    content_height: f64,
) -> Option<f64> {
    let node = engine
        .reader
        .box_dom_element(box_idx)
        .and_then(|raw| engine.reader.document().node_id_from_raw(raw))?;
    let element = engine.reader.document().element_ref(node)?;
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
        box_content_intrinsic_widths_impl(engine, box_idx, available_content);
    let outer_min = content_min + extras;
    let outer_max = content_max + extras;
    let intrinsic_value = |size: PreferredSize, auto: f64| match size {
        PreferredSize::Auto | PreferredSize::Percent(_) | PreferredSize::Stretch => auto,
        PreferredSize::MinContent => outer_min,
        PreferredSize::MaxContent => outer_max,
        PreferredSize::FitContent => available_width.map_or(outer_max, |available| {
            outer_max.min(available.max(outer_min))
        }),
        PreferredSize::Px(_) => super::resolve_definite_outer_inline_size(
            size,
            style.box_sizing(),
            padding_border,
            margins,
        )
        .expect("pixel sizes are definite"),
        // During intrinsic sizing the percentage has an indefinite basis;
        // preserve the definite length contribution of a linear calc.
        PreferredSize::Calc {
            percentage_dependent: false,
            ..
        } => super::resolve_definite_outer_inline_size(
            size,
            style.box_sizing(),
            padding_border,
            margins,
        )
        .expect("percentage-independent calc is definite"),
        PreferredSize::Calc {
            percentage_dependent: true,
            ..
        } => auto,
        PreferredSize::Comparison { .. } if size.percentage_dependent() => auto,
        PreferredSize::Comparison { .. } => super::resolve_definite_outer_inline_size(
            size,
            style.box_sizing(),
            padding_border,
            margins,
        )
        .expect("percentage-independent comparison is definite"),
    };

    let mut preferred_min = intrinsic_value(style.width(), outer_min);
    let mut preferred_max = intrinsic_value(style.width(), outer_max);
    let minimum = intrinsic_value(style.min_width(), extras);
    let maximum = intrinsic_value(style.max_width(), f64::INFINITY).max(minimum);
    preferred_min = preferred_min.clamp(minimum, maximum);
    preferred_max = preferred_max.clamp(minimum, maximum).max(preferred_min);
    (preferred_min, preferred_max)
}

pub(crate) fn box_content_intrinsic_widths(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
) -> (f64, f64) {
    box_content_intrinsic_widths_impl(engine, box_idx, None)
}

/// Intrinsic content contributions when the formatting context already knows
/// the available inline size. This is the context required by `fit-content`:
/// unlike min/max-content, its contribution is not meaningful without the
/// stretch-fit limit supplied by the containing block.
pub(crate) fn box_content_intrinsic_widths_with_available(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    available_width: f64,
) -> (f64, f64) {
    box_content_intrinsic_widths_impl(engine, box_idx, Some(available_width.max(0.0)))
}

fn box_content_intrinsic_widths_impl(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    available_width: Option<f64>,
) -> (f64, f64) {
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
        ),
        Some(LayoutMode::Grid(container)) => crate::flex_grid::intrinsic_widths_with_available(
            engine,
            box_idx,
            &container.children,
            TaffyContainerKind::Grid,
            available_width,
        ),
        None => (0.0, 0.0),
    }
}

/// A full-width percentage table has an indefinite max-content
/// contribution until a containing width is chosen. Preserve that fact
/// through auto-table shrink-to-fit sizing without feeding infinities into
/// the concrete column-width solver.
pub(crate) fn contains_full_width_percentage_table(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
) -> bool {
    if matches!(
        engine.reader.box_layout_mode(box_idx),
        Some(LayoutMode::Table(_))
    ) && matches!(engine.reader.style(box_idx).width(), PreferredSize::Percent(value) if value >= 1.0)
    {
        return true;
    }

    let children_contain = |children: &Children| match children {
        Children::Blocks(children) => children
            .iter()
            .any(|&child| contains_full_width_percentage_table(engine, child as usize)),
        Children::InlineItems(range) => range.clone().any(|run_idx| {
            engine
                .text
                .inline_item(run_idx as usize)
                .is_some_and(|run| match &run.kind {
                    InlineItemKind::AtomicBox { box_idx }
                    | InlineItemKind::FloatAnchor { box_idx } => {
                        contains_full_width_percentage_table(engine, *box_idx as usize)
                    }
                    InlineItemKind::AbsoluteAnchor { .. } => false,
                    _ => false,
                })
        }),
        Children::Empty => false,
    };

    match engine.reader.box_layout_mode(box_idx) {
        Some(LayoutMode::Block(block)) => children_contain(&block.children),
        Some(LayoutMode::Table(table)) => table
            .rows
            .iter()
            .chain(&table.captions_top)
            .chain(&table.captions_bottom)
            .any(|&child| contains_full_width_percentage_table(engine, child as usize)),
        Some(LayoutMode::TableRow(row)) => row
            .cells
            .iter()
            .any(|&child| contains_full_width_percentage_table(engine, child as usize)),
        Some(LayoutMode::TableCell(cell)) => children_contain(&cell.children),
        Some(LayoutMode::Flex(container)) => container
            .children
            .iter()
            .any(|&child| contains_full_width_percentage_table(engine, child as usize)),
        Some(LayoutMode::Grid(container)) => container
            .children
            .iter()
            .any(|&child| contains_full_width_percentage_table(engine, child as usize)),
        Some(LayoutMode::Inline(range)) | Some(LayoutMode::Anonymous(range)) => {
            range.clone().any(|run_idx| {
                engine
                    .text
                    .inline_item(run_idx as usize)
                    .is_some_and(|run| match &run.kind {
                        InlineItemKind::AtomicBox { box_idx }
                        | InlineItemKind::FloatAnchor { box_idx } => {
                            contains_full_width_percentage_table(engine, *box_idx as usize)
                        }
                        InlineItemKind::AbsoluteAnchor { .. } => false,
                        _ => false,
                    })
            })
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
    let mut min_content = 0.0f64;
    let mut max_content = 0.0f64;
    let mut current_line = 0.0f64;
    let mut segment = 0.0f64;
    let mut pending_collapsible_space = 0.0f64;
    // Max-content only starts a new line at a forced break. Min-content also
    // starts a hypothetical line at every soft wrap opportunity. Track the
    // two independently so text-indent is charged to exactly the lines to
    // which it would apply during layout.
    let mut max_starts_indented_line = true;
    let mut min_starts_indented_line = true;
    let mut max_indent_applied = false;
    let mut min_indent_applied = false;

    let apply_max_indent =
        |current_line: &mut f64, applied: &mut bool, starts_indented_line: bool| {
            if !*applied {
                *current_line += super::inline::tokens::indent_for_line(
                    text_indent,
                    hanging_indent,
                    starts_indented_line,
                );
                *applied = true;
            }
        };
    let apply_min_indent = |segment: &mut f64, applied: &mut bool, starts_indented_line: bool| {
        if !*applied {
            *segment += super::inline::tokens::indent_for_line(
                text_indent,
                hanging_indent,
                starts_indented_line,
            );
            *applied = true;
        }
    };

    for run in &engine.text.inline_items()[run_range.start as usize..run_range.end as usize] {
        let ownership_box = super::inline::tokens::inline_item_ownership_box(engine, run);
        if !ownership_box.is_some_and(|box_idx| {
            super::inline::run_belongs_to_inline_context(engine, box_idx, container_box_idx)
        }) {
            continue;
        }
        let style = engine.reader.style(run.box_idx as usize);
        let tab_reference_style = super::inline::tab_reference_style(engine, run.box_idx as usize);
        match &run.kind {
            InlineItemKind::Text { glyphs } => {
                for glyph_idx in glyphs.clone() {
                    let white_space = style.white_space();
                    let unit =
                        super::inline::canonical_text_unit(engine, glyph_idx, style, white_space);
                    let c = unit.character;
                    let break_kind = unit.break_kind;
                    if break_kind == super::inline::BreakKind::Hard {
                        min_content = min_content.max(segment.max(0.0));
                        max_content = max_content.max(current_line.max(0.0));
                        segment = 0.0;
                        current_line = 0.0;
                        pending_collapsible_space = 0.0;
                        max_starts_indented_line = each_line_indent;
                        min_starts_indented_line = each_line_indent;
                        max_indent_applied = false;
                        min_indent_applied = false;
                        continue;
                    }

                    let advance = if c == '\u{00ad}' {
                        0.0
                    } else if c == '\t' && white_space.preserves_spaces() {
                        let (interval, minimum_advance) = super::inline::preserved_tab_metrics(
                            style,
                            tab_reference_style,
                            engine.reader.box_uses_ahem(run.box_idx as usize),
                        );
                        if interval <= 0.0 {
                            0.0
                        } else {
                            let remainder = current_line.rem_euclid(interval);
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
                    if break_kind == super::inline::BreakKind::Discretionary {
                        min_content = min_content.max(segment.max(0.0));
                        segment = 0.0;
                        min_starts_indented_line = false;
                        min_indent_applied = false;
                        continue;
                    }
                    if break_kind == super::inline::BreakKind::Soft {
                        min_content = min_content.max(segment.max(0.0));
                        segment = 0.0;
                        min_starts_indented_line = false;
                        min_indent_applied = false;
                        if white_space.collapses_spaces() && !white_space.preserves_spaces() {
                            if current_line > 0.0 {
                                pending_collapsible_space = advance;
                            }
                            continue;
                        }
                    } else {
                        apply_min_indent(
                            &mut segment,
                            &mut min_indent_applied,
                            min_starts_indented_line,
                        );
                        segment += advance;
                    }
                    apply_max_indent(
                        &mut current_line,
                        &mut max_indent_applied,
                        max_starts_indented_line,
                    );
                    current_line += pending_collapsible_space + advance;
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
                apply_max_indent(
                    &mut current_line,
                    &mut max_indent_applied,
                    max_starts_indented_line,
                );
                apply_min_indent(
                    &mut segment,
                    &mut min_indent_applied,
                    min_starts_indented_line,
                );
                current_line += pending_collapsible_space + width;
                pending_collapsible_space = 0.0;
                segment += width;
            }
            InlineItemKind::AtomicBox { box_idx } => {
                let (atomic_min, atomic_max) = box_intrinsic_widths(engine, *box_idx as usize);
                apply_max_indent(
                    &mut current_line,
                    &mut max_indent_applied,
                    max_starts_indented_line,
                );
                // Atomic inline boxes admit a soft wrap on either side. They
                // remain adjacent for max-content sizing, but each atomic box
                // is its own unbreakable segment for min-content sizing.
                if segment != 0.0 || min_indent_applied {
                    min_content = min_content.max(segment.max(0.0));
                    segment = 0.0;
                    min_starts_indented_line = false;
                    min_indent_applied = false;
                }
                apply_min_indent(
                    &mut segment,
                    &mut min_indent_applied,
                    min_starts_indented_line,
                );
                current_line += pending_collapsible_space + atomic_max;
                pending_collapsible_space = 0.0;
                segment += atomic_min;
                min_content = min_content.max(segment.max(0.0));
                segment = 0.0;
                min_starts_indented_line = false;
                min_indent_applied = false;
            }
            InlineItemKind::Break { .. } => {
                min_content = min_content.max(segment.max(0.0));
                max_content = max_content.max(current_line.max(0.0));
                segment = 0.0;
                current_line = 0.0;
                pending_collapsible_space = 0.0;
                max_starts_indented_line = each_line_indent;
                min_starts_indented_line = each_line_indent;
                max_indent_applied = false;
                min_indent_applied = false;
            }
            InlineItemKind::FloatAnchor { box_idx } => {
                let (float_min, float_max) = box_intrinsic_widths(engine, *box_idx as usize);
                // Consecutive floats may share a line at max-content size,
                // while the min-content contribution only needs room for
                // the widest individual float.
                min_content = min_content.max(float_min);
                current_line += pending_collapsible_space + float_max;
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
                apply_max_indent(
                    &mut current_line,
                    &mut max_indent_applied,
                    max_starts_indented_line,
                );
                apply_min_indent(
                    &mut segment,
                    &mut min_indent_applied,
                    min_starts_indented_line,
                );
                current_line += pending_collapsible_space + width;
                pending_collapsible_space = 0.0;
                segment += width;
            }
            // Markers are positioned outside the containing inline flow.
            InlineItemKind::Marker { .. } => {}
        }
    }

    min_content = min_content.max(segment.max(0.0));
    max_content = max_content.max(current_line.max(0.0));
    (min_content, max_content.max(min_content))
}
