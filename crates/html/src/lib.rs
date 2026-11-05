//! Public facade for the framework-agnostic HTML engine.
//!
//! Applications should depend on this crate instead of the engine's internal
//! packages. Page composition is engine-owned; viewport management and platform
//! effects live in the host.


use html_pipeline::engine::pagination_queries;
mod engine_load;
mod engine_resources;
pub mod configuration;
pub mod export;
pub mod geometry;
pub mod pagination;
pub mod search;
pub mod scoped;
pub mod revisions;
pub mod reading;
pub mod selection;
mod structure;
pub mod targets;

mod error;
mod facade;
mod overlays;
pub use facade::{document, layout, render, resources, style, text};
use html_pipeline as pipeline;

pub use engine::Document;
pub use engine_load::{DocumentOptions, LoadTimings, PreparedDocument};
pub use engine_resources::{ImageSourceRequest, ResourcePoll};
use engine_resources::DocumentImages;
pub use error::Error;

mod engine {
    use std::ops::Range;
    use std::sync::Arc;

    use crate::geometry::Geometry;
    use crate::text::GlyphShaper;
    use html_pipeline::engine::{
        GlyphResourceStore, LaidOutDocument, LayoutConstraints, NoteLayout,
    };
    use crate::pipeline::{
        EarliestStage, PipelineChanges, PipelineError, PipelineInputs, PipelineSession,
    };
    use crate::resources::ResourceProvider;
    use crate::DocumentImages;
    use crate::revisions::DocumentRevisions;
    use crate::selection::Selection;
    use crate::text::FontFace;

    /// Opaque document service. Hosts submit input changes and borrow published
    /// content, geometry and paint products. The document owns its source,
    /// incremental pipeline and resources; shaping borrows the host's backend.
    pub struct Document {
        pub(super) session: PipelineSession,
        pub(super) images: DocumentImages,
        pub(super) provider: Arc<dyn ResourceProvider>,
        pub(super) revisions: DocumentRevisions,
    }

    impl Document {
        pub(crate) fn new(provider: Arc<dyn ResourceProvider>) -> Self {
            Self {
                session: PipelineSession::new(provider.clone()),
                images: DocumentImages::new(provider.clone()),
                provider,
                revisions: DocumentRevisions::new(),
            }
        }

        /// Load or replace a complete set of owned document inputs. For
        /// subsequent updates, use [`Self::apply_changes`] to retain unchanged
        /// source text and configuration in the session.
        pub(crate) fn update(
            &mut self,
            inputs: PipelineInputs,
            glyph_shaper: &mut impl GlyphShaper,
        ) -> Result<EarliestStage, PipelineError> {
            let stage = self.session.attach(glyph_shaper).update(inputs)?;
            self.sync_resources();
            self.revisions.invalidate_from(stage);
            Ok(stage)
        }

        /// Product versions local to this engine. Cache owners also retain
        /// their own document identity when storing results across engines.
        pub fn revisions(&self) -> DocumentRevisions {
            self.revisions
        }

        /// The session owns the committed source and configuration. Hosts can
        /// borrow these values instead of retaining a second input snapshot.
        pub(crate) fn inputs(&self) -> Option<&PipelineInputs> {
            self.session.inputs()
        }

        /// Applies proposed fields while borrowing unchanged source and inputs.
        pub(crate) fn apply_changes(
            &mut self,
            changes: PipelineChanges,
            glyph_shaper: &mut impl GlyphShaper,
        ) -> Result<EarliestStage, PipelineError> {
            let was_published = self.session.document().is_some();
            let stage = self.session.attach(glyph_shaper).apply_changes(changes)?;
            if !was_published || stage <= EarliestStage::Prepare {
                self.sync_resources();
            }
            self.revisions.invalidate_from(stage);
            Ok(stage)
        }

        /// Prepares provider-backed HTML through parse and style without
        /// borrowing a platform text system.
        pub(crate) fn prepare_through_style(
            &mut self,
            inputs: PipelineInputs,
        ) -> Result<(), PipelineError> {
            self.session.prepare_through_style(inputs)?;
            self.revisions.invalidate_from(EarliestStage::Parse);
            Ok(())
        }

        /// Shared snapshot of active font declarations for the committed inputs.
        /// URL sources are resolved against their declaring stylesheet.
        pub fn font_faces(&self) -> Arc<[FontFace]> {
            self.session.font_faces()
        }

        /// Lays out one note body under its own constraints, for embedders
        /// that hold notes out of the reading flow and present them elsewhere.
        pub(crate) fn layout_note(
            &mut self,
            id: &str,
            constraints: LayoutConstraints,
            glyph_shaper: &mut impl GlyphShaper,
        ) -> Result<Option<NoteLayout>, PipelineError> {
            self.session
                .attach(glyph_shaper)
                .layout_note(id, constraints)
        }

        pub(crate) fn document(&self) -> Option<&LaidOutDocument<'static>> {
            self.session.document()
        }

        pub(crate) fn glyph_resources(&self) -> &GlyphResourceStore {
            self.session.glyph_resources()
        }

        /// Version of the native resources used by the published text.
        pub fn glyph_resource_generation(&self) -> crate::text::GlyphResourceGeneration {
            self.session.glyph_resources().generation()
        }

        /// Select document content without creating a reader or a viewport.
        pub(crate) fn selection(&self, range: Range<u32>) -> Option<Selection<'_>> {
            Some(Selection::new(self.render_view()?, range))
        }


        /// Document-space queries shared by scrolling and paginated surfaces.
        pub(crate) fn geometry(&self) -> Option<Geometry<'_>> {
            Some(Geometry::new(self.render_view()?))
        }

        pub fn render_view(&self) -> Option<crate::layout::RenderView<'_>> {
            self.session.render_view()
        }
    }
}

// Existing engine regressions also cover implementation-only operations.
#[cfg(test)]
#[path = "../tests/document_api.rs"]
mod document_api_tests;
#[cfg(test)]
#[path = "../tests/document_queries.rs"]
mod document_queries_tests;
#[cfg(test)]
#[path = "../tests/font_faces.rs"]
mod font_faces_tests;
#[cfg(test)]
#[path = "../tests/markdown.rs"]
mod markdown_tests;
