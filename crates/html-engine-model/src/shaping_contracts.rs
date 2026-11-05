//! Renderer-neutral shaping contracts and measured font metrics.

use crate::*;
use crate::caret_positions::CaretStorage;
use html_dom::MemoryUsageReport;
use html_style_model::{
    ComputedStyles, FontFace, StyleIndices, StyleView, TextSpacing, UsedStyleView,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

/// Backend-owned immutable shaped-line identifier. It is only meaningful for
/// the `GlyphShaper` instance that produced it.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TextRunId(pub u32);

/// Renderer-neutral geometry for a styled logical text run.
///
/// Advances are indexed by Unicode scalar value rather than glyph. A shaped
/// cluster may cover several scalar values; `cluster_boundaries` identifies
/// the only boundaries at which layout may split the run.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapedTextRun {
    advances: Arc<[f32]>,
    cluster_boundaries: Arc<[bool]>,
    native: Option<NativeRunGeometry>,
}

#[derive(Clone, Debug, PartialEq)]
struct NativeRunGeometry {
    run: TextRunId,
    caret_stops: CaretPositions,
    ascent: f32,
}

impl ShapedTextRun {

    /// Share geometry with resource ledgers and layout without
    /// copying character arrays at adapter boundaries.
    pub(crate) fn shared_advances(&self) -> Arc<[f32]> {
        self.advances.clone()
    }

}

/// Font extents measured from the faces that actually supplied each character.
#[derive(Clone, Debug)]
pub enum RunFontMetrics {
    Uniform(FontMetrics),
    PerCharacter(Arc<[FontMetrics]>),
}

/// A backend run result. The engine owns registration and layout
/// glyph IDs; native paint handles belong to the backend's resource generation.
#[derive(Clone, Debug)]
pub struct ShapedRun {
    geometry: ShapedTextRun,
    paint: Option<RunPaint>,
}

#[derive(Clone, Debug)]
struct RunPaint {
    first_handle: GlyphHandle,
    metrics: RunFontMetrics,
}

/// A request to shape one already-normalized, single-style logical run.
/// CSS values have been resolved before this reaches the renderer backend.
pub struct TextRunShapeRequest<'a> {
    text: &'a str,
    style: ShapingStyle<'a>,
    paint_colors: Option<&'a [u32]>,
}

#[derive(Debug)]
pub struct ShapedTextGeometry {
    len: usize,
    pub bidi: ResolvedBidi,
    pub mirrored_glyphs: FxHashMap<u32, GlyphId>,
    ranges: Vec<TextGeometryRange>,
    runs: Vec<Option<TextGeometryRun>>,
    unused_runs: Vec<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct TextGeometryRange {
    pub(crate) source: Range<u32>,
    offset: usize,
    run: usize,
    apply_start_boundary: bool,
}

#[derive(Debug)]
struct TextGeometryRun {
    geometry: ShapedTextRun,
    placement_required: bool,
    /// Number of source ranges using this slot. Slots with no ranges are
    /// released and reused; no references escape the owning geometry.
    range_count: usize,
}

impl ShapedTextGeometry {
    pub fn with_len(len: usize) -> Self {
        Self {
            len,
            bidi: ResolvedBidi::default(),
            mirrored_glyphs: FxHashMap::default(),
            ranges: Vec::new(),
            runs: Vec::new(),
            unused_runs: Vec::new(),
        }
    }

    fn geometry(&self, range: &TextGeometryRange) -> &ShapedTextRun {
        &self.runs[range.run]
            .as_ref()
            .expect("live geometry run")
            .geometry
    }

    pub(crate) fn ranges(&self) -> &[TextGeometryRange] {
        &self.ranges
    }

    pub(crate) fn native_run(&self, range: &TextGeometryRange) -> Option<NativeRunView<'_>> {
        let run = self.runs[range.run].as_ref().expect("live geometry run");
        let native = run.geometry.native.as_ref()?;
        Some(NativeRunView {
            source_range: range.source.clone(),
            run_offset: range.offset as u32,
            backend_run: native.run,
            caret_stops: &native.caret_stops,
            ascent: native.ascent,
            placement_required: run.placement_required,
        })
    }

    fn store_run(
        &mut self,
        geometry: ShapedTextRun,
        placement_required: bool,
        range_count: usize,
    ) -> usize {
        let run = Some(TextGeometryRun {
            geometry,
            placement_required,
            range_count,
        });
        if let Some(index) = self.unused_runs.pop() {
            self.runs[index] = run;
            index
        } else {
            let index = self.runs.len();
            self.runs.push(run);
            index
        }
    }

    fn range_at(&self, index: usize) -> Option<&TextGeometryRange> {
        let position = self
            .ranges
            .partition_point(|run| run.source.start as usize <= index);
        self.ranges
            .get(position.checked_sub(1)?)
            .filter(|run| index < run.source.end as usize)
    }

    pub fn has_character_geometry(&self, index: usize) -> bool {
        self.range_at(index).is_some()
    }

    pub fn next_geometry_range(&self, index: u32) -> Option<Range<u32>> {
        let position = self.ranges.partition_point(|run| run.source.end <= index);
        self.ranges.get(position).map(|run| run.source.clone())
    }

    pub fn advance(&self, character_index: usize) -> Option<f32> {
        if character_index >= self.len {
            return None;
        }
        Some(self.range_at(character_index).map_or(0.0, |run| {
            self.geometry(run).advances[run.offset + character_index - run.source.start as usize]
        }))
    }

    pub fn paint_glyph(&self, index: u32, fallback: GlyphId) -> GlyphId {
        self.mirrored_glyphs
            .get(&index)
            .copied()
            .unwrap_or(fallback)
    }

    #[cfg(any(test, feature = "testing"))]
    #[inline]
    pub fn is_cluster_boundary(&self, character_boundary: usize) -> bool {
        let mut hint = 0;
        self.boundary_with_hint(character_boundary, &mut hint)
    }

    #[inline]
    fn range_with_hint(&self, index: usize, hint: &mut usize) -> Option<&TextGeometryRange> {
        if let Some(run) = self.ranges.get(*hint)
            && run.source.start as usize <= index
            && index < run.source.end as usize
        {
            return Some(run);
        }
        if let Some(run) = self.ranges.get(hint.saturating_add(1))
            && run.source.start as usize <= index
            && index < run.source.end as usize
        {
            *hint += 1;
            return Some(run);
        }
        *hint = self
            .ranges
            .partition_point(|run| run.source.start as usize <= index)
            .checked_sub(1)?;
        self.ranges
            .get(*hint)
            .filter(|run| index < run.source.end as usize)
    }

    #[inline]
    pub fn advance_with_hint(&self, index: usize, hint: &mut usize) -> Option<f32> {
        if index >= self.len {
            return None;
        }
        Some(self.range_with_hint(index, hint).map_or(0.0, |run| {
            self.geometry(run).advances[run.offset + index - run.source.start as usize]
        }))
    }

    #[inline]
    pub fn character_geometry_with_hint(
        &self,
        index: usize,
        hint: &mut usize,
    ) -> (Option<f32>, bool) {
        if index >= self.len {
            return (None, self.boundary_with_hint(index, hint));
        }
        let Some(run) = self.range_with_hint(index, hint) else {
            return (Some(0.0), true);
        };
        let offset = run.offset + index - run.source.start as usize;
        let geometry = self.geometry(run);
        (
            Some(geometry.advances[offset]),
            (index == run.source.start as usize && !run.apply_start_boundary)
                || geometry.cluster_boundaries[offset],
        )
    }

    #[inline]
    pub fn boundary_with_hint(&self, index: usize, hint: &mut usize) -> bool {
        // A whole run ends on a validated cluster boundary. At the end of a
        // discontiguous source slice, no boundary was contributed by that
        // slice. Only the run containing the following character can override
        // the default true value at a shared source boundary.
        self.range_with_hint(index, hint).is_none_or(|run| {
            (index == run.source.start as usize && !run.apply_start_boundary)
                || self.geometry(run).cluster_boundaries
                    [run.offset + index - run.source.start as usize]
        })
    }

    /// Store a run once. Discontiguous DOM spans identify its slices by run ID
    /// and offset, and native paint queries borrow the same geometry.
    pub fn push_run(&mut self, indices: &[u32], geometry: ShapedTextRun, placement_required: bool) {
        self.push_run_iter(indices.iter().copied(), geometry, placement_required);
    }

    /// Store streamed source indices without an intermediate index buffer.
    pub fn push_run_iter(
        &mut self,
        indices: impl Iterator<Item = u32>,
        geometry: ShapedTextRun,
        placement_required: bool,
    ) {
        let mut indices = indices.enumerate().peekable();
        if indices.peek().is_none() {
            return;
        }
        let run = self.store_run(geometry, placement_required, 0);
        let mut range_count = 0;
        while let Some((offset, start)) = indices.next() {
            let mut end = start + 1;
            while indices.next_if(|&(_, index)| index == end).is_some() {
                end += 1;
            }
            self.ranges.push(TextGeometryRange {
                source: start..end,
                offset,
                run,
                apply_start_boundary: true,
            });
            range_count += 1;
        }
        self.runs[run].as_mut().unwrap().range_count = range_count;
    }

    pub fn finish_runs(&mut self) {
        self.ranges.sort_by_key(|run| run.source.start);
    }

    pub fn replace_range(
        &mut self,
        range: Range<u32>,
        geometry: ShapedTextRun,
        placement_required: bool,
    ) {
        self.replace_indexed_range(range.clone(), range, geometry, placement_required);
    }

    /// Replace a source range with geometry whose native character positions
    /// can omit source-only controls such as conditional hyphens.
    pub fn replace_indexed_range(
        &mut self,
        range: Range<u32>,
        indices: impl Iterator<Item = u32>,
        geometry: ShapedTextRun,
        placement_required: bool,
    ) {
        if range.is_empty() {
            return;
        }
        let mut ranges = Vec::with_capacity(self.ranges.len() + 2);
        for old in self.ranges.drain(..) {
            if old.source.end <= range.start || old.source.start >= range.end {
                ranges.push(old);
                continue;
            }
            let keep_prefix = old.source.start < range.start;
            let keep_suffix = old.source.end > range.end;
            let record = self.runs[old.run].as_mut().expect("live geometry run");
            record.range_count =
                record.range_count - 1 + usize::from(keep_prefix) + usize::from(keep_suffix);
            if record.range_count == 0 {
                self.runs[old.run] = None;
                self.unused_runs.push(old.run);
            }
            if keep_prefix {
                ranges.push(TextGeometryRange {
                    source: old.source.start..range.start,
                    ..old.clone()
                });
            }
            if keep_suffix {
                ranges.push(TextGeometryRange {
                    apply_start_boundary: false,
                    offset: old.offset + (range.end - old.source.start) as usize,
                    source: range.end..old.source.end,
                    ..old
                });
            }
        }
        self.ranges = ranges;
        self.push_run_iter(indices, geometry, placement_required);
        self.finish_runs();
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn memory_usage_bytes(&self) -> usize {
        self.memory_usage_bytes_with_shared(&mut FxHashSet::default())
    }

    pub fn memory_usage_bytes_with_shared(&self, seen: &mut FxHashSet<*const ()>) -> usize {
        use std::mem::{size_of, size_of_val};

        fn allocation_bytes<T>(values: &Arc<[T]>, seen: &mut FxHashSet<*const ()>) -> usize {
            if values.is_empty() || !seen.insert(values.as_ptr().cast()) {
                0
            } else {
                size_of_val(values.as_ref())
            }
        }

        // Source ranges only hold IDs. Each live run owns its geometry handles
        // once, including the carets borrowed by native paint queries.
        self.ranges.capacity() * size_of::<TextGeometryRange>()
            + self.runs.capacity() * size_of::<Option<TextGeometryRun>>()
            + self.unused_runs.capacity() * size_of::<usize>()
            + self.bidi.memory_usage_bytes()
            + self.mirrored_glyphs.capacity() * size_of::<(u32, GlyphId)>()
            + self
                .runs
                .iter()
                .flatten()
                .map(|run| {
                    allocation_bytes(&run.geometry.advances, seen)
                        + allocation_bytes(&run.geometry.cluster_boundaries, seen)
                        + shaped_text_run_caret_stops(&run
                            .geometry)
                            .map_or(0, |stops| stops.memory_usage_bytes_with_shared(seen))
                })
                .sum::<usize>()
    }

    pub fn authoritative_runs(&self) -> impl Iterator<Item = NativeRunView<'_>> {
        self.ranges
            .iter()
            .filter_map(|range| self.native_run(range))
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn update_range(&mut self, range: Range<u32>, shaped: &ShapedTextRun) -> bool {
        if range.start >= range.end
            || range.end as usize > self.len
            || shaped_text_run_advances(&shaped).len() != (range.end - range.start) as usize
        {
            return false;
        }
        let mut cumulative_delta = 0.0f32;
        let advances_changed = range.clone().zip(shaped_text_run_advances(&shaped)).any(|(index, new)| {
            cumulative_delta += new - self.advance(index as usize).unwrap_or(0.0);
            cumulative_delta.abs() > 0.01
        });
        let changed = advances_changed
            || (range.start..=range.end)
                .zip(shaped_text_run_cluster_boundaries(&shaped))
                .any(|(index, new)| self.is_cluster_boundary(index as usize) != *new);
        self.replace_range(range, shaped.clone(), false);
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
    /// text. The immutable representation is shared with the backend shape cache.
    pub caret_stops: CaretPositions,
    /// True at logical character boundaries where a line may split without
    /// bisecting a native shaping cluster.
    pub cluster_boundaries: Arc<[bool]>,
}

/// Resolved font posture. An oblique angle exists only for oblique text.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub enum FontSlant {
    #[default]
    Normal,
    Italic,
    Oblique {
        angle: ObliqueAngle,
    },
}

/// An angle in degrees, attached only to [`FontSlant::Oblique`].
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub struct ObliqueAngle(u32);

impl fmt::Debug for ObliqueAngle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&oblique_angle_degrees(*self), formatter)
    }
}

/// Measurements normalized by the em size. Backends supply the available
/// values directly; missing vertical measurements retain engine defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontRelativeMetrics {
    pub x_height_ratio: f32,
    pub ch_advance_ratio: f32,
    pub space_advance_ratio: f32,
    pub cap_height_ratio: f32,
    pub line_metrics: Option<FontLineMetrics>,
    pub normal_line_height_ratio: Option<f32>,
}

/// Measured font extents normalized by the em size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontLineMetrics {
    pub ascent_ratio: f32,
    pub descent_ratio: f32,
}

impl Default for FontRelativeMetrics {
    fn default() -> Self {
        Self {
            x_height_ratio: 0.5,
            ch_advance_ratio: 0.5,
            space_advance_ratio: 0.5,
            cap_height_ratio: 0.8,
            line_metrics: None,
            normal_line_height_ratio: None,
        }
    }
}

/// Engine-owned metrics assembled by shaping for the immutable layout topology.
/// Layout borrows the finished collections without repeating assembly checks.
#[derive(Clone)]
pub struct ShapedFontMetrics {
    box_metrics: BoxFontMetrics,
    non_box_metrics: RequiredFontMetrics<FxHashMap<StyleIndices, FontRelativeMetrics>>,
    root_ch_px: f32,
    root_cap_height_px: f32,
    root_line_height_px: f32,
}

#[derive(Clone)]
pub enum BoxFontMetrics {
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

impl BoxFontMetrics {
    pub fn for_box(&self, box_idx: usize) -> FontRelativeMetrics {
        match self {
            Self::NotRequired => FontRelativeMetrics::default(),
            Self::Dense(values) => values[box_idx],
            Self::Indexed {
                metric_ids,
                metrics,
            } => metric_ids[box_idx]
                .checked_sub(1)
                .map_or_else(FontRelativeMetrics::default, |id| metrics[id as usize]),
        }
    }
}

#[derive(Clone)]
pub enum RequiredFontMetrics<T> {
    NotRequired,
    Required(T),
}

impl ShapedFontMetrics {
    pub fn new(
        box_metrics: BoxFontMetrics,
        non_box_metrics: RequiredFontMetrics<FxHashMap<StyleIndices, FontRelativeMetrics>>,
        root_ch_px: f32,
        root_cap_height_px: f32,
        root_line_height_px: f32,
    ) -> Self {
        Self {
            box_metrics,
            non_box_metrics,
            root_ch_px,
            root_cap_height_px,
            root_line_height_px,
        }
    }

    pub fn for_box(&self, box_idx: usize) -> FontRelativeMetrics {
        self.box_metrics.for_box(box_idx)
    }

    pub fn for_non_box_style(&self, indices: StyleIndices) -> FontRelativeMetrics {
        match &self.non_box_metrics {
            RequiredFontMetrics::NotRequired => FontRelativeMetrics::default(),
            RequiredFontMetrics::Required(values) => values[&indices],
        }
    }

    pub fn used_style<'styles>(
        &self,
        styles: &'styles ComputedStyles,
        indices: StyleIndices,
        box_idx: usize,
    ) -> Option<UsedStyleView<'styles>> {
        self.used_style_with_metrics(styles, indices, self.for_box(box_idx))
    }

    pub fn used_non_box_style<'styles>(
        &self,
        styles: &'styles ComputedStyles,
        indices: StyleIndices,
    ) -> Option<UsedStyleView<'styles>> {
        self.used_style_with_metrics(styles, indices, self.for_non_box_style(indices))
    }

    pub fn resolved_font_size(&self, style: StyleView<'_>, box_idx: usize) -> f32 {
        let metrics = self.for_box(box_idx);
        style
            .resolved_font_size_with_root(
                metrics.x_height_ratio,
                metrics.ch_advance_ratio,
                metrics.cap_height_ratio,
                self.root_ch_px,
                self.root_cap_height_px,
                self.root_line_height_px,
            )
            .unwrap_or_else(|| style.font_size())
    }

    /// Resolve inherited font-relative spacing against its declaring font.
    pub fn resolve_text_spacing(
        &self,
        styles: &ComputedStyles,
        spacing: TextSpacing,
        used_font_size: f32,
    ) -> f32 {
        let relative = styles
            .text_spacing_origin(spacing)
            .map_or_else(FontRelativeMetrics::default, |origin| {
                self.for_non_box_style(origin)
            });
        spacing.resolve_with_metrics(
            used_font_size,
            relative.x_height_ratio,
            relative.ch_advance_ratio,
            relative.cap_height_ratio,
        )
    }

    fn used_style_with_metrics<'styles>(
        &self,
        styles: &'styles ComputedStyles,
        indices: StyleIndices,
        metrics: FontRelativeMetrics,
    ) -> Option<UsedStyleView<'styles>> {
        styles
            .used_view_with_root(
                indices,
                metrics.x_height_ratio,
                metrics.ch_advance_ratio,
                metrics.cap_height_ratio,
                self.root_ch_px,
                self.root_cap_height_px,
                self.root_line_height_px,
            )
            .map(|style| {
                let computed = styles.view(indices).expect("validated style");
                let resolve =
                    |spacing| self.resolve_text_spacing(styles, spacing, style.font_size());
                style
                    .with_normal_line_height_ratio(metrics.normal_line_height_ratio)
                    .with_resolved_text_spacing(
                        resolve(computed.text.letter_spacing),
                        resolve(computed.text.word_spacing),
                    )
            })
    }

    pub fn memory_usage_bytes(&self) -> usize {
        let boxes = match &self.box_metrics {
            BoxFontMetrics::NotRequired => 0,
            BoxFontMetrics::Dense(values) => {
                values.capacity() * std::mem::size_of::<FontRelativeMetrics>()
            }
            BoxFontMetrics::Indexed {
                metric_ids,
                metrics,
            } => {
                metric_ids.capacity() * std::mem::size_of::<u32>()
                    + metrics.capacity() * std::mem::size_of::<FontRelativeMetrics>()
            }
        };
        let non_boxes = match &self.non_box_metrics {
            RequiredFontMetrics::NotRequired => 0,
            RequiredFontMetrics::Required(values) => {
                values.capacity() * std::mem::size_of::<(StyleIndices, FontRelativeMetrics)>()
            }
        };
        boxes + non_boxes
    }
}

/// Caps synthesis allowed when the selected font lacks native caps glyphs.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CapsSynthesis {
    #[default]
    None,
    SmallCaps,
    AllSmallCaps,
}

/// Cursive connections preserved when a font or style change splits text into
/// shaping runs. Previous and next refer to logical character order, including
/// in right-to-left text; these are not visual left and right edges.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursiveJoining {
    #[default]
    Isolated,
    ConnectedToPrevious,
    ConnectedToNext,
    ConnectedToBoth,
}

/// Resolved properties shared by font measurement, glyph and run shaping.
/// Requests expose these values for reading; the engine constructs the styles.
/// All text and font data remain borrowed from the current document.
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub struct ShapingStyle<'a> {
    pub direction: TextDirection,
    pub font_size: f32,
    pub font_weight: u16,
    pub font_slant: FontSlant,
    /// Resolved font width as a percentage.
    pub font_stretch: f32,
    pub font_synthesis_style: FontSynthesisStyle,
    pub color: u32,
    pub font_family: Option<&'a str>,
    pub cjk_font_family: Option<&'a str>,
    pub language: Option<&'a ContentLanguage>,
    pub font_features: &'a FontFeatures<'a>,
    pub caps_synthesis: CapsSynthesis,
    /// Cursive connections to the preceding and following shaping runs.
    pub cursive_joining: CursiveJoining,
}

/// Engine-side construction and geometry adjustment. Backend-facing request
/// types expose no builders.
/// This module is intentionally absent from the application facade.
pub mod shaping {
    use super::*;

    /// Classify the engine-only first-line shaping requirement.
    pub fn requires_first_line_shaper(error: &ShapeError) -> bool {
        matches!(error.kind, ShapeErrorKind::FirstLineShaperRequired)
    }

    /// Report layout that needs its shaping backend for first-line styles.
    pub fn first_line_shaper_required() -> ShapeError {
        ShapeError {
            kind: ShapeErrorKind::FirstLineShaperRequired,
        }
    }

    /// Report first-line refinement that did not reach a stable layout.
    pub fn first_line_refinement_did_not_converge() -> ShapeError {
        ShapeError {
            kind: ShapeErrorKind::FirstLineRefinementDidNotConverge,
        }
    }

    /// Report a document paired with another generation of native resources.
    pub fn resource_generation_mismatch() -> ShapeError {
        ShapeError {
            kind: ShapeErrorKind::ResourceGenerationMismatch,
        }
    }

    /// Report repreparation that requires an owning document.
    pub fn borrowed_document() -> ShapeError {
        ShapeError {
            kind: ShapeErrorKind::BorrowedDocument,
        }
    }

    /// Apply replayable per-character spacing without materializing a buffer.
    /// Iterator clones must produce the same finite deltas in logical source
    /// order, with one delta per advance and finite resulting coordinates.
    pub fn apply_spacing(
        mut run: ShapedTextRun,
        deltas: impl ExactSizeIterator<Item = f32> + Clone,
        right_to_left: bool,
    ) -> ShapedTextRun {
        if !deltas.clone().any(|delta| delta != 0.0) {
            return run;
        }
        for (advance, delta) in Arc::make_mut(&mut run.advances)
            .iter_mut()
            .zip(deltas.clone())
        {
            *advance += delta;
        }
        if let Some(native) = &mut run.native {
            let stops = &mut native.caret_stops;
            let derived = matches!(stops.0, CaretStorage::Derived(_));
            if derived {
                *stops = stops.to_dense().into();
            }
            let CaretStorage::Dense(dense) = &mut stops.0 else {
                unreachable!()
            };
            let mut cumulative = 0.0;
            let mut deltas = deltas;
            for stop in Arc::make_mut(dense) {
                *stop += if right_to_left {
                    -cumulative
                } else {
                    cumulative
                };
                if let Some(delta) = deltas.next() {
                    cumulative += delta;
                }
            }
            if derived {
                *stops = CaretPositions::from_advances(&run.advances, std::mem::take(dense));
            }
        }
        run
    }

    pub fn preserve_graphemes(run: &mut ShapedTextRun, text: &str) {
        profile_scope!("shape.preserve_graphemes");
        if text.is_ascii() && !text.as_bytes().contains(&b'\r') {
            return;
        }
        let graphemes = crate::bidi::grapheme_boundaries(text, run.advances.len());
        if run
            .cluster_boundaries
            .iter()
            .zip(graphemes.iter())
            .any(|(&native, &grapheme)| native && !grapheme)
        {
            for (native, &grapheme) in Arc::make_mut(&mut run.cluster_boundaries)
                .iter_mut()
                .zip(graphemes.iter())
            {
                *native &= grapheme;
            }
        }
    }

    /// Suppress character widths and shift native carets together. The mask is
    /// indexed by logical scalar value; native cached geometry stays unchanged.
    pub fn suppress_advances(
        run: &mut ShapedTextRun,
        suppressed: impl ExactSizeIterator<Item = bool> + Clone,
        right_to_left: bool,
    ) {
        let Some(first) = suppressed
            .clone()
            .zip(run.advances.iter())
            .position(|(suppress, &advance)| suppress && advance != 0.0)
        else {
            return;
        };
        let derived = run
            .native
            .as_ref()
            .is_some_and(|native| matches!(native.caret_stops.0, CaretStorage::Derived(_)));
        if let Some(native) = &mut run.native {
            let mut suppressed = suppressed.clone();
            let mut removed = 0.0;
            for (index, stop) in native.caret_stops.make_dense().iter_mut().enumerate() {
                *stop += if right_to_left { removed } else { -removed };
                if let Some(&advance) = run.advances.get(index)
                    && suppressed.next().expect("validated suppression mask")
                {
                    removed += advance;
                }
            }
        }
        for (advance, suppress) in Arc::make_mut(&mut run.advances)
            .iter_mut()
            .zip(suppressed)
            .skip(first)
        {
            if suppress {
                *advance = 0.0;
            }
        }
        if derived {
            let native = run.native.as_mut().expect("native run with derived carets");
            let CaretStorage::Dense(stops) = &mut native.caret_stops.0 else {
                unreachable!()
            };
            native.caret_stops =
                CaretPositions::from_advances(&run.advances, std::mem::take(stops));
        }
    }

    pub fn font_features<'a>(
        settings: &'a mut Vec<OpenTypeFeature>,
        explicit: &[OpenTypeFeature],
        has_tracking: bool,
    ) -> FontFeatures<'a> {
        crate::font_features::resolve_font_features(settings, explicit, has_tracking)
    }

    /// Combine connections computed by span planning or native run grouping.
    #[inline]
    pub const fn cursive_joining(previous: bool, next: bool) -> CursiveJoining {
        match (previous, next) {
            (false, false) => CursiveJoining::Isolated,
            (true, false) => CursiveJoining::ConnectedToPrevious,
            (false, true) => CursiveJoining::ConnectedToNext,
            (true, true) => CursiveJoining::ConnectedToBoth,
        }
    }

    pub fn style<'a>(
        font_size: f32,
        font_weight: u16,
        font_slant: FontSlant,
        color: u32,
        font_family: Option<&'a str>,
    ) -> ShapingStyle<'a> {
        // CSS permits zero-size text, which still needs character identities.
        ShapingStyle {
            direction: TextDirection::Ltr,
            font_size,
            font_weight,
            font_slant,
            font_stretch: 100.0,
            font_synthesis_style: FontSynthesisStyle::Auto,
            color,
            font_family,
            cjk_font_family: None,
            language: None,
            font_features: &FontFeatures::DEFAULT,
            caps_synthesis: CapsSynthesis::None,
            cursive_joining: CursiveJoining::Isolated,
        }
    }

    #[inline]
    pub fn glyph<'a>(character: char, style: &'a ShapingStyle<'a>) -> GlyphRequest<'a> {
        GlyphRequest { character, style }
    }

    pub fn run<'a>(
        text: &'a str,
        style: ShapingStyle<'a>,
        paint_colors: Option<&'a [u32]>,
    ) -> TextRunShapeRequest<'a> {
        TextRunShapeRequest {
            text,
            style,
            paint_colors,
        }
    }
}

/// Shaping failure propagated by the engine or reported by a backend.
/// Backends construct resource-capacity and unsupported-transaction errors;
/// document lifecycle failures are constructed by the engine implementation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeError {
    kind: ShapeErrorKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ShapeErrorKind {
    RegistryCapacityExhausted { metrics_len: usize },
    FirstLineRefinementDidNotConverge,
    FirstLineShaperRequired,
    AppendTransactionUnsupported,
    ResourceGenerationMismatch,
    BorrowedDocument,
}

impl ShapeError {

    pub(crate) fn append_transaction_unsupported() -> Self {
        Self {
            kind: ShapeErrorKind::AppendTransactionUnsupported,
        }
    }
}

impl fmt::Display for ShapeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ShapeErrorKind::RegistryCapacityExhausted { metrics_len } => {
                write!(
                    formatter,
                    "glyph registry exceeded capacity after {metrics_len} registered metrics"
                )
            }
            ShapeErrorKind::FirstLineShaperRequired => formatter
                .write_str("first-line styles require layout_with_shaper or relayout_with_shaper"),
            ShapeErrorKind::FirstLineRefinementDidNotConverge => {
                formatter.write_str("first-line layout did not converge")
            }
            ShapeErrorKind::AppendTransactionUnsupported => {
                formatter.write_str("glyph shaper does not support append rollback")
            }
            ShapeErrorKind::BorrowedDocument => formatter.write_str(
                "repreparation requires the owning document; this stage borrows its preparation",
            ),
            ShapeErrorKind::ResourceGenerationMismatch => {
                formatter.write_str("glyph shaper resources do not belong to this document")
            }
        }
    }
}

impl std::error::Error for ShapeError {}

/// Glyph metrics owned once by the pipeline session or standalone caller.
/// Shaping borrows the store mutably; layout and queries borrow its metrics.
///
/// ```compile_fail
/// use html_engine_model::GlyphResourceStore;
/// let resources = GlyphResourceStore::default();
/// let duplicate = resources.clone();
/// ```
///
/// A metric borrow prevents registrations or resets until its last use:
///
/// ```compile_fail
/// use html_engine_model::GlyphResourceStore;
/// let mut resources = GlyphResourceStore::default();
/// let metrics = resources.metrics();
/// resources.clear();
/// assert_eq!(metrics.len(), 0);
/// ```
#[derive(Default)]
pub struct GlyphResourceStore {
    metrics: GlyphMetrics,
    generation: GlyphResourceGeneration,
    shaped_ids: FxHashMap<(GlyphHandle, char, u32, u32, u32), GlyphId>,
    paint_invalid: bool,
    paint_lease: OnceLock<GlyphResourceLease>,
}

/// Validity of retained paint commands. Owning the lease does not keep a
/// replaced backend alive; consumers must check validity before replaying.
#[derive(Clone)]
pub struct GlyphResourceLease(Arc<AtomicBool>);

impl GlyphResourceLease {
    pub fn is_valid(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl Drop for GlyphResourceStore {
    fn drop(&mut self) {
        self.invalidate_paint_resources();
    }
}

/// An append-only ledger checkpoint. It owns no glyph data and can be consumed
/// once to discard registrations made after it. Use only while this store has
/// been appended to; document replacement and branching rollback invalidate it.
#[derive(Debug)]
pub struct GlyphResourceCheckpoint {
    generation: GlyphResourceGeneration,
    len: usize,
}

/// Backend-owned append boundary. Restoring it preserves the resource generation
/// and every handle in its prefix; callers must first release appended views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeResourceCheckpoint {
    pub generation: GlyphResourceGeneration,
    pub glyph_count: usize,
    pub run_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GlyphResourceGeneration(u64);

static NEXT_GLYPH_RESOURCE_GENERATION: AtomicU64 = AtomicU64::new(1);

impl GlyphResourceGeneration {
    pub const UNBOUND: Self = Self(0);
    fn new() -> Self {
        Self(
            NEXT_GLYPH_RESOURCE_GENERATION
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .expect("glyph-resource generation space exhausted"),
        )
    }
}

impl Default for GlyphResourceGeneration {
    fn default() -> Self {
        Self::new()
    }
}

impl GlyphResourceStore {
    pub fn new(generation: GlyphResourceGeneration) -> Self {
        Self {
            metrics: GlyphMetrics::default(),
            generation,
            shaped_ids: FxHashMap::default(),
            paint_invalid: false,
            paint_lease: OnceLock::new(),
        }
    }

    #[inline]
    pub fn paint_resources_valid(&self) -> bool {
        !self.paint_invalid
    }

    pub fn paint_lease(&self) -> GlyphResourceLease {
        self.paint_lease
            .get_or_init(|| GlyphResourceLease(Arc::new(AtomicBool::new(!self.paint_invalid))))
            .clone()
    }

    /// Backend invalidation preserves the metric ledger for geometry queries.
    pub fn invalidate_paint_resources(&mut self) {
        self.paint_invalid = true;
        self.invalidate_render_scenes();
    }

    /// Successful publication replaces commands while preserving native handles.
    #[inline]
    pub fn invalidate_render_scenes(&mut self) {
        if self.paint_lease.get().is_none() {
            return;
        }
        if let Some(lease) = self.paint_lease.take() {
            lease.0.store(false, Ordering::Release);
        }
    }

    pub fn metrics(&self) -> &GlyphMetrics {
        &self.metrics
    }

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        self.memory_usage_report_with_shared(&mut FxHashSet::default())
    }

    pub fn memory_usage_report_with_shared(
        &self,
        seen: &mut FxHashSet<*const ()>,
    ) -> MemoryUsageReport {
        let mut report = self.metrics.memory_usage_report_with_shared(seen);
        report.add_hash_map_storage("GlyphResourceStore.shaped_ids", &self.shaped_ids);
        report
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn generation(&self) -> GlyphResourceGeneration {
        self.generation
    }

    pub fn checkpoint(&self) -> GlyphResourceCheckpoint {
        GlyphResourceCheckpoint {
            generation: self.generation,
            len: self.len(),
        }
    }

    /// Restore an append checkpoint without cloning the ledger or its arrays.
    /// Returns false without changing this store if it belongs to another
    /// generation or its prefix has already been discarded.
    pub fn rollback_to(&mut self, checkpoint: GlyphResourceCheckpoint) -> bool {
        if checkpoint.generation != self.generation || checkpoint.len > self.len() {
            return false;
        }
        self.metrics.truncate(checkpoint.len);
        self.shaped_ids
            .retain(|_, id| (id.0 as usize) < checkpoint.len);
        true
    }

    /// Ask the backend for native geometry and paint handles, then assign layout
    /// IDs here. Backends never receive the engine's registry or glyph buffer.
    pub fn shape_run(
        &mut self,
        shaper: &mut (impl GlyphShaper + ?Sized),
        request: TextRunShapeRequest<'_>,
        glyphs: &mut [GlyphId],
    ) -> Result<Option<ShapedTextRun>, ShapeError> {
        let text = text_run_shape_request_text(&request);
        let style = *text_run_shape_request_style(&request);
        let shaped = shaper.shape_run(request)?;
        if let Some(ShapedRun {
            geometry,
            paint:
                Some(RunPaint {
                    first_handle,
                    metrics,
                }),
        }) = shaped
        {
            let range = self
                .metrics
                .register_run(text, geometry.shared_advances(), metrics, first_handle)
                .map_err(|_| shape_error_registry_capacity_exhausted(self.len()))?;
            for (slot, id) in glyphs.iter_mut().zip(range) {
                *slot = GlyphId(id);
            }
            return Ok(Some(geometry));
        }
        let mut ascii = [GlyphId(u32::MAX); 128];
        let mut non_ascii = FxHashMap::<char, GlyphId>::default();
        for (slot, character) in glyphs.iter_mut().zip(text.chars()) {
            let cached = if character.is_ascii() {
                ascii[character as usize]
            } else {
                non_ascii
                    .get(&character)
                    .copied()
                    .unwrap_or(GlyphId(u32::MAX))
            };
            *slot = if cached != GlyphId(u32::MAX) {
                cached
            } else {
                let id = self.shape_glyph(shaper, shaping::glyph(character, &style))?;
                if character.is_ascii() {
                    ascii[character as usize] = id;
                } else {
                    non_ascii.insert(character, id);
                }
                id
            };
        }
        Ok(shaped.map(shaped_run_into_geometry))
    }

    /// Shape and register each character independently, using the run's font
    /// selection. Native contextual run shaping is left to the caller.
    pub fn shape_characters(
        &mut self,
        shaper: &mut (impl GlyphShaper + ?Sized),
        request: &TextRunShapeRequest<'_>,
        glyphs: &mut [GlyphId],
    ) -> Result<(), ShapeError> {
        debug_assert_eq!(text_run_shape_request_text(&request).chars().count(), glyphs.len());
        let style = text_run_shape_request_style(&request);
        for (slot, character) in glyphs.iter_mut().zip(text_run_shape_request_text(&request).chars()) {
            *slot = self.shape_glyph(shaper, shaping::glyph(character, style))?;
        }
        Ok(())
    }

    /// Shape and intern a native result, assigning its layout ID here.
    pub fn shape_glyph(
        &mut self,
        shaper: &mut (impl GlyphShaper + ?Sized),
        request: GlyphRequest<'_>,
    ) -> Result<GlyphId, ShapeError> {
        let shaped = shaper.shape_glyph(request)?;
        let metrics = shaped.metrics();
        let key = (
            shaped.handle(),
            request.character,
            shaped.advance().to_bits(),
            metrics.ascent.to_bits(),
            metrics.descent.to_bits(),
        );
        if let Some(&id) = self.shaped_ids.get(&key) {
            return Ok(id);
        }
        let id = self.register_shaped(request.character, shaped)?;
        self.shaped_ids.insert(key, id);
        Ok(id)
    }

    /// Register a native single-character result without interning.
    fn register_shaped(
        &mut self,
        character: char,
        shaped: ShapedGlyph,
    ) -> Result<GlyphId, ShapeError> {
        let id = self.register(GlyphMetric::from_shaped(character, shaped))?;
        self.metrics.set_paint_handle(id, shaped.handle());
        Ok(id)
    }

    fn register(&mut self, metric: GlyphMetric) -> Result<GlyphId, ShapeError> {
        self.metrics
            .register(metric)
            .map_err(|_| shape_error_registry_capacity_exhausted(self.len()))
    }

    /// Number of stored metric records, independent of the logical ID count.
    pub fn record_count(&self) -> usize {
        self.metrics.record_count()
    }

    pub fn len(&self) -> usize {
        self.metrics.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, glyph: GlyphId) -> Option<GlyphMetric> {
        self.metrics.get_checked(glyph)
    }

    pub fn contains(&self, glyph: GlyphId) -> bool {
        (glyph.0 as usize) < self.len()
    }
}

/// Backend supplied by a renderer adapter for measuring styled characters
/// and returning handles to its native paint resources. HtmlEngine assigns
/// layout glyph IDs and stores the returned measurements.
/// Basic adapters can use [`crate::ResourceShaper`] to supply a shaping callback
/// with automatic native-resource lifecycle handling.
pub trait GlyphShaper {
    /// Optional reclamation of completed scope batches. Backends which support
    /// this must preserve all prefix handles and retained painter snapshots.
    fn resource_checkpoint(&self) -> Option<NativeResourceCheckpoint> {
        None
    }
    fn restore_resource_checkpoint(&mut self, _checkpoint: NativeResourceCheckpoint) -> bool {
        false
    }

    /// Selects the publication `@font-face` declarations and reader font
    /// policy of the document about to be shaped. The engine calls this before
    /// every document and append shaping pass, so a backend's active faces
    /// always belong to the document it shapes. Backends that register
    /// publication fonts resolve authored families through these faces;
    /// others ignore them.
    fn select_publication_fonts(&mut self, _faces: &[FontFace], _overrides: &ReaderStyleOverrides) {
    }

    /// Identity of the backend's active paint resources. Reset must assign a
    /// fresh generation; a failed document replacement restores the old one.
    fn resource_generation(&self) -> GlyphResourceGeneration;

    fn reset(&mut self);

    /// Checkpoint active resources before an operation that appends glyphs.
    /// The default rejects appends so failed shaping cannot leave native
    /// resources ahead of the published glyph metrics. Resource-backed shapers should
    /// restore all glyph and run resources in `rollback_append_shaping`.
    fn begin_append_shaping(&mut self) -> Result<(), ShapeError> {
        Err(ShapeError::append_transaction_unsupported())
    }

    fn commit_append_shaping(&mut self) {}

    fn rollback_append_shaping(&mut self) {}

    /// Starts replacing the renderer-owned resources for a shaped document.
    ///
    /// Resource-backed implementations should retain the previous document's
    /// resources until either [`GlyphShaper::commit_document_shaping`] or
    /// [`GlyphShaper::rollback_document_shaping`] is called. Implementations
    /// without renderer-owned resources can rely on this reset-only default;
    /// `reset` must assign a fresh resource generation.
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
    fn font_relative_metrics(
        &mut self,
        _style: &ShapingStyle<'_>,
    ) -> Result<FontRelativeMetrics, ShapeError> {
        Ok(FontRelativeMetrics::default())
    }

    /// Shape one character using the native text system. Return its paint
    /// handle, advance, and measured font extents; no metric registration is needed.
    fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError>;

    /// Shape a complete logical run using the supplied resolved style. Font
    /// selection, supported feature translation and native caching belong to
    /// the backend. Actual advances and cluster geometry drive engine layout.
    /// Returning `None` selects the engine's per-character fallback.
    fn shape_run(
        &mut self,
        _request: TextRunShapeRequest<'_>,
    ) -> Result<Option<ShapedRun>, ShapeError> {
        Ok(None)
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    #[test]
    fn font_extents_preserve_selected_line_ratios_and_fallback_without_metrics() {
        let selected = FontRelativeMetrics {
            x_height_ratio: 0.5,
            ch_advance_ratio: 0.5,
            cap_height_ratio: 0.8,
            line_metrics: Some(FontLineMetrics {
                ascent_ratio: 1.0,
                descent_ratio: 0.25,
            }),
            ..FontRelativeMetrics::default()
        };
        assert_eq!(font_relative_metrics_line_box_extents(selected, 20.0), (20.0, 5.0));
        assert_eq!(font_relative_metrics_line_box_extents(selected, 0.0), (0.0, 0.0));
        assert_eq!(
            font_relative_metrics_line_box_extents(FontRelativeMetrics::default(), 20.0),
            (16.0, 4.0)
        );
    }

    #[test]
    fn append_checkpoints_own_no_run_data_and_preserve_the_prefix() {
        let mut resources = GlyphResourceStore::default();
        let generation = resources.generation();
        let advances: Arc<[f32]> = [2.0, 3.0].into();
        let run = shaped_text_run_new(advances.clone(), vec![true; 3].into());
        resources
            .metrics
            .register_run(
                "ab",
                run.shared_advances(),
                RunFontMetrics::Uniform(FontMetrics {
                    ascent: 8.0,
                    descent: 2.0,
                }),
                GlyphHandle(0),
            )
            .unwrap();
        let owners = Arc::strong_count(&advances);
        let checkpoint = resources.checkpoint();
        assert_eq!(Arc::strong_count(&advances), owners);
        let extra = GlyphMetric::new('x', 1.0, 8.0, 2.0);
        assert_eq!(resources.register(extra).unwrap(), GlyphId(2));
        assert!(resources.rollback_to(checkpoint));
        assert_eq!(resources.generation(), generation);
        assert_eq!(resources.len(), 2);
        assert_eq!(resources.record_count(), 1);
        assert_eq!(resources.get(GlyphId(0)).unwrap().ch(), 'a');
        assert!(resources.get(GlyphId(2)).is_none());
        assert_eq!(resources.register(extra).unwrap(), GlyphId(2));
    }

    #[test]
    fn invalid_append_checkpoints_leave_the_active_ledger_unchanged() {
        let mut resources = GlyphResourceStore::default();
        let other = GlyphResourceStore::default().checkpoint();
        let empty = resources.checkpoint();
        let extra = GlyphMetric::new('x', 1.0, 8.0, 2.0);
        resources.register(extra).unwrap();
        assert!(!resources.rollback_to(other));
        assert_eq!(resources.len(), 1);
        let appended = resources.checkpoint();
        assert!(resources.rollback_to(empty));
        assert!(!resources.rollback_to(appended));
        assert_eq!(resources.len(), 0);
        let stale = resources.checkpoint();
        resources.clear();
        resources.register(extra).unwrap();
        assert!(!resources.rollback_to(stale));
        assert_eq!(resources.len(), 1);
    }

    fn fallback(len: usize, advance: f32) -> ShapedTextRun {
        shaped_text_run_new(vec![advance; len].into(), vec![true; len + 1].into())
    }

    #[test]
    fn scattered_ranges_and_native_queries_borrow_one_stored_run() {
        let advances: Arc<[f32]> = [1.0, 2.0, 3.0, 4.0].into();
        let boundaries: Arc<[bool]> = [true, false, true, false, true].into();
        let carets: Arc<[f32]> = [0.0, 1.0, 3.0, 6.0, 10.0].into();
        let run = shaped_text_run_with_backend_run(advances.clone(),
            boundaries.clone(),
            TextRunId(7),
            carets.clone().into(),
            8.0,);
        let mut geometry = ShapedTextGeometry::with_len(10);
        geometry.push_run(&[4, 5, 8, 9], run, true);
        geometry.finish_runs();

        assert_eq!(geometry.runs.len(), 1);
        assert_eq!(geometry.advance(8), Some(3.0));
        let native = TextGeometryView::new(&geometry, None)
            .authoritative_runs(5..9)
            .collect::<Vec<_>>();
        assert_eq!(native.len(), 2);
        assert_eq!(native[0].source_range, 5..6);
        assert_eq!(native[0].run_offset, 1);
        assert_eq!(native[1].source_range, 8..9);
        assert_eq!(native[1].run_offset, 2);
        for run in native {
            assert!(run.placement_required);
            assert_eq!(run.backend_run, TextRunId(7));
            assert_eq!(run.caret_stops.to_dense().as_ptr(), carets.as_ptr());
        }
        for count in [
            Arc::strong_count(&advances),
            Arc::strong_count(&boundaries),
            Arc::strong_count(&carets),
        ] {
            assert_eq!(
                count, 2,
                "one test handle and one stored run; ranges and views only borrow"
            );
        }
    }

    #[test]
    fn replacements_preserve_slices_and_release_runs_after_the_last_range() {
        let carets: Arc<[f32]> = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0].into();
        let run = shaped_text_run_with_backend_run(vec![1.0; 6].into(),
            vec![true; 7].into(),
            TextRunId(3),
            carets.clone().into(),
            8.0,);
        let mut geometry = ShapedTextGeometry::with_len(6);
        geometry.push_run(&[0, 1, 2, 3, 4, 5], run, false);
        geometry.replace_range(2..4, fallback(2, 2.0), false);
        let native = geometry.authoritative_runs().collect::<Vec<_>>();
        assert_eq!(native.len(), 2);
        assert_eq!(native[0].source_range, 0..2);
        assert_eq!(native[1].source_range, 4..6);
        assert_eq!(native[1].run_offset, 4);
        assert_eq!(
            native[0].caret_stops.to_dense().as_ptr(),
            native[1].caret_stops.to_dense().as_ptr()
        );
        assert_eq!(Arc::strong_count(&carets), 2);
        drop(native);

        geometry.replace_range(0..2, fallback(2, 3.0), false);
        assert_eq!(
            Arc::strong_count(&carets),
            2,
            "suffix still uses the original run"
        );
        geometry.replace_range(4..6, fallback(2, 4.0), false);
        assert_eq!(
            Arc::strong_count(&carets),
            1,
            "last source range releases the original run"
        );
        assert_eq!(geometry.authoritative_runs().count(), 0);
        assert_eq!(
            (0..6)
                .map(|index| geometry.advance(index).unwrap())
                .collect::<Vec<_>>(),
            [3.0, 3.0, 2.0, 2.0, 4.0, 4.0]
        );

        let slots = geometry.runs.len();
        for index in 0..100 {
            geometry.replace_range(0..6, fallback(6, index as f32), false);
            assert_eq!(geometry.advance(5), Some(index as f32));
            assert!(
                geometry.runs.len() <= slots,
                "replacement must reuse released slots"
            );
        }
    }

    #[test]
    fn spacing_updates_owned_geometry_without_changing_backend_arrays() {
        for (right_to_left, expected) in [(false, [0.0, 3.0, 5.5]), (true, [0.0, 1.0, 4.5])] {
            let advances: Arc<[f32]> = [2.0, 3.0].into();
            let carets: Arc<[f32]> = [0.0, 2.0, 5.0].into();
            let mut run = shaped_text_run_with_backend_run(advances.clone(),
                vec![true; 3].into(),
                TextRunId(0),
                carets.clone().into(),
                8.0,);
            run = shaping::apply_spacing(run, [0.0, 0.0].into_iter(), right_to_left);
            assert_eq!(shaped_text_run_advances(&run).as_ptr(), advances.as_ptr());
            assert_eq!(
                shaped_text_run_caret_stops(&run).unwrap().to_dense().as_ptr(),
                carets.as_ptr()
            );
            run = shaping::apply_spacing(run, [1.0, -0.5].into_iter(), right_to_left);
            assert_eq!(shaped_text_run_advances(&run), [3.0, 2.5]);
            assert_eq!(shaped_text_run_caret_stops(&run).unwrap().to_dense().as_ref(), expected);
            assert_eq!(&*advances, &[2.0, 3.0]);
            assert_eq!(&*carets, &[0.0, 2.0, 5.0]);
        }
    }

    #[test]
    fn empty_ranges_do_not_retain_geometry_or_replace_existing_data() {
        let mut geometry = ShapedTextGeometry::with_len(2);
        geometry.push_run(&[], fallback(0, 1.0), false);
        assert!(geometry.runs.is_empty());
        geometry.push_run(&[0, 1], fallback(2, 2.0), false);
        geometry.replace_range(1..1, fallback(0, 3.0), false);
        assert_eq!(geometry.runs.len(), 1);
        assert_eq!(geometry.advance(1), Some(2.0));
    }
}

/// Geometry whose characters occupy consecutive native paint handles.
/// The adapter supplies a representable handle range and either uniform
/// extents or one font metric per character. Shared arrays are retained.
pub fn shaped_run_painted(
    geometry: ShapedTextRun,
    first_handle: GlyphHandle,
    metrics: RunFontMetrics,
) -> ShapedRun {
    ShapedRun {
        geometry,
        paint: Some(RunPaint {
            first_handle,
            metrics,
        }),
    }
}

pub fn shaped_text_run_caret_stops(value: &ShapedTextRun) -> Option<&CaretPositions> {
    value.native.as_ref().map(|native| &native.caret_stops)
}

/// Native geometry with individual glyph painting supplied by `shape_glyph`.
pub fn shaped_run_geometry(geometry: ShapedTextRun) -> ShapedRun {
    ShapedRun {
        geometry,
        paint: None,
    }
}

/// The caller supplies a resolved CSS angle from -90 through 90 degrees.
pub fn font_slant_oblique(degrees: f32) -> FontSlant {
    FontSlant::Oblique {
        angle: ObliqueAngle(degrees.to_bits()),
    }
}

pub fn text_run_shape_request_style<'a, 's>(
    value: &'s TextRunShapeRequest<'a>,
) -> &'s ShapingStyle<'a> {
    &value.style
}

/// Per-character overrides. `None` means every character uses
/// [`ShapingStyle::color`] from the shaping style.
pub fn text_run_shape_request_paint_colors<'a>(
    value: &TextRunShapeRequest<'a>,
) -> Option<&'a [u32]> {
    value.paint_colors
}

pub fn shaped_text_run_advances(value: &ShapedTextRun) -> &[f32] {
    &value.advances
}

/// The adapter supplies finite, non-negative advances and one more cluster
/// boundary than advances, with both outer boundaries enabled.
pub fn shaped_text_run_new(advances: Arc<[f32]>, cluster_boundaries: Arc<[bool]>) -> ShapedTextRun {
    ShapedTextRun {
        advances,
        cluster_boundaries,
        native: None,
    }
}

#[inline]
pub const fn cursive_joining_connects_to_next(value: CursiveJoining) -> bool {
    matches!(
        value,
        CursiveJoining::ConnectedToNext | CursiveJoining::ConnectedToBoth
    )
}

pub fn shaped_text_run_cluster_boundaries(value: &ShapedTextRun) -> &[bool] {
    &value.cluster_boundaries
}

/// The adapter also supplies finite ascent and caret positions, with one
/// caret per character boundary. All arrays refer to the same logical run.
pub fn shaped_text_run_with_backend_run(
    advances: Arc<[f32]>,
    cluster_boundaries: Arc<[bool]>,
    backend_run: TextRunId,
    caret_stops: CaretPositions,
    ascent: f32,
) -> ShapedTextRun {
    ShapedTextRun {
        advances,
        cluster_boundaries,
        native: Some(NativeRunGeometry {
            run: backend_run,
            caret_stops,
            ascent,
        }),
    }
}

pub fn text_run_shape_request_text<'a>(value: &TextRunShapeRequest<'a>) -> &'a str {
    value.text
}

#[inline]
pub const fn cursive_joining_connects_to_previous(value: CursiveJoining) -> bool {
    matches!(
        value,
        CursiveJoining::ConnectedToPrevious | CursiveJoining::ConnectedToBoth
    )
}

/// Select a character's resolved family, including the reader's CJK override.
pub fn shaping_style_font_family_for_character<'a>(
    value: &ShapingStyle<'a>,
    character: char,
) -> Option<&'a str> {
    value
        .cjk_font_family
        .filter(|_| html_style_model::is_cjk_character(character))
        .or(value.font_family)
}

/// Font content extents at a CSS font size, before line-height leading.
pub fn font_relative_metrics_line_box_extents(
    value: FontRelativeMetrics,
    font_size: f64,
) -> (f64, f64) {
    value.line_metrics.map_or_else(
        || (font_size * 0.8, font_size * 0.2),
        |metrics| {
            (
                font_size * f64::from(metrics.ascent_ratio),
                font_size * f64::from(metrics.descent_ratio),
            )
        },
    )
}

pub fn shape_error_registry_capacity_exhausted(metrics_len: usize) -> ShapeError {
    ShapeError {
        kind: ShapeErrorKind::RegistryCapacityExhausted { metrics_len },
    }
}

pub fn oblique_angle_degrees(value: ObliqueAngle) -> f32 {
    f32::from_bits(value.0)
}

pub fn shaped_text_run_backend_run(value: &ShapedTextRun) -> Option<TextRunId> {
    value.native.as_ref().map(|native| native.run)
}

pub fn shaped_text_run_ascent(value: &ShapedTextRun) -> f32 {
    value.native.as_ref().map_or(0.0, |native| native.ascent)
}

pub fn shaped_run_into_geometry(value: ShapedRun) -> ShapedTextRun {
    value.geometry
}
