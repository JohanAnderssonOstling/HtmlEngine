use super::super::border_geometry::{
    emit_outline_fragments, emit_uniform_3d_border, physical_borders,
};
use super::super::decorations::{DecorationEmitter, block_border_box_rect};
use html_style_model::BorderStyle;
use kurbo::Rect;

impl DecorationEmitter<'_, '_, '_> {
    /// Returns the used border box produced by layout. Margins affect its
    /// position and available size earlier; they are not part of its stored size.
    fn block_border_box_rect(&self, box_idx: usize) -> Rect {
        self.retained_rect
            .unwrap_or_else(|| block_border_box_rect(self.geometry, box_idx))
    }

    fn record_block_decoration_owners(&mut self, box_idx: usize, start: usize) {
        self.fragments
            .record_decoration_owner_since(box_idx as u32, start);
    }

    pub(in crate::layout) fn emit_block_background(&mut self, box_idx: usize) {
        if self.reader.background_paints_on_canvas(box_idx) {
            return;
        }
        let style = self.reader.paint_style(box_idx);
        if style.visibility() == html_style_model::Visibility::Hidden {
            return;
        }
        let background_color = style.background_color();
        if background_color & 0xFF == 0 {
            return;
        }
        let start = self.fragments.decoration_len();
        let rect = self.block_border_box_rect(box_idx);
        let radii = style.border_radii().resolve(rect.width(), rect.height());
        self.push_background(
            rect,
            background_color,
            false,
            (!radii.is_zero()).then_some(radii),
        );
        self.record_block_decoration_owners(box_idx, start);
    }

    pub(in crate::layout) fn emit_block_border_and_outline(&mut self, box_idx: usize) {
        let style = self.reader.paint_style(box_idx);
        if style.visibility() == html_style_model::Visibility::Hidden {
            return;
        }
        let borders = physical_borders(&style);
        let top_border = borders.top.solid();
        let right_border = borders.right.solid();
        let bottom_border = borders.bottom.solid();
        let left_border = borders.left.solid();
        let uniform_border = borders.uniform();
        let uniform_3d = uniform_border
            .filter(|border| matches!(border.style, BorderStyle::Groove | BorderStyle::Ridge));
        let uniform_solid = uniform_border.and_then(|border| border.solid());
        let solid_borders = [top_border, right_border, bottom_border, left_border];
        let has_any_border = uniform_3d.is_some() || solid_borders.iter().any(Option::is_some);
        let outline = style.outline();
        let outline_color = style.outline_color();

        let start = self.fragments.decoration_len();
        if has_any_border {
            let rect = self.block_border_box_rect(box_idx);
            let radii = style.border_radii().resolve(rect.width(), rect.height());
            if let Some(border) = uniform_3d {
                emit_uniform_3d_border(
                    self.fragments.decoration_store_mut(),
                    rect,
                    border.width,
                    border.color,
                    border.style,
                    false,
                );
            } else {
                self.emit_solid_border(rect, radii, uniform_solid, solid_borders, false);
            }
        }
        if outline.style != BorderStyle::None && outline.width() > 0.0 {
            let rect = self.block_border_box_rect(box_idx);
            emit_outline_fragments(
                self.fragments.decoration_store_mut(),
                rect,
                outline.width() as f64,
                outline.offset() as f64,
                outline_color,
                outline.style,
                false,
            );
        }
        self.record_block_decoration_owners(box_idx, start);
    }

    pub(in crate::layout) fn emit_color_rect_for_owner(
        &mut self,
        owner_box_idx: usize,
        rect: Rect,
        color: u32,
    ) {
        if color & 0xFF == 0
            || self.reader.paint_style(owner_box_idx).visibility() == html_style_model::Visibility::Hidden
        {
            return;
        }
        let start = self.fragments.decoration_len();
        self.push_background(rect, color, false, None);
        self.record_block_decoration_owners(owner_box_idx, start);
    }
}
