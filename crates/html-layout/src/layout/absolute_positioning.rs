use html_style_model::{BoxSizing, PositionMode, TextDirection, UsedPreferredSize as PreferredSize, resolve_used_preferred_size};
use kurbo::{Point, Size, Vec2};

use super::LayoutEngine;
use super::box_model::ResolvedBoxModel;

#[derive(Clone, Copy, Debug)]
struct PendingAbsoluteBox {
    box_idx: usize,
    containing_block: Option<usize>,
    static_position: Point,
    static_direction: TextDirection,
}

#[derive(Default)]
pub(super) struct AbsolutePositioningState {
    pending: Vec<PendingAbsoluteBox>,
}

impl AbsolutePositioningState {
    pub(super) fn clear(&mut self) {
        self.pending.clear();
    }

    fn defer(&mut self, pending: PendingAbsoluteBox) {
        self.pending.push(pending);
    }

    fn take_for(&mut self, containing_block: Option<usize>) -> Option<PendingAbsoluteBox> {
        let index = self.pending.iter().position(|pending| pending.containing_block == containing_block)?;
        Some(self.pending.remove(index))
    }
}

pub(super) fn defer(engine: &mut LayoutEngine<'_, '_>, box_idx: usize, static_position: Point) {
    let mut ancestor = engine.reader.get_parent(box_idx);
    let mut containing_block = None;
    while let Some(candidate) = ancestor {
        if engine.reader.style(candidate).position() != PositionMode::Static {
            containing_block = Some(candidate);
            break;
        }
        ancestor = engine.reader.get_parent(candidate);
    }
    let static_direction = engine.reader.get_parent(box_idx).map(|parent| engine.reader.style(parent).direction()).unwrap_or(TextDirection::Ltr);
    engine.absolute_positioning.defer(PendingAbsoluteBox { box_idx, containing_block, static_position, static_direction });
}

pub(super) fn layout_for_box(engine: &mut LayoutEngine<'_, '_>, containing_block: usize) {
    let style = engine.reader.style(containing_block);
    let border_box = engine.geometry.size(containing_block);
    let origin = engine.geometry.point(containing_block) + Vec2::new(style.border_left_width() as f64, style.border_top_width() as f64);
    let size = Size::new((border_box.width - style.border_left_width() as f64 - style.border_right_width() as f64).max(0.0), (border_box.height - style.border_top_width() as f64 - style.border_bottom_width() as f64).max(0.0));
    layout_pending(engine, Some(containing_block), origin, size);
}

pub(super) fn layout_initial_containing_block(engine: &mut LayoutEngine<'_, '_>) {
    let size = Size::new(engine.config.viewport_width(), engine.config.viewport_height().unwrap_or(0.0));
    layout_pending(engine, None, Point::ZERO, size);
}

fn layout_pending(engine: &mut LayoutEngine<'_, '_>, target: Option<usize>, containing_origin: Point, containing_size: Size) {
    while let Some(pending) = engine.absolute_positioning.take_for(target) {
        layout_one(engine, pending, containing_origin, containing_size);
    }
}

fn layout_one(engine: &mut LayoutEngine<'_, '_>, pending: PendingAbsoluteBox, containing_origin: Point, containing_size: Size) {
    let style = engine.reader.style(pending.box_idx);
    let model = ResolvedBoxModel::new(style, containing_size.width);
    let left = style.inset_left().map(|value| value.resolve(containing_size.width));
    let right = style.inset_right().map(|value| value.resolve(containing_size.width));
    let top = style.inset_top().map(|value| value.resolve(containing_size.height));
    let bottom = style.inset_bottom().map(|value| value.resolve(containing_size.height));

    // Start at the static position so auto insets have a stable CSS static-
    // position fallback. The final translation below also moves every emitted
    // descendant fragment, decoration, and image as one subtree.
    engine.geometry.set_point(pending.box_idx, pending.static_position);

    let stretch_width = if matches!(style.width(), PreferredSize::Auto | PreferredSize::Stretch) {
        match (left, right) {
            (Some(left), Some(right)) if engine.reader.image_intrinsic_size(pending.box_idx).is_none() => {
                Some(constrain_border_width(style, &model, containing_size.width, (containing_size.width - left - right - model.horizontal_margin()).max(0.0)))
            }
            _ => None,
        }
    } else {
        None
    };
    let stretch_height = if matches!(style.height(), PreferredSize::Auto | PreferredSize::Stretch) {
        match (top, bottom) {
            (Some(top), Some(bottom)) if engine.reader.image_intrinsic_size(pending.box_idx).is_none() => {
                Some(constrain_border_height(style, &model, containing_size.height, (containing_size.height - top - bottom - model.vertical_margin()).max(0.0)))
            }
            _ => None,
        }
    } else {
        None
    };
    let layout = if stretch_width.is_some() || stretch_height.is_some() {
        engine.layout_box(super::BoxLayoutRequest::absolute_assigned(pending.box_idx, containing_size.width, containing_size.height, stretch_width, stretch_height))
    } else {
        engine.layout_box(super::BoxLayoutRequest::normal(pending.box_idx, containing_size.width, Some(containing_size.height)))
    };
    let size = layout.size;

    let margin_left = used_horizontal_start_margin(style, containing_size.width, size.width, left, right, model.margin_left, model.margin_right);
    let margin_top = used_vertical_start_margin(style, containing_size.height, size.height, top, bottom, model.margin_top, model.margin_bottom);
    let x = match (left, right) {
        (Some(left), _) => containing_origin.x + left + margin_left,
        (None, Some(right)) => containing_origin.x + containing_size.width - right - size.width - model.margin_right,
        (None, None) if pending.static_direction == TextDirection::Rtl => containing_origin.x + containing_size.width - size.width - model.margin_right,
        (None, None) => pending.static_position.x,
    };
    let y = match (top, bottom) {
        (Some(top), _) => containing_origin.y + top + margin_top,
        (None, Some(bottom)) => containing_origin.y + containing_size.height - bottom - size.height - model.margin_bottom,
        (None, None) => pending.static_position.y,
    };
    let offset = Point::new(x, y) - pending.static_position;
    // Auto/zero positioned descendants participate in the same source-ordered
    // positioned paint stream as relatively positioned boxes. Treating every
    // absolute box as an independent later sublayer lets an earlier absolute
    // background cover a later relative box (CSS2 9.9 requires the reverse).
    engine.fragments.mark_positioned_layer(&layout.output, style.z_index().is_some_and(|z| z < 0), false);
    super::translate_laid_out_subtree_output(engine, pending.box_idx, true, &layout.output, offset);
}

fn constrain_border_width(style: html_style_model::UsedStyleView<'_>, model: &ResolvedBoxModel, containing_width: f64, width: f64) -> f64 {
    let inset = model.horizontal_padding_border();
    let resolve = |value: PreferredSize, fallback: f64| {
        let specified = match value {
            PreferredSize::Auto => return fallback,
            PreferredSize::Stretch => containing_width,
            _ => resolve_used_preferred_size(value, fallback, containing_width),
        };
        match style.box_sizing() {
            BoxSizing::ContentBox => specified + inset,
            BoxSizing::BorderBox => specified.max(inset),
        }
    };
    let min = resolve(style.min_width(), inset);
    let max = resolve(style.max_width(), f64::INFINITY).max(min);
    width.clamp(min, max)
}

fn constrain_border_height(style: html_style_model::UsedStyleView<'_>, model: &ResolvedBoxModel, containing_height: f64, height: f64) -> f64 {
    let inset = model.vertical_padding_border();
    let resolve = |value: PreferredSize, fallback: f64| {
        let specified = match value {
            PreferredSize::Auto => return fallback,
            PreferredSize::Stretch => containing_height,
            PreferredSize::Px(px) => px as f64,
            PreferredSize::Percent(percent) => containing_height * percent as f64,
            PreferredSize::Calc { absolute_px, percentage, .. } => absolute_px as f64 + containing_height * percentage as f64,
            PreferredSize::Comparison { .. } => resolve_used_preferred_size(value, fallback, containing_height),
            PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent => return fallback,
        };
        match style.box_sizing() {
            BoxSizing::ContentBox => specified + inset,
            BoxSizing::BorderBox => specified.max(inset),
        }
    };
    let min = resolve(style.min_height(), inset);
    let max = resolve(style.max_height(), f64::INFINITY).max(min);
    height.clamp(min, max)
}

fn used_horizontal_start_margin(style: html_style_model::UsedStyleView<'_>, containing_width: f64, border_width: f64, left: Option<f64>, right: Option<f64>, authored_left: f64, authored_right: f64) -> f64 {
    if let (Some(left), Some(right)) = (left, right) {
        let free = containing_width - left - right - border_width - if style.margin_left_auto() { 0.0 } else { authored_left } - if style.margin_right_auto() { 0.0 } else { authored_right };
        return match (style.margin_left_auto(), style.margin_right_auto()) {
            (true, true) if free >= 0.0 => free / 2.0,
            (true, true) if style.direction() == TextDirection::Rtl => free,
            (true, true) => 0.0,
            (true, false) => free,
            _ => authored_left,
        };
    }
    if style.margin_left_auto() { 0.0 } else { authored_left }
}

fn used_vertical_start_margin(style: html_style_model::UsedStyleView<'_>, containing_height: f64, border_height: f64, top: Option<f64>, bottom: Option<f64>, authored_top: f64, authored_bottom: f64) -> f64 {
    if let (Some(top), Some(bottom)) = (top, bottom) {
        let free = containing_height - top - bottom - border_height - if style.margin_top_auto() { 0.0 } else { authored_top } - if style.margin_bottom_auto() { 0.0 } else { authored_bottom };
        return match (style.margin_top_auto(), style.margin_bottom_auto()) {
            (true, true) => free / 2.0,
            (true, false) => free,
            _ => authored_top,
        };
    }
    if style.margin_top_auto() { 0.0 } else { authored_top }
}
