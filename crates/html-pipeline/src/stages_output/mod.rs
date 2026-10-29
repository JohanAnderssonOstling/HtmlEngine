use super::DocumentRead;
use crate::layout_model::{
    AnchorPosition, DecorationFragment, DecorationStore, EllipsisFragment, GlyphAdvanceRun,
    GlyphId, GlyphOffsetRun, HyphenFragment, ImageFragment, InlineItem, LayoutMode,
    Line, ListItemMarker,
};
use html_dom::{Document, ImageResource, ImageSource, NodeRef};
use html_style_model::{ListStylePosition, TextDecorationLines, UsedBorderRadii};
use kurbo::{Point, Size};
use rustc_data_structures::fx::FxHashMap;
use std::ops::Range;

mod addressing;
mod boxes;
mod fragments;
mod geometry;
mod markdown;
mod reading;
mod search;
mod semantics;
mod source;
mod text;
pub use geometry::{CaretAffinity, PositionedShapedLine, RenderGeometryView};
pub use markdown::RenderExportView;
pub use reading::{ReadingFragment, ReadingTarget, is_navigation_section_title};
pub use search::{TextSearchOptions, find_text_matches, text_matches};
pub use semantics::{ContentRole, ContentSemantics, RenderSemanticsView};
pub use source::SourceLocation;

pub use addressing::{
    RenderAddressingView, RenderAnchorPosition, RenderAnchorPositions, SourceElementStep,
    SourcePosition,
};
pub use boxes::{
    BoxTextFormat, RenderBoxView, RenderForcedBreak, RenderListItemMarker, RenderTable,
    RenderTableCell, RenderTableRow,
};
pub use fragments::{
    RenderDecoration, RenderDecorationPattern, RenderDecorations, RenderFragmentView,
    RenderImageFragment, RenderImageFragments, RenderOverflowClip, RenderPaintItem,
};
pub use html_engine_model::ImageMetrics;
pub use text::{
    RenderAuthoritativeTextRun, RenderEllipsisFragment, RenderGlyphAdvanceRun,
    RenderGlyphAdvanceRuns, RenderGlyphOffsetRun, RenderGlyphOffsetRuns, RenderHyphenFragment,
    RenderLine, RenderLineTextFragment, RenderLineTextFragments, RenderLines, RenderTextRun,
    RenderTextRuns, RenderTextView, merge_text_ranges,
};

#[derive(Clone, Copy)]
pub struct RenderView<'a> {
    pub(super) doc: DocumentRead<'a>,
    pub(super) glyph_metrics: &'a crate::layout_model::GlyphMetrics,
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

    pub fn semantics(self) -> RenderSemanticsView<'a> {
        RenderSemanticsView {
            doc: self.doc,
            glyph_metrics: self.glyph_metrics,
        }
    }

    pub fn geometry(self) -> RenderGeometryView<'a> {
        RenderGeometryView {
            doc: self.doc,
            glyph_metrics: self.glyph_metrics,
        }
    }

    pub fn root_font_size(&self) -> f32 {
        self.doc.inputs.document.root_font_size()
    }

    /// The CSS canvas background color, after root/body propagation.
    ///
    /// A root background suppresses body propagation even when its only
    /// drawable layer is an image that this renderer cannot paint yet.
    pub fn canvas_background_color(&self) -> Option<u32> {
        let root_box = self.doc.inputs.layout_tree.root_box()?;
        let root_indices = self
            .doc
            .inputs
            .layout_tree
            .get_box_style_indices(root_box)?;
        let root = self.doc.inputs.styles.view(root_indices)?;
        let root_color = self
            .doc
            .inputs
            .paint
            .box_colors(root_box)
            .map_or_else(|| root.background_color(), |paint| paint.background);
        if root.background_image_present() || root_color & 0xFF != 0 {
            return (root.visibility() == html_style_model::Visibility::Visible
                && root_color & 0xFF != 0)
                .then_some(root_color);
        }

        let body_box = self.doc.inputs.layout_tree.body_box()?;
        let body_indices = self
            .doc
            .inputs
            .layout_tree
            .get_box_style_indices(body_box)?;
        let body = self.doc.inputs.styles.view(body_indices)?;
        let body_color = self
            .doc
            .inputs
            .paint
            .box_colors(body_box)
            .map_or_else(|| body.background_color(), |paint| paint.background);
        (body.visibility() == html_style_model::Visibility::Visible && body_color & 0xFF != 0)
            .then_some(body_color)
    }

    pub fn title(&self) -> Option<&'a str> {
        self.doc.inputs.document.title()
    }

    pub fn images(&self) -> &'a [ImageResource] {
        self.doc.inputs.document.images()
    }

    /// Preserved source for equation-specific actions; ordinary images return None.
    pub fn image_mathml(&self, image_idx: u32) -> Option<&'a str> {
        match &self.images().get(image_idx as usize)?.source {
            ImageSource::MathMl { markup, .. } => Some(markup),
            _ => None,
        }
    }

    pub fn image_uri(&self, image_idx: u32) -> Option<&'a str> {
        match &self
            .doc
            .inputs
            .document
            .images()
            .get(image_idx as usize)?
            .source
        {
            ImageSource::Uri(uri) => Some(uri.as_str()),
            ImageSource::InlineSvg { .. } | ImageSource::MathMl { .. } => None,
        }
    }

    pub fn string(&self, index: u16) -> &'a str {
        self.doc.inputs.document.string(index)
    }

    pub fn style_string(&self, index: html_style_model::StyleStringId) -> Option<&'a str> {
        self.doc.inputs.styles.string(index)
    }

    pub fn lookup_string(&self, value: &str) -> Option<u16> {
        self.doc.inputs.document.lookup_string(value)
    }
}
