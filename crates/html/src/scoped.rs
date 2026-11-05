//! Independently laid-out content from one document. Hosts choose its placement.

use html_render_core as render_core;
use crate::{Document, Error};
use crate::export::DocumentExport;
use crate::geometry::Geometry;
use crate::{layout::RenderView, text::GlyphShaper};
use crate::render::RenderScene;
use crate::render::SceneImagePolicy;
use html_render_core::SceneOptions;
use crate::selection::{
    Selection, SelectionController, SelectionInput, SelectionLayout, SelectionOutcome,
    SelectionPaintScratch,
};
use crate::render::{LinePlacement, Painter};
use html_render_core::RegionProjection;
use kurbo::{Point, Rect};
use peniko::Color;
use html_pipeline::engine::{LayoutConstraints, NoteLayout};

/// A subtree's layout and logical selection, sharing DOM, styles and native
/// resources with its owner. Queries return `None` after owner replacement or
/// native resource invalidation. No publication identity or popup state is kept.
pub struct ScopedContent {
    id: String,
    layout: NoteLayout,
    selection: SelectionController,
    constraints: LayoutConstraints,
}

impl Document {
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
            width,
            f64::from(inputs.style_environment.root_font_size.get() as u32),
        )?;
        let Some(layout) = self.layout_note(id, constraints, shaper)? else {
            return Ok(None);
        };
        Ok(Some(ScopedContent {
            id: id.to_owned(),
            layout,
            selection: SelectionController::default(),
            constraints,
        }))
    }

    /// Record the laid-out document at its configured width and height.
    pub(crate) fn scene(&self, image_policy: SceneImagePolicy) -> Option<RenderScene> {
        let configuration = self.configuration();
        let options = SceneOptions::new(configuration.width(), configuration.height())
            .ok()?
            .with_image_policy(image_policy);
        Some(render_core::render_scene_for_view_with_images(self.render_view()?,
            &options,
            self.images(),))
    }
}

impl ScopedContent {
    pub fn render_view<'a>(&'a self, owner: &'a Document) -> Option<RenderView<'a>> {
        self.layout
            .render_view(owner.document()?, owner.glyph_resources())
    }

    /// Refresh an existing scope after its owner's image dimensions change.
    /// Retain the logical selection while replacing only layout products.
    pub fn refresh_resources(
        &mut self,
        owner: &mut Document,
        shaper: &mut impl GlyphShaper,
    ) -> Result<bool, Error> {
        if self.render_view(owner).is_some()
            || !owner.document().is_some_and(|document| self.layout.belongs_to(document))
        {
            return Ok(false);
        }
        let Some(layout) = owner.layout_note(&self.id, self.constraints, shaper)? else {
            return Ok(false);
        };
        self.layout = layout;
        Ok(true)
    }

    pub(crate) fn geometry<'a>(&'a self, owner: &'a Document) -> Option<Geometry<'a>> {
        Some(Geometry::new(self.render_view(owner)?))
    }

    pub(crate) fn export<'a>(&'a self, owner: &'a Document) -> Option<DocumentExport<'a>> {
        Some(DocumentExport::new(self.render_view(owner)?).with_images(owner.images()))
    }

    pub fn scene(&self, owner: &Document) -> Option<RenderScene> {
        let options = SceneOptions::new(self.constraints.viewport_width(), None)
            .ok()?
            .with_image_policy(SceneImagePolicy::Reference);
        Some(render_core::render_scene_for_view_with_images(self.render_view(owner)?,
            &options,
            owner.images(),))
    }

    /// The current selection. Export plain text, Markdown, or both on demand
    /// through the returned selection.
    pub fn selection<'a>(&'a self, owner: &'a Document) -> Option<Selection<'a>> {
        self.selection.selection(self.render_view(owner)?)
    }

    pub fn interact(&mut self, owner: &Document, input: SelectionInput) -> SelectionOutcome {
        let Some(document) = owner.document() else {
            return SelectionOutcome::default();
        };
        let Some(view) = self.layout.render_view(document, owner.glyph_resources()) else {
            return SelectionOutcome::default();
        };
        let placements = view.text().lines().iter().map(|line| LinePlacement {
            line: line.id(),
            origin: line.point(),
        });
        let projection = RegionProjection {
            source: Rect::new(0.0, 0.0, self.constraints.viewport_width(), f64::MAX),
            destination: Point::ZERO,
        };
        self.selection.interact(
            SelectionLayout {
                view,
                placements,
                projection: &projection,
            },
            input,
        )
    }

    pub fn paint_selection(
        &self,
        owner: &Document,
        color: Color,
        painter: &mut impl Painter,
        scratch: &mut SelectionPaintScratch,
    ) {
        let Some(view) = self.render_view(owner) else {
            return;
        };
        let placements = view.text().lines().iter().map(|line| LinePlacement {
            line: line.id(),
            origin: line.point(),
        });
        let projection = RegionProjection {
            source: Rect::new(0.0, 0.0, self.constraints.viewport_width(), f64::MAX),
            destination: Point::ZERO,
        };
        self.selection.paint(
            SelectionLayout {
                view,
                placements,
                projection: &projection,
            },
            color,
            painter,
            scratch,
        );
    }
}
