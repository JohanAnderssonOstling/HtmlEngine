//! Provider-backed loading for the public engine facade.

use crate::document::read_document_source;
use crate::engine::{Engine, Error};
use crate::layout::GlyphShaper;
use crate::pipeline::{
    FontEnvironmentRevision, ImageMetricsRevision, ImageSizingPolicy, LayoutConstraints,
    MediaEnvironment, NoteFlow, PaintSettingsRevision, PipelineInputs, ReaderStyleOverrides,
    ResourceRevision, SourceRevision, StyleEnvironment, StylesheetRevision, TextCompositionPolicy,
};
use crate::resources::ResourceProvider;
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
    pub fn total(&self) -> Duration {
        self.resource + self.pipeline
    }
    pub fn add_assign(&mut self, other: &Self) {
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
    pub width: f64,
    pub root_font_size: f32,
    pub user_styles: Vec<String>,
    pub reader_overrides: ReaderStyleOverrides,
    pub image_sizing_policy: ImageSizingPolicy,
    pub text_composition_policy: TextCompositionPolicy,
    pub note_flow: NoteFlow,
}

impl DocumentOptions {
    pub fn new(width: f64) -> Self {
        Self {
            width,
            root_font_size: 16.0,
            user_styles: Vec::new(),
            reader_overrides: ReaderStyleOverrides::default(),
            image_sizing_policy: ImageSizingPolicy::WebCompatible,
            text_composition_policy: TextCompositionPolicy::WebCompatible,
            note_flow: NoteFlow::InFlow,
        }
    }

    /// Read and decode a resource, honoring its declared charset and media type.
    /// Linked and inline CSS are handled by the engine during preparation.
    ///
    /// Most consumers should use [`Engine::load`] or [`Engine::prepare`]. This
    /// lower-level entry point lets adapters instrument loading or customize
    /// pipeline revisions before submitting the initial inputs.
    pub fn read(self, provider: &dyn ResourceProvider, uri: &str) -> Result<PipelineInputs, Error> {
        let media = MediaEnvironment::screen(self.width, None)
            .ok_or_else(|| Error("document width must be finite and positive".into()))?;
        if !self.root_font_size.is_finite() || self.root_font_size <= 0.0 {
            return Err(Error("root font size must be finite and positive".into()));
        }
        let (source, markup_syntax) = read_document_source(provider, uri)
            .map_err(|error| Error(format!("failed to load HTML resource {uri}: {error}")))?;
        Ok(PipelineInputs {
            source,
            markup_syntax,
            user_styles: self.user_styles,
            reader_overrides: self.reader_overrides,
            note_flow: self.note_flow,
            source_revision: SourceRevision::INITIAL,
            base_uri: uri.to_owned(),
            resource_revision: ResourceRevision::INITIAL,
            stylesheet_revision: StylesheetRevision::INITIAL,
            style_environment: StyleEnvironment {
                root_font_size: self.root_font_size.max(1.0).round() as u32,
                media,
                direction: 0,
            },
            font_environment: FontEnvironmentRevision::INITIAL,
            image_metrics_revision: ImageMetricsRevision::INITIAL,
            layout: LayoutConstraints {
                viewport_width: self.width,
                viewport_height: None,
                line_height: f64::from(self.root_font_size),
                image_sizing_policy: self.image_sizing_policy,
                text_composition_policy: self.text_composition_policy,
            },
            image_metrics: Default::default(),
            paint: PaintSettingsRevision::INITIAL,
        })
    }
}

impl Engine {
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
        engine.update(inputs, shaper)?;
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
        let inputs = options.read(provider.as_ref(), uri)?;
        let mut engine = Self::new(provider);
        engine.update(inputs, shaper)?;
        Ok(engine)
    }

    /// Load and style a document without a platform text backend.
    ///
    /// Inspect [`Self::font_faces`] and load fonts on the host, then finish with
    /// [`Self::rehydrate_glyphs`] on the thread owning the shaper. Preparation
    /// can run on a worker; it creates no pagination or UI state.
    pub fn prepare(
        provider: Arc<dyn ResourceProvider>,
        uri: &str,
        options: DocumentOptions,
    ) -> Result<Self, Error> {
        let inputs = options.read(provider.as_ref(), uri)?;
        let mut engine = Self::new(provider);
        engine.prepare_through_style(inputs)?;
        Ok(engine)
    }
}

#[cfg(test)]
mod tests {
    use super::DocumentOptions;
    use crate::document::MarkupSyntax;
    use crate::resources::{ResourceMetadata, ResourceProvider};
    use crate::search::SearchIndex;
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct CountingProvider {
        reads: AtomicUsize,
        metadata_reads: AtomicUsize,
        fail_read: bool,
        fail_metadata: bool,
    }

    impl ResourceProvider for CountingProvider {
        fn read_bytes(&self, _: &str) -> io::Result<Vec<u8>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            if self.fail_read {
                return Err(io::Error::new(io::ErrorKind::NotFound, "missing fixture"));
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
                media_type: Some(" Application/XHTML+XML ; charset=windows-1252".into()),
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
            .read(&provider, "chapter.html")
            .unwrap();
        assert_eq!(provider.reads.load(Ordering::Relaxed), 1);
        assert_eq!(provider.metadata_reads.load(Ordering::Relaxed), 1);
        assert_eq!(inputs.source, "<p>café</p>");
        assert_eq!(inputs.markup_syntax, MarkupSyntax::Xml);
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
            .read(&provider, "chapter.xhtml")
            .unwrap();
        assert_eq!(inputs.markup_syntax, MarkupSyntax::Xml);
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
}
