//! Paint access requires a palette and cannot reach layout-only snapshots.
use crate::engine::StyleReader;
use html_engine_model::PaintView;
use html_style_model::{StyleIndices, UsedStyleView};
use std::ops::Deref;

pub(crate) struct PaintReader<'input> {
    pub(crate) shared: StyleReader<'input>,
    pub(crate) paint: PaintView<'input>,
}

impl<'input> Deref for PaintReader<'input> {
    type Target = StyleReader<'input>;
    fn deref(&self) -> &Self::Target {
        &self.shared
    }
}

impl<'input> PaintReader<'input> {
    pub fn paint_style(&self, box_idx: usize) -> UsedStyleView<'input> {
        let style = self.style(box_idx);
        self.paint
            .box_colors(box_idx)
            .map_or(style, |colors| style.with_paint_colors(colors))
    }

    pub fn paint_used_style(&self, indices: StyleIndices, context: usize) -> UsedStyleView<'input> {
        let style = self.used_style(indices);
        self.paint
            .style_colors(indices, context)
            .map_or(style, |colors| style.with_paint_colors(colors))
    }

    /// Whether this box supplies the canvas background instead of painting a
    /// separate element background. CSS propagates the root background to the
    /// canvas; for HTML documents whose root has no background, the body
    /// background is propagated instead.
    pub fn background_paints_on_canvas(&self, box_idx: usize) -> bool {
        let Some(root_box) = self.root_box() else {
            return false;
        };
        let root = self.paint_style(root_box);
        if root.background_image_present() || root.background_color() & 0xFF != 0 {
            return box_idx == root_box;
        }
        let Some(body_box) = self.body_box() else {
            return false;
        };
        let body = self.paint_style(body_box);
        (body.background_image_present() || body.background_color() & 0xFF != 0)
            && box_idx == body_box
    }
}
