//! Public facade for the framework-agnostic HTML engine.
//!
//! Applications should depend on this crate instead of the engine's internal
//! packages. Reader pagination and UI behavior deliberately live outside this
//! facade.

pub mod document {
    pub use html_dom::*;
    pub use html_parse::{
        HtmlFragmentContext, HtmlParserOptions, HtmlQuirksMode, HtmlScriptingMode, HtmlSyntaxAttribute, HtmlSyntaxElement, HtmlSyntaxNode, HtmlSyntaxTree, MarkupSyntax, ParsedHtml, ParsedHtmlFragment, StylesheetReference, XmlParseError,
        build_dom_document, decode_html_bytes, parse_document, parse_dom_document, parse_html_document, parse_html_document_bytes, parse_html_document_with_options, parse_html_fragment, parse_xml_document, plain_text_from_fragment,
    };
}

/// Transitional access to parsing diagnostics. Normal consumers should prefer
/// [`document`] and [`engine`].
pub mod parse {
    pub use html_parse::*;
}

pub mod resources {
    pub use html_resources::*;
    pub use html_source::{ResourceMetadata, ResourceProvider, TocEntry};

    pub use html_source::ResourceMetadata as Metadata;
    pub use html_source::ResourceProvider as Provider;
}

pub mod style {
    pub use html_style::*;
    pub use html_style_model::*;
}

pub mod text {
    pub use html_layout::{
        CharacterPlacement, FontMetricsRequest, FontRelativeMetrics, FontSlant, GlyphId, GlyphMetric, GlyphMetricError, GlyphRegistry, GlyphShaper, OpenTypeFeature, ShapeError, ShapedLine, ShapedTextRun, TextRunId, TextRunShapeRequest,
        TextShapeRequest, TextStyleSpan,
    };
}

pub mod layout {
    pub use html_layout::*;
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
    pub use html_wpt_test_support::*;
}

pub mod engine {
    use std::sync::Arc;

    use crate::layout::{GlyphShaper, LaidOutDocument};
    use crate::pipeline::{PipelineError, PipelineInputs, PipelineSession, PipelineUpdate};
    use crate::resources::ResourceProvider;

    /// Stable facade over the internal incremental pipeline.
    ///
    /// Revision management remains exposed through `PipelineInputs` during the
    /// migration. It can move behind focused setters without changing which
    /// Cargo package applications depend on.
    pub struct Engine {
        session: PipelineSession,
    }

    impl Engine {
        pub fn new(provider: Arc<dyn ResourceProvider>) -> Self {
            Self { session: PipelineSession::new(provider) }
        }

        pub fn update(&mut self, inputs: PipelineInputs, glyph_shaper: &mut impl GlyphShaper) -> Result<PipelineUpdate, PipelineError> {
            self.session.update(inputs, glyph_shaper)
        }

        pub fn rehydrate_glyphs(&mut self, glyph_shaper: &mut impl GlyphShaper) -> Result<LaidOutDocument, PipelineError> {
            self.session.rehydrate_glyphs(glyph_shaper)
        }

        pub fn document(&self) -> Option<&LaidOutDocument> {
            self.session.document()
        }

        pub fn document_mut(&mut self) -> Option<&mut LaidOutDocument> {
            self.session.document_mut()
        }
    }

    pub use crate::pipeline::{PipelineError as Error, PipelineInputs as Input, PipelineUpdate as Update};
}
