use html_style_model::{
    BoxSizing, PositionMode, TextDirection, UsedLengthPct, UsedPreferredSize as PreferredSize,
};
use kurbo::{Point, Size, Vec2};
use std::collections::VecDeque;

use super::box_constraints::resolve_definite_size_value;
use super::box_model::ResolvedBoxModel;
use super::{BoxLayoutRequest, LayoutEngine};

#[derive(Clone, Copy, Debug)]
pub(super) struct PendingAbsoluteBox {
    box_idx: usize,
    containing_block: Option<usize>,
    static_position: Point,
}

pub(super) type AbsolutePositioningState = VecDeque<PendingAbsoluteBox>;

fn take_for(
    pending: &mut AbsolutePositioningState,
    containing_block: Option<usize>,
) -> Option<PendingAbsoluteBox> {
    let index = pending
        .iter()
        .position(|pending| pending.containing_block == containing_block)?;
    pending.remove(index)
}

pub(super) fn defer(engine: &mut LayoutEngine<'_, '_>, box_idx: usize, static_position: Point) {
    let containing_block = engine
        .reader
        .layout_tree()
        .ancestors(box_idx)
        .find(|&ancestor| engine.reader.style(ancestor).position() != PositionMode::Static);
    engine.absolute_positioning.push_back(PendingAbsoluteBox {
        box_idx,
        containing_block,
        static_position,
    });
}

/// Consume matching deferred boxes in source order, resolving their shared
/// containing block only when the queue has work for it.
pub(super) fn layout_pending(engine: &mut LayoutEngine<'_, '_>, target: Option<usize>) {
    let mut containing = None;
    while let Some(pending) = take_for(&mut engine.absolute_positioning, target) {
        let (origin, size) = *containing.get_or_insert_with(|| containing_block(engine, target));
        layout_one(engine, pending, origin, size);
    }
}

fn containing_block(engine: &LayoutEngine<'_, '_>, target: Option<usize>) -> (Point, Size) {
    let Some(containing_block) = target else {
        return (
            Point::ZERO,
            Size::new(
                engine.config.viewport_width(),
                engine.config.viewport_height().unwrap_or(0.0),
            ),
        );
    };
    let style = engine.reader.style(containing_block);
    let border_box = engine.geometry.size(containing_block);
    let origin = engine.geometry.point(containing_block)
        + Vec2::new(
            style.border_left_width() as f64,
            style.border_top_width() as f64,
        );
    let size = Size::new(
        (border_box.width - style.border_left_width() as f64 - style.border_right_width() as f64)
            .max(0.0),
        (border_box.height - style.border_top_width() as f64 - style.border_bottom_width() as f64)
            .max(0.0),
    );
    (origin, size)
}

fn layout_one(
    engine: &mut LayoutEngine<'_, '_>,
    pending: PendingAbsoluteBox,
    containing_origin: Point,
    containing_size: Size,
) {
    let style = engine.reader.style(pending.box_idx);
    let model = ResolvedBoxModel::new(style, containing_size.width);
    let horizontal = AbsoluteAxis::new(
        containing_size.width,
        (style.inset_left(), style.inset_right()),
        (model.margin_left, model.margin_right),
        (style.margin_left_auto(), style.margin_right_auto()),
    );
    let vertical = AbsoluteAxis::new(
        containing_size.height,
        (style.inset_top(), style.inset_bottom()),
        (model.margin_top, model.margin_bottom),
        (style.margin_top_auto(), style.margin_bottom_auto()),
    );

    // Start at the static position so auto insets have a stable CSS static-
    // position fallback. The final translation below also moves every emitted
    // descendant fragment, decoration, and image as one subtree.
    engine
        .geometry
        .set_point(pending.box_idx, pending.static_position);

    let (stretch_width, stretch_height) = if engine.reader.image_idx(pending.box_idx).is_some() {
        (None, None)
    } else {
        (
            horizontal.stretched_border_size(
                style.width(),
                (style.min_width(), style.max_width()),
                style.box_sizing(),
                model.horizontal_padding_border(),
            ),
            vertical.stretched_border_size(
                style.height(),
                (style.min_height(), style.max_height()),
                style.box_sizing(),
                model.vertical_padding_border(),
            ),
        )
    };
    let request = if stretch_width.is_some() || stretch_height.is_some() {
        BoxLayoutRequest::absolute_assigned(
            pending.box_idx,
            containing_size.width,
            containing_size.height,
            stretch_width,
            stretch_height,
        )
    } else {
        BoxLayoutRequest::normal(
            pending.box_idx,
            containing_size.width,
            Some(containing_size.height),
        )
    };
    let layout = engine.layout_box(request);
    let size = layout.size;
    let static_x = if engine
        .reader
        .get_parent(pending.box_idx)
        .is_some_and(|parent| engine.reader.style(parent).direction() == TextDirection::Rtl)
    {
        containing_origin.x + containing_size.width - size.width - model.margin_right
    } else {
        pending.static_position.x
    };
    let horizontal_overflow = if style.direction() == TextDirection::Rtl {
        AutoMarginOverflow::Start
    } else {
        AutoMarginOverflow::End
    };
    let x = horizontal.border_start(
        containing_origin.x,
        size.width,
        static_x,
        horizontal_overflow,
    );
    let y = vertical.border_start(
        containing_origin.y,
        size.height,
        pending.static_position.y,
        AutoMarginOverflow::Split,
    );
    let offset = Point::new(x, y) - pending.static_position;
    // Auto/zero positioned descendants participate in the same source-ordered
    // positioned paint stream as relatively positioned boxes. Treating every
    // absolute box as an independent later sublayer lets an earlier absolute
    // background cover a later relative box (CSS2 9.9 requires the reverse).
    let output = layout.output;
    engine
        .fragments
        .mark_positioned_layer(&output, style.z_index().is_some_and(|z| z < 0), false);
    super::translate_laid_out_output(engine, &output, offset);
}

/// The same physical-axis equation serves inline and block positioning.
/// Margins are already resolved against the containing block's inline size.
struct AbsoluteAxis {
    available: f64,
    insets: (Option<f64>, Option<f64>),
    margins: (f64, f64),
    auto_margins: (bool, bool),
}

/// Horizontal overflow belongs to the direction-dependent end margin;
/// vertical auto margins split overflow equally.
enum AutoMarginOverflow {
    Start,
    End,
    Split,
}

impl AbsoluteAxis {
    fn new(
        available: f64,
        insets: (Option<UsedLengthPct>, Option<UsedLengthPct>),
        margins: (f64, f64),
        auto_margins: (bool, bool),
    ) -> Self {
        Self {
            available,
            insets: (
                insets.0.map(|value| value.resolve(available)),
                insets.1.map(|value| value.resolve(available)),
            ),
            margins,
            auto_margins,
        }
    }

    fn stretched_border_size(
        &self,
        preferred: PreferredSize,
        bounds: (PreferredSize, PreferredSize),
        box_sizing: BoxSizing,
        padding_border: f64,
    ) -> Option<f64> {
        if !matches!(preferred, PreferredSize::Auto | PreferredSize::Stretch) {
            return None;
        }
        let (Some(start), Some(end)) = self.insets else {
            return None;
        };
        let resolve = |value: PreferredSize, fallback: f64| {
            let specified = match value {
                PreferredSize::Auto => return fallback,
                PreferredSize::Stretch => self.available,
                PreferredSize::MinContent
                | PreferredSize::MaxContent
                | PreferredSize::FitContent => {
                    return fallback;
                }
                _ => resolve_definite_size_value(value, Some(self.available)).unwrap_or(fallback),
            };
            match box_sizing {
                BoxSizing::ContentBox => specified + padding_border,
                BoxSizing::BorderBox => specified.max(padding_border),
            }
        };
        let min = resolve(bounds.0, padding_border);
        let max = resolve(bounds.1, f64::INFINITY).max(min);
        Some(
            (self.available - start - end - (self.margins.0 + self.margins.1))
                .max(0.0)
                .clamp(min, max),
        )
    }

    fn border_start(
        &self,
        origin: f64,
        border_size: f64,
        static_position: f64,
        overflow: AutoMarginOverflow,
    ) -> f64 {
        match self.insets {
            (Some(start), _) => origin + start + self.start_margin(border_size, overflow),
            (None, Some(end)) => origin + self.available - end - border_size - self.margins.1,
            (None, None) => static_position,
        }
    }

    fn start_margin(&self, border_size: f64, overflow: AutoMarginOverflow) -> f64 {
        let (Some(start), Some(end)) = self.insets else {
            return if self.auto_margins.0 {
                0.0
            } else {
                self.margins.0
            };
        };
        let free = self.available
            - start
            - end
            - border_size
            - if self.auto_margins.0 {
                0.0
            } else {
                self.margins.0
            }
            - if self.auto_margins.1 {
                0.0
            } else {
                self.margins.1
            };
        match self.auto_margins {
            (true, true) if free >= 0.0 => free / 2.0,
            (true, true) => match overflow {
                AutoMarginOverflow::Start => free,
                AutoMarginOverflow::End => 0.0,
                AutoMarginOverflow::Split => free / 2.0,
            },
            (true, false) => free,
            _ => self.margins.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocation_test_support::count_allocations;

    #[test]
    fn pending_boxes_keep_source_order_across_containing_blocks_without_allocating() {
        let record = |box_idx, containing_block| PendingAbsoluteBox {
            box_idx,
            containing_block,
            static_position: Point::ZERO,
        };
        let mut pending = [(0, None), (1, Some(10)), (2, None), (3, Some(20)), (4, Some(10))]
            .map(|(index, target)| record(index, target))
            .into_iter()
            .collect::<AbsolutePositioningState>();
        let capacity = pending.capacity();
        let (selected, allocations) = count_allocations(|| {
            [
                take_for(&mut pending, Some(10)).map(|pending| pending.box_idx),
                take_for(&mut pending, Some(10)).map(|pending| pending.box_idx),
                take_for(&mut pending, Some(10)).map(|pending| pending.box_idx),
            ]
        });
        assert_eq!(selected, [Some(1), Some(4), None]);
        assert_eq!(allocations, 0, "consuming deferred boxes needs no new storage");

        pending.push_back(record(5, Some(10)));
        assert_eq!(take_for(&mut pending, Some(10)).unwrap().box_idx, 5);
        assert_eq!(take_for(&mut pending, None).unwrap().box_idx, 0);
        assert_eq!(take_for(&mut pending, None).unwrap().box_idx, 2);
        assert!(take_for(&mut pending, None).is_none());
        assert_eq!(take_for(&mut pending, Some(20)).unwrap().box_idx, 3);
        assert!(pending.is_empty());
        pending.clear();
        assert_eq!(pending.capacity(), capacity, "relayout retains queue storage");
    }
}
