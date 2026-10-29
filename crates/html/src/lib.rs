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

pub mod document {
    pub use html_dom::*;
    pub use html_parse::{
        DocumentTextIndex, HtmlFragmentContext, HtmlParserOptions, HtmlQuirksMode,
        HtmlScriptingMode, HtmlSyntaxAttribute, HtmlSyntaxElement, HtmlSyntaxNode, HtmlSyntaxTree,
        MarkupSyntax, ParsedHtml, ParsedHtmlFragment, SourceTextPosition, StylesheetReference,
        XmlParseError, build_dom_document, decode_html_bytes, parse_document, parse_dom_document,
        parse_html_document, parse_html_document_bytes, parse_html_document_with_options,
        parse_html_fragment, parse_xml_document, plain_text_from_fragment, read_document_source,
    };

    /// Declared media type takes precedence over the resource filename.
    pub fn markup_syntax_for_resource(
        provider: &dyn crate::resources::ResourceProvider,
        uri: &str,
    ) -> MarkupSyntax {
        let media_type = provider
            .metadata(uri)
            .ok()
            .and_then(|metadata| metadata.media_type);
        MarkupSyntax::from_resource(uri, media_type.as_deref())
    }

    /// Build a parse-only text/source index for search without loading fonts,
    /// images, styles, or a reader viewport.
    pub fn text_index_for_resource(
        source: &str,
        provider: &dyn crate::resources::ResourceProvider,
        uri: &str,
    ) -> Option<DocumentTextIndex> {
        let parsed = parse_document(source, markup_syntax_for_resource(provider, uri)).ok()?;
        DocumentTextIndex::from_parsed(&parsed).ok()
    }
}

/// Transitional access to parsing diagnostics. Normal consumers should prefer
/// [`document`] and [`engine`].
pub mod parse {
    pub use html_parse::*;
}

pub mod resources {
    pub use crate::engine_resources::image_metric_changes;
    pub use html_resources::*;
    pub use html_dom::{ResourceMetadata, ResourceProvider};

    pub use html_dom::ResourceMetadata as Metadata;
    pub use html_dom::ResourceProvider as Provider;
}

pub mod style {
    pub use html_style::*;
    pub use html_style_model::*;
}

pub mod text {
    pub use html_pipeline::engine::{
        CharacterPlacement, ContentLanguage, FontMetrics, FontMetricsRequest, FontRelativeMetrics,
        FontSlant, GlyphHandle, GlyphId, GlyphMetric, GlyphMetricError, GlyphRequest,
        GlyphResourceCheckpoint, GlyphResourceGeneration, GlyphResourceStore, GlyphResources,
        GlyphShaper, LanguageTag, OpenTypeFeature, ResourceShaper, ShapeError, ShapedGlyph,
        ShapedLine, ShapedTextRun, TextDirection, TextRunId, TextRunShapeRequest, TextShapeRequest,
        TextStyleSpan,
    };
    pub use html_pipeline::engine::{TextSearchOptions, find_text_matches, text_matches};
}

pub mod layout {
    pub use html_pipeline::engine::*;
}

/// Incremental parse/style/layout engine API.
pub mod pipeline {
    pub use html_pipeline::*;
}

/// Framework-neutral painting and basic, non-paginated fragment rendering.
pub mod render {
    pub use html_render_core::*;
}

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
    use crate::layout::{GlyphShaper, LaidOutDocument};
    use crate::pipeline::{
        EarliestStage, PipelineChanges, PipelineError, PipelineInputs, PipelineSession, PipelineUpdate,
    };
    use crate::render::DocumentPainter;
    use crate::resources::{ImagePipeline, ResourceProvider};
    use crate::search::Search;
    use crate::revisions::DocumentRevisions;
    use crate::reading::Reading;
    use crate::selection::Selection;
    use crate::style::FontFace;
    use crate::structure::Structure;
    use crate::targets::Targets;

    /// Facade over the incremental pipeline. The session owns its source and
    /// committed inputs; hosts submit changes and borrow the published state.
    /// Each shaping operation borrows the host's text backend.
    pub struct Engine {
        pub(super) session: PipelineSession,
        pub(super) images: ImagePipeline,
        pub(super) provider: Arc<dyn ResourceProvider>,
        pub(super) pending_image_metrics: Vec<u32>,
        pub(super) revisions: DocumentRevisions,
    }

    impl Engine {
        pub fn new(provider: Arc<dyn ResourceProvider>) -> Self {
            Self {
                session: PipelineSession::new(provider.clone()),
                images: ImagePipeline::new(provider.clone()),
                provider,
                pending_image_metrics: Vec::new(),
                revisions: DocumentRevisions::default(),
            }
        }

        /// Load or replace a complete set of owned document inputs. For
        /// subsequent updates, use [`Self::apply_changes`] to retain unchanged
        /// source text and configuration in the session.
        pub fn update(
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
        pub fn inputs(&self) -> Option<&PipelineInputs> {
            self.session.inputs()
        }

        /// Applies proposed fields while borrowing unchanged source and inputs.
        pub fn apply_changes(
            &mut self,
            changes: PipelineChanges,
            glyph_shaper: &mut impl GlyphShaper,
        ) -> Result<PipelineUpdate, PipelineError> {
            let update = self.session.attach(glyph_shaper).apply_changes(changes)?;
            if update.stage <= EarliestStage::Prepare {
                self.sync_resources();
            }
            self.revisions.invalidate_from(update.stage);
            Ok(update)
        }

        /// Prepares provider-backed HTML through parse and style without
        /// borrowing a platform text system. Finish with [`Self::rehydrate_glyphs`]
        /// on the host thread that owns its glyph shaper.
        pub fn prepare_through_style(
            &mut self,
            inputs: PipelineInputs,
        ) -> Result<(), PipelineError> {
            self.session.prepare_through_style(inputs)?;
            self.revisions.invalidate_from(EarliestStage::Parse);
            Ok(())
        }

        /// Active font declarations, available before shaping after
        /// [`Self::prepare_through_style`]. Load/register these faces in the
        /// host text backend, then call [`Self::rehydrate_glyphs`].
        /// URL sources are resolved against their declaring stylesheet.
        pub fn font_faces(&self) -> &[FontFace] {
            self.session.font_faces()
        }

        pub fn media_environment_change_affects_style(
            &self,
            media: crate::pipeline::MediaEnvironment,
            root_font_size: u32,
        ) -> bool {
            self.session
                .media_environment_change_affects_style(media, root_font_size)
        }

        /// Rebuild glyph resources and publish the layout in place. Borrow
        /// the resulting chapter with [`Self::document`].
        pub fn rehydrate_glyphs(
            &mut self,
            glyph_shaper: &mut impl GlyphShaper,
        ) -> Result<(), PipelineError> {
            self.session.attach(glyph_shaper).rehydrate_glyphs()?;
            self.sync_resources();
            self.revisions.invalidate_from(EarliestStage::Shape);
            Ok(())
        }

        /// Lays out one note body under its own constraints, for embedders
        /// that hold notes out of the reading flow and present them elsewhere.
        pub fn layout_note(
            &mut self,
            id: &str,
            constraints: crate::layout::LayoutConstraints,
            glyph_shaper: &mut impl GlyphShaper,
        ) -> Option<crate::layout::NoteLayout> {
            self.session
                .attach(glyph_shaper)
                .layout_note(id, constraints)
        }

        /// Ids of the current document's note bodies, in document order.
        pub fn note_ids(&self) -> Vec<&str> {
            self.session.note_ids()
        }

        pub fn document(&self) -> Option<&LaidOutDocument<'static>> {
            self.session.document()
        }

        pub fn glyph_resources(&self) -> &crate::text::GlyphResourceStore {
            self.session.glyph_resources()
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
            Some(DocumentPainter::new(self.render_view()?).with_images(self.images()))
        }

        pub fn render_view(&self) -> Option<crate::layout::RenderView<'_>> {
            self.session.render_view()
        }

        pub fn document_mut_and_resources(
            &mut self,
        ) -> Option<(
            &mut LaidOutDocument<'static>,
            &crate::text::GlyphResourceStore,
        )> {
            self.session.document()?;
            self.revisions.invalidate_from(EarliestStage::Layout);
            self.session.document_mut_and_resources()
        }

        pub fn document_mut(&mut self) -> Option<&mut LaidOutDocument<'static>> {
            self.session.document()?;
            self.revisions.invalidate_from(EarliestStage::Layout);
            self.session.document_mut()
        }
    }

    pub use crate::pipeline::{
        PipelineError as Error, PipelineInputs as Input, PipelineUpdate as Update,
    };
}
