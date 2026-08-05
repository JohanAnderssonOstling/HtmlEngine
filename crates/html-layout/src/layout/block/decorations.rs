use super::super::border_geometry::{emit_border_fragments, emit_outline_fragments, emit_uniform_3d_border, physical_borders, uniform_solid_border};
use super::super::decorations::DecorationEmitter;
use crate::layout_model::RoundedDecoration;
use html_style_model::BorderStyle;
use kurbo::Rect;

impl DecorationEmitter<'_, '_, '_> {
    /// Returns the used border box produced by layout. Margins affect its
    /// position and available size earlier; they are not part of its stored size.
    fn block_border_box_rect(&self, box_idx: usize) -> Rect {
        self.geometry.decoration_rect(box_idx).map(|rect| rect + self.geometry.point(box_idx).to_vec2()).unwrap_or_else(|| Rect::from_origin_size(self.geometry.point(box_idx), self.geometry.size(box_idx)))
    }

    fn record_block_decoration_owners(&mut self, box_idx: usize, start: usize) {
        self.fragments.record_decoration_owner_since(box_idx as u32, start);
    }

    pub(in crate::layout) fn emit_block_background(&mut self, box_idx: usize) {
        if self.reader.background_paints_on_canvas(box_idx) {
            return;
        }
        let style = self.reader.style(box_idx);
        let background_color = style.background_color();
        if background_color & 0xFF == 0 {
            return;
        }
        let start = self.fragments.decoration_len();
        let rect = self.block_border_box_rect(box_idx);
        let radii = style.border_radii().resolve(rect.width(), rect.height());
        self.push_background(rect, background_color, false, (!radii.is_zero()).then_some(RoundedDecoration { radii, border_width: None }));
        self.record_block_decoration_owners(box_idx, start);
    }

    pub(in crate::layout) fn emit_block_decorations(&mut self, box_idx: usize) {
        self.emit_block_background(box_idx);
        self.emit_block_border_and_outline(box_idx);
    }

    pub(in crate::layout) fn emit_block_border_and_outline(&mut self, box_idx: usize) {
        let style = self.reader.style(box_idx);
        let borders = physical_borders(&style);
        let top_border = borders.top.solid();
        let right_border = borders.right.solid();
        let bottom_border = borders.bottom.solid();
        let left_border = borders.left.solid();
        let uniform_border = uniform_solid_border(&style);
        let uniform_3d = [BorderStyle::Groove, BorderStyle::Ridge].into_iter().find_map(|border_style| {
            let width = style.border_top_width();
            let color = style.border_top_color();
            (width > 0.0
                && [style.border_top_style(), style.border_right_style(), style.border_bottom_style(), style.border_left_style()].into_iter().all(|side| side == border_style)
                && [style.border_right_width(), style.border_bottom_width(), style.border_left_width()].into_iter().all(|other| other == width)
                && [style.border_right_color(), style.border_bottom_color(), style.border_left_color()].into_iter().all(|other| other == color))
            .then_some((width as f64, color, border_style))
        });
        let has_any_border = uniform_3d.is_some() || top_border.is_some() || right_border.is_some() || bottom_border.is_some() || left_border.is_some();
        let outline = style.outline();
        let outline_color = style.outline_color();

        let start = self.fragments.decoration_len();
        if has_any_border {
            let rect = self.block_border_box_rect(box_idx);
            let radii = style.border_radii().resolve(rect.width(), rect.height());
            if let Some((width, color, border_style)) = uniform_3d {
                emit_uniform_3d_border(self.fragments.decorations_mut(), rect, width, color, border_style, false);
            } else if let Some((width, color)) = uniform_border.filter(|_| !radii.is_zero()) {
                self.push_decoration(rect, color, false, Some(RoundedDecoration { radii, border_width: Some(width as f32) }));
            } else {
                emit_border_fragments(self.fragments.decorations_mut(), rect, top_border, right_border, bottom_border, left_border, false);
            }
        }
        if outline.style != BorderStyle::None && outline.width() > 0.0 {
            let rect = self.block_border_box_rect(box_idx);
            emit_outline_fragments(self.fragments.decorations_mut(), rect, outline.width() as f64, outline_color, outline.style, false);
        }
        self.record_block_decoration_owners(box_idx, start);
    }

    pub(in crate::layout) fn emit_color_rect_for_owner(&mut self, owner_box_idx: usize, rect: Rect, color: u32) {
        if color & 0xFF == 0 {
            return;
        }
        let start = self.fragments.decoration_len();
        self.push_background(rect, color, false, None);
        self.record_block_decoration_owners(owner_box_idx, start);
    }
}
