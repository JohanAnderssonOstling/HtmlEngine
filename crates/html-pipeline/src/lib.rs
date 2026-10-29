#[macro_use(profile, profile_scope)]
extern crate html_engine_model;

#[cfg(feature = "profiling")]
pub use html_engine_model::profiling;
#[path = "parser/mod.rs"]
mod parser;
mod session;
mod types;

pub use html_dom::RootFontSize;
use html_engine_model as layout_model;
use html_layout as layout;
use html_shape as shaping;
mod stages;
pub use html_engine_model::{
    ContentLanguage, FontMetrics, FontMetricsRequest, FontRelativeMetrics, FontSlant, GlyphHandle,
    GlyphId, GlyphMetric, GlyphMetricError, GlyphRequest, GlyphResourceCheckpoint,
    GlyphResourceGeneration, GlyphResourceStore, GlyphResources, GlyphShaper, GlyphSlice,
    PreparedTextRunFragment, ResourceShaper, ShapeError, ShapedGlyph, ShapedLine, ShapedTextRun,
    TextDirection, TextRunId, TextRunShapeRequest, TextShapeRequest, TextStyleSpan,
};
pub use html_layout::LayoutTimings;
pub use stages::{
    DetachedLayout, FormattingDocument, LaidOutDocument, NoteLayout, PreparedDocument,
    ReadingFragment, ReadingTarget, RenderExportView, RenderTable, RenderView, ShapeLayoutTimings,
    ShapedDocument, TextSearchOptions, find_text_matches, text_matches,
};

/// Document lifecycle and render capabilities, orchestrated above the stage crates.
pub mod engine {
    pub use crate::stages::*;
    #[cfg(feature = "profiling")]
    pub use html_engine_model::profiling;
    pub use html_engine_model::{
        CharacterPlacement, ContentLanguage, FontMetrics, FontMetricsRequest, FontRelativeMetrics,
        FontSlant, FontStyle, GlyphHandle, GlyphId, GlyphMetric, GlyphMetricError, GlyphRequest,
        GlyphResourceCheckpoint, GlyphResourceGeneration, GlyphResourceStore, GlyphResources,
        GlyphShaper, GlyphSlice, LanguageTag, LineCharacterPlacement, LineId, LineTextPlacement,
        ListStylePosition, OpenTypeFeature,
        PreparedTextRunFragment, ReaderStyleOverrides, ResourceShaper, ShapeError, ShapedGlyph,
        ShapedLine, ShapedTextRun, StyleStringId, TextAlign, TextDecorationLines, TextDirection,
        TextRunId, TextRunShapeRequest, TextShapeRequest, TextStyleSpan, ThemeAdaptation,
        UnicodeBidi, UsedBorderRadii,
    };
    pub use html_layout::LayoutTimings;
}
#[cfg(test)]
include!("../../../test-support/stage-fixtures.rs");
pub use html_engine_model::{ImageSizingPolicy, NoteFlow, TextCompositionPolicy};
pub use html_engine_model::{ReaderStyleOverrides, TextAlign, ThemeAdaptation};
pub use html_parse::{
    DocumentTextIndex, MarkupSyntax, ParsedHtml, SourceTextPosition, XmlParseError,
    decode_html_bytes, parse_document, parse_html_document, parse_html_document_bytes,
    parse_xml_document, plain_text_from_fragment,
};
pub use html_style::{MediaEnvironment, MediaType};
pub use parser::{
    BookStylesheetCache, BookStylesheetCacheStats, DocumentFactory, PreparationTimings,
    ResourceContext, ResourceDocumentFactory,
};
pub use session::{
    ActivePipelineSession, CacheEntry, OwnedActivePipelineSession, PipelineCacheState,
    PipelineSession,
};
pub use types::{
    EarliestStage, FontEnvironmentRevision, ImageMetricsRevision, LayoutConstraints,
    PaintSettingsRevision, PipelineCacheKey, PipelineChange, PipelineChangeMask, PipelineChanges,
    PipelineError, PipelineInputs, PipelineUpdate, ResourceRevision, Reuse, ReuseReport,
    SourceRevision, StyleCacheEnvironment, StyleEnvironment, StylesheetRevision,
};

#[cfg(test)]
mod stage_boundary_tests;

#[cfg(feature = "testing")]
pub use html_layout::testing as layout_testing;
