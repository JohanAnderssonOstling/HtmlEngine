use super::PaintReader;
use super::borders::{emit_outline_fragments, emit_uniform_3d_border};
use super::decorations::{emit_solid_border, push_background};
use crate::decorations::physical_borders;
use html_engine_model::DecorationStore;
use html_style_model::BorderStyle;
use kurbo::Rect;

pub(super) fn emit_block_background(
    reader: &PaintReader<'_>,
    fragments: &mut DecorationStore,
    box_idx: usize,
    rect: Rect,
) {
    if reader.background_paints_on_canvas(box_idx) {
        return;
    }
    let style = reader.paint_style(box_idx);
    if style.visibility() == html_style_model::Visibility::Hidden {
        return;
    }
    let background_color = style.background_color();
    if background_color & 0xFF == 0 {
        return;
    }
    let radii = style.border_radii().resolve(rect.width(), rect.height());
    push_background(fragments, rect, background_color, false, radii);
}

pub(super) fn emit_block_border_and_outline(
    reader: &PaintReader<'_>,
    fragments: &mut DecorationStore,
    box_idx: usize,
    rect: Rect,
) {
    let style = reader.paint_style(box_idx);
    if style.visibility() == html_style_model::Visibility::Hidden {
        return;
    }
    let borders = physical_borders(&style);
    let uniform_border = borders.uniform();
    let uniform_3d = uniform_border
        .filter(|border| matches!(border.style, BorderStyle::Groove | BorderStyle::Ridge));
    let uniform_solid = uniform_border.and_then(|border| border.solid());
    let solid_borders = borders.solid_edges(true, true);
    let has_any_border = uniform_3d.is_some() || solid_borders.iter().any(Option::is_some);
    let outline = style.outline();

    if has_any_border {
        let radii = style.border_radii().resolve(rect.width(), rect.height());
        if let Some(border) = uniform_3d {
            emit_uniform_3d_border(
                fragments,
                rect,
                border.width,
                border.color,
                border.style,
                false,
            );
        } else {
            emit_solid_border(fragments, rect, radii, uniform_solid, solid_borders, false);
        }
    }
    emit_outline_fragments(
        fragments,
        rect,
        outline.width() as f64,
        outline.offset() as f64,
        style.outline_color(),
        outline.style,
        false,
    );
}
