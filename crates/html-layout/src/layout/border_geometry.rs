use crate::layout_model::{DecorationFragment, DecorationPattern, DecorationStore};
use html_style_model::{BorderStyle, TextDecorationStyle, UsedStyleView};
use kurbo::Rect;

#[derive(Clone, Copy)]
pub(crate) struct PhysicalBorderEdge {
    pub style: BorderStyle,
    pub width: f64,
    pub color: u32,
}

impl PhysicalBorderEdge {
    pub(crate) fn solid(self) -> Option<(f64, u32)> {
        matches!(self.style, BorderStyle::Solid).then_some((self.width, self.color))
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PhysicalBorders {
    pub top: PhysicalBorderEdge,
    pub right: PhysicalBorderEdge,
    pub bottom: PhysicalBorderEdge,
    pub left: PhysicalBorderEdge,
}

pub(crate) fn physical_borders(style: &UsedStyleView<'_>) -> PhysicalBorders {
    PhysicalBorders {
        top: PhysicalBorderEdge { style: style.border_top_style(), width: style.border_top_width() as f64, color: style.border_top_color() },
        right: PhysicalBorderEdge { style: style.border_right_style(), width: style.border_right_width() as f64, color: style.border_right_color() },
        bottom: PhysicalBorderEdge { style: style.border_bottom_style(), width: style.border_bottom_width() as f64, color: style.border_bottom_color() },
        left: PhysicalBorderEdge { style: style.border_left_style(), width: style.border_left_width() as f64, color: style.border_left_color() },
    }
}

fn emit_border_rect_layer(fragments: &mut DecorationStore, rect: Rect, border_width: f64, color: u32, is_inline: bool, foreground: bool) {
    let bw = border_width.min(rect.width().min(rect.height()));
    let x0 = rect.x0;
    let x1 = rect.x1;
    let y0 = rect.y0;
    let y1 = rect.y1;
    let top = Rect::new(x0, y0, x1, y0 + bw);
    let bottom = Rect::new(x0, y1 - bw, x1, y1);
    let left = Rect::new(x0, y0, x0 + bw, y1);
    let right = Rect::new(x1 - bw, y0, x1, y1);
    for rect in [top, right, bottom, left] {
        fragments.push(DecorationFragment::border_rect(rect, color, is_inline, foreground));
    }
}

pub(super) fn emit_horizontal_pattern(fragments: &mut DecorationStore, rect: Rect, color: u32, style: TextDecorationStyle, is_inline: bool, foreground: bool, is_border: bool) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let pattern = match style {
        TextDecorationStyle::Solid => DecorationPattern::Solid,
        TextDecorationStyle::Double => DecorationPattern::DoubleHorizontal,
        TextDecorationStyle::Dotted => DecorationPattern::DottedHorizontal,
        TextDecorationStyle::Dashed => DecorationPattern::DashedHorizontal,
    };
    let rect = if style == TextDecorationStyle::Double { Rect::new(rect.x0, rect.y0, rect.x1, rect.y0 + rect.height() * 3.0) } else { rect };
    fragments.push(if is_border { DecorationFragment::border_patterned(rect, color, is_inline, foreground, pattern) } else { DecorationFragment::patterned(rect, color, is_inline, foreground, pattern) });
}

fn emit_vertical_pattern(fragments: &mut DecorationStore, rect: Rect, color: u32, style: BorderStyle, is_inline: bool, foreground: bool) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let Some(pattern) = border_pattern(style, false) else { return };
    fragments.push(DecorationFragment::border_patterned(rect, color, is_inline, foreground, pattern));
}

pub(crate) fn border_pattern(style: BorderStyle, horizontal: bool) -> Option<DecorationPattern> {
    match (style, horizontal) {
        (BorderStyle::Solid, _) => Some(DecorationPattern::Solid),
        (BorderStyle::Dotted, true) => Some(DecorationPattern::DottedHorizontal),
        (BorderStyle::Dashed, true) => Some(DecorationPattern::DashedHorizontal),
        (BorderStyle::Dotted, false) => Some(DecorationPattern::DottedVertical),
        (BorderStyle::Dashed, false) => Some(DecorationPattern::DashedVertical),
        (BorderStyle::Groove | BorderStyle::Ridge, _) => Some(DecorationPattern::Solid),
        (BorderStyle::None | BorderStyle::Hidden, _) => None,
    }
}

pub(super) fn emit_outline_fragments(fragments: &mut DecorationStore, rect: Rect, width: f64, color: u32, style: BorderStyle, is_inline: bool) {
    if width <= 0.0 || style == BorderStyle::None || color & 0xFF == 0 {
        return;
    }
    let outer = Rect::new(rect.x0 - width, rect.y0 - width, rect.x1 + width, rect.y1 + width);
    match style {
        BorderStyle::Solid => emit_border_rect_layer(fragments, outer, width, color, is_inline, true),
        BorderStyle::Dotted | BorderStyle::Dashed => {
            let text_style = if style == BorderStyle::Dotted { TextDecorationStyle::Dotted } else { TextDecorationStyle::Dashed };
            emit_horizontal_pattern(fragments, Rect::new(outer.x0, outer.y0, outer.x1, outer.y0 + width), color, text_style, is_inline, true, true);
            emit_horizontal_pattern(fragments, Rect::new(outer.x0, outer.y1 - width, outer.x1, outer.y1), color, text_style, is_inline, true, true);
            emit_vertical_pattern(fragments, Rect::new(outer.x0, outer.y0 + width, outer.x0 + width, outer.y1 - width), color, style, is_inline, true);
            emit_vertical_pattern(fragments, Rect::new(outer.x1 - width, outer.y0 + width, outer.x1, outer.y1 - width), color, style, is_inline, true);
        }
        BorderStyle::Groove | BorderStyle::Ridge => emit_uniform_3d_border(fragments, outer, width, color, style, is_inline),
        BorderStyle::None | BorderStyle::Hidden => {}
    }
}

pub(crate) fn emit_uniform_3d_border(fragments: &mut DecorationStore, rect: Rect, width: f64, color: u32, style: BorderStyle, is_inline: bool) {
    let half = (width * 0.5).max(0.5);
    let (r, g, b, a) = (((color >> 24) & 0xff) as u8, ((color >> 16) & 0xff) as u8, ((color >> 8) & 0xff) as u8, (color & 0xff) as u8);
    let light_channel = |channel: u8| channel.saturating_add((255 - channel) / 2);
    let light = ((light_channel(r) as u32) << 24) | ((light_channel(g) as u32) << 16) | ((light_channel(b) as u32) << 8) | a as u32;
    let (outer, inner) = if style == BorderStyle::Groove { (color, light) } else { (light, color) };
    emit_border_fragments(fragments, rect, Some((half, outer)), Some((half, light)), Some((half, light)), Some((half, outer)), is_inline);
    let inner_rect = Rect::new(rect.x0 + half, rect.y0 + half, rect.x1 - half, rect.y1 - half);
    if inner_rect.width() > 0.0 && inner_rect.height() > 0.0 {
        emit_border_fragments(fragments, inner_rect, Some((half, inner)), Some((half, color)), Some((half, color)), Some((half, inner)), is_inline);
    }
}

pub(super) fn uniform_solid_border(style: &UsedStyleView<'_>) -> Option<(f64, u32)> {
    let width = style.border_top_width();
    let color = style.border_top_color();
    (width > 0.0
        && [style.border_top_style(), style.border_right_style(), style.border_bottom_style(), style.border_left_style()].into_iter().all(|side| matches!(side, BorderStyle::Solid))
        && [style.border_right_width(), style.border_bottom_width(), style.border_left_width()].into_iter().all(|other| other == width)
        && [style.border_right_color(), style.border_bottom_color(), style.border_left_color()].into_iter().all(|other| other == color))
    .then_some((width as f64, color))
}

pub(super) fn emit_border_fragments(fragments: &mut DecorationStore, rect: Rect, top: Option<(f64, u32)>, right: Option<(f64, u32)>, bottom: Option<(f64, u32)>, left: Option<(f64, u32)>, is_inline: bool) {
    if let Some((w, c)) = top {
        let h = w.min(rect.height());
        if h > 0.0 {
            fragments.push(DecorationFragment::border_rect(Rect::new(rect.x0, rect.y0, rect.x1, rect.y0 + h), c, is_inline, false));
        }
    }
    if let Some((w, c)) = right {
        let bw = w.min(rect.width());
        if bw > 0.0 {
            fragments.push(DecorationFragment::border_rect(Rect::new(rect.x1 - bw, rect.y0, rect.x1, rect.y1), c, is_inline, false));
        }
    }
    if let Some((w, c)) = bottom {
        let h = w.min(rect.height());
        if h > 0.0 {
            fragments.push(DecorationFragment::border_rect(Rect::new(rect.x0, rect.y1 - h, rect.x1, rect.y1), c, is_inline, false));
        }
    }
    if let Some((w, c)) = left {
        let bw = w.min(rect.width());
        if bw > 0.0 {
            fragments.push(DecorationFragment::border_rect(Rect::new(rect.x0, rect.y0, rect.x0 + bw, rect.y1), c, is_inline, false));
        }
    }
}
