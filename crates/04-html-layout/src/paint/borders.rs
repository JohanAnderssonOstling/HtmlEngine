//! Convert resolved border shapes into decoration primitives.
use crate::decorations::border_pattern;
use html_engine_model::paint_program::BorderSide;
use html_engine_model::{DecorationFragment, DecorationPattern, DecorationStore};
use html_style_model::{BorderStyle, TextDecorationStyle};
use kurbo::Rect;

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

pub(super) fn emit_uniform_3d_border(
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
    for (side, border) in BorderSide::ALL.into_iter().zip(borders) {
        let Some((width, color)) = border else {
            continue;
        };
        let extent = if side.horizontal() {
            rect.height()
        } else {
            rect.width()
        };
        let width = width.min(extent);
        if width > 0.0 {
            let edge = border_edge_rect(rect, width, side);
            fragments.push(DecorationFragment::border_rect(
                edge, color, is_inline, false,
            ));
        }
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
    for side in BorderSide::ALL {
        let rect = border_edge_rect(rect, bw, side);
        fragments.push(DecorationFragment::border_rect(
            rect, color, is_inline, foreground,
        ));
    }
}

fn border_edge_rect(rect: Rect, width: f64, side: BorderSide) -> Rect {
    match side {
        BorderSide::Top => Rect::new(rect.x0, rect.y0, rect.x1, rect.y0 + width),
        BorderSide::Right => Rect::new(rect.x1 - width, rect.y0, rect.x1, rect.y1),
        BorderSide::Bottom => Rect::new(rect.x0, rect.y1 - width, rect.x1, rect.y1),
        BorderSide::Left => Rect::new(rect.x0, rect.y0, rect.x0 + width, rect.y1),
    }
}
