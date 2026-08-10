use crate::layout_model::{GlyphId, GlyphMetric, GlyphMetricError, GlyphMetrics, InlineContent, InlineItemKind, LayoutMode, LayoutTree, WhitespaceWrapOverride};
use html_dom::Document;
use html_style_model::{BorderStyle, ComputedStyles, FontStyle, OpenTypeFeature, StyleIndices, StyleView, TextOverflow, TextTransform, VerticalAlignValue};
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use std::fmt;
use std::ops::Range;
use std::sync::Arc;
use unicode_categories::UnicodeCategories;
use unicode_segmentation::UnicodeSegmentation;

/// Backend-owned immutable shaped-line identifier. It is only meaningful for
/// the `GlyphShaper` instance that produced it.
pub type TextRunId = u32;

/// Renderer-neutral geometry for a styled logical text run.
///
/// Advances are indexed by Unicode scalar value rather than glyph. A shaped
/// cluster may cover several scalar values; `cluster_boundaries` identifies
/// the only boundaries at which layout may split the run.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapedTextRun {
    advances: Arc<[f32]>,
    cluster_boundaries: Arc<[bool]>,
    backend_run: Option<TextRunId>,
    caret_stops: Option<Arc<[f32]>>,
    ascent: f32,
}

impl ShapedTextRun {
    pub fn new(advances: Arc<[f32]>, cluster_boundaries: Arc<[bool]>) -> Option<Self> {
        if cluster_boundaries.len() != advances.len().checked_add(1)?
            || !cluster_boundaries.first().copied().unwrap_or(false)
            || !cluster_boundaries.last().copied().unwrap_or(false)
            || advances.iter().any(|advance| !advance.is_finite() || *advance < 0.0)
        {
            return None;
        }
        Some(Self { advances, cluster_boundaries, backend_run: None, caret_stops: None, ascent: 0.0 })
    }

    pub fn with_backend_run(advances: Arc<[f32]>, cluster_boundaries: Arc<[bool]>, backend_run: TextRunId, caret_stops: Arc<[f32]>, ascent: f32) -> Option<Self> {
        let run = Self::new(advances, cluster_boundaries)?;
        (caret_stops.len() == run.advances.len() + 1 && caret_stops.iter().all(|stop| stop.is_finite()) && ascent.is_finite()).then_some(Self { backend_run: Some(backend_run), caret_stops: Some(caret_stops), ascent, ..run })
    }

    pub fn advances(&self) -> &[f32] {
        &self.advances
    }

    pub fn cluster_boundaries(&self) -> &[bool] {
        &self.cluster_boundaries
    }

    pub fn backend_run(&self) -> Option<TextRunId> {
        self.backend_run
    }

    pub fn caret_stops(&self) -> Option<&[f32]> {
        self.caret_stops.as_deref()
    }

    pub fn ascent(&self) -> f32 {
        self.ascent
    }

    pub(crate) fn memory_usage_bytes(&self) -> usize {
        self.advances.len() * std::mem::size_of::<f32>() + self.cluster_boundaries.len() * std::mem::size_of::<bool>() + self.caret_stops.as_ref().map_or(0, |stops| stops.len() * std::mem::size_of::<f32>())
    }
}

/// A request to measure one already-normalized, single-style logical run.
/// CSS values have been resolved before this reaches the renderer backend.
pub struct TextRunShapeRequest<'a> {
    text: &'a str,
    style: TextStyleSpan<'a>,
    paint_colors: Option<&'a [u32]>,
}

impl<'a> TextRunShapeRequest<'a> {
    pub fn new(text: &'a str, style: TextStyleSpan<'a>) -> Option<Self> {
        (style.byte_range() == (0..text.len()) && !text.is_empty()).then_some(Self { text, style, paint_colors: None })
    }

    /// Attach paint colors without turning them into shaping boundaries.
    /// There is one color per Unicode scalar value in `text`.
    pub fn with_paint_colors(mut self, paint_colors: &'a [u32]) -> Option<Self> {
        (paint_colors.len() == self.text.chars().count()).then(|| {
            self.paint_colors = Some(paint_colors);
            self
        })
    }

    pub fn text(&self) -> &'a str {
        self.text
    }

    pub fn style(&self) -> &TextStyleSpan<'a> {
        &self.style
    }

    pub fn paint_colors(&self) -> Option<&'a [u32]> {
        self.paint_colors
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ShapedTextGeometry {
    advances: Vec<f32>,
    cluster_boundaries: Vec<bool>,
    authoritative_runs: Vec<AuthoritativeShapedRun>,
}

#[derive(Clone, Debug)]
pub(crate) struct AuthoritativeShapedRun {
    pub(crate) source_range: Range<u32>,
    pub(crate) backend_run: TextRunId,
    pub(crate) caret_stops: Arc<[f32]>,
    pub(crate) ascent: f32,
    pub(crate) placement_required: bool,
}

impl ShapedTextGeometry {
    fn with_len(len: usize) -> Self {
        Self { advances: vec![0.0; len], cluster_boundaries: vec![true; len.saturating_add(1)], authoritative_runs: Vec::new() }
    }

    pub(crate) fn advance(&self, character_index: usize) -> Option<f32> {
        self.advances.get(character_index).copied()
    }

    pub(crate) fn is_cluster_boundary(&self, character_boundary: usize) -> bool {
        self.cluster_boundaries.get(character_boundary).copied().unwrap_or(true)
    }

    pub(crate) fn memory_usage_bytes(&self) -> usize {
        self.advances.capacity() * std::mem::size_of::<f32>()
            + self.cluster_boundaries.capacity() * std::mem::size_of::<bool>()
            + self.authoritative_runs.capacity() * std::mem::size_of::<AuthoritativeShapedRun>()
            + self.authoritative_runs.iter().map(|run| run.caret_stops.len() * std::mem::size_of::<f32>()).sum::<usize>()
    }

    pub(crate) fn authoritative_runs(&self) -> &[AuthoritativeShapedRun] {
        &self.authoritative_runs
    }

    pub(crate) fn update_range(&mut self, range: Range<u32>, shaped: &ShapedTextRun) -> bool {
        let start = range.start as usize;
        let end = range.end as usize;
        if start >= end || shaped.advances().len() != end - start || shaped.cluster_boundaries().len() != end - start + 1 {
            return false;
        }
        // Layout observes caret positions (prefix sums), not isolated scalar
        // advances. Individually sub-epsilon changes can accumulate enough to
        // alter wrapping or fragment placement across a long line.
        let mut cumulative_delta = 0.0f32;
        let advances_changed = self.advances[start..end].iter().zip(shaped.advances()).any(|(old, new)| {
            cumulative_delta += new - old;
            cumulative_delta.abs() > 0.01
        });
        let changed = advances_changed || self.cluster_boundaries[start..=end] != *shaped.cluster_boundaries();
        self.advances[start..end].copy_from_slice(shaped.advances());
        self.cluster_boundaries[start..=end].copy_from_slice(shaped.cluster_boundaries());
        changed
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShapedLine {
    pub line_index: usize,
    pub text_range: Range<u32>,
    pub run: TextRunId,
    pub ascent: f32,
    /// One horizontal caret position per logical text boundary. Positions are
    /// relative to the shaped line and may be non-monotonic for bidirectional
    /// text. The immutable allocation is shared with the backend shape cache.
    pub caret_stops: Arc<[f32]>,
    /// True at logical character boundaries where a line may split without
    /// bisecting a native shaping cluster.
    pub cluster_boundaries: Arc<[bool]>,
}

impl ShapedLine {
    pub fn x_for_text_position(&self, position: u32) -> Option<f64> {
        if position < self.text_range.start || position > self.text_range.end {
            return None;
        }
        let offset = position.checked_sub(self.text_range.start)? as usize;
        self.caret_stops.get(offset).copied().map(f64::from)
    }

    pub fn closest_text_position(&self, x: f64) -> u32 {
        let mut closest = self.text_range.start;
        let mut distance = f64::INFINITY;
        for (offset, stop) in self.caret_stops.iter().copied().enumerate() {
            let position = (self.text_range.start + offset as u32).min(self.text_range.end);
            let candidate_distance = (x - f64::from(stop)).abs();
            if candidate_distance < distance {
                closest = position;
                distance = candidate_distance;
            }
        }
        closest
    }
}

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub enum FontSlant {
    #[default]
    Normal,
    Italic,
    Oblique,
}

/// Renderer-neutral description of the font face whose relative metrics are
/// needed before layout. It is valid for boxes with no text content as well as
/// ordinary text runs.
#[derive(Clone, Copy, Debug)]
pub struct FontMetricsRequest<'a> {
    font_size: f32,
    font_weight: u16,
    font_slant: FontSlant,
    font_family: Option<&'a str>,
}

impl<'a> FontMetricsRequest<'a> {
    pub fn new(font_size: f32, font_weight: u16, font_slant: FontSlant, font_family: Option<&'a str>) -> Option<Self> {
        (font_size.is_finite() && font_size > 0.0 && font_weight > 0).then_some(Self { font_size, font_weight, font_slant, font_family })
    }

    pub fn font_size(self) -> f32 {
        self.font_size
    }
    pub fn font_weight(self) -> u16 {
        self.font_weight
    }
    pub fn font_slant(self) -> FontSlant {
        self.font_slant
    }
    pub fn font_family(self) -> Option<&'a str> {
        self.font_family
    }
}

/// Font-relative metrics normalized by the em size. Construction rejects
/// unusable backend values so downstream layout always receives a valid ratio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontRelativeMetrics {
    x_height_ratio: f32,
    ch_advance_ratio: f32,
    cap_height_ratio: f32,
    ascent_ratio: f32,
    descent_ratio: f32,
}

/// Complete font-relative metrics produced by shaping for the immutable
/// topology consumed by layout. Its fields are private so incomplete metric
/// collections cannot be assembled at the layout boundary.
#[derive(Clone)]
pub(crate) struct ShapedFontMetrics {
    box_metrics: BoxFontMetrics,
    non_box_metrics: RequiredFontMetrics<FxHashMap<StyleIndices, FontRelativeMetrics>>,
    root_ch_px: f32,
    root_cap_height_px: f32,
    root_line_height_px: f32,
}

#[derive(Clone)]
enum BoxFontMetrics {
    NotRequired,
    Dense(Vec<FontRelativeMetrics>),
    /// Zero is the fallback metric; positive IDs index `metrics` plus one.
    /// This keeps painted-inline support to four bytes per layout box while
    /// sharing each distinct selected-face metric tuple.
    Indexed {
        metric_ids: Vec<u32>,
        metrics: Vec<FontRelativeMetrics>,
    },
}

#[derive(Clone)]
enum RequiredFontMetrics<T> {
    NotRequired,
    Required(T),
}

impl ShapedFontMetrics {
    fn new(box_metrics: BoxFontMetrics, non_box_metrics: RequiredFontMetrics<FxHashMap<StyleIndices, FontRelativeMetrics>>, expected_box_count: usize, root_ch_px: f32, root_cap_height_px: f32, root_line_height_px: f32) -> Self {
        match &box_metrics {
            BoxFontMetrics::NotRequired => {}
            BoxFontMetrics::Dense(values) => assert_eq!(values.len(), expected_box_count, "dense shaping metrics must provide one entry per layout box"),
            BoxFontMetrics::Indexed { metric_ids, metrics } => {
                assert_eq!(metric_ids.len(), expected_box_count, "indexed shaping metrics must provide one ID per layout box");
                assert!(metric_ids.iter().all(|id| *id == 0 || (*id as usize) <= metrics.len()), "indexed shaping metric ID is out of range");
            }
        }
        Self { box_metrics, non_box_metrics, root_ch_px, root_cap_height_px, root_line_height_px }
    }

    pub(crate) fn for_box(&self, box_idx: usize) -> FontRelativeMetrics {
        match &self.box_metrics {
            BoxFontMetrics::NotRequired => FontRelativeMetrics::fallback(),
            BoxFontMetrics::Dense(values) => values[box_idx],
            BoxFontMetrics::Indexed { metric_ids, metrics } => metric_ids[box_idx].checked_sub(1).map_or_else(FontRelativeMetrics::fallback, |id| metrics[id as usize]),
        }
    }

    pub(crate) fn for_non_box_style(&self, indices: StyleIndices) -> FontRelativeMetrics {
        match &self.non_box_metrics {
            RequiredFontMetrics::NotRequired => FontRelativeMetrics::fallback(),
            RequiredFontMetrics::Required(values) => values[&indices],
        }
    }

    pub(crate) fn root_ch_px(&self) -> f32 {
        self.root_ch_px
    }
    pub(crate) fn root_cap_height_px(&self) -> f32 {
        self.root_cap_height_px
    }

    pub(crate) fn root_line_height_px(&self) -> f32 {
        self.root_line_height_px
    }

    pub(crate) fn memory_usage_bytes(&self) -> usize {
        let boxes = match &self.box_metrics {
            BoxFontMetrics::NotRequired => 0,
            BoxFontMetrics::Dense(values) => values.capacity() * std::mem::size_of::<FontRelativeMetrics>(),
            BoxFontMetrics::Indexed { metric_ids, metrics } => metric_ids.capacity() * std::mem::size_of::<u32>() + metrics.capacity() * std::mem::size_of::<FontRelativeMetrics>(),
        };
        let non_boxes = match &self.non_box_metrics {
            RequiredFontMetrics::NotRequired => 0,
            RequiredFontMetrics::Required(values) => values.capacity() * std::mem::size_of::<(StyleIndices, FontRelativeMetrics)>(),
        };
        boxes + non_boxes
    }
}

impl FontRelativeMetrics {
    pub fn new(x_height_ratio: f32, ch_advance_ratio: f32, cap_height_ratio: f32) -> Option<Self> {
        (x_height_ratio.is_finite() && x_height_ratio > 0.0 && ch_advance_ratio.is_finite() && ch_advance_ratio >= 0.0 && cap_height_ratio.is_finite() && cap_height_ratio > 0.0).then_some(Self {
            x_height_ratio,
            ch_advance_ratio,
            cap_height_ratio,
            ascent_ratio: 0.0,
            descent_ratio: 0.0,
        })
    }

    pub fn from_ratios(x_height_ratio: f32, ch_advance_ratio: f32, cap_height_ratio: f32, ascent_ratio: f32) -> Option<Self> {
        (x_height_ratio.is_finite() && x_height_ratio > 0.0 && ch_advance_ratio.is_finite() && ch_advance_ratio >= 0.0 && cap_height_ratio.is_finite() && cap_height_ratio > 0.0 && ascent_ratio.is_finite() && ascent_ratio > 0.0)
            .then_some(Self { x_height_ratio, ch_advance_ratio, cap_height_ratio, ascent_ratio, descent_ratio: (1.0 - ascent_ratio).max(0.0) })
    }

    pub fn from_line_ratios(x_height_ratio: f32, ch_advance_ratio: f32, cap_height_ratio: f32, ascent_ratio: f32, descent_ratio: f32) -> Option<Self> {
        (x_height_ratio.is_finite()
            && x_height_ratio > 0.0
            && ch_advance_ratio.is_finite()
            && ch_advance_ratio >= 0.0
            && cap_height_ratio.is_finite()
            && cap_height_ratio > 0.0
            && ascent_ratio.is_finite()
            && ascent_ratio > 0.0
            && descent_ratio.is_finite()
            && descent_ratio >= 0.0)
            .then_some(Self { x_height_ratio, ch_advance_ratio, cap_height_ratio, ascent_ratio, descent_ratio })
    }

    pub fn fallback() -> Self {
        Self { x_height_ratio: 0.5, ch_advance_ratio: 0.5, cap_height_ratio: 0.8, ascent_ratio: 0.0, descent_ratio: 0.0 }
    }
    pub fn x_height_ratio(self) -> f32 {
        self.x_height_ratio
    }
    pub fn ch_advance_ratio(self) -> f32 {
        self.ch_advance_ratio
    }
    pub fn cap_height_ratio(self) -> f32 {
        self.cap_height_ratio
    }
    pub fn ascent_ratio(self) -> Option<f32> {
        (self.ascent_ratio > 0.0).then_some(self.ascent_ratio)
    }
    pub fn line_box_ratios(self) -> Option<(f32, f32)> {
        (self.ascent_ratio > 0.0).then_some((self.ascent_ratio, self.descent_ratio))
    }
}

impl From<FontStyle> for FontSlant {
    fn from(style: FontStyle) -> Self {
        match style {
            FontStyle::Normal => Self::Normal,
            FontStyle::Italic => Self::Italic,
            FontStyle::Oblique => Self::Oblique,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TextStyleSpan<'a> {
    byte_range: Range<usize>,
    font_size: f32,
    font_weight: u16,
    font_slant: FontSlant,
    color: u32,
    font_family: Option<&'a str>,
    font_features: Arc<[OpenTypeFeature]>,
}

impl<'a> TextStyleSpan<'a> {
    pub fn new(byte_range: Range<usize>, font_size: f32, font_weight: u16, font_slant: FontSlant, color: u32, font_family: Option<&'a str>) -> Option<Self> {
        // CSS permits `font-size: 0`. It still needs a real shaping request so
        // character identities and backend paint resources stay registered;
        // the resulting advances and ink geometry are simply zero-sized.
        if byte_range.start >= byte_range.end || !font_size.is_finite() || font_size < 0.0 {
            return None;
        }
        Some(Self { byte_range, font_size, font_weight, font_slant, color, font_family, font_features: Arc::from([]) })
    }

    pub fn with_font_features(mut self, font_features: Vec<OpenTypeFeature>) -> Self {
        self.font_features = font_features.into();
        self
    }

    pub fn byte_range(&self) -> Range<usize> {
        self.byte_range.clone()
    }

    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    pub fn font_weight(&self) -> u16 {
        self.font_weight
    }

    pub fn font_slant(&self) -> FontSlant {
        self.font_slant
    }

    pub fn color(&self) -> u32 {
        self.color
    }

    pub fn font_family(&self) -> Option<&'a str> {
        self.font_family
    }

    pub fn font_features(&self) -> &[OpenTypeFeature] {
        &self.font_features
    }
}

/// Framework-neutral placement requested for one logical character after the
/// platform shaper has produced its natural geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterPlacement {
    extra_advance: f32,
    advance_override: Option<f32>,
    baseline_shift: f32,
    paint: bool,
}

impl CharacterPlacement {
    pub fn new(extra_advance: f32, advance_override: Option<f32>, paint: bool) -> Option<Self> {
        Self::with_baseline_shift(extra_advance, advance_override, 0.0, paint)
    }

    pub fn with_baseline_shift(extra_advance: f32, advance_override: Option<f32>, baseline_shift: f32, paint: bool) -> Option<Self> {
        if !extra_advance.is_finite() || !baseline_shift.is_finite() || advance_override.is_some_and(|advance| !advance.is_finite() || advance < 0.0) {
            return None;
        }
        Some(Self { extra_advance, advance_override, baseline_shift, paint })
    }

    pub fn extra_advance(self) -> f32 {
        self.extra_advance
    }

    pub fn advance_override(self) -> Option<f32> {
        self.advance_override
    }

    /// Positive values move the shaped character upward from the line baseline.
    pub fn baseline_shift(self) -> f32 {
        self.baseline_shift
    }

    pub fn paints(self) -> bool {
        self.paint
    }
}

impl Default for CharacterPlacement {
    fn default() -> Self {
        Self { extra_advance: 0.0, advance_override: None, baseline_shift: 0.0, paint: true }
    }
}

pub struct TextShapeRequest<'a> {
    line_index: usize,
    text_range: Range<u32>,
    text: &'a str,
    styles: &'a [TextStyleSpan<'a>],
    /// One entry per logical character in `text`. The layout layer resolves
    /// CSS semantics before this request reaches a renderer backend.
    placements: &'a [CharacterPlacement],
}

impl<'a> TextShapeRequest<'a> {
    pub fn new(line_index: usize, text_range: Range<u32>, text: &'a str, styles: &'a [TextStyleSpan<'a>], placements: &'a [CharacterPlacement]) -> Option<Self> {
        let character_count = text.chars().count();
        if text.is_empty() || styles.is_empty() || character_count != text_range.end.checked_sub(text_range.start)? as usize || placements.len() != character_count {
            return None;
        }
        let mut next_byte = 0;
        for style in styles {
            let range = style.byte_range();
            if range.start != next_byte || range.end > text.len() || !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
                return None;
            }
            next_byte = range.end;
        }
        if next_byte != text.len() {
            return None;
        }
        Some(Self { line_index, text_range, text, styles, placements })
    }

    pub fn line_index(&self) -> usize {
        self.line_index
    }

    pub fn text_range(&self) -> Range<u32> {
        self.text_range.clone()
    }

    pub fn text(&self) -> &'a str {
        self.text
    }

    pub fn styles(&self) -> &'a [TextStyleSpan<'a>] {
        self.styles
    }

    pub fn placements(&self) -> &'a [CharacterPlacement] {
        self.placements
    }

    pub fn adjusted_caret_stops(&self, raw: &[f32]) -> Option<Arc<[f32]>> {
        if raw.len() != self.placements.len().checked_add(1)? || raw.iter().any(|value| !value.is_finite()) {
            return None;
        }
        let mut adjusted = Vec::with_capacity(raw.len());
        adjusted.push(raw[0]);
        for (index, placement) in self.placements.iter().enumerate() {
            let natural_advance = raw[index + 1] - raw[index];
            let direction = if natural_advance < 0.0 { -1.0 } else { 1.0 };
            let advance = placement.advance_override().map_or(natural_advance + direction * placement.extra_advance(), |advance| direction * advance);
            adjusted.push(adjusted[index] + advance);
        }
        Some(Arc::from(adjusted))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeError {
    kind: ShapeErrorKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ShapeErrorKind {
    UnregisteredGlyphId { glyph_id: GlyphId, metrics_len: usize },
    RejectedMetric { reason: GlyphMetricError },
    RegistryCapacityExhausted { metrics_len: usize },
}

impl ShapeError {
    pub fn unregistered_glyph_id(glyph_id: GlyphId, metrics_len: usize) -> Self {
        Self { kind: ShapeErrorKind::UnregisteredGlyphId { glyph_id, metrics_len } }
    }

    pub fn rejected_metric(reason: GlyphMetricError) -> Self {
        Self { kind: ShapeErrorKind::RejectedMetric { reason } }
    }

    pub fn registry_capacity_exhausted(metrics_len: usize) -> Self {
        Self { kind: ShapeErrorKind::RegistryCapacityExhausted { metrics_len } }
    }

    pub fn glyph_id(self) -> GlyphId {
        match self.kind {
            ShapeErrorKind::UnregisteredGlyphId { glyph_id, .. } => glyph_id,
            ShapeErrorKind::RejectedMetric { .. } => GlyphId::MAX,
            ShapeErrorKind::RegistryCapacityExhausted { .. } => GlyphId::MAX,
        }
    }

    pub fn metrics_len(self) -> usize {
        match self.kind {
            ShapeErrorKind::UnregisteredGlyphId { metrics_len, .. } => metrics_len,
            ShapeErrorKind::RejectedMetric { .. } => 0,
            ShapeErrorKind::RegistryCapacityExhausted { metrics_len } => metrics_len,
        }
    }

    pub fn reason(self) -> Option<GlyphMetricError> {
        match self.kind {
            ShapeErrorKind::RejectedMetric { reason } => Some(reason),
            ShapeErrorKind::UnregisteredGlyphId { .. } | ShapeErrorKind::RegistryCapacityExhausted { .. } => None,
        }
    }
}

impl fmt::Display for ShapeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ShapeErrorKind::UnregisteredGlyphId { glyph_id, metrics_len } => {
                write!(formatter, "glyph shaper returned ID {} with only {} registered metrics", glyph_id, metrics_len)
            }
            ShapeErrorKind::RejectedMetric { reason } => write!(formatter, "glyph shaper returned invalid glyph metric: {reason:?}"),
            ShapeErrorKind::RegistryCapacityExhausted { metrics_len } => {
                write!(formatter, "glyph registry exceeded capacity after {metrics_len} registered metrics")
            }
        }
    }
}

impl std::error::Error for ShapeError {}

/// Append-only access to the glyph metrics owned by an active shaping phase.
///
/// The backing store is intentionally not part of the public API:
///
/// ```compile_fail
/// let _ = html_layout::GlyphMetrics::default();
/// ```
pub struct GlyphRegistry<'a> {
    glyph_metrics: &'a mut GlyphMetrics,
}

impl<'a> GlyphRegistry<'a> {
    pub(crate) fn new(glyph_metrics: &'a mut GlyphMetrics) -> Self {
        Self { glyph_metrics }
    }

    pub fn register(&mut self, metric: GlyphMetric) -> Result<GlyphId, ShapeError> {
        self.glyph_metrics.register(metric).map_err(|_| ShapeError::registry_capacity_exhausted(self.len()))
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.glyph_metrics.len()
    }

    #[inline]
    pub fn get(&self, glyph: GlyphId) -> Option<GlyphMetric> {
        self.glyph_metrics.get_checked(glyph)
    }

    #[inline]
    pub fn contains(&self, glyph: GlyphId) -> bool {
        self.glyph_metrics.get_checked(glyph).is_some()
    }
}

/// Backend supplied by a renderer adapter for turning styled characters into
/// dense glyph IDs and framework-neutral metrics.
pub trait GlyphShaper {
    fn reset(&mut self);

    /// Starts replacing the renderer-owned resources for a shaped document.
    ///
    /// Resource-backed implementations should retain the previous document's
    /// resources until either [`GlyphShaper::commit_document_shaping`] or
    /// [`GlyphShaper::rollback_document_shaping`] is called. Implementations
    /// without renderer-owned resources can rely on this reset-only default.
    fn begin_document_shaping(&mut self) {
        self.reset();
    }

    /// Commits resources produced since `begin_document_shaping`.
    fn commit_document_shaping(&mut self) {}

    /// Restores resources retained by `begin_document_shaping` after failure.
    fn rollback_document_shaping(&mut self) {}

    /// Resolve metrics for the selected primary font face. Backends without a
    /// metrics API use the CSS-defined 0.5em x-height fallback; layout derives
    /// missing vertical metrics from its shaped line geometry.
    fn font_relative_metrics(&mut self, _request: FontMetricsRequest<'_>) -> Result<FontRelativeMetrics, ShapeError> {
        Ok(FontRelativeMetrics::fallback())
    }

    fn shape_glyph<'a>(&mut self, glyph_metrics: &mut GlyphRegistry<'a>, ch: char, font_size: f32, font_weight: u16, font_slant: FontSlant, color: u32, family: Option<&str>) -> Result<GlyphId, ShapeError>;

    /// Shapes one normalized, single-style logical run into a caller-owned
    /// dense glyph-ID buffer. Backends can override this to resolve shared
    /// style state once and batch character lookup; the default preserves the
    /// established per-character contract.
    fn shape_glyph_run<'a>(&mut self, glyph_metrics: &mut GlyphRegistry<'a>, request: TextRunShapeRequest<'_>, glyphs: &mut [GlyphId]) -> Result<Option<ShapedTextRun>, ShapeError> {
        debug_assert_eq!(request.text().chars().count(), glyphs.len());
        let style = request.style();
        for (slot, character) in glyphs.iter_mut().zip(request.text().chars()) {
            *slot = self.shape_glyph(glyph_metrics, character, style.font_size(), style.font_weight(), style.font_slant(), style.color(), style.font_family())?;
        }
        self.shape_text_run(request)
    }

    /// Optionally supplies native cluster geometry for a logical style run.
    /// Backends without run shaping retain the per-character metric fallback.
    fn shape_text_run(&mut self, _request: TextRunShapeRequest<'_>) -> Result<Option<ShapedTextRun>, ShapeError> {
        Ok(None)
    }

    /// Optionally measures a complete candidate line at its actual start and
    /// end boundaries without creating a paint resource.
    fn measure_line(&mut self, _request: TextShapeRequest<'_>) -> Result<Option<ShapedTextRun>, ShapeError> {
        Ok(None)
    }

    /// Optionally shapes a final visual line as one platform text layout. The
    /// default keeps non-native backends on the established glyph path.
    fn shape_line(&mut self, _request: TextShapeRequest<'_>) -> Result<Option<ShapedLine>, ShapeError> {
        Ok(None)
    }

    fn begin_line_shaping(&mut self) {}
}

/// Shapes all styled text in a document. Whitespace normalization and CSS text
/// transforms are framework-neutral. The renderer supplies native run geometry
/// when available and the established per-character metrics remain the
/// renderer-independent fallback.
pub(crate) fn shape_document(
    document: &Document, styles: &ComputedStyles, layout_tree: &LayoutTree, inline_content: &mut InlineContent, glyph_shaper: &mut impl GlyphShaper, glyph_metrics: &mut GlyphMetrics,
) -> Result<(FxHashMap<u32, GlyphId>, FxHashMap<u32, GlyphId>, ShapedTextGeometry, ShapedFontMetrics), ShapeError> {
    if !inline_content.glyphs().is_empty() {
        collapse_whitespace(styles, layout_tree, inline_content);
    }
    let mut first_letter_styles = first_letter_style_overrides(document, styles, layout_tree, inline_content);
    if !inline_content.glyphs().is_empty() {
        transform_text(styles, layout_tree, inline_content, &mut first_letter_styles);
    }
    let mut glyph_metrics = GlyphRegistry::new(glyph_metrics);

    let mut box_family: Vec<Option<String>> = vec![None; layout_tree.box_count()];
    let mut boxes_with_conditional_hyphens = vec![false; layout_tree.box_count()];
    for (box_idx, range) in text_runs(inline_content) {
        boxes_with_conditional_hyphens[box_idx] = range.clone().any(|glyph_idx| inline_content.glyph_at(glyph_idx as usize) == Some('\u{00ad}' as GlyphId));
    }
    let mut ellipsis_boxes = Vec::new();
    let mut hyphen_boxes = Vec::new();
    let empty_inline_metrics_required = inline_content.inline_items().iter().any(|item| matches!(item.kind, InlineItemKind::InlineBoundary { inline_start, inline_end } if inline_start == inline_end));
    let mut dense_box_metrics_required = empty_inline_metrics_required;
    let mut font_content_extents_required = vec![false; layout_tree.box_count()];
    for (idx, family) in box_family.iter_mut().enumerate() {
        let style = get_style(styles, layout_tree, idx);
        let paints_inline_content_box = matches!(layout_tree.box_at(idx).map(|layout_box| layout_box.layout_mode()), Some(LayoutMode::Inline(_)))
            && (style.background_color() & 0xff != 0
                || style.background_image_present()
                || !matches!(style.border_top_style(), BorderStyle::None | BorderStyle::Hidden)
                || !matches!(style.border_right_style(), BorderStyle::None | BorderStyle::Hidden)
                || !matches!(style.border_bottom_style(), BorderStyle::None | BorderStyle::Hidden)
                || !matches!(style.border_left_style(), BorderStyle::None | BorderStyle::Hidden)
                || !matches!(style.outline().style, BorderStyle::None | BorderStyle::Hidden));
        // A non-replaced inline's painted content box uses the selected
        // face's ascent and descent, independently of line-height. Request
        // those metrics even when no font-relative CSS length needs them.
        dense_box_metrics_required |= style.requires_font_metrics();
        font_content_extents_required[idx] = !style.text_decoration().lines.is_empty() || paints_inline_content_box;
        if let Some(family_idx) = style.font_family() {
            *family = styles.string(family_idx).map(str::to_owned);
        }
        if style.text_overflow() == TextOverflow::Ellipsis && style.overflow_x().clips() {
            ellipsis_boxes.push(idx);
        }
        if style.hyphens() == html_style_model::Hyphens::Auto || (style.hyphens() == html_style_model::Hyphens::Manual && boxes_with_conditional_hyphens[idx]) {
            hyphen_boxes.push(idx);
        }
    }

    // The inline content box is positioned within a line whose root strut is
    // owned by the first non-inline ancestor. Keep that container and every
    // intervening inline ancestor on the same selected-font metric path.
    let extent_boxes = font_content_extents_required.iter().enumerate().filter_map(|(box_idx, required)| required.then_some(box_idx)).collect::<Vec<_>>();
    for box_idx in extent_boxes {
        let mut current = layout_tree.get_box_parent(box_idx);
        while let Some(parent_idx) = current {
            font_content_extents_required[parent_idx] = true;
            if !matches!(layout_tree.get_box_layout_mode(parent_idx), Some(LayoutMode::Inline(_))) {
                break;
            }
            current = layout_tree.get_box_parent(parent_idx);
        }
    }

    let mut font_metric_cache = FxHashMap::<(u32, u16, FontSlant, Option<String>), FontRelativeMetrics>::default();
    let box_metrics = if dense_box_metrics_required {
        let mut metrics = Vec::with_capacity(layout_tree.box_count());
        for (box_idx, family) in box_family.iter().enumerate() {
            let style = get_style(styles, layout_tree, box_idx);
            let key = (style.font_size().to_bits(), style.font_weight(), style.font_style().into(), family.clone());
            let font_metrics = if let Some(metrics) = font_metric_cache.get(&key).copied() {
                metrics
            } else {
                let metrics = if let Some(request) = FontMetricsRequest::new(style.font_size(), style.font_weight(), style.font_style().into(), family.as_deref()) {
                    glyph_shaper.font_relative_metrics(request)?
                } else {
                    FontRelativeMetrics::fallback()
                };
                font_metric_cache.insert(key, metrics);
                metrics
            };
            metrics.push(font_metrics);
        }
        BoxFontMetrics::Dense(metrics)
    } else if font_content_extents_required.iter().any(|required| *required) {
        let mut metric_ids = vec![0; layout_tree.box_count()];
        let mut distinct_metrics = Vec::<FontRelativeMetrics>::new();
        for (box_idx, family) in box_family.iter().enumerate().filter(|(box_idx, _)| font_content_extents_required[*box_idx]) {
            let style = get_style(styles, layout_tree, box_idx);
            let key = (style.font_size().to_bits(), style.font_weight(), style.font_style().into(), family.clone());
            let font_metrics = if let Some(metrics) = font_metric_cache.get(&key).copied() {
                metrics
            } else {
                let metrics = if let Some(request) = FontMetricsRequest::new(style.font_size(), style.font_weight(), style.font_style().into(), family.as_deref()) {
                    glyph_shaper.font_relative_metrics(request)?
                } else {
                    FontRelativeMetrics::fallback()
                };
                font_metric_cache.insert(key, metrics);
                metrics
            };
            let metric_idx = distinct_metrics.iter().position(|metrics| *metrics == font_metrics).unwrap_or_else(|| {
                distinct_metrics.push(font_metrics);
                distinct_metrics.len() - 1
            });
            metric_ids[box_idx] = u32::try_from(metric_idx + 1).expect("too many distinct shaped font metrics");
        }
        BoxFontMetrics::Indexed { metric_ids, metrics: distinct_metrics }
    } else {
        BoxFontMetrics::NotRequired
    };

    // Table columns are style-bearing CSS boxes but do not generate layout
    // boxes. Resolve metrics for their opaque style handles here so layout
    // never needs to inspect an unresolved computed width.
    let mut non_box_styles = Vec::new();
    for box_idx in 0..layout_tree.box_count() {
        let Some(LayoutMode::Table(table)) = layout_tree.box_at(box_idx).map(|layout_box| layout_box.layout_mode()) else {
            continue;
        };
        for hint in &table.column_width_hints {
            if !non_box_styles.contains(&hint.style) {
                non_box_styles.push(hint.style);
            }
        }
    }
    let non_box_metrics_required = non_box_styles.iter().copied().any(|indices| styles.view(indices).expect("validated style handle").requires_font_metrics());
    let non_box_metrics = if non_box_metrics_required {
        let mut metrics = FxHashMap::default();
        for indices in non_box_styles {
            let style = styles.view(indices).expect("validated style handle");
            let family = style.font_family().and_then(|family_idx| styles.string(family_idx));
            let key = (style.font_size().to_bits(), style.font_weight(), style.font_style().into(), family.map(str::to_owned));
            let font_metrics = if let Some(metrics) = font_metric_cache.get(&key).copied() {
                metrics
            } else {
                let metrics =
                    if let Some(request) = FontMetricsRequest::new(style.font_size(), style.font_weight(), style.font_style().into(), family) { glyph_shaper.font_relative_metrics(request)? } else { FontRelativeMetrics::fallback() };
                font_metric_cache.insert(key, metrics);
                metrics
            };
            metrics.insert(indices, font_metrics);
        }
        RequiredFontMetrics::Required(metrics)
    } else {
        RequiredFontMetrics::NotRequired
    };
    let (root_ch_px, root_cap_height_px, root_line_height_px) = layout_tree
        .root_box()
        .map(|root_box| {
            let style = get_style(styles, layout_tree, root_box);
            let metrics = match &box_metrics {
                BoxFontMetrics::NotRequired => FontRelativeMetrics::fallback(),
                BoxFontMetrics::Dense(values) => values[root_box],
                BoxFontMetrics::Indexed { metric_ids, metrics } => metric_ids[root_box].checked_sub(1).map_or_else(FontRelativeMetrics::fallback, |id| metrics[id as usize]),
            };
            let font_size = style.resolved_font_size(metrics.x_height_ratio(), metrics.ch_advance_ratio(), metrics.cap_height_ratio()).unwrap_or_else(|| style.font_size());
            let line_height = if style.line_height_is_normal() { font_size * 1.2 } else { style.resolved_line_height(metrics.x_height_ratio()).unwrap_or_else(|| style.line_height()).max(0.0) };
            (font_size * metrics.ch_advance_ratio(), font_size * metrics.cap_height_ratio(), line_height)
        })
        .unwrap_or((0.0, 0.0, 0.0));
    let font_metrics = ShapedFontMetrics::new(box_metrics, non_box_metrics, layout_tree.box_count(), root_ch_px, root_cap_height_px, root_line_height_px);

    let mut text_geometry = ShapedTextGeometry::with_len(inline_content.glyphs().len());
    let mut source_indices = Vec::<u32>::new();
    for span in plan_shaping_spans(styles, layout_tree, inline_content, &first_letter_styles, &font_metrics) {
        let box_idx = span.shaping_box;
        source_indices.clear();
        source_indices.extend(span.character_indices());
        debug_assert_eq!(source_indices.len(), span.character_count());
        let contiguous_range = span.contiguous_range();
        let style_override = span.style_override;
        let style = style_override.and_then(|indices| styles.view(indices)).unwrap_or_else(|| get_style(styles, layout_tree, box_idx));
        let metrics = font_metrics.for_box(box_idx);
        let font_size = style
            .resolved_font_size_with_root(metrics.x_height_ratio(), metrics.ch_advance_ratio(), metrics.cap_height_ratio(), font_metrics.root_ch_px(), font_metrics.root_cap_height_px(), font_metrics.root_line_height_px())
            .unwrap_or_else(|| style.font_size());
        let font_weight = style.font_weight();
        let font_style = style.font_style();
        let color = style.color();
        let family = style.font_family().and_then(|family_idx| styles.string(family_idx));
        let font_features = style.open_type_features();

        let mut normalized_text = String::new();
        let mut normalized_characters = Vec::with_capacity(source_indices.len());
        for &i in &source_indices {
            let character = char::from_u32(inline_content.glyph_at(i as usize).unwrap_or('?' as GlyphId)).unwrap_or('?');

            normalized_text.push(character);
            normalized_characters.push(character);
        }

        let style_span = TextStyleSpan::new(0..normalized_text.len(), font_size, font_weight, font_style.into(), color, family).map(|style| style.with_font_features(font_features));
        let paint_colors = span.paint_colors(styles, layout_tree);
        let mut scattered_glyphs = contiguous_range.is_none().then(|| source_indices.iter().map(|&index| inline_content.glyph_at(index as usize).unwrap_or_default()).collect::<Vec<_>>());
        let native_geometry = if let Some(request) = style_span.and_then(|style| TextRunShapeRequest::new(&normalized_text, style)).and_then(|request| request.with_paint_colors(&paint_colors)) {
            let run_glyphs = if let Some(range) = &contiguous_range { &mut inline_content.glyphs_mut()[range.start as usize..range.end as usize] } else { scattered_glyphs.as_mut().expect("discontiguous shaping buffer").as_mut_slice() };
            let geometry = if normalized_characters.contains(&'\n') {
                // Segment breaks terminate shaping runs. Asking a backend to
                // shape text and a newline together can change the preceding
                // line's ascent/baseline (and is invalid for several native
                // shaping APIs), so register these glyphs independently.
                for (glyph, &character) in run_glyphs.iter_mut().zip(&normalized_characters) {
                    *glyph = glyph_shaper.shape_glyph(&mut glyph_metrics, character, font_size, font_weight, font_style.into(), color, family)?;
                }
                None
            } else {
                glyph_shaper.shape_glyph_run(&mut glyph_metrics, request, run_glyphs)?
            };
            for &glyph in run_glyphs.iter() {
                if !glyph_metrics.contains(glyph) {
                    return Err(ShapeError::unregistered_glyph_id(glyph, glyph_metrics.len()));
                }
            }
            geometry
        } else {
            None
        };
        if let Some(glyphs) = scattered_glyphs {
            for (&index, glyph) in source_indices.iter().zip(glyphs) {
                inline_content.glyphs_mut()[index as usize] = glyph;
            }
        }
        if let Some(native_geometry) = native_geometry.filter(|geometry| geometry.advances().len() == normalized_characters.len()) {
            for (offset, &index) in source_indices.iter().enumerate() {
                text_geometry.advances[index as usize] = native_geometry.advances()[offset];
                text_geometry.cluster_boundaries[index as usize] &= native_geometry.cluster_boundaries()[offset];
            }
            if let Some(&last) = source_indices.last() {
                text_geometry.cluster_boundaries[last as usize + 1] &= native_geometry.cluster_boundaries()[source_indices.len()];
            }
            if let Some(range) = contiguous_range
                && let (Some(backend_run), Some(caret_stops)) = (native_geometry.backend_run(), native_geometry.caret_stops())
            {
                text_geometry.authoritative_runs.push(AuthoritativeShapedRun {
                    source_range: range.clone(),
                    backend_run,
                    caret_stops: Arc::from(caret_stops),
                    ascent: native_geometry.ascent(),
                    placement_required: span.placement_required(styles, layout_tree),
                });
            }
        } else {
            for (offset, &index) in source_indices.iter().enumerate() {
                let glyph = inline_content.glyph_at(index as usize).unwrap_or_default();
                let metric = glyph_metrics.get(glyph).expect("registered glyph metric");
                text_geometry.advances[index as usize] = metric.advance();
                debug_assert_eq!(metric.ch(), normalized_characters[offset]);
            }
        }
        // U+00AD is a conditional break marker, not visible inline content.
        // Some shaping backends already report a zero advance, but enforcing
        // it here keeps layout independent of backend/font behavior.
        for (offset, character) in normalized_characters.iter().copied().enumerate() {
            if character == '\u{00ad}' {
                text_geometry.advances[source_indices[offset] as usize] = 0.0;
            }
        }
    }

    // Text-overflow markers are shaped once per applicable container so width
    // changes can relayout without calling the renderer's font backend again.
    // They are not inserted into InlineContent: synthetic paint must not become
    // selectable or acquire a DOM source position.
    let mut ellipsis_glyphs = FxHashMap::default();
    for box_idx in ellipsis_boxes {
        let style = get_style(styles, layout_tree, box_idx);
        let family = box_family[box_idx].as_deref();
        let metrics = font_metrics.for_box(box_idx);
        let font_size = style
            .resolved_font_size_with_root(metrics.x_height_ratio(), metrics.ch_advance_ratio(), metrics.cap_height_ratio(), font_metrics.root_ch_px(), font_metrics.root_cap_height_px(), font_metrics.root_line_height_px())
            .unwrap_or_else(|| style.font_size());
        let glyph = glyph_shaper.shape_glyph(&mut glyph_metrics, '\u{2026}', font_size, style.font_weight(), style.font_style().into(), style.color(), family)?;
        if !glyph_metrics.contains(glyph) {
            return Err(ShapeError::unregistered_glyph_id(glyph, glyph_metrics.len()));
        }
        ellipsis_glyphs.insert(box_idx as u32, glyph);
    }
    // Automatic hyphens are synthetic paint artifacts, just like ellipses:
    // shape them once per applicable style without assigning DOM positions.
    let mut hyphen_glyphs = FxHashMap::default();
    for box_idx in hyphen_boxes {
        let style = get_style(styles, layout_tree, box_idx);
        let family = box_family[box_idx].as_deref();
        let metrics = font_metrics.for_box(box_idx);
        let font_size = style
            .resolved_font_size_with_root(metrics.x_height_ratio(), metrics.ch_advance_ratio(), metrics.cap_height_ratio(), font_metrics.root_ch_px(), font_metrics.root_cap_height_px(), font_metrics.root_line_height_px())
            .unwrap_or_else(|| style.font_size());
        let glyph = glyph_shaper.shape_glyph(&mut glyph_metrics, '\u{2010}', font_size, style.font_weight(), style.font_style().into(), style.color(), family)?;
        if !glyph_metrics.contains(glyph) {
            return Err(ShapeError::unregistered_glyph_id(glyph, glyph_metrics.len()));
        }
        hyphen_glyphs.insert(box_idx as u32, glyph);
    }
    Ok((ellipsis_glyphs, hyphen_glyphs, text_geometry, font_metrics))
}

fn collapse_whitespace(styles: &ComputedStyles, layout_tree: &LayoutTree, inline_content: &mut InlineContent) {
    let mut new_glyphs = Vec::with_capacity(inline_content.glyphs().len());
    let mut new_source_offsets = Vec::with_capacity(inline_content.glyphs().len());
    let mut new_whitespace_wrap_before = Vec::with_capacity(inline_content.glyphs().len());
    let mut remap = vec![u32::MAX; inline_content.glyphs().len()];
    let mut at_line_start = true;
    let mut pending_space: Option<(usize, u32, bool)> = None;
    let mut current_context = None;

    // Non-text inline items participate in whitespace collapsing too. A
    // pending segment-break space before an image or atomic inline box must be
    // committed, while whitespace at the true end of a formatting context is
    // discarded. Preserve run order here instead of filtering down to text
    // runs and losing those boundaries.
    let inline_items = inline_content.inline_items().to_vec();
    for run in &inline_items {
        // Out-of-flow anchors and formatting-only boundaries are absent from
        // the inline text stream. They neither commit nor discard pending
        // whitespace and cannot start a new whitespace context.
        if matches!(run.kind, InlineItemKind::FloatAnchor { .. } | InlineItemKind::AbsoluteAnchor { .. } | InlineItemKind::InlineBoundary { .. }) {
            continue;
        }
        let context_box = match &run.kind {
            InlineItemKind::Image { .. } | InlineItemKind::AtomicBox { .. } => layout_tree.get_box_parent(run.box_idx as usize).unwrap_or(run.box_idx as usize),
            _ => run.box_idx as usize,
        };
        let context = whitespace_context_root(layout_tree, context_box);
        if current_context != Some(context) {
            if current_context.is_some_and(|previous| layout_box_is_descendant_of(layout_tree, context, previous)) {
                // Atomic descendants allocate their runs before the carrier
                // token in the parent's run arena. Entering that nested
                // formatting context therefore follows the parent's pending
                // whitespace logically; commit it before shaping descendants.
                if let Some((source, offset, wrap_before)) = pending_space.take() {
                    remap[source] = new_glyphs.len() as u32;
                    new_glyphs.push(' ' as GlyphId);
                    new_source_offsets.push(offset);
                    new_whitespace_wrap_before.push(WhitespaceWrapOverride::retained(wrap_before));
                }
            } else {
                // A trailing collapsible space is discarded between sibling
                // formatting contexts and must not leak into the next block.
                pending_space = None;
            }
            at_line_start = true;
            current_context = Some(context);
        }
        let (InlineItemKind::Text { glyphs: range } | InlineItemKind::Marker { glyphs: range }) = &run.kind else {
            match run.kind {
                InlineItemKind::Image { .. } | InlineItemKind::AtomicBox { .. } => {
                    if let Some((source, offset, wrap_before)) = pending_space.take() {
                        remap[source] = new_glyphs.len() as u32;
                        new_glyphs.push(' ' as GlyphId);
                        new_source_offsets.push(offset);
                        new_whitespace_wrap_before.push(WhitespaceWrapOverride::retained(wrap_before));
                    }
                    at_line_start = false;
                }
                InlineItemKind::Break { .. } => {
                    pending_space = None;
                    at_line_start = true;
                }
                InlineItemKind::FloatAnchor { .. } | InlineItemKind::AbsoluteAnchor { .. } | InlineItemKind::InlineBoundary { .. } => unreachable!("non-textual boundaries are filtered before whitespace context selection"),
                InlineItemKind::Text { .. } | InlineItemKind::Marker { .. } => unreachable!(),
            }
            continue;
        };
        let white_space = get_style(styles, layout_tree, run.box_idx as usize).white_space();
        let collapse = white_space.collapses_spaces();
        let preserve_newlines = white_space.preserves_newlines();

        for i in range.clone() {
            let i = i as usize;
            let char_code = inline_content.glyph_at(i).unwrap_or('?' as GlyphId);
            let mut character = char::from_u32(char_code).unwrap_or('?');
            // CSS Text treats CRLF as one segment break and a standalone CR as
            // an LF. Character references can introduce CR after HTML input
            // preprocessing, so this normalization belongs at the whitespace
            // boundary rather than only in the byte parser.
            if character == '\r' {
                let followed_by_lf = (i + 1) < range.end as usize && inline_content.glyph_at(i + 1).and_then(char::from_u32) == Some('\n');
                if followed_by_lf {
                    continue;
                }
                character = '\n';
            }
            if character == '\n' {
                if preserve_newlines {
                    // Collapsible spaces immediately before a segment break
                    // are removed before that break is preserved.
                    pending_space = None;
                    remap[i] = new_glyphs.len() as u32;
                    new_glyphs.push('\n' as GlyphId);
                    new_source_offsets.push(inline_content.raw_glyph_source_offset(i));
                    new_whitespace_wrap_before.push(WhitespaceWrapOverride::Style);
                    at_line_start = true;
                } else if collapse && !at_line_start {
                    // The segment break becomes one collapsible space. Keep an
                    // already-pending space as the representative of the
                    // sequence so its originating inline style paints the
                    // collapsed glyph (CSS2 collapsing can cross element and
                    // display:none boundaries).
                    if let Some((_, _, wrap_before)) = pending_space.as_mut() {
                        *wrap_before |= white_space.allows_wrap();
                    } else {
                        pending_space = Some((i, inline_content.raw_glyph_source_offset(i), white_space.allows_wrap()));
                    }
                }
            } else if is_css_collapsible_space(character) {
                if collapse {
                    if !at_line_start {
                        if let Some((_, _, wrap_before)) = pending_space.as_mut() {
                            *wrap_before |= white_space.allows_wrap();
                        } else {
                            pending_space = Some((i, inline_content.raw_glyph_source_offset(i), white_space.allows_wrap()));
                        }
                    }
                } else {
                    if let Some((source, offset, _)) = pending_space.take() {
                        remap[source] = new_glyphs.len() as u32;
                        new_glyphs.push(' ' as GlyphId);
                        new_source_offsets.push(offset);
                        // A collapsible space immediately adjoining a
                        // preserved space cannot introduce a break into the
                        // unwrappable preserved sequence.
                        new_whitespace_wrap_before.push(WhitespaceWrapOverride::Suppress);
                    }
                    remap[i] = new_glyphs.len() as u32;
                    new_glyphs.push(char_code);
                    new_source_offsets.push(inline_content.raw_glyph_source_offset(i));
                    new_whitespace_wrap_before.push(WhitespaceWrapOverride::Style);
                    at_line_start = false;
                }
            } else {
                if let Some((source, offset, wrap_before)) = pending_space.take() {
                    remap[source] = new_glyphs.len() as u32;
                    new_glyphs.push(' ' as GlyphId);
                    new_source_offsets.push(offset);
                    new_whitespace_wrap_before.push(WhitespaceWrapOverride::retained(wrap_before));
                }
                remap[i] = new_glyphs.len() as u32;
                new_glyphs.push(char_code);
                new_source_offsets.push(inline_content.raw_glyph_source_offset(i));
                new_whitespace_wrap_before.push(WhitespaceWrapOverride::Style);
                at_line_start = false;
            }
        }
    }

    for run in inline_content.inline_items_mut() {
        let (InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs }) = &mut run.kind else {
            continue;
        };
        let old_start = glyphs.start as usize;
        let old_end = glyphs.end as usize;
        let new_start = (old_start..old_end).find_map(|i| remap.get(i).copied().filter(|mapped| *mapped != u32::MAX));
        let new_end = (old_start..old_end).filter_map(|i| remap.get(i).copied().filter(|mapped| *mapped != u32::MAX)).next_back().map(|mapped| mapped + 1);
        *glyphs = match (new_start, new_end) {
            (Some(start), Some(end)) => start..end,
            _ => 0..0,
        };
    }
    inline_content.replace_glyphs(new_glyphs, new_source_offsets, new_whitespace_wrap_before);
}

/// Applies CSS case conversion to the normalized character stream before any
/// shaping spans are selected. Case conversion is not necessarily one-to-one
/// (`ß` uppercases to `SS`), so this pass rebuilds the dense stream and keeps
/// every generated scalar attached to the originating DOM source offset.
fn transform_text(styles: &ComputedStyles, layout_tree: &LayoutTree, inline_content: &mut InlineContent, first_letter_styles: &mut Vec<Option<StyleIndices>>) {
    let old_len = inline_content.glyphs().len();
    let mut transforms = vec![TextTransform::None; old_len];
    for item in inline_content.inline_items() {
        let (InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs }) = &item.kind else { continue };
        let base_style = get_style(styles, layout_tree, item.box_idx as usize);
        for glyph_idx in glyphs.clone() {
            let index = glyph_idx as usize;
            transforms[index] = first_letter_styles.get(index).copied().flatten().and_then(|indices| styles.view(indices)).unwrap_or(base_style).text_transform();
        }
    }
    if transforms.iter().all(|transform| *transform == TextTransform::None) {
        return;
    }

    let capitalize_starts = capitalization_starts(layout_tree, inline_content);
    let mut new_glyphs = Vec::with_capacity(old_len);
    let mut new_source_offsets = Vec::with_capacity(old_len);
    let mut new_whitespace_wrap_before = Vec::with_capacity(old_len);
    let mut new_first_letter_styles = Vec::with_capacity(old_len);
    let mut remap_start = vec![0u32; old_len];
    let mut remap_end = vec![0u32; old_len];

    for index in 0..old_len {
        remap_start[index] = new_glyphs.len() as u32;
        let character = char::from_u32(inline_content.glyph_at(index).unwrap_or('?' as GlyphId)).unwrap_or('?');
        let source_offset = inline_content.raw_glyph_source_offset(index);
        let wrap_override = inline_content.whitespace_wrap_before(index);
        let first_letter_style = first_letter_styles.get(index).copied().flatten();
        let mut first_output = true;
        let mut append = |transformed: char| {
            new_glyphs.push(transformed as GlyphId);
            new_source_offsets.push(source_offset);
            new_whitespace_wrap_before.push(if first_output { wrap_override } else { WhitespaceWrapOverride::Style });
            new_first_letter_styles.push(first_letter_style);
            first_output = false;
        };

        match transforms[index] {
            TextTransform::Uppercase => append_css_uppercase(character, &mut append),
            TextTransform::Lowercase => character.to_lowercase().for_each(&mut append),
            TextTransform::Capitalize if capitalize_starts.contains(&index) => append_css_uppercase(character, &mut append),
            TextTransform::Capitalize | TextTransform::None => append(character),
        }
        remap_end[index] = new_glyphs.len() as u32;
    }

    for item in inline_content.inline_items_mut() {
        let (InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs }) = &mut item.kind else { continue };
        if glyphs.start >= glyphs.end {
            *glyphs = 0..0;
            continue;
        }
        let old_start = glyphs.start as usize;
        let old_end = glyphs.end as usize;
        *glyphs = remap_start[old_start]..remap_end[old_end - 1];
    }
    inline_content.replace_glyphs(new_glyphs, new_source_offsets, new_whitespace_wrap_before);
    *first_letter_styles = new_first_letter_styles;
}

fn append_css_uppercase(character: char, append: &mut impl FnMut(char)) {
    // The pinned CSS2 oracle treats Georgian Mkhedruli as a unicase script.
    // Keep that compatibility mapping explicit instead of inheriting changes
    // in the Rust toolchain's Unicode data version.
    if ('\u{10d0}'..='\u{10ff}').contains(&character) {
        append(character);
    } else {
        character.to_uppercase().for_each(append);
    }
}

/// Returns glyph indices containing the first typographic letter of each
/// Unicode word. Formatting boundaries are deliberately absent from the word
/// string, while forced breaks and atomic inline content introduce separators.
fn capitalization_starts(layout_tree: &LayoutTree, inline_content: &InlineContent) -> FxHashSet<usize> {
    fn publish(text: &mut String, glyphs_by_byte: &mut Vec<(usize, usize)>, starts: &mut FxHashSet<usize>) {
        for (word_start, word) in text.unicode_word_indices() {
            let Some((relative, _)) = word.char_indices().find(|(_, character)| character.is_alphabetic()) else { continue };
            let letter_byte = word_start + relative;
            if let Ok(position) = glyphs_by_byte.binary_search_by_key(&letter_byte, |(byte, _)| *byte) {
                starts.insert(glyphs_by_byte[position].1);
            }
        }
        text.clear();
        glyphs_by_byte.clear();
    }

    let mut starts = FxHashSet::default();
    let mut text = String::new();
    let mut glyphs_by_byte = Vec::new();
    let mut current_context = None;
    for item in inline_content.inline_items() {
        // Out-of-flow boxes and formatting boundaries have no textual content
        // and must not affect word boundaries on either side of them.
        if matches!(item.kind, InlineItemKind::FloatAnchor { .. } | InlineItemKind::AbsoluteAnchor { .. } | InlineItemKind::InlineBoundary { .. }) {
            continue;
        }
        let context_box = match item.kind {
            InlineItemKind::Image { .. } | InlineItemKind::AtomicBox { .. } => layout_tree.get_box_parent(item.box_idx as usize).unwrap_or(item.box_idx as usize),
            _ => item.box_idx as usize,
        };
        let context = whitespace_context_root(layout_tree, context_box);
        if current_context != Some(context) {
            publish(&mut text, &mut glyphs_by_byte, &mut starts);
            current_context = Some(context);
        }
        match &item.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => {
                for glyph_idx in glyphs.clone() {
                    let character = char::from_u32(inline_content.glyph_at(glyph_idx as usize).unwrap_or('?' as GlyphId)).unwrap_or('?');
                    glyphs_by_byte.push((text.len(), glyph_idx as usize));
                    text.push(character);
                }
            }
            InlineItemKind::Break { .. } | InlineItemKind::Image { .. } | InlineItemKind::AtomicBox { .. } => text.push('\n'),
            InlineItemKind::FloatAnchor { .. } | InlineItemKind::AbsoluteAnchor { .. } | InlineItemKind::InlineBoundary { .. } => unreachable!("non-textual boundaries are filtered before context selection"),
        }
    }
    publish(&mut text, &mut glyphs_by_byte, &mut starts);
    starts
}

/// CSS white-space collapsing applies to document spaces and tabs, not every
/// Unicode character classified as whitespace. In particular, typographic
/// spaces such as EN QUAD (U+2000) and IDEOGRAPHIC SPACE (U+3000) remain
/// visible under `white-space: normal`.
fn is_css_collapsible_space(character: char) -> bool {
    matches!(character, ' ' | '\t')
}

pub(crate) fn whitespace_context_root(layout_tree: &LayoutTree, mut box_idx: usize) -> usize {
    while matches!(layout_tree.box_at(box_idx).map(|box_| box_.layout_mode()), Some(LayoutMode::Inline(_))) {
        let Some(parent) = layout_tree.get_box_parent(box_idx) else { break };
        box_idx = parent;
    }
    box_idx
}

fn layout_box_is_descendant_of(layout_tree: &LayoutTree, mut box_idx: usize, ancestor: usize) -> bool {
    while let Some(parent) = layout_tree.get_box_parent(box_idx) {
        if parent == ancestor {
            return true;
        }
        box_idx = parent;
    }
    false
}

fn get_style<'a>(styles: &'a ComputedStyles, layout_tree: &LayoutTree, box_idx: usize) -> StyleView<'a> {
    let indices = layout_tree.get_box_style_indices(box_idx).unwrap_or_else(|| styles.default_indices());
    styles.view(indices).expect("validated style handle")
}

/// Resolve the characters generated by `::first-letter` before shaping. CSS2
/// includes adjacent Ps/Pe/Pi/Pf/Po punctuation in the pseudo-element; doing
/// this here lets the backend shape, size, and paint the whole typographic
/// unit with the pseudo style instead of trying to resize already-shaped
/// glyphs during line layout.
pub(crate) fn first_letter_style_overrides(document: &Document, styles: &ComputedStyles, layout_tree: &LayoutTree, inline_content: &InlineContent) -> Vec<Option<StyleIndices>> {
    // Preserve source order. An ancestor's `::first-letter` belongs to the
    // first in-flow descendant that contributes text, not to every descendant
    // formatting context carrying that ancestor in its box chain.
    let mut root_indices = FxHashMap::<usize, usize>::default();
    let mut content_by_root = Vec::<(usize, Vec<(u32, char, usize)>)>::new();
    for run in inline_content.inline_items() {
        let InlineItemKind::Text { glyphs } = &run.kind else { continue };
        let root = whitespace_context_root(layout_tree, run.box_idx as usize);
        let entry = if let Some(&entry) = root_indices.get(&root) {
            entry
        } else {
            let entry = content_by_root.len();
            content_by_root.push((root, Vec::new()));
            root_indices.insert(root, entry);
            entry
        };
        content_by_root[entry].1.extend(glyphs.clone().map(|glyph_idx| {
            let character = char::from_u32(inline_content.glyph_at(glyph_idx as usize).unwrap_or('?' as GlyphId)).unwrap_or('?');
            (glyph_idx, character, run.box_idx as usize)
        }));
    }

    let mut overrides = vec![None; inline_content.glyphs().len()];
    let mut consumed_owners = FxHashSet::<u32>::default();
    for (root, content) in content_by_root {
        let Some(mut start) = content.iter().position(|(_, character, _)| !character.is_whitespace()) else { continue };
        consume_blocked_first_letter_owners(document, styles, layout_tree, root, &mut consumed_owners);
        let Some((owner, pseudo_style, eligible)) = pseudo_style_owner(document, styles, layout_tree, root, PseudoStyleKind::FirstLetter) else { continue };
        if !consumed_owners.insert(owner.raw()) {
            continue;
        }
        if !eligible {
            continue;
        }

        // Leading qualifying punctuation belongs to the first-letter unit.
        let selection_start = start;
        while start < content.len() && is_first_letter_punctuation(content[start].1) {
            start += 1;
        }

        // Select the first typographic character after the punctuation. A
        // punctuation-only block still selects its leading punctuation.
        let mut selection_end = start.min(content.len());
        if start < content.len() && !content[start].1.is_whitespace() {
            selection_end = start + 1;
            while selection_end < content.len() && content[selection_end].1.is_mark() {
                selection_end += 1;
            }
        }
        while selection_end < content.len() && is_first_letter_punctuation(content[selection_end].1) {
            selection_end += 1;
        }

        let before_style = styles.before_style_for_node(owner).map(|(style, _, _)| style);
        let generated_style = styles.before_first_letter_style_for_node(owner);
        for &(glyph_idx, _, box_idx) in &content[selection_start..selection_end] {
            let comes_from_before = before_style.is_some_and(|style| layout_tree.get_box_style_indices(box_idx) == Some(style));
            overrides[glyph_idx as usize] = Some(if comes_from_before { generated_style.unwrap_or(pseudo_style) } else { pseudo_style });
        }
    }
    overrides
}

/// A flex or grid container does not expose a first formatted letter from its
/// items to its own (or an ancestor's) `::first-letter`. Those blocked owners
/// still have to be consumed in source order: otherwise an ancestor pseudo
/// incorrectly skips the container and styles text in a later sibling.
fn consume_blocked_first_letter_owners(document: &Document, styles: &ComputedStyles, layout_tree: &LayoutTree, mut root: usize, consumed_owners: &mut FxHashSet<u32>) {
    let mut crossed_flex_or_grid = false;
    loop {
        if crossed_flex_or_grid {
            if let Some(node) = layout_tree.get_box_dom_element(root).and_then(|raw| document.node_id_from_raw(raw)) {
                if styles.first_letter_style_for_node(node).is_some() {
                    consumed_owners.insert(node.raw());
                }
            }
        }

        let Some(parent) = layout_tree.get_box_parent(root) else { break };
        if matches!(layout_tree.box_at(parent).map(|box_| box_.layout_mode()), Some(LayoutMode::Flex(_) | LayoutMode::Grid(_))) {
            crossed_flex_or_grid = true;
        }
        root = parent;
    }
}

fn pseudo_owner_is_blocked_at_root(layout_tree: &LayoutTree, mut root: usize, owner: html_dom::DomNodeId) -> bool {
    let mut crossed_flex_or_grid = false;
    loop {
        if crossed_flex_or_grid && layout_tree.get_box_dom_element(root) == Some(owner.raw()) {
            return true;
        }
        let Some(parent) = layout_tree.get_box_parent(root) else { return false };
        if matches!(layout_tree.box_at(parent).map(|box_| box_.layout_mode()), Some(LayoutMode::Flex(_) | LayoutMode::Grid(_))) {
            crossed_flex_or_grid = true;
        }
        root = parent;
    }
}

#[derive(Clone, Copy)]
pub(crate) enum PseudoStyleKind {
    FirstLine,
    FirstLetter,
}

/// Find the nearest originating element whose pseudo-element can apply to an
/// inline formatting context. Walking layout ancestry is essential because
/// CSS defines the first formatted line/letter through in-flow block
/// descendants, not only through text directly owned by the styled element.
pub(crate) fn pseudo_style_owner(document: &Document, styles: &ComputedStyles, layout_tree: &LayoutTree, mut root: usize, kind: PseudoStyleKind) -> Option<(html_dom::DomNodeId, StyleIndices, bool)> {
    let mut eligible = true;
    loop {
        if let Some(node) = layout_tree.get_box_dom_element(root).and_then(|raw| document.node_id_from_raw(raw)) {
            let style = match kind {
                PseudoStyleKind::FirstLine => styles.first_line_style_for_node(node),
                PseudoStyleKind::FirstLetter => styles.first_letter_style_for_node(node),
            };
            if let Some(style) = style {
                return Some((node, style, eligible));
            }
        }
        let parent = layout_tree.get_box_parent(root)?;
        // Pseudos on an item itself remain eligible, but crossing into its
        // flex/grid container blocks pseudos found on that container or any
        // ancestor. Keep searching so the blocked owner is still consumed in
        // source order instead of incorrectly applying to a later sibling.
        if matches!(layout_tree.box_at(parent).map(|box_| box_.layout_mode()), Some(LayoutMode::Flex(_) | LayoutMode::Grid(_))) {
            eligible = false;
        }
        root = parent;
    }
}

/// Resolve `::first-line` for one inline formatting context, including the
/// CSS2 case where an ancestor's first formatted line occurs inside its first
/// in-flow block descendant. Only the first text-contributing root owned by a
/// pseudo-element is eligible.
pub(crate) fn first_line_style_for_inline_root(document: &Document, styles: &ComputedStyles, layout_tree: &LayoutTree, inline_content: &InlineContent, root: usize) -> Option<StyleIndices> {
    let (owner, style, eligible) = pseudo_style_owner(document, styles, layout_tree, root, PseudoStyleKind::FirstLine)?;
    if !eligible {
        return None;
    }
    for run in inline_content.inline_items() {
        let (InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs }) = &run.kind else { continue };
        if glyphs.start >= glyphs.end {
            continue;
        }
        let candidate_root = whitespace_context_root(layout_tree, run.box_idx as usize);
        let Some((candidate_owner, _, candidate_eligible)) = pseudo_style_owner(document, styles, layout_tree, candidate_root, PseudoStyleKind::FirstLine) else { continue };
        if candidate_owner == owner {
            return (candidate_root == root && candidate_eligible).then_some(style);
        }
    }
    None
}

pub(crate) fn first_letter_style_for_inline_root(document: &Document, styles: &ComputedStyles, layout_tree: &LayoutTree, inline_content: &InlineContent, root: usize) -> Option<StyleIndices> {
    let (owner, style, eligible) = pseudo_style_owner(document, styles, layout_tree, root, PseudoStyleKind::FirstLetter)?;
    if !eligible {
        return None;
    }
    for run in inline_content.inline_items() {
        let InlineItemKind::Text { glyphs } = &run.kind else { continue };
        if glyphs.start >= glyphs.end {
            continue;
        }
        let candidate_root = whitespace_context_root(layout_tree, run.box_idx as usize);
        if pseudo_owner_is_blocked_at_root(layout_tree, candidate_root, owner) {
            return None;
        }
        let Some((candidate_owner, _, candidate_eligible)) = pseudo_style_owner(document, styles, layout_tree, candidate_root, PseudoStyleKind::FirstLetter) else { continue };
        if candidate_owner == owner {
            return (candidate_root == root && candidate_eligible).then_some(style);
        }
    }
    None
}

/// Re-shape one normalized source range with a dynamically selected pseudo
/// style. Ordinary document shaping remains independent of viewport width;
/// this path is entered only for `::first-line` after composition identifies
/// the affected range.
pub(crate) fn reshape_range_with_style(
    styles: &ComputedStyles, inline_content: &mut InlineContent, glyph_metrics: &mut GlyphMetrics, text_geometry: &mut ShapedTextGeometry, range: Range<u32>, style_indices: StyleIndices, base_style_indices: StyleIndices,
    glyph_shaper: &mut impl GlyphShaper,
) -> Result<(), ShapeError> {
    if range.start >= range.end {
        return Ok(());
    }
    let style = styles.view(style_indices).expect("validated pseudo style handle");
    let base_style = styles.view(base_style_indices).expect("validated originating style handle");
    let characters = range.clone().map(|index| glyph_metrics.get(inline_content.glyph_at(index as usize).unwrap_or_default()).ch()).collect::<Vec<_>>();
    let text = characters.iter().collect::<String>();
    // Inline token construction still contributes the originating run's CSS
    // spacing. Put only the pseudo/base delta into authoritative geometry so
    // the final total is the computed `::first-line` spacing.
    let letter_spacing_delta = style.letter_spacing() - base_style.letter_spacing();
    let word_spacing_delta = style.word_spacing() - base_style.word_spacing();
    let spacing_deltas = characters.iter().map(|character| letter_spacing_delta + if *character == ' ' { word_spacing_delta } else { 0.0 }).collect::<Vec<_>>();
    let family = style.font_family().and_then(|family_idx| styles.string(family_idx));
    let Some(span) = TextStyleSpan::new(0..text.len(), style.font_size(), style.font_weight(), style.font_style().into(), style.color(), family).map(|span| span.with_font_features(style.open_type_features())) else {
        return Ok(());
    };
    let Some(request) = TextRunShapeRequest::new(&text, span) else { return Ok(()) };
    let start = range.start as usize;
    let end = range.end as usize;
    let mut registry = GlyphRegistry::new(glyph_metrics);
    let native = glyph_shaper.shape_glyph_run(&mut registry, request, &mut inline_content.glyphs_mut()[start..end])?;
    for &glyph in &inline_content.glyphs()[start..end] {
        if !registry.contains(glyph) {
            return Err(ShapeError::unregistered_glyph_id(glyph, registry.len()));
        }
    }

    text_geometry.authoritative_runs.retain(|run| run.source_range.end <= range.start || run.source_range.start >= range.end);
    if let Some(native) = native.filter(|geometry| geometry.advances().len() == characters.len()) {
        for ((target, advance), spacing) in text_geometry.advances[start..end].iter_mut().zip(native.advances()).zip(&spacing_deltas) {
            *target = *advance + *spacing;
        }
        text_geometry.cluster_boundaries[start..=end].copy_from_slice(native.cluster_boundaries());
        if let (Some(backend_run), Some(caret_stops)) = (native.backend_run(), native.caret_stops()) {
            let mut adjusted_stops = Vec::with_capacity(caret_stops.len());
            let mut cumulative_spacing = 0.0;
            for (offset, stop) in caret_stops.iter().copied().enumerate() {
                adjusted_stops.push(stop + cumulative_spacing);
                if let Some(delta) = spacing_deltas.get(offset) {
                    cumulative_spacing += *delta;
                }
            }
            text_geometry.authoritative_runs.push(AuthoritativeShapedRun {
                source_range: range,
                backend_run,
                caret_stops: Arc::from(adjusted_stops),
                ascent: native.ascent(),
                placement_required: style.letter_spacing() != 0.0 || style.word_spacing() != 0.0 || letter_spacing_delta != 0.0 || word_spacing_delta != 0.0,
            });
        }
    } else {
        for index in start..end {
            let glyph = inline_content.glyph_at(index).unwrap_or_default();
            text_geometry.advances[index] = registry.get(glyph).expect("registered pseudo glyph").advance() + spacing_deltas[index - start];
        }
        text_geometry.cluster_boundaries[start..=end].fill(true);
    }
    Ok(())
}

fn is_first_letter_punctuation(character: char) -> bool {
    character.is_punctuation_open() || character.is_punctuation_close() || character.is_punctuation_initial_quote() || character.is_punctuation_final_quote() || character.is_punctuation_other()
}

#[derive(Clone, Debug)]
struct ShapingPaintSpan {
    characters: Range<u32>,
    box_idx: usize,
    style_override: Option<StyleIndices>,
}

/// A contiguous unit sent to the font shaper. Source and paint ownership may
/// change inside a span, while font shaping remains contextual across those
/// boundaries.
#[derive(Clone, Debug)]
struct ShapingSpan {
    shaping_box: usize,
    formatting_root: usize,
    style_override: Option<StyleIndices>,
    paint_spans: Vec<ShapingPaintSpan>,
}

impl ShapingSpan {
    fn character_count(&self) -> usize {
        self.paint_spans.iter().map(|span| span.characters.len()).sum()
    }

    fn character_indices(&self) -> impl Iterator<Item = u32> + '_ {
        self.paint_spans.iter().flat_map(|span| span.characters.clone())
    }

    fn contiguous_range(&self) -> Option<Range<u32>> {
        let first = self.paint_spans.first()?.characters.clone();
        let mut range = first;
        for span in &self.paint_spans[1..] {
            if range.end != span.characters.start {
                return None;
            }
            range.end = span.characters.end;
        }
        Some(range)
    }

    fn paint_colors(&self, styles: &ComputedStyles, layout_tree: &LayoutTree) -> Vec<u32> {
        let mut colors = Vec::with_capacity(self.character_count());
        for span in &self.paint_spans {
            let style = span.style_override.and_then(|indices| styles.view(indices)).unwrap_or_else(|| get_style(styles, layout_tree, span.box_idx));
            colors.extend(std::iter::repeat_n(style.color(), span.characters.len()));
        }
        debug_assert_eq!(colors.len(), self.character_count());
        colors
    }

    fn placement_required(&self, styles: &ComputedStyles, layout_tree: &LayoutTree) -> bool {
        self.paint_spans.iter().any(|span| {
            let style = span.style_override.and_then(|indices| styles.view(indices)).unwrap_or_else(|| get_style(styles, layout_tree, span.box_idx));
            style.letter_spacing() != 0.0 || style.word_spacing() != 0.0
        })
    }
}

/// Derive typographic spans from the complete inline item stream. This must
/// observe non-text items: numerically adjacent glyph ranges are not
/// necessarily typographically adjacent when an empty atomic item, image, or
/// forced break occurs between them.
fn plan_shaping_spans(styles: &ComputedStyles, layout_tree: &LayoutTree, inline_content: &InlineContent, style_overrides: &[Option<StyleIndices>], font_metrics: &ShapedFontMetrics) -> Vec<ShapingSpan> {
    #[derive(Clone, Copy, Default)]
    struct ContextState {
        previous_span: Option<usize>,
        may_join: bool,
    }

    let mut result = Vec::new();
    let mut contexts = FxHashMap::<usize, ContextState>::default();

    for item in inline_content.inline_items() {
        let range = match &item.kind {
            InlineItemKind::Text { glyphs } if glyphs.start < glyphs.end => glyphs.clone(),
            InlineItemKind::Marker { glyphs } if glyphs.start < glyphs.end => {
                let root = whitespace_context_root(layout_tree, item.box_idx as usize);
                append_shaping_range(styles, layout_tree, font_metrics, &mut result, item.box_idx as usize, glyphs.clone(), None, None);
                contexts.insert(root, ContextState::default());
                continue;
            }
            InlineItemKind::InlineBoundary { inline_start, inline_end } => {
                if inline_boundary_breaks_shaping(styles, layout_tree, font_metrics, item.box_idx as usize, *inline_start, *inline_end) {
                    contexts.insert(whitespace_context_root(layout_tree, item.box_idx as usize), ContextState::default());
                }
                continue;
            }
            InlineItemKind::FloatAnchor { .. } | InlineItemKind::AbsoluteAnchor { .. } => continue,
            InlineItemKind::Text { .. } | InlineItemKind::Marker { .. } => continue,
            InlineItemKind::Image { .. } | InlineItemKind::Break { .. } | InlineItemKind::AtomicBox { .. } => {
                let context_box = match item.kind {
                    InlineItemKind::Image { .. } | InlineItemKind::AtomicBox { .. } => layout_tree.get_box_parent(item.box_idx as usize).unwrap_or(item.box_idx as usize),
                    _ => item.box_idx as usize,
                };
                contexts.insert(whitespace_context_root(layout_tree, context_box), ContextState::default());
                continue;
            }
        };
        let formatting_root = whitespace_context_root(layout_tree, item.box_idx as usize);

        let mut start = range.start;
        while start < range.end {
            let style_override = style_overrides[start as usize];
            let mut end = start + 1;
            while end < range.end && style_overrides[end as usize] == style_override {
                end += 1;
            }

            let mut part_start = start;
            for index in start..end {
                if inline_content.glyph_at(index as usize).and_then(char::from_u32) != Some('\n') {
                    continue;
                }
                if part_start < index {
                    let state = contexts.get(&formatting_root).copied().unwrap_or_default();
                    let previous = state.may_join.then_some(state.previous_span).flatten();
                    let span = append_shaping_range(styles, layout_tree, font_metrics, &mut result, item.box_idx as usize, part_start..index, style_override, previous);
                    contexts.insert(formatting_root, ContextState { previous_span: Some(span), may_join: true });
                }
                append_shaping_range(styles, layout_tree, font_metrics, &mut result, item.box_idx as usize, index..index + 1, style_override, None);
                contexts.insert(formatting_root, ContextState::default());
                part_start = index + 1;
            }
            if part_start < end {
                let state = contexts.get(&formatting_root).copied().unwrap_or_default();
                let previous = state.may_join.then_some(state.previous_span).flatten();
                let span = append_shaping_range(styles, layout_tree, font_metrics, &mut result, item.box_idx as usize, part_start..end, style_override, previous);
                contexts.insert(formatting_root, ContextState { previous_span: Some(span), may_join: true });
            }
            start = end;
        }
    }
    result
}

fn inline_boundary_breaks_shaping(styles: &ComputedStyles, layout_tree: &LayoutTree, font_metrics: &ShapedFontMetrics, box_idx: usize, inline_start: bool, inline_end: bool) -> bool {
    let indices = layout_tree.get_box_style_indices(box_idx).unwrap_or_else(|| styles.default_indices());
    let metrics = font_metrics.for_box(box_idx);
    let style = styles
        .used_view_with_root(indices, metrics.x_height_ratio(), metrics.ch_advance_ratio(), metrics.cap_height_ratio(), font_metrics.root_ch_px(), font_metrics.root_cap_height_px(), font_metrics.root_line_height_px())
        .expect("validated inline-boundary style");
    if vertical_align_breaks_shaping(style.vertical_align()) {
        return true;
    }
    let start_breaks = inline_start && (!style.margin_left().is_zero() || !style.padding_left().is_zero() || style.border_left_width() != 0.0);
    let end_breaks = inline_end && (!style.margin_right().is_zero() || !style.padding_right().is_zero() || style.border_right_width() != 0.0);
    start_breaks || end_breaks
}

fn vertical_align_breaks_shaping(value: VerticalAlignValue) -> bool {
    match value {
        VerticalAlignValue::Baseline | VerticalAlignValue::Length(0.0) | VerticalAlignValue::Percent(0.0) => false,
        VerticalAlignValue::Calc { absolute_px, line_height_fraction, x_height_px } => absolute_px != 0.0 || line_height_fraction != 0.0 || x_height_px != 0.0,
        _ => true,
    }
}

fn append_shaping_range(
    styles: &ComputedStyles, layout_tree: &LayoutTree, font_metrics: &ShapedFontMetrics, result: &mut Vec<ShapingSpan>, box_idx: usize, characters: Range<u32>, style_override: Option<StyleIndices>, previous_span: Option<usize>,
) -> usize {
    if characters.start >= characters.end {
        return previous_span.unwrap_or(result.len());
    }
    let formatting_root = whitespace_context_root(layout_tree, box_idx);
    let paint_span = ShapingPaintSpan { characters: characters.clone(), box_idx, style_override };
    if let Some(previous_index) = previous_span
        && let Some(previous) = result.get_mut(previous_index)
        && previous.formatting_root == formatting_root
        && shaping_styles_are_equivalent(styles, layout_tree, font_metrics, previous.shaping_box, previous.style_override, box_idx, style_override)
    {
        previous.paint_spans.push(paint_span);
        return previous_index;
    }
    let index = result.len();
    result.push(ShapingSpan { shaping_box: box_idx, formatting_root, style_override, paint_spans: vec![paint_span] });
    index
}

fn shaping_styles_are_equivalent(styles: &ComputedStyles, layout_tree: &LayoutTree, font_metrics: &ShapedFontMetrics, left_box: usize, left_override: Option<StyleIndices>, right_box: usize, right_override: Option<StyleIndices>) -> bool {
    let left = left_override.and_then(|indices| styles.view(indices)).unwrap_or_else(|| get_style(styles, layout_tree, left_box));
    let right = right_override.and_then(|indices| styles.view(indices)).unwrap_or_else(|| get_style(styles, layout_tree, right_box));

    if left_box != right_box && (vertical_align_breaks_shaping(left.vertical_align()) || vertical_align_breaks_shaping(right.vertical_align())) {
        return false;
    }

    let left_metrics = font_metrics.for_box(left_box);
    let right_metrics = font_metrics.for_box(right_box);
    let left_size = left
        .resolved_font_size_with_root(left_metrics.x_height_ratio(), left_metrics.ch_advance_ratio(), left_metrics.cap_height_ratio(), font_metrics.root_ch_px(), font_metrics.root_cap_height_px(), font_metrics.root_line_height_px())
        .unwrap_or_else(|| left.font_size());
    let right_size = right
        .resolved_font_size_with_root(right_metrics.x_height_ratio(), right_metrics.ch_advance_ratio(), right_metrics.cap_height_ratio(), font_metrics.root_ch_px(), font_metrics.root_cap_height_px(), font_metrics.root_line_height_px())
        .unwrap_or_else(|| right.font_size());

    left_size.to_bits() == right_size.to_bits()
        && left.font_weight() == right.font_weight()
        && left.font_style() == right.font_style()
        && left.font_family() == right.font_family()
        && left.open_type_features() == right.open_type_features()
        && left.language() == right.language()
        && left.direction() == right.direction()
}

fn text_runs(inline_content: &InlineContent) -> Vec<(usize, std::ops::Range<u32>)> {
    inline_content
        .inline_items()
        .iter()
        .filter_map(|run| match &run.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } if glyphs.start < glyphs.end => Some((run.box_idx as usize, glyphs.clone())),
            _ => None,
        })
        .collect()
}

include!("shaping_tests.rs");
