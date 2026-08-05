#[path = "parser/mod.rs"]
mod parser;
mod session;
mod types;

pub use html_dom::RootFontSize;
pub use html_layout::PreparedDocument;
pub use html_layout::{ImageSizingPolicy, TextCompositionPolicy};
pub use html_layout::{ReaderStyleOverrides, TextAlign};
pub use html_parse::{MarkupSyntax, ParsedHtml, XmlParseError, decode_html_bytes, parse_document, parse_html_document, parse_html_document_bytes, parse_xml_document, plain_text_from_fragment};
pub use html_style::{MediaEnvironment, MediaType};
pub use parser::{BookStylesheetCache, BookStylesheetCacheStats, BuildPipelineTimings, DocumentFactory};
pub use session::{PipelineCacheState, PipelineSession};
pub use types::{
    EarliestStage, FontEnvironmentRevision, ImageMetricsRevision, LayoutConstraints, PaintSettingsRevision, PipelineCacheKey, PipelineChange, PipelineChangeMask, PipelineError, PipelineInputs, PipelineRetainedBytes, PipelineStageCounts,
    PipelineTimings, PipelineUpdate, ResourceRevision, Reuse, ReuseReport, SourceRevision, StyleCacheEnvironment, StyleEnvironment, StylesheetRevision,
};
