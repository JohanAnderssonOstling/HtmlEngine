use crate::layout_model::{DecorationFragment, DecorationPattern, DecorationStore};
use html_style_model::{BorderStyle, TextDecorationStyle, UsedStyleView};
use kurbo::Rect;

#[derive(Clone, Copy)]
pub struct PhysicalBorderEdge {
    pub style: BorderStyle,
    pub width: f64,
    pub color: u32,
    pub side: u8,
}

impl PhysicalBorderEdge {
    pub fn solid(self) -> Option<(f64, u32)> {
        matches!(self.style, BorderStyle::Solid).then_some((self.width, self.color))
    }
}

#[derive(Clone, Copy)]
pub struct PhysicalBorders {
    pub top: PhysicalBorderEdge,
    pub right: PhysicalBorderEdge,
    pub bottom: PhysicalBorderEdge,
    pub left: PhysicalBorderEdge,
}

impl PhysicalBorders {
    pub fn uniform(self) -> Option<PhysicalBorderEdge> {
        (self.top.width > 0.0
            && [self.right, self.bottom, self.left].into_iter().all(|edge| {
                edge.style == self.top.style
                    && edge.width == self.top.width
                    && edge.color == self.top.color
            }))
        .then_some(self.top)
    }
}

pub fn physical_borders(style: &UsedStyleView<'_>) -> PhysicalBorders {
    PhysicalBorders {
        top: PhysicalBorderEdge {
            side: 0,
            style: style.border_top_style(),
            width: style.border_top_width() as f64,
            color: style.border_top_color(),
        },
        right: PhysicalBorderEdge {
            side: 1,
            style: style.border_right_style(),
            width: style.border_right_width() as f64,
            color: style.border_right_color(),
        },
        bottom: PhysicalBorderEdge {
            side: 2,
            style: style.border_bottom_style(),
            width: style.border_bottom_width() as f64,
            color: style.border_bottom_color(),
        },
        left: PhysicalBorderEdge {
            side: 3,
            style: style.border_left_style(),
            width: style.border_left_width() as f64,
            color: style.border_left_color(),
        },
    }
}

fn emit_border_rect_layer(
    fragments: &mut DecorationStore,
    rect: Rect,
    border_width: f64,
    color: u32,
    is_inline: bool,
    foreground: bool,
) {
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
        fragments.push(DecorationFragment::border_rect(
            rect, color, is_inline, foreground,
        ));
    }
}

pub(super) fn emit_horizontal_pattern(
    fragments: &mut DecorationStore,
    rect: Rect,
    color: u32,
    style: TextDecorationStyle,
    is_inline: bool,
    foreground: bool,
    is_border: bool,
) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let pattern = match style {
        TextDecorationStyle::Solid => DecorationPattern::Solid,
        TextDecorationStyle::Double => DecorationPattern::DoubleHorizontal,
        TextDecorationStyle::Dotted => DecorationPattern::DottedHorizontal,
        TextDecorationStyle::Dashed => DecorationPattern::DashedHorizontal,
    };
    let rect = if style == TextDecorationStyle::Double {
        Rect::new(rect.x0, rect.y0, rect.x1, rect.y0 + rect.height() * 3.0)
    } else {
        rect
    };
    fragments.push(if is_border {
        DecorationFragment::border_patterned(rect, color, is_inline, foreground, pattern)
    } else {
        DecorationFragment::patterned(rect, color, is_inline, foreground, pattern)
    });
}

pub fn border_pattern(style: BorderStyle, horizontal: bool) -> Option<DecorationPattern> {
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

pub(super) fn emit_outline_fragments(
    fragments: &mut DecorationStore,
    rect: Rect,
    width: f64,
    offset: f64,
    color: u32,
    style: BorderStyle,
    is_inline: bool,
) {
    if width <= 0.0 || style == BorderStyle::None || color & 0xFF == 0 {
        return;
    }
    let offset_rect = Rect::new(
        rect.x0 - offset,
        rect.y0 - offset,
        rect.x1 + offset,
        rect.y1 + offset,
    );
    if offset_rect.width() <= 0.0 || offset_rect.height() <= 0.0 {
        return;
    }
    let outer = Rect::new(
        offset_rect.x0 - width,
        offset_rect.y0 - width,
        offset_rect.x1 + width,
        offset_rect.y1 + width,
    );
    match style {
        BorderStyle::Solid => {
            emit_border_rect_layer(fragments, outer, width, color, is_inline, true)
        }
        BorderStyle::Dotted | BorderStyle::Dashed => {
            for (horizontal, edge) in [
                (
                    true,
                    Rect::new(outer.x0, outer.y0, outer.x1, outer.y0 + width),
                ),
                (
                    true,
                    Rect::new(outer.x0, outer.y1 - width, outer.x1, outer.y1),
                ),
                (
                    false,
                    Rect::new(
                        outer.x0,
                        outer.y0 + width,
                        outer.x0 + width,
                        outer.y1 - width,
                    ),
                ),
                (
                    false,
                    Rect::new(
                        outer.x1 - width,
                        outer.y0 + width,
                        outer.x1,
                        outer.y1 - width,
                    ),
                ),
            ] {
                if edge.width() <= 0.0 || edge.height() <= 0.0 {
                    continue;
                }
                if let Some(pattern) = border_pattern(style, horizontal) {
                    fragments.push(DecorationFragment::border_patterned(
                        edge, color, is_inline, true, pattern,
                    ));
                }
            }
        }
        BorderStyle::Groove | BorderStyle::Ridge => {
            emit_uniform_3d_border(fragments, outer, width, color, style, is_inline)
        }
        BorderStyle::None | BorderStyle::Hidden => {}
    }
}

pub fn emit_uniform_3d_border(
    fragments: &mut DecorationStore,
    rect: Rect,
    width: f64,
    color: u32,
    style: BorderStyle,
    is_inline: bool,
) {
    let half = (width * 0.5).max(0.5);
    let [r, g, b, a] = color.to_be_bytes();
    let light_channel = |channel: u8| channel.saturating_add((255 - channel) / 2);
    let light = u32::from_be_bytes([light_channel(r), light_channel(g), light_channel(b), a]);
    let (outer, inner) = if style == BorderStyle::Groove {
        (color, light)
    } else {
        (light, color)
    };
    emit_border_fragments(
        fragments,
        rect,
        [
            Some((half, outer)),
            Some((half, light)),
            Some((half, light)),
            Some((half, outer)),
        ],
        is_inline,
    );
    let inner_rect = Rect::new(
        rect.x0 + half,
        rect.y0 + half,
        rect.x1 - half,
        rect.y1 - half,
    );
    if inner_rect.width() > 0.0 && inner_rect.height() > 0.0 {
        emit_border_fragments(
            fragments,
            inner_rect,
            [
                Some((half, inner)),
                Some((half, color)),
                Some((half, color)),
                Some((half, inner)),
            ],
            is_inline,
        );
    }
}

pub(super) fn emit_border_fragments(
    fragments: &mut DecorationStore,
    rect: Rect,
    borders: [Option<(f64, u32)>; 4],
    is_inline: bool,
) {
    for (side, border) in borders.into_iter().enumerate() {
        let Some((width, color)) = border else {
            continue;
        };
        let extent = if side == 0 || side == 2 {
            rect.height()
        } else {
            rect.width()
        };
        let width = width.min(extent);
        if width > 0.0 {
            let edge = match side {
                0 => Rect::new(rect.x0, rect.y0, rect.x1, rect.y0 + width),
                1 => Rect::new(rect.x1 - width, rect.y0, rect.x1, rect.y1),
                2 => Rect::new(rect.x0, rect.y1 - width, rect.x1, rect.y1),
                _ => Rect::new(rect.x0, rect.y0, rect.x0 + width, rect.y1),
            };
            fragments.push(DecorationFragment::border_rect(
                edge, color, is_inline, false,
            ));
        }
    }
}
