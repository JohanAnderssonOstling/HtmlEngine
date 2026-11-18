//! Record geometry-dependent decoration commands during layout.
use crate::LayoutEngine;
use crate::boxes::model::block_border_box_rect;
use crate::paint_program::{PaintOperation, PaintStyleSource};
use html_engine_model::paint_program::BorderSide;
use kurbo::Rect;

pub fn record_block_background(engine: &mut LayoutEngine<'_, '_>, box_idx: usize) {
    if engine.read.reader.background_can_change(box_idx) {
        let rect = block_border_box_rect(engine.geometry, box_idx);
        engine.fragments.record_paint(
            engine.placement,
            PaintOperation::Background {
                owner: box_idx as u32,
                rect,
            },
        );
    }
}

pub fn record_block_decorations(engine: &mut LayoutEngine<'_, '_>, box_idx: usize) {
    record_block_background(engine, box_idx);
    record_block_border_and_outline(engine, box_idx);
}

pub fn record_block_border_and_outline(engine: &mut LayoutEngine<'_, '_>, box_idx: usize) {
    let style = engine.read.reader.style(box_idx);
    if [
        style.border_top_width(),
        style.border_right_width(),
        style.border_bottom_width(),
        style.border_left_width(),
    ]
    .into_iter()
    .any(|width| width > 0.0)
        || (style.outline().style != html_style_model::BorderStyle::None
            && style.outline().width() > 0.0)
    {
        let rect = block_border_box_rect(engine.geometry, box_idx);
        engine.fragments.record_paint(
            engine.placement,
            PaintOperation::BorderOutline {
                owner: box_idx as u32,
                rect,
            },
        );
    }
}

pub fn record_bound_background(
    engine: &mut LayoutEngine<'_, '_>,
    owner: usize,
    rect: Rect,
    source: PaintStyleSource,
) {
    engine.fragments.record_paint(
        engine.placement,
        PaintOperation::BackgroundRect {
            owner: owner as u32,
            source,
            rect,
        },
    );
}

pub fn record_bound_border(
    engine: &mut LayoutEngine<'_, '_>,
    owner: usize,
    fragment_owner: usize,
    rect: Rect,
    source: PaintStyleSource,
    side: BorderSide,
    pattern: html_engine_model::DecorationPattern,
) {
    engine.fragments.record_paint(
        engine.placement,
        PaintOperation::CollapsedBorder {
            owner: owner as u32,
            fragment_owner: fragment_owner as u32,
            source,
            side,
            pattern,
            rect,
        },
    );
}
