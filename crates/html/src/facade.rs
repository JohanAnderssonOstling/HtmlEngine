//! Deliberate public contracts. Public methods on re-exported types are also
//! application contracts; implementation modules stay outside this facade.
//! Each exported contract has one canonical module:
//! document addresses, resources, CSS, text backends, layout products,
//! or painting. Pipeline orchestration stays inside the engine.

pub mod document {

    pub use html_dom::{ImageId, RootFontSize, SourceElementStep, SourcePosition};
    pub use html_parse::SourceTextPosition;
}

pub mod resources {
    pub use html_resources::{
        BlockingJob, BlockingJobSpawner, DecodedImages, FileSystemProvider, ImageLoader,
        ImageService, InlineJobSpawner, ResourceMetadata, ResourceProvider,
    };
}

/// Reader preferences applied over the publication's authored stylesheets.
pub mod style {
    pub use html_style_model::{ReaderStyleOverrides, ThemeAdaptation};
}

pub mod text {
    pub use html_pipeline::engine::TextSearchOptions;
    pub use html_pipeline::engine::{
        FontMetrics, FontSlant, GlyphHandle, GlyphRequest, GlyphResourceGeneration, GlyphShaper,
        ShapeError, ShapedGlyph,
    };
    pub use html_style_model::FontFace;
}

pub mod layout {
    pub(crate) use html_pipeline::engine::{
        CaretAffinity, ContentRole, ContentSemantics, RenderForcedBreak, merge_text_ranges,
    };
    pub use html_pipeline::engine::{
        BoxId, ImageSizingPolicy, LineId, NoteFlow, RenderAddressingView, RenderBoxView,
        RenderExportView, RenderFragmentView, RenderLine, RenderLines, RenderTextView, RenderView,
        SourceLocation, TextCompositionPolicy,
    };
}

/// Framework-neutral painting and recorded scenes.
pub mod render {
    pub use crate::overlays::{OverlayPaintScratch, OverlayStyle, TextOverlay};
    pub(crate) use crate::overlays::paint_overlays;
    pub use html_render_core::{
        DocumentPainter, LinePlacement, PaintCandidates, PaintFragment, PaintProjection, Painter,
        ProjectedLine, RenderScene, SceneImagePolicy,
    };
}
