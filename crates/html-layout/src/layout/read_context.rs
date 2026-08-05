use crate::layout_model::{Children, GlyphMetrics, InlineContent, InlineItemKind, LayoutBox, LayoutMode, LayoutTree, ListItemMarker};
use crate::shaping::{FontRelativeMetrics, ShapedFontMetrics};
use html_style_model::{ComputedStyles, Float, PositionMode, StyleIndices, UsedStyleView};

/// Shared read-only access to styled layout topology. Specialized layout
/// contexts compose this capability instead of reimplementing style lookup and
/// basic tree traversal.
pub(crate) struct LayoutReader<'input> {
    styles: &'input ComputedStyles,
    topology: &'input LayoutTree,
    inline_content: &'input InlineContent,
    glyph_metrics: &'input GlyphMetrics,
    font_metrics: &'input ShapedFontMetrics,
}

impl<'input> LayoutReader<'input> {
    pub(crate) fn new(styles: &'input ComputedStyles, topology: &'input LayoutTree, inline_content: &'input InlineContent, glyph_metrics: &'input GlyphMetrics, font_metrics: &'input ShapedFontMetrics) -> Self {
        Self { styles, topology, inline_content, glyph_metrics, font_metrics }
    }

    pub(crate) fn style(&self, box_idx: usize) -> UsedStyleView<'input> {
        let indices = self.topology.get_box_style_indices(box_idx).unwrap_or_else(|| self.styles.default_indices());
        let metrics = self.font_metrics.for_box(box_idx);
        self.styles
            .used_view_with_root(indices, metrics.x_height_ratio(), metrics.ch_advance_ratio(), metrics.cap_height_ratio(), self.font_metrics.root_ch_px(), self.font_metrics.root_cap_height_px(), self.font_metrics.root_line_height_px())
            .expect("validated style handle")
    }

    pub(crate) fn used_style(&self, indices: StyleIndices) -> UsedStyleView<'input> {
        let metrics = self.font_metrics.for_non_box_style(indices);
        self.styles
            .used_view_with_root(indices, metrics.x_height_ratio(), metrics.ch_advance_ratio(), metrics.cap_height_ratio(), self.font_metrics.root_ch_px(), self.font_metrics.root_cap_height_px(), self.font_metrics.root_line_height_px())
            .expect("validated style handle")
    }

    pub(crate) fn font_metrics(&self, box_idx: usize) -> FontRelativeMetrics {
        self.font_metrics.for_box(box_idx)
    }

    pub(crate) fn styles(&self) -> &'input ComputedStyles {
        self.styles
    }

    pub(crate) fn layout_tree(&self) -> &'input LayoutTree {
        self.topology
    }

    pub(crate) fn box_at(&self, box_idx: usize) -> Option<&LayoutBox> {
        self.topology.box_at(box_idx)
    }

    pub(crate) fn box_style_indices(&self, box_idx: usize) -> Option<StyleIndices> {
        self.topology.get_box_style_indices(box_idx)
    }

    pub(crate) fn inline_fragment_edges(&self, box_idx: usize) -> (bool, bool) {
        self.topology.inline_fragment_edges(box_idx)
    }

    pub(crate) fn split_inline_position_ancestors(&self, box_idx: usize) -> &[StyleIndices] {
        self.topology.split_inline_position_ancestors(box_idx)
    }

    pub(crate) fn box_count(&self) -> usize {
        self.topology.box_count()
    }

    pub(crate) fn box_layout_mode(&self, box_idx: usize) -> Option<&LayoutMode> {
        self.topology.get_box_layout_mode(box_idx)
    }

    /// Whether an inline-run child range creates an in-flow line box. Floats
    /// and absolutely positioned descendants leave anchors in the run stream,
    /// but those anchors do not give their containing block a line box or
    /// prevent its vertical margins from collapsing through.
    pub(crate) fn inline_items_establish_line_box(&self, container_box_idx: usize, runs: &std::ops::Range<u32>) -> bool {
        self.inline_content.inline_items()[runs.start as usize..runs.end as usize].iter().any(|run| {
            let ownership_box = match run.kind {
                InlineItemKind::AtomicBox { box_idx } => self.get_parent(box_idx as usize),
                _ => Some(run.box_idx as usize),
            };
            if !ownership_box.is_some_and(|box_idx| self.run_belongs_to_inline_context(box_idx, container_box_idx)) {
                return false;
            }
            match &run.kind {
                InlineItemKind::FloatAnchor { .. } | InlineItemKind::AbsoluteAnchor { .. } => false,
                InlineItemKind::Text { glyphs } => {
                    let preserves_space = self.style(run.box_idx as usize).white_space().preserves_spaces();
                    glyphs.clone().any(|glyph_idx| {
                        let character = self.inline_content.glyph_at(glyph_idx as usize).map(|glyph| self.glyph_metrics.get(glyph).ch()).unwrap_or_default();
                        preserves_space || !character.is_whitespace() || character == '\u{00A0}'
                    })
                }
                InlineItemKind::Image { .. } | InlineItemKind::Break { .. } | InlineItemKind::InlineBoundary { .. } | InlineItemKind::AtomicBox { .. } | InlineItemKind::Marker { .. } => true,
            }
        })
    }

    /// Float sides represented only by anchors in this block's inline-run
    /// stream. Anchor-only streams do not establish a line box, but their
    /// float sources still participate in adjoining-margin/clearance rules.
    pub(crate) fn inline_item_float_sides(&self, container_box_idx: usize, runs: &std::ops::Range<u32>) -> (bool, bool) {
        let mut left = false;
        let mut right = false;
        for run in &self.inline_content.inline_items()[runs.start as usize..runs.end as usize] {
            let InlineItemKind::FloatAnchor { box_idx } = run.kind else { continue };
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
            if matches!(self.box_layout_mode(box_idx), Some(LayoutMode::Block(_) | LayoutMode::Table(_) | LayoutMode::TableRow(_) | LayoutMode::TableCell(_) | LayoutMode::Flex(_) | LayoutMode::Grid(_))) {
                return false;
            }
            current = self.get_parent(box_idx);
        }
        false
    }

    pub(crate) fn get_parent(&self, box_idx: usize) -> Option<usize> {
        self.topology.get_box_parent(box_idx)
    }

    pub(crate) fn root_box(&self) -> Option<usize> {
        self.topology.root_box()
    }

    pub(crate) fn body_box(&self) -> Option<usize> {
        self.topology.body_box()
    }

    /// Whether this box supplies the canvas background instead of painting a
    /// separate element background. CSS propagates the root background to the
    /// canvas; for HTML documents whose root has no background, the body
    /// background is propagated instead.
    pub(crate) fn background_paints_on_canvas(&self, box_idx: usize) -> bool {
        let Some(root_box) = self.root_box() else { return false };
        let root = self.style(root_box);
        if root.background_image_present() || root.background_color() & 0xFF != 0 {
            return box_idx == root_box;
        }
        let Some(body_box) = self.body_box() else { return false };
        let body = self.style(body_box);
        (body.background_image_present() || body.background_color() & 0xFF != 0) && box_idx == body_box
    }

    pub(crate) fn is_table_cell_box(&self, box_idx: usize) -> bool {
        matches!(self.box_layout_mode(box_idx), Some(LayoutMode::TableCell(_)))
    }

    pub(crate) fn is_block_box(&self, box_idx: usize) -> bool {
        matches!(self.box_layout_mode(box_idx), Some(LayoutMode::Block(_)))
    }

    pub(crate) fn is_flex_grid_box(&self, box_idx: usize) -> bool {
        matches!(self.box_layout_mode(box_idx), Some(LayoutMode::Flex(_) | LayoutMode::Grid(_)))
    }

    pub(crate) fn is_anonymous_box(&self, box_idx: usize) -> bool {
        matches!(self.box_layout_mode(box_idx), Some(LayoutMode::Anonymous(_)))
    }

    pub(crate) fn is_table_box(&self, box_idx: usize) -> bool {
        matches!(self.box_layout_mode(box_idx), Some(LayoutMode::Table(_)))
    }

    pub(crate) fn is_inline_box(&self, box_idx: usize) -> bool {
        matches!(self.box_layout_mode(box_idx), Some(LayoutMode::Inline(_)))
    }

    pub(crate) fn is_only_block_child(&self, box_idx: usize) -> bool {
        let Some(parent) = self.get_parent(box_idx) else { return false };
        matches!(
            self.box_layout_mode(parent),
            Some(LayoutMode::Block(block)) if matches!(&block.children, Children::Blocks(children) if children.as_slice() == [box_idx as u32])
        )
    }

    pub(crate) fn list_marker(&self, box_idx: usize) -> Option<ListItemMarker> {
        self.topology.list_marker(box_idx)
    }

    pub(crate) fn box_uses_float_context(&self, box_idx: usize) -> bool {
        let style = self.style(box_idx);
        let is_flex_grid_item = self.get_parent(box_idx).is_some_and(|parent| self.is_flex_grid_box(parent));
        self.is_table_box(box_idx)
            || super::block::establishes_formatting_context(
                style.display(),
                style.overflow_x(),
                style.overflow_y(),
                matches!(style.float(), Float::Left | Float::Right),
                self.root_box() == Some(box_idx),
                self.is_table_cell_box(box_idx),
                is_flex_grid_item,
            )
            || style.position() == PositionMode::Absolute
    }

    pub(crate) fn box_uses_ahem(&self, box_idx: usize) -> bool {
        let indices = self.topology.get_box_style_indices(box_idx).unwrap_or_else(|| self.styles.default_indices());
        self.styles.view(indices).and_then(|style| style.font_family()).and_then(|family| self.styles.string(family)).is_some_and(|family| family.eq_ignore_ascii_case("Ahem"))
    }
}
