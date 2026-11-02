//! Independently laid-out content from one document. Hosts choose its placement.

use crate::engine::{Engine, Error};
use crate::export::DocumentExport;
use crate::geometry::{Geometry, WordHitPolicy};
use crate::{layout::RenderView, text::GlyphShaper};
use crate::render::{FragmentImagePolicy, FragmentRenderOptions, RenderScene};
use crate::selection::{
    Selection, SelectionGeometry, SelectionMode, SelectionState, SelectionText,
};
use kurbo::Point;
use html_pipeline::engine::{LayoutConstraints, NoteLayout};
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
    /// An absent element returns `Ok(None)`; backend failures return `Err`.
    pub fn scope(
        &mut self,
        id: &str,
        width: f64,
        shaper: &mut impl GlyphShaper,
    ) -> Result<Option<ScopedContent>, Error> {
        let inputs = self.inputs().expect("loaded engine owns document inputs");
        let constraints = LayoutConstraints::new(
            width, f64::from(inputs.style_environment.root_font_size.get() as u32),
        )?;
        let Some(layout) = self.layout_note(id, constraints, shaper)? else { return Ok(None) };
        Ok(Some(ScopedContent {
            layout,
            selection: SelectionState::default(),
            constraints,
        }))
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
        let options = FragmentRenderOptions::new(self.constraints.viewport_width(), None)
            .ok()?
            .with_image_policy(FragmentImagePolicy::Reference);
        Some(RenderScene::for_view_with_images(
            self.render_view(owner)?,
            &options,
            owner.images(),
        ))
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
            .and_then(|geometry| geometry.word_hit_at(point, WordHitPolicy::Nearest))
        else {
            return false;
        };
        self.selection.begin_at_word(hit, mode);
        true
    }

    pub fn extend_selection(&mut self, owner: &Engine, point: Point, mode: SelectionMode) -> bool {
        if !self.selection.interaction().is_selecting() {
            return false;
        }
        let Some(geometry) = self.geometry(owner) else {
            return false;
        };
        let hit = geometry.word_hit_at(point, WordHitPolicy::Nearest);
        self.selection.update_word_interaction(hit, mode)
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
