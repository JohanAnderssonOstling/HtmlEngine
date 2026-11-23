use super::LayoutInputs;
use super::preparation::{BoxSizeSnapshot, PassContext};
use crate::boxes::model::{ResolvedBoxModel, SizingPolicy};
use html_dom::{Document, DomNodeId};
use html_engine_model::{
    BoxFacts, Children, FontRelativeMetrics, ImageMetrics, LayoutMode, LayoutTree, ListItemMarker,
    ShapedFontMetrics, TableBox,
};
use html_style_model::{
    BorderCollapseMode, ComputedStyles, Display, LayoutStyle, StyleIndices, TextDirection,
    UsedStyleView,
};

use std::ops::Deref;

/// Style and topology access shared by layout and paint, without layout caches.
#[derive(Clone, Copy)]
pub(crate) struct StyleReader<'input> {
    document: &'input Document,
    styles: &'input ComputedStyles,
    topology: &'input LayoutTree,
    font_metrics: &'input ShapedFontMetrics,
    resolved_styles: &'input [UsedStyleView<'input>],
}

#[derive(Clone, Copy)]
pub(crate) struct LayoutReader<'input> {
    pub(crate) shared: StyleReader<'input>,
    image_metrics: &'input ImageMetrics,
    pass_context: &'input PassContext<'input>,
}

impl<'input> LayoutReader<'input> {
    pub(crate) fn new(
        inputs: &LayoutInputs<'input>,
        pass_context: &'input PassContext<'input>,
    ) -> Self {
        Self {
            shared: StyleReader::new(inputs, &pass_context.styles),
            image_metrics: inputs.image_metrics,
            pass_context,
        }
    }

    pub(crate) fn has_tracking_spacing(&self) -> bool {
        self.pass_context.tracking_spacing.is_some()
    }

    /// Boundary values are shared by intrinsic sizing and final token construction.
    pub(crate) fn tracking_spacing_after(&self, glyph: u32, owner: usize) -> f64 {
        self.pass_context
            .tracking_spacing
            .as_ref()
            .map_or(0.0, |spacing| {
                f64::from(
                    spacing
                        .get(&glyph)
                        .copied()
                        .unwrap_or_else(|| self.style(owner).letter_spacing()),
                )
            })
    }

    pub(crate) fn style_ref(&self, box_idx: usize) -> &'input UsedStyleView<'input> {
        &self.pass_context.styles[box_idx]
    }

    pub(crate) fn sizes(&self, box_idx: usize) -> &'input BoxSizeSnapshot {
        &self.pass_context.sizes[self.pass_context.size_indices[box_idx] as usize]
    }

    pub fn box_model(&self, box_idx: usize, containing_width: f64) -> ResolvedBoxModel {
        if containing_width.to_bits() == 0 {
            self.pass_context.box_models[self.pass_context.box_model_indices[box_idx] as usize]
        } else {
            ResolvedBoxModel::new(self.style(box_idx), containing_width)
        }
    }

    pub(crate) fn adjust_vertical_margins(
        &self,
        box_idx: usize,
        top: f64,
        bottom: f64,
    ) -> (f64, f64) {
        self.pass_context
            .paragraph_spacing
            .resolve(box_idx, top, bottom)
    }

    pub fn reader_text_indent(&self, box_idx: usize) -> Option<f64> {
        self.pass_context.paragraph_spacing.indent(box_idx)
    }

    pub fn reader_paragraph_gap(&self, box_idx: usize) -> Option<f64> {
        self.pass_context.paragraph_spacing.gap(box_idx)
    }

    pub fn is_reader_prose(&self, box_idx: usize) -> bool {
        self.pass_context.paragraph_spacing.is_prose(box_idx)
    }

    pub fn vertical_margins(&self, box_idx: usize, containing_width: f64) -> (f64, f64) {
        let style = self.style(box_idx);
        self.pass_context.paragraph_spacing.resolve(
            box_idx,
            style.margin_top().resolve(containing_width),
            style.margin_bottom().resolve(containing_width),
        )
    }

    pub fn image_display_size(&self, image_idx: u32) -> (f64, f64) {
        self.image_metrics.display_size(
            image_idx,
            self.document
                .image(image_idx)
                .expect("image index should be valid"),
        )
    }

    pub(crate) fn percentage_height_image_children(&self, box_idx: usize) -> &[usize] {
        self.topology
            .get_box_dom_element(box_idx)
            .and_then(|node| self.pass_context.percentage_height_images.get(&node))
            .map_or(&[], Vec::as_slice)
    }
}

impl<'input> Deref for LayoutReader<'input> {
    type Target = StyleReader<'input>;
    fn deref(&self) -> &Self::Target {
        &self.shared
    }
}

impl<'input> StyleReader<'input> {
    pub(crate) fn new(
        inputs: &LayoutInputs<'input>,
        resolved_styles: &'input [UsedStyleView<'input>],
    ) -> Self {
        Self {
            document: inputs.document,
            styles: inputs.styles,
            topology: inputs.topology,
            font_metrics: inputs.font_metrics,
            resolved_styles,
        }
    }

    pub fn document(&self) -> &'input Document {
        self.document
    }

    /// Geometry reads never consult the reader palette.
    pub fn style(&self, box_idx: usize) -> UsedStyleView<'input> {
        self.resolved_styles[box_idx]
    }

    pub fn background_can_change(&self, box_idx: usize) -> bool {
        let indices = self.resolved_style_indices(box_idx);
        self.style(box_idx).background_color() & 0xff != 0
            || self
                .styles
                .background_style(indices)
                .is_some_and(|background| background.background_color_current_color)
            || self.topology.root_box() == Some(box_idx)
            || self.topology.body_box() == Some(box_idx)
    }

    pub fn tracks_overflow_clips(&self) -> bool {
        self.topology.tracks_overflow_clips
    }

    pub fn has_floats(&self) -> bool {
        self.topology.has_floats
    }

    pub fn used_style(&self, indices: StyleIndices) -> UsedStyleView<'input> {
        self.font_metrics
            .used_non_box_style(self.styles, indices)
            .expect("validated style handle")
    }

    pub fn font_metrics(&self, box_idx: usize) -> FontRelativeMetrics {
        self.font_metrics.for_box(box_idx)
    }

    pub fn style_font_metrics(&self, indices: StyleIndices) -> FontRelativeMetrics {
        self.font_metrics.for_non_box_style(indices)
    }

    pub fn styles(&self) -> &'input ComputedStyles {
        self.styles
    }

    pub fn layout_tree(&self) -> &'input LayoutTree {
        self.topology
    }

    pub fn box_dom_element(&self, box_idx: usize) -> Option<DomNodeId> {
        self.topology.get_box_dom_element(box_idx)
    }

    pub fn image_idx(&self, box_idx: usize) -> Option<u32> {
        self.topology.get_box_image_idx(box_idx)
    }

    pub fn box_style_indices(&self, box_idx: usize) -> Option<StyleIndices> {
        self.topology.get_box_style_indices(box_idx)
    }

    fn resolved_style_indices(&self, box_idx: usize) -> StyleIndices {
        self.box_style_indices(box_idx)
            .unwrap_or_else(|| self.styles.default_indices())
    }

    pub fn layout_style(&self, box_idx: usize) -> &'input LayoutStyle {
        let indices = self.resolved_style_indices(box_idx);
        self.styles
            .layout_style(indices)
            .expect("validated style handle")
    }

    pub fn inline_fragment_edges(&self, box_idx: usize) -> (bool, bool) {
        self.topology.inline_fragment_edges(box_idx)
    }

    pub fn split_inline_position_ancestors(&self, box_idx: usize) -> &[StyleIndices] {
        self.topology.split_inline_position_ancestors(box_idx)
    }

    pub fn box_count(&self) -> usize {
        self.topology.box_count()
    }

    pub fn box_layout_mode(&self, box_idx: usize) -> Option<&'input LayoutMode> {
        self.topology.get_box_layout_mode(box_idx)
    }

    pub fn box_table(&self, box_idx: usize) -> Option<&'input TableBox> {
        self.topology.get_box_table(box_idx)
    }

    pub fn get_parent(&self, box_idx: usize) -> Option<usize> {
        self.topology.get_box_parent(box_idx)
    }

    pub(crate) fn parent_direction(&self, box_idx: usize) -> Option<TextDirection> {
        self.get_parent(box_idx)
            .map(|parent| self.style(parent).direction())
    }

    pub fn root_box(&self) -> Option<usize> {
        self.topology.root_box()
    }

    pub fn body_box(&self) -> Option<usize> {
        self.topology.body_box()
    }

    pub fn box_facts(&self, box_idx: usize) -> &BoxFacts {
        &self.topology.boxes[box_idx].facts
    }

    pub fn sizing_policy(&self, box_idx: usize) -> SizingPolicy {
        let (width_is_minimum, height_is_minimum, ignores_inline_sizes) =
            match self.box_layout_mode(box_idx) {
                Some(LayoutMode::Table(_)) => (true, true, false),
                Some(LayoutMode::TableRow(_)) => (false, true, false),
                Some(LayoutMode::TableCell(_)) => (false, true, true),
                _ => (false, false, false),
            };
        SizingPolicy {
            width_is_minimum,
            height_is_minimum,
            ignores_inline_sizes,
            owns_edges: (width_is_minimum
                || matches!(
                    self.box_facts(box_idx).display,
                    Display::Table | Display::InlineTable
                ))
                && self.style(box_idx).border_collapse() == BorderCollapseMode::Collapse,
        }
    }

    pub fn is_table_cell_box(&self, box_idx: usize) -> bool {
        matches!(
            self.box_layout_mode(box_idx),
            Some(LayoutMode::TableCell(_))
        )
    }

    pub fn is_block_box(&self, box_idx: usize) -> bool {
        matches!(self.box_layout_mode(box_idx), Some(LayoutMode::Block(_)))
    }

    pub fn is_flex_grid_box(&self, box_idx: usize) -> bool {
        matches!(
            self.box_layout_mode(box_idx),
            Some(LayoutMode::Flex(_) | LayoutMode::Grid(_))
        )
    }

    pub fn is_anonymous_box(&self, box_idx: usize) -> bool {
        matches!(
            self.box_layout_mode(box_idx),
            Some(LayoutMode::Anonymous(_))
        )
    }

    pub fn is_table_box(&self, box_idx: usize) -> bool {
        matches!(self.box_layout_mode(box_idx), Some(LayoutMode::Table(_)))
    }

    pub fn is_inline_box(&self, box_idx: usize) -> bool {
        matches!(self.box_layout_mode(box_idx), Some(LayoutMode::Inline(_)))
    }

    pub fn is_only_block_child(&self, box_idx: usize) -> bool {
        let Some(parent) = self.get_parent(box_idx) else {
            return false;
        };
        matches!(
            self.box_layout_mode(parent),
            Some(LayoutMode::Block(Children::Blocks(children))) if self.topology.child_slice(children) == [box_idx as u32]
        )
    }

    pub fn list_marker(&self, box_idx: usize) -> Option<ListItemMarker> {
        self.topology.list_marker(box_idx)
    }

    pub fn box_uses_float_context(&self, box_idx: usize) -> bool {
        self.is_table_box(box_idx) || self.box_facts(box_idx).formatting_context
    }
}
