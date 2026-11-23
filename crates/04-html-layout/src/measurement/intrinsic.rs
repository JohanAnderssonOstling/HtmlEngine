use crate::LayoutEngine;
use crate::flex_grid::TaffyContainerKind;
use crate::inline::measure_inline_intrinsic;
use html_engine_model::{Children, InlineItemKind, LayoutMode};
use html_style_model::{Clear, Float, UsedPreferredSize as PreferredSize};
use std::mem::take;

/// Single-threaded memoized values for one immutable layout pass.
#[derive(Clone, Copy, Default)]
pub(crate) struct IntrinsicWidths([Option<(f64, f64)>; 2]);

#[derive(Clone, Copy)]
enum WidthKind {
    Outer,
    Content,
}

/// Shared intrinsic inline-size calculation for every formatting context.
/// Container-specific algorithms contribute through the dispatch in
/// `box_content_intrinsic_widths`; table track solving remains table-owned.
pub fn box_intrinsic_widths(engine: &mut LayoutEngine<'_, '_>, box_idx: usize) -> (f64, f64) {
    intrinsic_widths(engine, box_idx, None, WidthKind::Outer)
}

/// Width transferred from direct percentage-height image children when a
/// container receives a definite block size during shrink-to-fit measurement.
/// Returns None in book mode, which disables this image-width contribution.
pub fn percentage_height_image_width(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    content_height: f64,
) -> Option<f64> {
    if engine.read.constraints.book_optimized_text() {
        return None;
    }
    let mut width: Option<f64> = None;
    for &child_box_idx in engine.read.reader.percentage_height_image_children(box_idx) {
        let Some(intrinsic) = engine.read.reader.image_intrinsic(child_box_idx) else {
            continue;
        };
        let child_style = engine.read.reader.style(child_box_idx);
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

fn intrinsic_widths(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    available_width: Option<f64>,
    kind: WidthKind,
) -> (f64, f64) {
    if available_width.is_none()
        && let Some(widths) = engine.scratch.intrinsic_widths[box_idx].0[kind as usize]
    {
        return widths;
    }
    let widths = match kind {
        WidthKind::Outer => calculate_outer_widths(engine, box_idx, available_width),
        WidthKind::Content => calculate_content_widths(engine, box_idx, available_width),
    };
    if available_width.is_none() {
        engine.scratch.intrinsic_widths[box_idx].0[kind as usize] = Some(widths);
    }
    widths
}

fn calculate_outer_widths(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    available_width: Option<f64>,
) -> (f64, f64) {
    // Intrinsic sizing: percentage padding/margin count as zero.
    let style = engine.read.reader.style(box_idx);
    let sizes = engine.read.reader.sizes(box_idx);
    let policy = engine.read.reader.sizing_policy(box_idx);
    let box_model = policy.adjust_box_model(engine.read.reader.box_model(box_idx, 0.0));
    let margins = if policy.ignores_inline_sizes {
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
        PreferredSize::Auto
        | PreferredSize::Content
        | PreferredSize::Percent(_)
        | PreferredSize::Stretch => auto,
        PreferredSize::MinContent => outer_min,
        PreferredSize::MaxContent => outer_max,
        PreferredSize::FitContent => available_width.map_or(outer_max, |available| {
            outer_max.min(available.max(outer_min))
        }),
        // Intrinsic sizing has no percentage basis. The shared resolver
        // accepts numeric sizes only when they are independent of that basis.
        PreferredSize::Px(_) | PreferredSize::Calc { .. } | PreferredSize::Comparison { .. } => {
            crate::resolve_definite_outer_inline_size(
                size,
                style.box_sizing(),
                padding_border,
                margins,
            )
            .unwrap_or(auto)
        }
    };

    let mut preferred_min = intrinsic_value(sizes.width, outer_min);
    let mut preferred_max = intrinsic_value(sizes.width, outer_max);
    let minimum = intrinsic_value(sizes.min_width, extras);
    let maximum = intrinsic_value(sizes.max_width, f64::INFINITY).max(minimum);
    preferred_min = preferred_min.clamp(minimum, maximum);
    preferred_max = preferred_max.clamp(minimum, maximum).max(preferred_min);
    if policy.width_is_minimum {
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
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    available_width: Option<f64>,
) -> (f64, f64) {
    intrinsic_widths(engine, box_idx, available_width, WidthKind::Content)
}

fn calculate_content_widths(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    available_width: Option<f64>,
) -> (f64, f64) {
    let topology = engine.read.reader.layout_tree();
    let available_width = available_width.map(|width| width.max(0.0));
    if let Some(intrinsic) = engine.read.reader.image_intrinsic(box_idx) {
        return (intrinsic.size.width, intrinsic.size.width);
    }
    match engine.read.reader.box_layout_mode(box_idx) {
        Some(LayoutMode::Block(block)) => {
            children_intrinsic_widths(engine, block, box_idx, available_width)
        }
        Some(LayoutMode::TableCell(cell)) => {
            children_intrinsic_widths(engine, &cell.children, box_idx, available_width)
        }
        Some(LayoutMode::Inline(range)) | Some(LayoutMode::Anonymous(range)) => {
            measure_inline_intrinsic(engine, range.clone(), box_idx)
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
            topology.child_slice(container),
            TaffyContainerKind::Flex,
            None,
            None,
        ),
        Some(LayoutMode::Grid(container)) => crate::flex_grid::intrinsic_widths(
            engine,
            box_idx,
            topology.child_slice(container),
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
    if engine.read.reader.is_table_box(box_idx)
        && matches!(engine.read.reader.style(box_idx).width(), PreferredSize::Percent(value) if value >= 1.0)
    {
        return true;
    }

    engine
        .child_boxes(box_idx, |kind| match kind {
            InlineItemKind::AtomicBox { box_idx } | InlineItemKind::FloatAnchor { box_idx } => {
                Some(*box_idx)
            }
            _ => None,
        })
        .any(|child| contains_full_width_percentage_table(engine, child))
}

/// Floats on a cleared side occupy an earlier row and must not widen the
/// next row. Keep the opposite side because clearance need not pass it.
#[derive(Default)]
pub(crate) struct IntrinsicFloats {
    left: f64,
    right: f64,
}

impl IntrinsicFloats {
    /// Start the next float after clearance; return the width removed from its row.
    pub(crate) fn add(&mut self, side: Float, clear: Clear, width: f64) -> f64 {
        let mut removed = 0.0;
        if matches!(clear, Clear::Left | Clear::Both) {
            removed += take(&mut self.left);
        }
        if matches!(clear, Clear::Right | Clear::Both) {
            removed += take(&mut self.right);
        }
        match side {
            Float::Left => self.left += width,
            Float::Right => self.right += width,
            Float::None => {}
        }
        removed
    }

    fn width(&self) -> f64 {
        self.left + self.right
    }
}

fn children_intrinsic_widths(
    engine: &mut LayoutEngine<'_, '_>,
    children: &Children,
    container_box_idx: usize,
    available_width: Option<f64>,
) -> (f64, f64) {
    match children {
        Children::InlineItems(range) => {
            measure_inline_intrinsic(engine, range.clone(), container_box_idx)
        }
        Children::Blocks(indices) => {
            let mut min_w = 0.0f64;
            let mut max_w = 0.0f64;
            let mut floats = IntrinsicFloats::default();
            for &child_idx in engine.read.reader.layout_tree().child_slice(indices) {
                // Blockification makes an absolutely positioned inline child
                // a direct block-tree child, but it remains out of flow and
                // contributes nothing to its containing block's intrinsic
                // width. Including it here lets a late image/SVG intrinsic
                // size enlarge an auto-width absolute containing block.
                if engine.read.reader.style(child_idx as usize).position()
                    == html_style_model::PositionMode::Absolute
                {
                    continue;
                }
                let (child_min, child_max) = intrinsic_widths(
                    engine,
                    child_idx as usize,
                    available_width,
                    WidthKind::Outer,
                );
                min_w = min_w.max(child_min);
                let child_style = engine.read.reader.style(child_idx as usize);
                if matches!(child_style.float(), Float::Left | Float::Right) {
                    floats.add(child_style.float(), child_style.clear(), child_max);
                    max_w = max_w.max(floats.width());
                } else {
                    max_w = max_w.max(child_max);
                    floats = IntrinsicFloats::default();
                }
            }
            (min_w, max_w)
        }
        Children::Empty => (0.0, 0.0),
    }
}
