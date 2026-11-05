//! Provider-backed loading for the public engine facade.

use crate::configuration::{DocumentConfiguration, DocumentUpdate, normalized_font_size};
use html_parse::read_document_source;
use crate::{Document, Error};
use crate::text::{FontFace, GlyphShaper};
use crate::{
    layout::{ImageSizingPolicy, NoteFlow, TextCompositionPolicy},
    pipeline::{PipelineInputs, StyleEnvironment},
    style::ReaderStyleOverrides,
};
use html_parse::MarkupSyntax;
use crate::resources::ResourceProvider;
use html_style::MediaEnvironment;
use html_pipeline::engine::{LayoutConstraintError, LayoutConstraints};
use html_resources::resources::provider::NoResources;
use std::sync::Arc;
use std::time::Duration;
use web_time::Instant;

/// Measured loading phases. Pipeline includes parsing, CSS preparation,
/// shaping, and layout; unmeasured subphases are never reported as zero work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoadTimings {
    pub resource: Duration,
    pub pipeline: Duration,
}

impl LoadTimings {
    pub(crate) fn total(&self) -> Duration {
        self.resource + self.pipeline
    }
    pub(crate) fn add_assign(&mut self, other: &Self) {
        self.resource += other.resource;
        self.pipeline += other.pipeline;
    }
}

/// Options for loading one continuous document at an available width.
///
/// Defaults use web composition, 16px type, and notes in their authored flow.
/// Reader adapters can choose book composition and note handling explicitly.
/// Pagination, columns, and publication identity are outside this API.
#[derive(Clone, Debug)]
pub struct DocumentOptions {
    style_environment: StyleEnvironment,
    pub reader_overrides: ReaderStyleOverrides,
    pub image_sizing_policy: ImageSizingPolicy,
    pub text_composition_policy: TextCompositionPolicy,
    pub note_flow: NoteFlow,
}

impl DocumentOptions {
    /// Validate the available width before any resource is read. Zero is valid
    /// for hosts that prepare documents before their viewport is measured.
    pub fn new(width: f64) -> Result<Self, Error> {
        let media = MediaEnvironment::screen(width, None)
            .ok_or(LayoutConstraintError::InvalidViewportWidth)?;
        Ok(Self {
            style_environment: StyleEnvironment {
                media,
                ..StyleEnvironment::default()
            },
            reader_overrides: ReaderStyleOverrides::default(),
            image_sizing_policy: ImageSizingPolicy::WebCompatible,
            text_composition_policy: TextCompositionPolicy::WebCompatible,
            note_flow: NoteFlow::InFlow,
        })
    }

    /// Reject non-finite or non-positive sizes, then apply the loader's
    /// whole-pixel rounding with a minimum of one pixel.
    pub fn with_root_font_size(mut self, size: f32) -> Result<Self, Error> {
        self.style_environment.root_font_size = normalized_font_size(size)?;
        Ok(self)
    }

    /// Read and decode a resource, honoring its declared charset and media type.
    /// Linked and inline CSS are handled by the engine during preparation.
    ///
    /// Resource decoding and initial pipeline revisions are engine-owned.
    pub(crate) fn read(self, provider: &dyn ResourceProvider, uri: &str) -> Result<PipelineInputs, Error> {
        let (source, markup_syntax) = read_document_source(provider, uri)
            .map_err(|error| Error::from(error).with_context(format!("failed to load HTML resource {uri}")))?;
        self.into_inputs(source, markup_syntax, uri.to_owned())
    }

    pub(crate) fn into_inputs(
        self,
        source: String,
        markup_syntax: MarkupSyntax,
        base_uri: String,
    ) -> Result<PipelineInputs, Error> {
        let layout = LayoutConstraints::new(
            self.style_environment.media.viewport_width(),
            f64::from(self.style_environment.root_font_size.get() as u32),
        )?
        .with_image_sizing_policy(self.image_sizing_policy)
        .with_text_composition_policy(self.text_composition_policy);
        let mut inputs = PipelineInputs::new(
            source,
            markup_syntax,
            base_uri,
            self.style_environment,
            layout,
        );
        inputs.reader_overrides = self.reader_overrides;
        inputs.note_flow = self.note_flow;
        Ok(inputs)
    }
}

/// Parsed and styled document inputs awaiting a host text backend.
/// Load the declared fonts before consuming this value with [`Self::finish`].
/// Document queries and resource loading become available on the returned engine.
pub struct PreparedDocument {
    engine: Document,
}

impl PreparedDocument {
    pub fn configuration(&self) -> DocumentConfiguration<'_> {
        self.engine.configuration()
    }

    /// Shared font declarations, with URLs resolved against their stylesheet.
    pub fn font_faces(&self) -> Arc<[FontFace]> {
        self.engine.font_faces()
    }

    /// Shape and lay out the prepared document, applying any input changes in
    /// the same operation. Use a default update to retain the prepared inputs.
    pub fn finish(
        mut self,
        update: DocumentUpdate,
        shaper: &mut impl GlyphShaper,
    ) -> Result<Document, Error> {
        self.engine.apply(update, shaper)?;
        Ok(self.engine)
    }
}

impl Document {
    pub fn load_timed(
        provider: Arc<dyn ResourceProvider>,
        uri: &str,
        options: DocumentOptions,
        shaper: &mut impl GlyphShaper,
    ) -> Result<(Self, LoadTimings), Error> {
        let start = Instant::now();
        let inputs = options.read(provider.as_ref(), uri)?;
        let resource = start.elapsed();
        let start = Instant::now();
        let mut engine = Self::new(provider);
        engine.update(inputs, shaper)
            .map_err(|error| error.with_context(format!("failed to render document {uri}")))?;
        Ok((
            engine,
            LoadTimings {
                resource,
                pipeline: start.elapsed(),
            },
        ))
    }

    /// Load, style, shape, and lay out one provider-backed document.
    pub fn load(
        provider: Arc<dyn ResourceProvider>,
        uri: &str,
        options: DocumentOptions,
        shaper: &mut impl GlyphShaper,
    ) -> Result<Self, Error> {
        Self::load_timed(provider, uri, options, shaper).map(|(engine, _)| engine)
    }

    /// Lay out in-memory markup, such as a description or a standalone
    /// fragment. External resources are unavailable; replace the source or
    /// other inputs later through [`Self::apply`].
    pub(crate) fn from_source(
        source: String,
        options: DocumentOptions,
        shaper: &mut impl GlyphShaper,
    ) -> Result<Self, Error> {
        let inputs = options.into_inputs(source, MarkupSyntax::Html, "fragment.xhtml".to_owned())?;
        let mut engine = Self::new(Arc::new(NoResources));
        engine.update(inputs, shaper)?;
        Ok(engine)
    }

    /// Load and style a document without a platform text backend.
    ///
    /// Inspect [`PreparedDocument::font_faces`] and load fonts on the host, then
    /// call [`PreparedDocument::finish`] on the thread owning the shaper. Preparation
    /// can run on a worker; it creates no pagination or UI state.
    pub fn prepare(
        provider: Arc<dyn ResourceProvider>,
        uri: &str,
        options: DocumentOptions,
    ) -> Result<PreparedDocument, Error> {
        let inputs = options.read(provider.as_ref(), uri)?;
        let mut engine = Self::new(provider);
        engine.prepare_through_style(inputs)
            .map_err(|error| error.with_context(format!("failed to prepare document {uri}")))?;
        Ok(PreparedDocument { engine })
    }
}


#[cfg(test)]
mod tests {
    use super::DocumentOptions;
    use html_parse::MarkupSyntax;
    use crate::resources::{ResourceMetadata, ResourceProvider};
    use crate::search::SearchIndex;
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Default)]
    struct CountingProvider {
        reads: AtomicUsize,
        metadata_reads: AtomicUsize,
        fail_read: bool,
        fail_metadata: bool,
        malformed_xml: bool,
    }

    impl ResourceProvider for CountingProvider {
        fn read_bytes(&self, _: &str) -> io::Result<Vec<u8>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            if self.fail_read {
                return Err(io::Error::new(io::ErrorKind::NotFound, "missing fixture"));
            }
            if self.malformed_xml {
                return Ok(b"<root>\n<item></root>".to_vec());
            }
            Ok(b"<p>caf\xe9</p>".to_vec())
        }

        fn metadata(&self, _: &str) -> io::Result<ResourceMetadata> {
            self.metadata_reads.fetch_add(1, Ordering::Relaxed);
            if self.fail_metadata {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "metadata unavailable",
                ));
            }
            Ok(ResourceMetadata {
                media_type: Some(if self.malformed_xml { "application/xml" } else { " Application/XHTML+XML ; charset=windows-1252" }.into()),
                charset: Some("windows-1252".into()),
            })
        }

        fn exists(&self, _: &str) -> bool {
            unreachable!()
        }
        fn resolve(&self, _: &str, _: &str) -> String {
            unreachable!()
        }
        fn list_html_candidates(&self, _: &str) -> io::Result<Vec<String>> {
            unreachable!()
        }
    }

    #[test]
    fn document_and_search_loading_share_encoding_syntax_and_one_metadata_snapshot() {
        let provider = CountingProvider::default();
        let inputs = DocumentOptions::new(600.0)
            .unwrap()
            .read(&provider, "chapter.html")
            .unwrap();
        assert_eq!(provider.reads.load(Ordering::Relaxed), 1);
        assert_eq!(provider.metadata_reads.load(Ordering::Relaxed), 1);
        assert_eq!(inputs.source, "<p>café</p>");
        assert_eq!(inputs.markup_syntax, MarkupSyntax::Xhtml);
        let index = SearchIndex::load(&provider, "chapter.html").unwrap();
        assert_eq!(index.text(), "café");
        assert_eq!(provider.reads.load(Ordering::Relaxed), 2);
        assert_eq!(provider.metadata_reads.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn metadata_failure_keeps_default_decoding_and_filename_syntax_in_both_loaders() {
        let provider = CountingProvider {
            fail_metadata: true,
            ..Default::default()
        };
        let inputs = DocumentOptions::new(600.0)
            .unwrap()
            .read(&provider, "chapter.xhtml")
            .unwrap();
        assert_eq!(inputs.markup_syntax, MarkupSyntax::Xhtml);
        assert_eq!(inputs.source, "<p>caf�</p>");
        assert_eq!(
            SearchIndex::load(&provider, "chapter.xhtml")
                .unwrap()
                .text(),
            "caf�"
        );
        assert_eq!(provider.reads.load(Ordering::Relaxed), 2);
        assert_eq!(provider.metadata_reads.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn failed_reads_preserve_caller_errors_and_skip_metadata() {
        let provider = CountingProvider {
            fail_read: true,
            ..Default::default()
        };
        let error = DocumentOptions::new(600.0)
            .unwrap()
            .read(&provider, "chapter.html")
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "failed to load HTML resource chapter.html: missing fixture"
        );
        let error = SearchIndex::load(&provider, "chapter.html").err().unwrap();
        assert_eq!(error.to_string(), "missing fixture");
        assert_eq!(provider.reads.load(Ordering::Relaxed), 2);
        assert_eq!(provider.metadata_reads.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn preparation_errors_identify_the_resource_and_source_location() {
        let provider = Arc::new(CountingProvider {
            malformed_xml: true,
            ..Default::default()
        });
        let error = super::Document::prepare(
            provider,
            "OPS/chapter2.xml",
            DocumentOptions::new(600.0).unwrap(),
        )
        .err()
        .unwrap();
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("failed to prepare document OPS/chapter2.xml"), "{diagnostic}");
        assert!(diagnostic.contains("line 2, column"), "{diagnostic}");
        assert!(diagnostic.contains("root"), "{diagnostic}");
    }
}
