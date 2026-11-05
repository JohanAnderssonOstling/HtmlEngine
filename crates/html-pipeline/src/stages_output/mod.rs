use super::DocumentRead;
use crate::layout_model::{
    AnchorPosition, EllipsisFragment, GlyphAdvanceRun, GlyphId, GlyphOffsetRun, HyphenFragment,
    ImageFragment, InlineItem, LayoutMode, ListItemMarker,
};
use html_dom::{Document, ImageResource, ImageSource, NodeRef};
use html_style_model::{ListStylePosition, TextDecorationLines, CornerRadii, Visibility};
use kurbo::{Point, Size};
use rustc_data_structures::fx::FxHashMap;
use std::ops::Range;
use std::sync::Arc;
use html_engine_model::GlyphResourceLease;

pub mod addressing;
pub mod boxes;
pub mod fragments;
pub mod geometry;
pub mod markdown;
pub mod pagination;
pub mod reading;
pub mod search;
pub mod semantics;
pub mod source;
pub mod text;
mod words;
pub use geometry::{CaretAffinity, RenderGeometryView};
pub(super) use html_engine_model::WordIndex;
pub(super) use words::build_word_index;
pub use markdown::{RenderExportView, TableExportFormat, TableExportRequest, TextExport, TextExportFormat};
pub use pagination::{
    Pagination, PaginationGroup, PaginationLine, PaginationPosition, PaginationTarget,
};
pub use pagination as pagination_queries;
pub use reading::{ReadingFragment, ReadingPage, ReadingTarget};
pub use search::{
    SourceTextMatch, SourceTextMatches, TextPatternError, TextPatternMatch, TextPatternMatcher,
    TextSearchOptions, find_text_matches, text_matches,
};
pub use semantics::{ContentRole, ContentSemantics, RenderSemanticsView};
pub use source::SourceLocation;

pub use addressing::{
    DocumentTarget, ElementTarget, LinkTarget, RenderAddressingView, ResolvedDestination,
    SourceElementStep, SourcePosition, TextQuote,
};
pub use boxes as box_queries;
pub use fragments as fragment_paint;
pub use text as text_paint;

pub use boxes::{
    BoxTextFormat, RenderBoxView, RenderForcedBreak, RenderListItemMarker, RenderTable,
    RenderTableCell, RenderTableRow,
};
pub use fragments::{
    RenderDecoration, RenderDecorationPattern, RenderDecorations, RenderFragmentView,
    RenderImageFragment, RenderImageFragments, RenderOverflowClip, RenderPaintItem,
};
pub use html_dom::ImageId;
pub use html_engine_model::{BoxId, ImageMetrics, LineId};
pub use text::{
    RenderAuthoritativeTextRun, RenderEllipsisFragment, RenderGlyphAdvanceRun,
    RenderGlyphAdvanceRuns, RenderGlyphOffsetRun, RenderGlyphOffsetRuns, RenderHyphenFragment,
    RenderLine, RenderLinePlacement, RenderLineTextFragment, RenderLineTextFragments, RenderLines,
    RenderTextRun, RenderTextRuns, RenderTextView, merge_text_ranges,
};

/// Retains appended scope resources and detects replacement of paint products.
#[derive(Clone)]
pub struct RenderResourceLease {
    native: GlyphResourceLease,
    _scope: Option<Arc<()>>,
}

impl RenderResourceLease {
    pub fn is_valid(&self) -> bool {
        self.native.is_valid()
    }
}

#[derive(Clone, Copy)]
pub struct RenderView<'a> {
    pub(super) doc: DocumentRead<'a>,
    pub(super) glyph_metrics: &'a crate::layout_model::GlyphMetrics,
    pub(super) resources: Option<&'a crate::GlyphResourceStore>,
    pub(super) scope_lifetime: Option<&'a Arc<()>>,
}

/// Text and line-layout queries. This capability deliberately excludes box
/// topology, resolved decorations, and document source details.
impl<'a> RenderView<'a> {

    pub fn text(self) -> RenderTextView<'a> {
        RenderTextView {
            doc: self.doc,
            glyph_metrics: self.glyph_metrics,
        }
    }

    pub fn boxes(self) -> RenderBoxView<'a> {
        RenderBoxView {
            doc: self.doc,
            glyph_metrics: self.glyph_metrics,
        }
    }

    pub fn fragments(self) -> RenderFragmentView<'a> {
        RenderFragmentView { doc: self.doc }
    }

    pub fn addressing(self) -> RenderAddressingView<'a> {
        RenderAddressingView {
            doc: self.doc,
            glyph_metrics: self.glyph_metrics,
        }
    }

    /// Semantic selection export for clipboard or file output.
    pub fn export(self) -> RenderExportView<'a> {
        RenderExportView {
            doc: self.doc,
            glyph_metrics: self.glyph_metrics,
        }
    }





    pub fn root_font_size(&self) -> f32 {
        self.doc.inputs.document.root_font_size()
    }



    pub fn title(&self) -> Option<&'a str> {
        self.doc.inputs.document.title()
    }






}

pub use addressing as address_queries;
pub use geometry as geometry_queries;
pub use markdown as export_queries;
pub use reading as reading_queries;
pub use search as search_queries;
pub use semantics as semantic_queries;

pub use source as source_queries;
