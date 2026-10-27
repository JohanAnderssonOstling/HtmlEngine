use crate::layout_model::{GlyphAdvanceRun, GlyphId, GlyphMetric, GlyphMetrics, GlyphOffsetRun, InlineContent, InlineItem, Line};
use crate::layout_model::{GlyphSlice, NativeRuns, TextGeometryView};
use super::text_placement::{LineTextPlacement, tracking_boundary_after};
use html_style_model::TextDirection;
use rustc_data_structures::fx::FxHashMap;

/// Immutable shaped-text input used by inline layout and intrinsic sizing.
/// Keeping it separate from the token cache makes the distinction between
/// source data and mutable inline layout state explicit.
#[derive(Clone, Copy)]
pub struct InlineReader<'input> {
    content: &'input InlineContent,
    glyphs: GlyphSlice<'input>,
    glyph_metrics: &'input GlyphMetrics,
    text_geometry: Option<TextGeometryView<'input>>,
    ellipsis_glyphs: &'input FxHashMap<u32, GlyphId>,
    hyphen_glyphs: &'input FxHashMap<u32, GlyphId>,
}

impl<'input> InlineReader<'input> {
    pub fn new(
        content: &'input InlineContent,
        glyphs: GlyphSlice<'input>,
        glyph_metrics: &'input GlyphMetrics,
        text_geometry: Option<TextGeometryView<'input>>,
        ellipsis_glyphs: &'input FxHashMap<u32, GlyphId>,
        hyphen_glyphs: &'input FxHashMap<u32, GlyphId>,
    ) -> Self {
        debug_assert_eq!(content.characters().len(), glyphs.len());
        Self {
            content,
            glyphs,
            glyph_metrics,
            text_geometry,
            ellipsis_glyphs,
            hyphen_glyphs,
        }
    }

    pub fn cursor(self) -> TextCursor<'input> {
        TextCursor {
            text: self,
            metric_hint: 0,
            geometry_hint: 0,
        }
    }

    pub(super) fn line_placement<'a>(
        self,
        line: &'a Line,
        offsets: &'a [GlyphOffsetRun],
        advances: &'a [GlyphAdvanceRun],
    ) -> LineTextPlacement<'a>
    where 'input: 'a {
        LineTextPlacement::new(self.content, self.glyphs, self.glyph_metrics, self.text_geometry, line, offsets, advances)
    }

    pub(super) fn has_tracking_boundary_after(&self, index: u32, end: u32) -> bool {
        tracking_boundary_after(self.content, index, end, self.is_cluster_boundary(index as usize + 1))
    }

    pub fn inline_items(&self) -> &[InlineItem] {
        self.content.inline_items()
    }

    pub fn content(&self) -> &'input InlineContent {
        self.content
    }

    pub fn inline_item(&self, run_idx: usize) -> Option<&InlineItem> {
        self.content.inline_item(run_idx)
    }

    pub fn glyphs(&self) -> GlyphSlice<'input> {
        self.glyphs
    }

    pub fn glyph_at(&self, glyph_idx: usize) -> Option<GlyphId> {
        self.glyphs.get(glyph_idx).copied()
    }

    pub fn whitespace_wrap_before(
        &self,
        glyph_idx: usize,
    ) -> crate::layout_model::WhitespaceWrapOverride {
        self.content.whitespace_wrap_before(glyph_idx)
    }

    pub fn glyph(&self, glyph_idx: u32) -> Option<GlyphId> {
        self.glyph_at(glyph_idx as usize)
    }

    pub fn glyph_metric(&self, glyph: GlyphId) -> GlyphMetric {
        self.glyph_metrics.get(glyph)
    }

    pub fn authoritative_runs(&self, range: std::ops::Range<u32>) -> Option<NativeRuns<'input>> {
        self.text_geometry.map(|geometry| geometry.authoritative_runs(range))
    }

    pub fn text_advance(&self, character_index: usize, fallback: f32) -> f32 {
        self.text_geometry
            .and_then(|geometry| geometry.advance(character_index))
            .unwrap_or(fallback)
    }

    pub fn is_cluster_boundary(&self, character_boundary: usize) -> bool {
        self.text_geometry
            .is_none_or(|geometry| geometry.is_cluster_boundary(character_boundary))
    }

    pub fn bidi_level(&self, index: u32) -> u8 {
        self.text_geometry.map_or(0, |geometry| geometry.bidi().level(index))
    }

    pub fn item_bidi_level(&self, item: &InlineItem) -> u8 {
        self.text_geometry.map_or(0, |geometry| geometry.bidi().item_level(item))
    }

    pub fn paragraph_direction(&self, index: u32) -> TextDirection {
        self.text_geometry.map_or(TextDirection::Ltr, |geometry| geometry.bidi().paragraph_direction(index))
    }

    pub fn ellipsis_glyph(&self, box_idx: usize) -> Option<GlyphId> {
        self.ellipsis_glyphs.get(&(box_idx as u32)).copied()
    }

    pub fn hyphen_glyph(&self, box_idx: usize) -> Option<GlyphId> {
        self.hyphen_glyphs.get(&(box_idx as u32)).copied()
    }
}

/// Per-traversal hints stay local to each layout operation; shared document
/// snapshots remain immutable and parallel table measurement stays independent.
pub struct TextCursor<'input> {
    text: InlineReader<'input>,
    metric_hint: usize,
    geometry_hint: usize,
}

impl TextCursor<'_> {
    #[inline]
    pub fn character_geometry(&mut self, index: usize, fallback: f32) -> (f32, bool) {
        self.text.text_geometry.map_or((fallback, true), |geometry| {
            let (advance, boundary) = geometry.character_geometry_with_hint(index, &mut self.geometry_hint);
            (advance.unwrap_or(fallback), boundary)
        })
    }

    #[inline]
    pub fn glyph_metric(&mut self, glyph: GlyphId) -> Option<GlyphMetric> {
        self.text
            .glyph_metrics
            .get_with_hint(glyph, &mut self.metric_hint)
    }

    #[inline]
    pub fn text_advance(&mut self, index: usize, fallback: f32) -> f32 {
        self.text
            .text_geometry
            .and_then(|geometry| geometry.advance_with_hint(index, &mut self.geometry_hint))
            .unwrap_or(fallback)
    }

    #[inline]
    pub fn is_cluster_boundary(&mut self, index: usize) -> bool {
        self.text
            .text_geometry
            .is_none_or(|geometry| geometry.boundary_with_hint(index, &mut self.geometry_hint))
    }
}
