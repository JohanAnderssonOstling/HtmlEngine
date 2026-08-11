use super::{AncestorIter, LaidOutDocument};
use crate::layout_model::{AnchorPosition, DecorationFragment, DecorationStore, EllipsisFragment, GlyphAdvanceRun, GlyphId, GlyphOffsetRun, HyphenFragment, ImageFragment, InlineItem, InlineItemKind, LayoutMode, Line, ListItemMarker};
use html_dom::{Document, ImageResource, NodeRef};
use html_style_model::{ListStylePosition, TextDecorationLines, UsedBorderRadii};
use kurbo::{Point, Size};
use rustc_data_structures::fx::FxHashMap;
use std::ops::Range;

mod addressing;
mod boxes;
mod fragments;
mod text;

pub use addressing::{RenderAddressingView, RenderAnchorPosition, RenderAnchorPositions, SourceElementStep, SourcePosition};
pub use boxes::{BoxTextFormat, RenderBoxView, RenderForcedBreak, RenderListItemMarker, RenderTable, RenderTableCell, RenderTableRow};
pub use fragments::{RenderDecoration, RenderDecorationPattern, RenderDecorations, RenderFragmentView, RenderImageFragment, RenderImageFragments, RenderOverflowClip};
pub use text::{
    RenderAuthoritativeTextRun, RenderEllipsisFragment, RenderGlyphAdvanceRun, RenderGlyphAdvanceRuns, RenderGlyphOffsetRun, RenderGlyphOffsetRuns, RenderHyphenFragment, RenderLine, RenderLineTextFragment, RenderLineTextFragments,
    RenderLines, RenderTextRun, RenderTextRuns, RenderTextView,
};
#[derive(Clone, Debug, Default)]
pub struct ImageMetrics {
    sizes: Vec<(u32, u32)>,
}

impl ImageMetrics {
    pub(super) fn from_document(document: &Document) -> Self {
        let mut sizes = Vec::with_capacity(document.images().len());
        for image in document.images() {
            sizes.push((image.width, image.height));
        }
        Self { sizes }
    }

    pub fn set(&mut self, image_idx: u32, width: u32, height: u32) {
        let idx = image_idx as usize;
        if self.sizes.len() <= idx {
            self.sizes.resize(idx + 1, (0, 0));
        }
        self.sizes[idx] = (width, height);
    }

    pub fn get(&self, image_idx: u32) -> Option<(u32, u32)> {
        self.sizes.get(image_idx as usize).copied()
    }

    pub(crate) fn display_size(&self, image_idx: u32, resource: &ImageResource) -> (f64, f64) {
        let (width, height) = self.get(image_idx).unwrap_or((resource.width, resource.height));
        // HTML dimension attributes are CSS presentational hints, not natural
        // image dimensions. They are resolved by the style/layout sizing path;
        // folding them into the decoded metrics here would also change the
        // intrinsic aspect ratio and apply the same author input twice.
        (width.max(1) as f64, height.max(1) as f64)
    }

    pub(super) fn memory_usage_bytes(&self) -> usize {
        self.sizes.capacity() * std::mem::size_of::<(u32, u32)>()
    }

    pub(super) fn len(&self) -> usize {
        self.sizes.len()
    }
}

#[cfg(test)]
mod image_metrics_tests {
    use super::ImageMetrics;
    use html_dom::{ImageResource, ImageSource};

    #[test]
    fn html_dimensions_do_not_replace_decoded_intrinsic_metrics() {
        let resource = ImageResource { source: ImageSource::Uri("image.png".to_owned()), width: 320, height: 180, width_attr: Some(10), height_attr: Some(20) };
        let mut metrics = ImageMetrics::default();

        assert_eq!(metrics.display_size(0, &resource), (320.0, 180.0));
        metrics.set(0, 640, 360);
        assert_eq!(metrics.display_size(0, &resource), (640.0, 360.0));
    }
}

#[derive(Clone, Copy)]
pub struct RenderView<'a> {
    pub(super) doc: &'a LaidOutDocument,
}

/// Text and line-layout queries. This capability deliberately excludes box
/// topology, resolved decorations, and document source details.
impl<'a> RenderView<'a> {
    pub fn text(self) -> RenderTextView<'a> {
        RenderTextView { doc: self.doc }
    }

    pub fn boxes(self) -> RenderBoxView<'a> {
        RenderBoxView { doc: self.doc }
    }

    pub fn fragments(self) -> RenderFragmentView<'a> {
        RenderFragmentView { doc: self.doc }
    }

    pub fn addressing(self) -> RenderAddressingView<'a> {
        RenderAddressingView { doc: self.doc }
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
        let root_indices = self.doc.inputs.layout_tree.get_box_style_indices(root_box)?;
        let root = self.doc.inputs.styles.view(root_indices)?;
        if root.background_image_present() || root.background_color() & 0xFF != 0 {
            return (root.visibility() == html_style_model::Visibility::Visible && root.background_color() & 0xFF != 0).then_some(root.background_color());
        }

        let body_box = self.doc.inputs.layout_tree.body_box()?;
        let body_indices = self.doc.inputs.layout_tree.get_box_style_indices(body_box)?;
        let body = self.doc.inputs.styles.view(body_indices)?;
        (body.visibility() == html_style_model::Visibility::Visible && body.background_color() & 0xFF != 0).then_some(body.background_color())
    }

    pub fn title(&self) -> Option<&'a str> {
        self.doc.inputs.document.title()
    }

    pub fn images(&self) -> &'a [html_dom::ImageResource] {
        self.doc.inputs.document.images()
    }

    pub fn image_uri(&self, image_idx: u32) -> Option<&'a str> {
        match &self.doc.inputs.document.images().get(image_idx as usize)?.source {
            html_dom::ImageSource::Uri(uri) => Some(uri.as_str()),
            html_dom::ImageSource::Inline(_) => None,
        }
    }

    pub fn document_toc_entries(&self) -> &'a [html_dom::DocumentTocNode] {
        self.doc.inputs.document.document_toc_entries()
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
