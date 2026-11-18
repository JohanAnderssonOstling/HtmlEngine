//! Process each child through preparation, layout, and finishing in source order.
use super::floats::layout_block_float;
use super::margins::{FlowParticipation, parent_collapse_context};
use super::s01_prepare_child::prepare_child;
use super::s02_layout_child::layout_child;
use super::s03_finish_child::{finish_child, finish_flow};
use super::{BlockFlowInput, BlockFlowState};
use crate::boxes::absolute_positioning;
use crate::inline::ResolvedTextIndent;
use crate::{ContentLayoutResult, LayoutEngine, ResolvedBoxSizing};
use html_engine_model::Children;
use html_style_model::TextDirection;
use kurbo::{Point, Size, Vec2};

pub(crate) fn layout_flow_children(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    children: &Children,
    content_pos: Point,
    first_line_indent: f64,
) -> ContentLayoutResult {
    match children {
        Children::Blocks(indices) => {
            let children = engine.read.reader.layout_tree().child_slice(indices);
            layout_block_children(engine, resolved, children, content_pos, first_line_indent)
        }
        Children::InlineItems(range) => {
            let style = engine.read.reader.style(resolved.box_idx());
            let indent = ResolvedTextIndent {
                amount: first_line_indent,
                hanging: style.text_indent_hanging(),
                each_line: style.text_indent_each_line(),
                first_line: true,
            };
            crate::inline::layout_inline_box(engine, resolved, range.clone(), content_pos, indent)
                .into()
        }
        Children::Empty => Size::ZERO.into(),
    }
}

/// Child percentages use `available_width`; box sizing resolves the used content origin.
fn layout_block_children(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    children: &[u32],
    content_pos: Point,
    first_line_indent: f64,
) -> ContentLayoutResult {
    let parent_idx = resolved.box_idx();
    let input = BlockFlowInput {
        parent_idx,
        available_width: resolved.horizontal.content,
        parent_content_height: resolved.vertical.descendant_basis,
        position: content_pos,
        parent_ctx: parent_collapse_context(&engine.read.reader, parent_idx),
    };
    let mut flow = BlockFlowState {
        indent_remaining: first_line_indent,
        ..BlockFlowState::default()
    };
    for &child in children {
        let child_idx = child as usize;
        if engine.read.reader.box_facts(child_idx).absolute {
            // The hypothetical static position lies after the adjoining
            // margin accumulated from preceding in-flow content. Resolving it
            // here does not consume that margin; the next in-flow sibling
            // still collapses against the same strut.
            absolute_positioning::defer(
                engine,
                child_idx,
                input.position
                    + Vec2::new(
                        if engine.read.reader.style(input.parent_idx).direction()
                            == TextDirection::Rtl
                        {
                            input.available_width
                        } else {
                            0.0
                        },
                        flow.y_offset + flow.pending_margin.resolve(),
                    ),
            );
            continue;
        }
        if engine.read.reader.is_inline_box(child_idx) {
            continue;
        }

        let prepared = prepare_child(engine, &input, &flow, child_idx);
        if prepared.profile.participation == FlowParticipation::Float {
            layout_block_float(engine, &input, &mut flow, prepared);
        } else {
            let laid_out = layout_child(engine, &input, &mut flow, prepared);
            finish_child(&mut flow, laid_out);
        }
    }
    finish_flow(resolved, &input, &mut flow)
}
