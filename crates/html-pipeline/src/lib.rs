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
    CapsSynthesis, ContentLanguage, FontFeatures, FontMetrics, FontRelativeMetrics, FontSlant,
    ObliqueAngle, GlyphHandle, GlyphId, GlyphMetric, GlyphRequest, GlyphResourceCheckpoint,
    GlyphResourceGeneration, GlyphResourceStore, GlyphResources, GlyphShaper, GlyphSlice,
    CursiveJoining, PreparedTextRunFragment, ResourceShaper, RunFontMetrics, ShapeError,
    ShapedGlyph, ShapedLine, ShapedRun, ShapedTextRun, ShapingStyle, TextDirection, TextRunId,
    TextRunShapeRequest, TextStyleSpan,
};
pub use html_layout::LayoutTimings;
pub use html_engine_model::NativeResourceCheckpoint;
pub use stages::{
    DetachedLayout, FormattingDocument, LaidOutDocument, LayoutConstraintError, LayoutRequest,
    NoteLayout, PreparedDocument, ReadingFragment, ReadingTarget, RenderExportView, RenderTable,
    RenderResourceLease, RenderView, ShapeLayoutTimings, ShapedDocument, TextPatternMatch,
    TextPatternMatcher, TextSearchOptions, find_text_matches, text_matches,
};

/// Document lifecycle and render capabilities, orchestrated above the stage crates.
pub mod engine {
    pub use crate::stages::*;
    #[cfg(feature = "profiling")]
    pub use html_engine_model::profiling;
    pub use html_engine_model::{
        CapsSynthesis, FontFeatures, CaretCursor, CaretIter, CaretPositions, ContentLanguage,
        CornerRadii, FontMetrics, FontRelativeMetrics, FontSlant, ObliqueAngle, FontStyle,
        GlyphHandle, GlyphId, GlyphMetric, GlyphRequest, GlyphResourceCheckpoint,
        GlyphResourceGeneration, GlyphResourceStore, GlyphResources, GlyphShaper, GlyphSlice,
        CursiveJoining, LanguageTag, LineCharacterPlacement, LineId, LineTextPlacement,
        ListStylePosition, NativeResourceCheckpoint, OpenTypeFeature, OpenTypeFeatureKind,
        PreparedTextRunFragment, ReaderStyleOverrides, ResourceShaper, RunFontMetrics, ShapeError,
        ShapedGlyph, ShapedLine, ShapedRun, ShapedTextRun, ShapingStyle, StyleStringId, TextAlign,
        TextDecorationLines, TextDirection, TextRunId, TextRunShapeRequest, TextStyleSpan,
        ThemeAdaptation, UnicodeBidi,
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
    ActivePipelineSession, OwnedActivePipelineSession,
    PipelineSession,
};
pub use types::{
    EarliestStage, FontEnvironmentRevision, ImageMetricsRevision, LayoutConstraints,
    PaintSettingsRevision, PipelineChanges, PipelineError, PipelineErrorKind, PipelineInputs,
    PipelineUpdate, ResourceRevision, Reuse, ReuseReport, SourceRevision, StyleEnvironment,
    StylesheetRevision,
};

#[cfg(test)]
mod stage_boundary_tests;

#[cfg(feature = "testing")]
pub use html_layout::testing as layout_testing;
