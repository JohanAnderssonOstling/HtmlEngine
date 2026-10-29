//! Public styling inputs, timing records, and immutable styled-document output.
use crate::style::source::fonts::FontFace;
use html_dom::{Document, DomNodeId};
use html_style_model::{
    Background, Border, BorderRadii, BoxModel, ComputedStyles, Font, InheritedText, StyleIndices,
    StyleView,
};
use std::sync::Arc;

/// A DOM whose cascade has been resolved.
///
/// The wrapped document is deliberately immutable. Changing the DOM or its
/// style inputs requires consuming this value with [`Self::into_unstyled`],
/// which prevents layout data from silently outliving the styles it used.
pub struct StyledDocument {
    pub(super) document: Document,
    pub(super) styles: ComputedStyles,
    pub(super) font_faces: Arc<[FontFace]>,
}

impl StyledDocument {
    /// Active `@font-face` declarations in stylesheet order. Relative URLs
    /// remain relative when styling CSS directly; the pipeline resolves them.
    pub fn font_faces(&self) -> &[FontFace] {
        &self.font_faces
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn styles(&self) -> &ComputedStyles {
        &self.styles
    }

    pub fn style_for_node(&self, node_idx: DomNodeId) -> Option<StyleIndices> {
        self.styles.style_for_node(node_idx)
    }

    pub fn style_view(&self, indices: StyleIndices) -> Option<StyleView<'_>> {
        self.styles.view(indices)
    }

    pub fn font_style(&self, indices: StyleIndices) -> Option<&Font> {
        self.styles.font_style(indices)
    }

    pub fn text_style(&self, indices: StyleIndices) -> Option<&InheritedText> {
        self.styles.text_style(indices)
    }

    pub fn box_model_style(&self, indices: StyleIndices) -> Option<&BoxModel> {
        self.styles.box_model_style(indices)
    }

    pub fn border_style(&self, indices: StyleIndices) -> Option<&Border> {
        self.styles.border_style(indices)
    }

    pub fn border_radii_style(&self, indices: StyleIndices) -> Option<&BorderRadii> {
        self.styles.border_radii_style(indices)
    }

    pub fn background_style(&self, indices: StyleIndices) -> Option<&Background> {
        self.styles.background_style(indices)
    }

    pub fn into_unstyled(self) -> Document {
        self.document
    }

    pub fn into_parts(self) -> (Document, ComputedStyles) {
        (self.document, self.styles)
    }
}

#[derive(Clone, Copy)]
pub struct AuthorStylesheetInput<'a> {
    pub css: &'a str,
    /// The element that owns this stylesheet. It becomes the root for an
    /// `@scope` rule whose prelude is omitted.
    pub implicit_scope_root: Option<html_dom::DomNodeId>,
}
