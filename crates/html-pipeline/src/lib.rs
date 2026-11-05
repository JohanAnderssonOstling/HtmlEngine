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
// Engine contracts are published once, under `engine`; root aliases serve
// crate-internal paths only.
#[allow(unused_imports)]
pub(crate) use html_engine_model::{
    CapsSynthesis, ContentLanguage, FontFeatures, FontMetrics, FontRelativeMetrics,
    FontLineMetrics, FontSlant, ObliqueAngle, GlyphHandle, GlyphId, GlyphMetric, GlyphRequest,
    GlyphResourceCheckpoint, GlyphResourceGeneration, GlyphResourceStore, GlyphResources,
    GlyphWriter, GlyphShaper, GlyphSlice, CursiveJoining, PreparedTextRunFragment, ResourceShaper,
    RunFontMetrics, ShapeError, ShapedGlyph, ShapedRun, ShapedTextRun, ShapingStyle, TextDirection,
    TextRunId, TextRunShapeRequest,
};
#[allow(unused_imports)]
pub(crate) use html_layout::LayoutTimings;
pub(crate) use html_engine_model::NativeResourceCheckpoint;
#[allow(unused_imports)]
pub(crate) use stages::{
    DetachedLayout, FormattingDocument, LaidOutDocument, LayoutConstraintError, LayoutRequest,
    NoteLayout, PreparedDocument, ReadingFragment, ReadingTarget, RenderExportView, RenderTable,
    RenderResourceLease, RenderView, ShapeLayoutTimings, ShapedDocument, TextPatternMatch,
    TextPatternMatcher, TextSearchOptions, find_text_matches, text_matches,
};

/// Document lifecycle and render capabilities, orchestrated above the stage crates.
pub mod engine {
    pub use html_engine_model::{layout_state, shaping_contracts};
    pub use html_style_model::{computed_style, open_type, language};
    pub use html_dom::{image_id_new, image_id_raw, root_font_size_new};
    pub use crate::stages::*;
    pub use html_dom::ImageId;
    #[cfg(feature = "profiling")]
    pub use html_engine_model::profiling;
    pub use html_engine_model::{
        BoxId, CapsSynthesis, FontFeatures, CaretCursor, CaretIter, CaretPositions, CaretPositionsBuilder, ContentLanguage,
        CornerRadii, FontMetrics, FontRelativeMetrics, FontLineMetrics, FontSlant, ObliqueAngle,
        FontStyle, GlyphHandle, GlyphId, GlyphMetric, GlyphRequest, GlyphResourceCheckpoint,
        GlyphResourceGeneration, GlyphResourceStore, GlyphResources, GlyphWriter, GlyphShaper,
        GlyphSlice, CursiveJoining, LanguageTag, LineCharacterPlacement, LineId,
        ListStylePosition, NativeResourceCheckpoint, OpenTypeFeature, OpenTypeFeatureKind,
        PreparedTextRunFragment, ReaderStyleOverrides, ResourceShaper, RunFontMetrics, ShapeError,
        ShapedGlyph, ShapedRun, ShapedTextRun, ShapingStyle, StyleStringId, TextAlign,
        TextDecorationLines, TextDirection, TextRunId, TextRunShapeRequest, ThemeAdaptation,
        UnicodeBidi,
    };

    pub use html_layout::LayoutTimings;
}
#[cfg(test)]
include!("../../../test-support/stage-fixtures.rs");
#[allow(unused_imports)]
pub(crate) use html_engine_model::{ImageSizingPolicy, NoteFlow, TextCompositionPolicy};
#[allow(unused_imports)]
pub(crate) use html_engine_model::{ReaderStyleOverrides, TextAlign, ThemeAdaptation};
pub use html_parse::{
    DocumentTextIndex, MarkupSyntax, ParsedHtml, SourceTextPosition, XmlParseError,
    decode_html_bytes, markup_syntax_for_resource, parse_document, parse_html_document, parse_html_document_bytes,
    parse_xml_document, plain_text_from_fragment,
};
pub use html_style::{MediaEnvironment, MediaType};
pub use parser::{
    BookStylesheetCache, BookStylesheetCacheStats, DocumentFactory, PreparationTimings,
    ResourceContext, ResourceDocumentFactory,
};
pub use session::{ActivePipelineSession, PipelineSession};
pub use types::{
    EarliestStage, PipelineChanges, PipelineError, PipelineErrorKind, PipelineInputs,
    ResourceRevision, StyleEnvironment,
};
// Revisions are assigned by the session from reported changes.
#[allow(unused_imports)]
pub(crate) use types::{
    FontEnvironmentRevision, ImageMetricsRevision, SourceRevision, StylesheetRevision,
};

#[cfg(test)]
mod stage_boundary_tests;

#[cfg(feature = "testing")]
pub use html_layout::testing as layout_testing;
