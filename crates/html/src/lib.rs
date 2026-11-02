//! Public facade for the framework-agnostic HTML engine.
//!
//! Applications should depend on this crate instead of the engine's internal
//! packages. Reader pagination and UI behavior deliberately live outside this
//! facade.

mod engine_load;
mod engine_resources;
#[cfg(feature = "math-export")]
mod mathema;
pub mod content;
pub mod configuration;
pub mod annotations;
pub mod export;
pub mod geometry;
#[cfg(feature = "font-data")]
pub mod fonts;
pub mod search;
pub mod scoped;
pub mod revisions;
pub mod reading;
pub mod selection;
pub mod structure;
pub mod targets;

mod facade;
pub use facade::{document, layout, render, resources, style, text};
use html_pipeline as pipeline;

#[cfg(feature = "testing")]
pub mod testing {
    pub use html_parse::HtmlQuirksMode as QuirksMode;

    /// Classifies an HTML document without exposing parser implementation
    /// packages to adapter-level conformance suites.
    pub fn document_quirks_mode(bytes: &[u8]) -> QuirksMode {
        html_parse::parse_html_document_bytes(bytes, None).quirks_mode()
    }

    /// Reports whether a WPT document carries the server-dependent `http`
    /// flag. Static UI adapters use this to maintain an explicit unsupported
    /// ledger without depending on parser syntax-tree types.
    pub fn document_requires_http(bytes: &[u8], uri: &str) -> Result<bool, String> {
        let source = String::from_utf8_lossy(bytes);
        let parsed = html_parse::parse_document(&source, html_parse::MarkupSyntax::from_uri(uri))
            .map_err(|error| error.to_string())?;
        Ok(syntax_nodes_have_http_flag(&parsed.syntax_tree().nodes))
    }

    fn syntax_nodes_have_http_flag(nodes: &[html_parse::HtmlSyntaxNode]) -> bool {
        nodes.iter().any(|node| match node {
            html_parse::HtmlSyntaxNode::Element(element) => {
                let is_http_flag = element.local_name.eq_ignore_ascii_case("meta")
                    && element.attributes.iter().any(|attribute| {
                        attribute.local_name.eq_ignore_ascii_case("name")
                            && attribute.value.eq_ignore_ascii_case("flags")
                    })
                    && element
                        .attributes
                        .iter()
                        .find(|attribute| attribute.local_name.eq_ignore_ascii_case("content"))
                        .is_some_and(|attribute| {
                            attribute
                                .value
                                .split_ascii_whitespace()
                                .any(|flag| flag.eq_ignore_ascii_case("http"))
                        });
                is_http_flag || syntax_nodes_have_http_flag(&element.children)
            }
            html_parse::HtmlSyntaxNode::TemplateContents(children) => {
                syntax_nodes_have_http_flag(children)
            }
            _ => false,
        })
    }
}

pub mod engine {
    pub use crate::engine_load::{DocumentOptions, LoadTimings};
    pub use crate::engine_resources::{ImageSourceRequest, ResourcePoll, ResourceUpdate};
    use std::ops::Range;
    use std::sync::Arc;

    use crate::content::Content;
    use crate::geometry::Geometry;
    use crate::text::GlyphShaper;
    use html_pipeline::engine::{ImageMetrics, LaidOutDocument, LayoutConstraints, NoteLayout};
    use crate::pipeline::{
        EarliestStage, PipelineChanges, PipelineError, PipelineInputs, PipelineSession, PipelineUpdate,
    };
    use crate::render::DocumentPainter;
    use crate::resources::{ImagePipeline, ResourceProvider};
    use crate::search::Search;
    use crate::revisions::DocumentRevisions;
    use crate::reading::Reading;
    use crate::selection::Selection;
    use crate::text::FontFace;
    use crate::structure::Structure;
    use crate::targets::Targets;

    /// Facade over the incremental pipeline. The session owns its source and
    /// committed inputs; hosts submit changes and borrow the published state.
    /// Each shaping operation borrows the host's text backend.
    pub struct Engine {
        pub(super) session: PipelineSession,
        pub(super) images: ImagePipeline,
        pub(super) image_resource_revision: Option<crate::pipeline::ResourceRevision>,
        pub(super) provider: Arc<dyn ResourceProvider>,
        pub(super) pending_image_metrics: Vec<u32>,
        pub(super) pending_image_snapshot: Option<ImageMetrics>,
        pub(super) revisions: DocumentRevisions,
    }

    impl Engine {
        pub(crate) fn new(provider: Arc<dyn ResourceProvider>) -> Self {
            Self {
                session: PipelineSession::new(provider.clone()),
                images: ImagePipeline::new(provider.clone()),
                image_resource_revision: None,
                provider,
                pending_image_metrics: Vec::new(),
                pending_image_snapshot: None,
                revisions: DocumentRevisions::default(),
            }
        }

        /// Load or replace a complete set of owned document inputs. For
        /// subsequent updates, use [`Self::apply_changes`] to retain unchanged
        /// source text and configuration in the session.
        pub(crate) fn update(
            &mut self,
            inputs: PipelineInputs,
            glyph_shaper: &mut impl GlyphShaper,
        ) -> Result<PipelineUpdate, PipelineError> {
            let update = self.session.attach(glyph_shaper).update(inputs)?;
            self.sync_resources();
            self.revisions.invalidate_from(update.stage);
            Ok(update)
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
        ) -> Result<PipelineUpdate, PipelineError> {
            let was_published = self.session.document().is_some();
            let update = self.session.attach(glyph_shaper).apply_changes(changes)?;
            if !was_published || update.stage <= EarliestStage::Prepare {
                self.sync_resources();
            }
            self.revisions.invalidate_from(update.stage);
            Ok(update)
        }

        /// Prepares provider-backed HTML through parse and style without
        /// borrowing a platform text system. Finish with [`Self::apply`]
        /// on the host thread that owns its glyph shaper.
        pub(crate) fn prepare_through_style(
            &mut self,
            inputs: PipelineInputs,
        ) -> Result<(), PipelineError> {
            self.session.prepare_through_style(inputs)?;
            self.revisions.invalidate_from(EarliestStage::Parse);
            Ok(())
        }

        /// Active font declarations, available after [`Self::prepare`].
        /// Load/register these faces in the
        /// host text backend, then submit a default [`crate::configuration::DocumentUpdate`] to [`Self::apply`].
        /// URL sources are resolved against their declaring stylesheet.
        pub fn font_faces(&self) -> &[FontFace] {
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

        /// Ids of the current document's note bodies, in document order.
        pub fn note_ids(&self) -> Vec<&str> {
            self.session.note_ids()
        }

        pub(crate) fn document(&self) -> Option<&LaidOutDocument<'static>> {
            self.session.document()
        }

        pub(crate) fn glyph_resources(&self) -> &crate::text::GlyphResourceStore {
            self.session.glyph_resources()
        }

        /// Version of the native resources used by the published text.
        pub fn glyph_resource_generation(&self) -> crate::text::GlyphResourceGeneration {
            self.session.glyph_resources().generation()
        }

        /// Select document content without creating a reader or a viewport.
        pub fn selection(&self, range: Range<u32>) -> Option<Selection<'_>> {
            Some(Selection::new(self.render_view()?, range))
        }

        /// Document-space queries shared by scrolling and paginated surfaces.
        pub fn geometry(&self) -> Option<Geometry<'_>> {
            Some(Geometry::new(self.render_view()?))
        }

        pub fn targets(&self) -> Option<Targets<'_>> {
            Some(Targets::new(self.render_view()?))
        }

        pub fn structure(&self) -> Option<Structure<'_>> {
            Some(Structure::new(self.render_view()?))
        }

        pub fn reading(&self) -> Option<Reading<'_>> {
            Some(Reading::new(self.render_view()?))
        }

        pub fn content(&self) -> Option<Content<'_>> {
            Some(Content::new(self.render_view()?))
        }

        /// Search this document without choosing a viewport or publication scope.
        pub fn search(&self) -> Option<Search<'_>> {
            Some(Search::new(self.render_view()?))
        }

        /// Paint this document using any caller-selected geometric projection.
        pub fn painter(&self) -> Option<DocumentPainter<'_>> {
            Some(
                DocumentPainter::new(
                    self.render_view()
                        .filter(|view| view.paint_resources_valid())?,
                )
                .with_images(self.images()),
            )
        }

        pub fn render_view(&self) -> Option<crate::layout::RenderView<'_>> {
            self.session.render_view()
        }


    }

    pub use crate::pipeline::{PipelineError as Error, PipelineErrorKind as ErrorKind};
}
