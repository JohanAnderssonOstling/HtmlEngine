//! Independently laid-out content from one document. Hosts choose its placement.

use crate::engine::Engine;
use crate::export::DocumentExport;
use crate::geometry::Geometry;
use crate::layout::{GlyphShaper, LayoutConstraints, NoteLayout, RenderView};
use crate::render::{FragmentRenderOptions, RenderScene};
use crate::selection::{
    Selection, SelectionGeometry, SelectionMode, SelectionState, SelectionText,
};
use kurbo::Point;
use std::ops::Range;

/// A subtree's layout and logical selection, sharing DOM, styles and native
/// resources with its owner. Queries return `None` after owner replacement or
/// native resource invalidation. No publication identity or popup state is kept.
pub struct ScopedContent {
    layout: NoteLayout,
    selection: SelectionState,
    constraints: LayoutConstraints,
}

impl Engine {
    /// Lay out any element with this ID, including a note excluded from flow.
    /// Existing document paint resources remain valid while scope resources append.
    pub fn scope(
        &mut self,
        id: &str,
        constraints: LayoutConstraints,
        shaper: &mut impl GlyphShaper,
    ) -> Option<ScopedContent> {
        Some(ScopedContent {
            layout: self.layout_note(id, constraints, shaper)?,
            selection: SelectionState::default(),
            constraints,
        })
    }
}

impl ScopedContent {
    pub fn render_view<'a>(&'a self, owner: &'a Engine) -> Option<RenderView<'a>> {
        self.layout
            .render_view(owner.document()?, owner.glyph_resources())
    }

    pub fn geometry<'a>(&'a self, owner: &'a Engine) -> Option<Geometry<'a>> {
        Some(Geometry::new(self.render_view(owner)?))
    }

    pub fn export<'a>(&'a self, owner: &'a Engine) -> Option<DocumentExport<'a>> {
        Some(DocumentExport::new(self.render_view(owner)?).with_images(owner.images()))
    }

    pub fn scene(&self, owner: &Engine) -> Option<RenderScene> {
        let options = FragmentRenderOptions::new(self.constraints.viewport_width(), None).ok()?;
        Some(RenderScene::for_view(self.render_view(owner)?, &options))
    }

    pub fn selection<'a>(&'a self, owner: &'a Engine) -> Option<Selection<'a>> {
        self.selection.selection(self.render_view(owner)?)
    }

    pub fn selection_text(&self, owner: &Engine) -> Option<SelectionText> {
        Some(self.selection(owner)?.text())
    }

    pub fn selection_geometry(&self, owner: &Engine) -> Option<SelectionGeometry> {
        Some(self.selection(owner)?.geometry_plan())
    }

    pub fn select_range(&mut self, owner: &Engine, range: Range<u32>, mode: SelectionMode) -> bool {
        let Some(view) = self.render_view(owner) else {
            return false;
        };
        if range.start >= range.end || range.end as usize > view.text().glyph_count() {
            return false;
        }
        self.selection.select_range(range.start, range.end, mode);
        true
    }

    pub fn begin_selection(&mut self, owner: &Engine, point: Point, mode: SelectionMode) -> bool {
        let Some(hit) = self
            .geometry(owner)
            .and_then(|geometry| geometry.hit_test(point))
        else {
            return false;
        };
        self.selection.begin_at_glyph(hit.glyph, mode);
        true
    }

    pub fn extend_selection(&mut self, owner: &Engine, point: Point, mode: SelectionMode) -> bool {
        let Some(geometry) = self.geometry(owner) else {
            return false;
        };
        let glyph = geometry.hit_test(point).map(|hit| hit.glyph);
        self.selection.update_interaction(glyph, mode)
    }

    pub fn finish_selection(&mut self, owner: &Engine) -> bool {
        if self.render_view(owner).is_none() {
            return false;
        }
        self.selection.finish_interaction();
        true
    }

    pub fn clear_selection(&mut self) {
        self.selection = SelectionState::default();
    }
}
