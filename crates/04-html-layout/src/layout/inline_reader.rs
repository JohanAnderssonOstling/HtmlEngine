use crate::layout_model::{GlyphId, GlyphMetric, GlyphMetrics, InlineContent, InlineItem};

/// Immutable shaped-text input used by inline layout and intrinsic sizing.
/// Keeping it separate from the token cache makes the distinction between
/// source data and mutable inline layout state explicit.
#[derive(Clone, Copy)]
pub(crate) struct InlineReader<'input> {
    content: &'input InlineContent,
    glyph_metrics: &'input GlyphMetrics,
    text_geometry: Option<&'input crate::shaping::ShapedTextGeometry>,
    ellipsis_glyphs: &'input rustc_data_structures::fx::FxHashMap<u32, GlyphId>,
    hyphen_glyphs: &'input rustc_data_structures::fx::FxHashMap<u32, GlyphId>,
}

impl<'input> InlineReader<'input> {
    pub(crate) fn new(
        content: &'input InlineContent,
        glyph_metrics: &'input GlyphMetrics,
        text_geometry: Option<&'input crate::shaping::ShapedTextGeometry>,
        ellipsis_glyphs: &'input rustc_data_structures::fx::FxHashMap<u32, GlyphId>,
        hyphen_glyphs: &'input rustc_data_structures::fx::FxHashMap<u32, GlyphId>,
    ) -> Self {
        Self {
            content,
            glyph_metrics,
            text_geometry,
            ellipsis_glyphs,
            hyphen_glyphs,
        }
    }

    pub(crate) fn cursor(self) -> TextCursor<'input> {
        TextCursor {
            text: self,
            metric_hint: 0,
            geometry_hint: 0,
        }
    }

    pub(crate) fn inline_items(&self) -> &[InlineItem] {
        self.content.inline_items()
    }

    pub(crate) fn content(&self) -> &'input InlineContent {
        self.content
    }

    pub(crate) fn inline_item(&self, run_idx: usize) -> Option<&InlineItem> {
        self.content.inline_item(run_idx)
    }

    pub(crate) fn glyphs(&self) -> &[GlyphId] {
        self.content.glyphs()
    }

    pub(crate) fn glyph_at(&self, glyph_idx: usize) -> Option<GlyphId> {
        self.content.glyph_at(glyph_idx)
    }

    pub(crate) fn whitespace_wrap_before(
        &self,
        glyph_idx: usize,
    ) -> crate::layout_model::WhitespaceWrapOverride {
        self.content.whitespace_wrap_before(glyph_idx)
    }

    pub(crate) fn glyph(&self, glyph_idx: u32) -> Option<GlyphId> {
        self.glyph_at(glyph_idx as usize)
    }

    pub(crate) fn glyph_metric(&self, glyph: GlyphId) -> GlyphMetric {
        self.glyph_metrics.get(glyph)
    }

    pub(crate) fn authoritative_runs(&self) -> &[crate::shaping::AuthoritativeShapedRun] {
        self.text_geometry
            .map_or(&[], |geometry| geometry.authoritative_runs())
    }

    pub(crate) fn text_advance(&self, character_index: usize, fallback: f32) -> f32 {
        self.text_geometry
            .and_then(|geometry| geometry.advance(character_index))
            .unwrap_or(fallback)
    }

    pub(crate) fn is_cluster_boundary(&self, character_boundary: usize) -> bool {
        self.text_geometry
            .is_none_or(|geometry| geometry.is_cluster_boundary(character_boundary))
    }

    pub(crate) fn ellipsis_glyph(&self, box_idx: usize) -> Option<GlyphId> {
        self.ellipsis_glyphs.get(&(box_idx as u32)).copied()
    }

    pub(crate) fn hyphen_glyph(&self, box_idx: usize) -> Option<GlyphId> {
        self.hyphen_glyphs.get(&(box_idx as u32)).copied()
    }
}

/// Per-traversal hints stay local to each layout operation; shared document
/// snapshots remain immutable and parallel table measurement stays independent.
pub(crate) struct TextCursor<'input> {
    text: InlineReader<'input>,
    metric_hint: usize,
    geometry_hint: usize,
}

impl TextCursor<'_> {
    #[inline]
    pub(crate) fn glyph_metric(&mut self, glyph: GlyphId) -> Option<GlyphMetric> {
        self.text
            .glyph_metrics
            .get_with_hint(glyph, &mut self.metric_hint)
    }

    #[inline]
    pub(crate) fn text_advance(&mut self, index: usize, fallback: f32) -> f32 {
        self.text
            .text_geometry
            .and_then(|geometry| geometry.advance_with_hint(index, &mut self.geometry_hint))
            .unwrap_or(fallback)
    }

    #[inline]
    pub(crate) fn is_cluster_boundary(&mut self, index: usize) -> bool {
        self.text
            .text_geometry
            .is_none_or(|geometry| geometry.boundary_with_hint(index, &mut self.geometry_hint))
    }
}
