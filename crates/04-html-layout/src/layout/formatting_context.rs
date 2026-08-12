use super::LayoutEngine;
use super::box_constraints::constrain_content_width;
use super::inline::{InlineFormattingInput, InlineLayoutArea, ParagraphLayout, ResolvedTextIndent};
use crate::flex_grid::TaffyContainerKind;
use crate::layout_model::Children;
use html_style_model::UsedPreferredSize as PreferredSize;
use kurbo::{Point, Size};

pub(super) fn layout_flow_children(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    children: &Children,
    content_pos: Point,
    content_width: f64,
    containing_width: f64,
    percentage_height_basis: Option<f64>,
    first_line_indent: f64,
) -> Size {
    match children {
        Children::Blocks(indices) => {
            let indices = indices.clone();
            super::block::layout_block_children(
                engine,
                box_idx,
                &indices,
                content_width,
                containing_width,
                percentage_height_basis,
                first_line_indent,
            )
        }
        Children::InlineItems(range) => {
            let style = engine.reader.style(box_idx);
            let indent = ResolvedTextIndent {
                amount: first_line_indent,
                hanging: style.text_indent_hanging(),
                each_line: style.text_indent_each_line(),
            };
            layout_inline_range(
                engine,
                box_idx,
                range.clone(),
                content_pos,
                content_width,
                percentage_height_basis,
                indent,
            )
        }
        Children::Empty => Size::ZERO,
    }
}

pub(super) fn layout_inline_box(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    run_range: std::ops::Range<u32>,
    content_pos: Point,
    content_width: f64,
    percentage_height_basis: Option<f64>,
    first_line_indent: f64,
) -> Size {
    layout_inline_range(
        engine,
        box_idx,
        run_range,
        content_pos,
        content_width,
        percentage_height_basis,
        ResolvedTextIndent {
            amount: first_line_indent,
            hanging: false,
            each_line: false,
        },
    )
}

fn layout_inline_range(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    run_range: std::ops::Range<u32>,
    content_pos: Point,
    content_width: f64,
    percentage_height_basis: Option<f64>,
    indent: ResolvedTextIndent,
) -> Size {
    let style = engine.reader.style(box_idx);
    let paragraph = ParagraphLayout {
        indent,
        text_align: style.text_align(),
        text_align_last: style.text_align_last(),
    };
    super::inline::layout_inline_content(
        engine,
        InlineFormattingInput {
            container_box_idx: box_idx,
            run_range,
            area: InlineLayoutArea {
                origin: content_pos,
                width: content_width,
                percentage_height_basis,
            },
            paragraph,
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn layout_taffy_container(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    children: &[u32],
    kind: TaffyContainerKind,
    content_pos: Point,
    content_width: f64,
    percentage_height_basis: Option<f64>,
    min_height: Option<f64>,
    max_height: Option<f64>,
    shrink_to_fit: bool,
    available_width: f64,
    horizontal_noncontent: f64,
    min_width: PreferredSize,
    max_width: PreferredSize,
    border_box_inset: f64,
) -> Size {
    let layout_width = if shrink_to_fit {
        let (min_content, max_content) = crate::flex_grid::intrinsic_widths_with_constraints(
            engine,
            box_idx,
            children,
            kind,
            None,
            percentage_height_basis,
        );
        let available_content = (available_width - horizontal_noncontent).max(0.0);
        constrain_content_width(
            max_content.min(available_content.max(min_content)),
            min_width,
            max_width,
            available_width,
            border_box_inset,
        )
    } else {
        content_width
    };
    crate::flex_grid::layout(
        engine,
        box_idx,
        children,
        content_pos,
        layout_width,
        percentage_height_basis,
        min_height,
        max_height,
        kind,
    )
}
