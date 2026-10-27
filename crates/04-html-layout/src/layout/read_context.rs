use super::block::ParagraphSpacing;
use crate::layout_model::{
    Children, InlineContent, InlineItemKind, LayoutBox, LayoutMode, LayoutTree,
    ListItemMarker,
};
use crate::layout_model::{FontRelativeMetrics, ShapedFontMetrics};
use html_dom::Document;
use html_style_model::{
    ComputedStyles, Float, LayoutStyle, PositionMode, StyleIndices, UsedStyleView,
};
use kurbo::Size;

#[derive(Clone, Copy, Debug)]
pub struct ImageIntrinsic {
    pub size: Size,
    pub aspect_ratio: Option<f64>,
}

/// Shared read-only access to the document, styled layout topology, and
/// renderer-supplied metrics used during layout.
#[derive(Clone, Copy)]
pub struct LayoutReader<'input> {
    document: &'input Document,
    styles: &'input ComputedStyles,
    topology: &'input LayoutTree,
    inline_content: &'input InlineContent,
    font_metrics: &'input ShapedFontMetrics,
    image_metrics: &'input crate::ImageMetrics,
    box_styles: &'input [UsedStyleView<'input>],
    paragraph_spacing: &'input ParagraphSpacing,
    tracks_overflow_clips: bool,
    paint: Option<html_engine_model::PaintView<'input>>,
}

/// Layout-ready styles for boxes in topology order.
///
/// Computed styles and selected font metrics are immutable for the lifetime of
/// a shaped document. Joining them at every property read made the boundary
/// between shaping and layout needlessly expensive. Build that join once when
/// a layout session starts. This does not move value resolution: percentages
/// and other containing-block-dependent values remain symbolic inside
/// `UsedStyleView` until the owning layout algorithm supplies their basis.
pub(super) struct BoxStyleSnapshot<'input> {
    styles: Vec<UsedStyleView<'input>>,
    paragraph_spacing: ParagraphSpacing,
    tracks_overflow_clips: bool,
}

impl<'input> BoxStyleSnapshot<'input> {
    pub(super) fn new(
        document: &Document,
        inline_content: &InlineContent,
        styles: &'input ComputedStyles,
        topology: &'input LayoutTree,
        font_metrics: &'input ShapedFontMetrics,
    ) -> Self {
        let mut resolved = Vec::with_capacity(topology.box_count());
        let mut tracks_overflow_clips = false;
        for box_idx in 0..topology.box_count() {
            let indices = topology
                .get_box_style_indices(box_idx)
                .unwrap_or_else(|| styles.default_indices());
            let style = font_metrics
                .used_style(styles, indices, box_idx)
                .expect("validated style handle");
            tracks_overflow_clips |= style.overflow_x().clips() || style.overflow_y().clips();
            resolved.push(style);
        }
        let paragraph_spacing = ParagraphSpacing::new(document, styles, topology, inline_content, &resolved);
        Self {
            styles: resolved,
            paragraph_spacing,
            tracks_overflow_clips,
        }
    }
}

impl<'input> LayoutReader<'input> {
    pub(super) fn new(
        document: &'input Document,
        styles: &'input ComputedStyles,
        topology: &'input LayoutTree,
        inline_content: &'input InlineContent,
        font_metrics: &'input ShapedFontMetrics,
        image_metrics: &'input crate::ImageMetrics,
        box_styles: &'input BoxStyleSnapshot<'input>,
    ) -> Self {
        Self {
            document,
            styles,
            topology,
            inline_content,
            font_metrics,
            image_metrics,
            box_styles: &box_styles.styles,
            paragraph_spacing: &box_styles.paragraph_spacing,
            tracks_overflow_clips: box_styles.tracks_overflow_clips,
            paint: None,
        }
    }

    pub fn with_paint(mut self, paint: html_engine_model::PaintView<'input>) -> Self {
        self.paint = Some(paint);
        self
    }

    pub fn document(&self) -> &'input Document {
        self.document
    }

    /// Geometry reads never consult the reader palette.
    pub fn style(&self, box_idx: usize) -> UsedStyleView<'input> {
        self.box_styles[box_idx]
    }

    pub fn reader_text_indent(&self, box_idx: usize) -> Option<f64> {
        self.paragraph_spacing.indent(box_idx)
    }

    pub fn reader_paragraph_gap(&self, box_idx: usize) -> Option<f64> {
        self.paragraph_spacing.gap(box_idx)
    }

    pub fn is_reader_prose(&self, box_idx: usize) -> bool {
        self.paragraph_spacing.is_prose(box_idx)
    }

    pub fn vertical_margins(&self, box_idx: usize, containing_width: f64) -> (f64, f64) {
        let style = self.style(box_idx);
        self.paragraph_spacing.resolve(box_idx, style.margin_top().resolve(containing_width), style.margin_bottom().resolve(containing_width))
    }

    pub fn paint_style(&self, box_idx: usize) -> UsedStyleView<'input> {
        let style = self.style(box_idx);
        self.paint.and_then(|paint| paint.box_colors(box_idx)).map_or(style, |colors| style.with_paint_colors(colors))
    }

    pub fn background_can_change(&self, box_idx: usize) -> bool {
        let indices = self.topology.get_box_style_indices(box_idx).unwrap_or_else(|| self.styles.default_indices());
        self.style(box_idx).background_color() & 0xff != 0
            || self.styles.background_style(indices).is_some_and(|background| background.background_color_current_color)
            || self.topology.root_box() == Some(box_idx) || self.topology.body_box() == Some(box_idx)
    }

    pub fn tracks_overflow_clips(&self) -> bool {
        self.tracks_overflow_clips
    }

    pub fn used_style(&self, indices: StyleIndices) -> UsedStyleView<'input> {
        self.font_metrics.used_non_box_style(self.styles, indices).expect("validated style handle")
    }

    pub fn paint_used_style(&self, indices: StyleIndices, context: usize) -> UsedStyleView<'input> {
        let style = self.used_style(indices);
        self.paint.and_then(|paint| paint.style_colors(indices, context)).map_or(style, |colors| style.with_paint_colors(colors))
    }

    pub fn font_metrics(&self, box_idx: usize) -> FontRelativeMetrics {
        self.font_metrics.for_box(box_idx)
    }

    pub fn styles(&self) -> &'input ComputedStyles {
        self.styles
    }

    pub fn layout_tree(&self) -> &'input LayoutTree {
        self.topology
    }

    pub fn box_at(&self, box_idx: usize) -> Option<&LayoutBox> {
        self.topology.box_at(box_idx)
    }

    pub fn box_dom_element(&self, box_idx: usize) -> Option<u32> {
        self.topology.get_box_dom_element(box_idx)
    }

    pub fn image_idx(&self, box_idx: usize) -> Option<u32> {
        self.topology.get_box_image_idx(box_idx)
    }

    pub fn image_display_size(&self, image_idx: u32) -> (f64, f64) {
        self.image_metrics.display_size(
            image_idx,
            self.document
                .image(image_idx)
                .expect("image index should be valid"),
        )
    }

    pub fn image_intrinsic(&self, box_idx: usize) -> Option<ImageIntrinsic> {
        let image_idx = self.image_idx(box_idx)?;
        let (width, height) = self.image_display_size(image_idx);
        let size = Size::new(width, height);
        Some(ImageIntrinsic {
            size,
            aspect_ratio: self.image_intrinsic_ratio(box_idx, size),
        })
    }

    fn image_intrinsic_ratio(&self, box_idx: usize, intrinsic: Size) -> Option<f64> {
        if let Some(element) = self
            .box_dom_element(box_idx)
            .and_then(|raw| self.document.node_id_from_raw(raw))
            .and_then(|node| self.document.element_ref(node))
            && element.tag().eq_ignore_ascii_case("svg")
        {
            if let Some(view_box) = element.attr("viewBox").or_else(|| element.attr("viewbox")) {
                let values = view_box
                    .split(|character: char| character.is_ascii_whitespace() || character == ',')
                    .filter(|value| !value.is_empty())
                    .map(str::parse::<f64>)
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;
                if values.len() != 4 || values[2] <= 0.0 || values[3] <= 0.0 {
                    return None;
                }
                return Some(values[2] / values[3]);
            }
            return None;
        }
        (intrinsic.height > 0.0).then_some(intrinsic.width / intrinsic.height)
    }

    /// Returns a renderer-supplied column-filling width for an eligible
    /// standalone image, capped so its outer height fits the viewport.
    pub fn smart_standalone_image_width(
        &self,
        policy: crate::ImageSizingPolicy,
        box_idx: usize,
        intrinsic: Size,
        available_width: f64,
        viewport_height: Option<f64>,
        horizontal_padding_border: f64,
        vertical_outer_inset: f64,
        standalone: bool,
    ) -> Option<html_style_model::UsedPreferredSize> {
        const MIN_SOURCE_WIDTH: f64 = 96.0;

        if self.image_idx(box_idx).and_then(|index| self.document.image(index))
            .is_some_and(|image| matches!(image.source, html_dom::ImageSource::MathMl { .. }))
        {
            return None;
        }

        if policy != crate::ImageSizingPolicy::SmartStandalone
            || !standalone
            || intrinsic.width < MIN_SOURCE_WIDTH
        {
            return None;
        }
        let style = self.style(box_idx);
        if style.height() != html_style_model::UsedPreferredSize::Auto
            || style.min_height() != html_style_model::UsedPreferredSize::Auto
            || style.max_height() != html_style_model::UsedPreferredSize::Auto
        {
            return None;
        }

        let node = self
            .box_dom_element(box_idx)
            .and_then(|raw| self.document.node_id_from_raw(raw))?;
        let element = self.document.element_ref(node)?;
        if element.attr("height").is_some() {
            return None;
        }

        let mut expanded_content_width = (available_width - horizontal_padding_border).max(0.0);
        if intrinsic.height > 0.0
            && let Some(viewport_height) = viewport_height
        {
            let available_content_height = (viewport_height - vertical_outer_inset).max(0.0);
            expanded_content_width = expanded_content_width
                .min(available_content_height * intrinsic.width / intrinsic.height);
        }
        let preferred = match style.box_sizing() {
            html_style_model::BoxSizing::ContentBox => expanded_content_width,
            html_style_model::BoxSizing::BorderBox => {
                expanded_content_width + horizontal_padding_border
            }
        };
        Some(html_style_model::UsedPreferredSize::Px(preferred as f32))
    }

    pub fn box_style_indices(&self, box_idx: usize) -> Option<StyleIndices> {
        self.topology.get_box_style_indices(box_idx)
    }

    pub fn layout_style(&self, box_idx: usize) -> &'input LayoutStyle {
        let indices = self
            .box_style_indices(box_idx)
            .unwrap_or_else(|| self.styles.default_indices());
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

    pub fn box_layout_mode(&self, box_idx: usize) -> Option<&LayoutMode> {
        self.topology.get_box_layout_mode(box_idx)
    }

    /// Whether an inline-run child range creates an in-flow line box. Floats
    /// and absolutely positioned descendants leave anchors in the run stream,
    /// but those anchors do not give their containing block a line box or
    /// prevent its vertical margins from collapsing through.
    pub fn inline_items_establish_line_box(
        &self,
        container_box_idx: usize,
        runs: &std::ops::Range<u32>,
    ) -> bool {
        self.inline_content.inline_items()[runs.start as usize..runs.end as usize]
            .iter()
            .any(|run| {
                let ownership_box = match run.kind {
                    InlineItemKind::AtomicBox { box_idx } => self.get_parent(box_idx as usize),
                    _ => Some(run.box_idx as usize),
                };
                if !ownership_box.is_some_and(|box_idx| {
                    self.run_belongs_to_inline_context(box_idx, container_box_idx)
                }) {
                    return false;
                }
                match &run.kind {
                    InlineItemKind::FloatAnchor { .. } | InlineItemKind::AbsoluteAnchor { .. } => {
                        false
                    }
                    InlineItemKind::Text { glyphs } => {
                        let preserves_space = self
                            .style(run.box_idx as usize)
                            .white_space()
                            .preserves_spaces();
                        glyphs.clone().any(|glyph_idx| {
                            let character = self
                                .inline_content
                                .character_at(glyph_idx as usize)
                                .and_then(char::from_u32)
                                .unwrap_or_default();
                            preserves_space || !character.is_whitespace() || character == '\u{00A0}'
                        })
                    }
                    InlineItemKind::Image { .. }
                    | InlineItemKind::Break { .. }
                    | InlineItemKind::InlineBoundary { .. }
                    | InlineItemKind::AtomicBox { .. }
                    | InlineItemKind::Marker { .. } => true,
                }
            })
    }

    /// Float sides represented only by anchors in this block's inline-run
    /// stream. Anchor-only streams do not establish a line box, but their
    /// float sources still participate in adjoining-margin/clearance rules.
    pub fn inline_item_float_sides(
        &self,
        container_box_idx: usize,
        runs: &std::ops::Range<u32>,
    ) -> (bool, bool) {
        let mut left = false;
        let mut right = false;
        for run in &self.inline_content.inline_items()[runs.start as usize..runs.end as usize] {
            let InlineItemKind::FloatAnchor { box_idx } = run.kind else {
                continue;
            };
            if !self.run_belongs_to_inline_context(run.box_idx as usize, container_box_idx) {
                continue;
            }
            match self.style(box_idx as usize).float() {
                Float::Left => left = true,
                Float::Right => right = true,
                Float::None => {}
            }
        }
        (left, right)
    }

    fn run_belongs_to_inline_context(&self, run_box_idx: usize, container_box_idx: usize) -> bool {
        let mut current = Some(run_box_idx);
        while let Some(box_idx) = current {
            if box_idx == container_box_idx {
                return true;
            }
            if matches!(
                self.box_layout_mode(box_idx),
                Some(
                    LayoutMode::Block(_)
                        | LayoutMode::Table(_)
                        | LayoutMode::TableRow(_)
                        | LayoutMode::TableCell(_)
                        | LayoutMode::Flex(_)
                        | LayoutMode::Grid(_)
                )
            ) {
                return false;
            }
            current = self.get_parent(box_idx);
        }
        false
    }

    pub fn get_parent(&self, box_idx: usize) -> Option<usize> {
        self.topology.get_box_parent(box_idx)
    }

    pub fn root_box(&self) -> Option<usize> {
        self.topology.root_box()
    }

    pub fn body_box(&self) -> Option<usize> {
        self.topology.body_box()
    }

    /// CSS Overflow propagates the HTML body element's overflow to the
    /// viewport when both root axes are `visible`. The body's used overflow
    /// then becomes `visible`, so it must not create a block formatting
    /// context or clip its own descendants.
    pub fn effective_overflow_modes(
        &self,
        box_idx: usize,
    ) -> (
        html_style_model::OverflowMode,
        html_style_model::OverflowMode,
    ) {
        let style = self.style(box_idx);
        if self.body_box() == Some(box_idx)
            && self.root_box().is_some_and(|root_idx| {
                let root = self.style(root_idx);
                root.overflow_x() == html_style_model::OverflowMode::Visible
                    && root.overflow_y() == html_style_model::OverflowMode::Visible
            })
        {
            (
                html_style_model::OverflowMode::Visible,
                html_style_model::OverflowMode::Visible,
            )
        } else {
            (style.overflow_x(), style.overflow_y())
        }
    }

    /// Whether this box supplies the canvas background instead of painting a
    /// separate element background. CSS propagates the root background to the
    /// canvas; for HTML documents whose root has no background, the body
    /// background is propagated instead.
    pub fn background_paints_on_canvas(&self, box_idx: usize) -> bool {
        let Some(root_box) = self.root_box() else {
            return false;
        };
        let root = self.paint_style(root_box);
        if root.background_image_present() || root.background_color() & 0xFF != 0 {
            return box_idx == root_box;
        }
        let Some(body_box) = self.body_box() else {
            return false;
        };
        let body = self.paint_style(body_box);
        (body.background_image_present() || body.background_color() & 0xFF != 0)
            && box_idx == body_box
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
            Some(LayoutMode::Block(block)) if matches!(&block.children, Children::Blocks(children) if children.as_slice() == [box_idx as u32])
        )
    }

    pub fn list_marker(&self, box_idx: usize) -> Option<ListItemMarker> {
        self.topology.list_marker(box_idx)
    }

    pub fn box_uses_float_context(&self, box_idx: usize) -> bool {
        let style = self.style(box_idx);
        let (overflow_x, overflow_y) = self.effective_overflow_modes(box_idx);
        let is_flex_grid_item = self
            .get_parent(box_idx)
            .is_some_and(|parent| self.is_flex_grid_box(parent));
        self.is_table_box(box_idx)
            || super::block::establishes_formatting_context(
                style.display(),
                overflow_x,
                overflow_y,
                matches!(style.float(), Float::Left | Float::Right),
                self.root_box() == Some(box_idx),
                self.is_table_cell_box(box_idx),
                is_flex_grid_item,
            )
            || style.position() == PositionMode::Absolute
    }

    pub fn box_uses_ahem(&self, box_idx: usize) -> bool {
        let indices = self
            .topology
            .get_box_style_indices(box_idx)
            .unwrap_or_else(|| self.styles.default_indices());
        self.styles
            .view(indices)
            .and_then(|style| style.font_family())
            .and_then(|family| self.styles.string(family))
            .is_some_and(|family| family.eq_ignore_ascii_case("Ahem"))
    }
}
