//! Renderer-neutral shaping contracts and measured font metrics.

use crate::caret_positions::CaretStorage;
use crate::glyph_adapter::NativePaintSource;
use crate::*;
use html_dom::MemoryUsageReport;
use html_style_model::{
    ComputedStyles, FontFace, StyleIndices, StyleView, TextSpacing, UsedStyleView,
};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// An opaque native-run key. Retained results receive document-local keys;
/// borrowed results use the backend's resource-generation namespace.
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
    run: Option<TextRunId>,
    caret_stops: CaretPositions,
    ascent: f32,
}

impl ShapedTextRun {
    /// Share geometry with resource ledgers and layout without
    /// copying character arrays at adapter boundaries.
    pub(crate) fn shared_advances(&self) -> Arc<[f32]> {
        self.advances.clone()
    }

    pub fn caret_stops(&self) -> Option<&CaretPositions> {
        self.native.as_ref().map(|native| &native.caret_stops)
    }

    pub fn advances(&self) -> &[f32] {
        &self.advances
    }

    /// The adapter supplies finite, non-negative advances and one more cluster
    /// boundary than advances, with both outer boundaries enabled.

    pub fn new(advances: Arc<[f32]>, cluster_boundaries: Arc<[bool]>) -> ShapedTextRun {
        ShapedTextRun {
            advances,
            cluster_boundaries,
            native: None,
        }
    }

    pub fn cluster_boundaries(&self) -> &[bool] {
        &self.cluster_boundaries
    }

    /// Native caret geometry without a paint key. Retained backends return this
    /// with their native entry; engine registration assigns its document-local key.
    /// Supply finite ascent and one caret per character boundary, all referring to
    /// the same logical run. Shared arrays are retained without copying.

    pub fn with_carets(
        advances: Arc<[f32]>,
        cluster_boundaries: Arc<[bool]>,
        caret_stops: CaretPositions,
        ascent: f32,
    ) -> ShapedTextRun {
        ShapedTextRun {
            advances,
            cluster_boundaries,
            native: Some(NativeRunGeometry {
                run: None,
                caret_stops,
                ascent,
            }),
        }
    }

    /// Native geometry with a borrowed backend paint key. Supply finite ascent and
    /// one caret per character boundary, all referring to the same logical run.
    /// Retained backends use [`ShapedTextRun::with_carets`] instead.

    pub fn with_backend_run(
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
                run: Some(backend_run),
                caret_stops,
                ascent,
            }),
        }
    }

    /// The registered engine key or borrowed backend key, when one is assigned.
    /// Unregistered caret geometry has no paint key.

    pub fn backend_run(&self) -> Option<TextRunId> {
        self.native.as_ref()?.run
    }

    pub fn ascent(&self) -> f32 {
        self.native.as_ref().map_or(0.0, |native| native.ascent)
    }
}

/// Font extents measured from the faces that actually supplied each character.
#[derive(Clone, Debug)]
pub enum RunFontMetrics {
    Uniform(FontMetrics),
    PerCharacter(Arc<[FontMetrics]>),
}

/// A backend run result. The engine owns registration and layout
/// glyph IDs. Native data can be retained by entry reference, or borrowed from
/// a backend resource generation. Use one ownership mode per native backend.
#[derive(Clone, Debug)]
pub struct ShapedRun {
    geometry: Option<ShapedTextRun>,
    paint: RunPaint,
}

#[derive(Clone, Debug)]
enum RunPaint {
    Native {
        source: NativePaintSource,
        metrics: RunFontMetrics,
    },
    Character(ShapedGlyph),
    Characters(Vec<(char, ShapedGlyph)>),
}

/// A request to shape one already-normalized, single-style logical run.
/// CSS geometry has been resolved before this reaches the renderer backend.
/// Foreground colors belong to paint fragments and are absent from shaping requests.
pub struct TextRunShapeRequest<'a> {
    text: &'a str,
    style: ShapingStyle<'a>,
}

#[derive(Debug)]
pub struct ShapedTextGeometry {
    len: usize,
    pub bidi: ResolvedBidi,
    pub mirrored_glyphs: FxHashMap<u32, GlyphId>,
    ranges: Vec<TextGeometryRange>,
    runs: Vec<TextGeometryRun>,
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
}

impl ShapedTextGeometry {
    pub fn with_len(len: usize) -> Self {
        Self {
            len,
            bidi: ResolvedBidi::default(),
            mirrored_glyphs: FxHashMap::default(),
            ranges: Vec::new(),
            runs: Vec::new(),
        }
    }

    fn geometry(&self, range: &TextGeometryRange) -> &ShapedTextRun {
        &self.runs[range.run].geometry
    }

    pub(crate) fn ranges(&self) -> &[TextGeometryRange] {
        &self.ranges
    }

    pub(crate) fn native_run(&self, range: &TextGeometryRange) -> Option<NativeRunView<'_>> {
        let run = &self.runs[range.run];
        let native = run.geometry.native.as_ref()?;
        Some(NativeRunView {
            source_range: range.source.clone(),
            run_offset: range.offset as u32,
            backend_run: native.run?,
            caret_stops: &native.caret_stops,
            ascent: native.ascent,
            placement_required: run.placement_required,
        })
    }

    fn store_run(&mut self, geometry: ShapedTextRun, placement_required: bool) -> usize {
        let index = self.runs.len();
        self.runs.push(TextGeometryRun {
            geometry,
            placement_required,
        });
        index
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
        let run = self.store_run(geometry, placement_required);
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
        }
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
        // Replacement is a separate editing path. Release dead geometry and
        // remap surviving run IDs without maintaining a free list on append.
        let mut remap = vec![usize::MAX; self.runs.len()];
        for range in &self.ranges {
            remap[range.run] = 0;
        }
        let mut old = 0;
        let mut next = 0;
        self.runs.retain(|_| {
            let keep = remap[old] != usize::MAX;
            if keep {
                remap[old] = next;
                next += 1;
            }
            old += 1;
            keep
        });
        for range in &mut self.ranges {
            range.run = remap[range.run];
        }
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
            + self.runs.capacity() * size_of::<TextGeometryRun>()
            + self.bidi.memory_usage_bytes()
            + self.mirrored_glyphs.capacity() * size_of::<(u32, GlyphId)>()
            + self
                .runs
                .iter()
                .map(|run| {
                    allocation_bytes(&run.geometry.advances, seen)
                        + allocation_bytes(&run.geometry.cluster_boundaries, seen)
                        + (run.geometry)
                            .caret_stops()
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
            || shaped.advances().len() != (range.end - range.start) as usize
        {
            return false;
        }
        let mut cumulative_delta = 0.0f32;
        let advances_changed = range.clone().zip(shaped.advances()).any(|(index, new)| {
            cumulative_delta += new - self.advance(index as usize).unwrap_or(0.0);
            cumulative_delta.abs() > 0.01
        });
        let changed = advances_changed
            || (range.start..=range.end)
                .zip(shaped.cluster_boundaries())
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
        fmt::Debug::fmt(&(*self).degrees(), formatter)
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
    non_box_metrics: FxHashMap<StyleIndices, u32>,
    root_ch_px: f32,
    root_cap_height_px: f32,
    root_line_height_px: f32,
}

#[derive(Clone, Default)]
pub struct BoxFontMetrics {
    /// Zero is the fallback metric; positive IDs index `metrics` plus one.
    pub metric_ids: Vec<u32>,
    pub metrics: Vec<FontRelativeMetrics>,
}

impl BoxFontMetrics {
    pub fn for_box(&self, box_idx: usize) -> FontRelativeMetrics {
        self.metric_ids
            .get(box_idx)
            .copied()
            .unwrap_or(0)
            .checked_sub(1)
            .map_or_else(FontRelativeMetrics::default, |id| self.metrics[id as usize])
    }
}

impl ShapedFontMetrics {
    pub fn new(
        box_metrics: BoxFontMetrics,
        non_box_metrics: FxHashMap<StyleIndices, u32>,
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

    /// Identity within this metrics table; zero denotes fallback metrics.
    pub fn box_metric_id(&self, box_idx: usize) -> u32 {
        self.box_metrics
            .metric_ids
            .get(box_idx)
            .copied()
            .unwrap_or(0)
    }

    pub fn for_non_box_style(&self, indices: StyleIndices) -> FontRelativeMetrics {
        self.non_box_metrics
            .get(&indices)
            .map_or_else(FontRelativeMetrics::default, |id| {
                self.box_metrics.metrics[*id as usize - 1]
            })
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
        self.box_metrics.metric_ids.capacity() * std::mem::size_of::<u32>()
            + self.box_metrics.metrics.capacity() * std::mem::size_of::<FontRelativeMetrics>()
            + self.non_box_metrics.capacity() * std::mem::size_of::<(StyleIndices, u32)>()
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
        if run.advances.len() <= 1 || (text.is_ascii() && !text.as_bytes().contains(&b'\r')) {
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
            font_family,
            cjk_font_family: None,
            language: None,
            font_features: &FontFeatures::DEFAULT,
            caps_synthesis: CapsSynthesis::None,
            cursive_joining: CursiveJoining::Isolated,
        }
    }

    #[inline]
    pub fn run<'a>(text: &'a str, style: ShapingStyle<'a>) -> TextRunShapeRequest<'a> {
        TextRunShapeRequest { text, style }
    }
}

/// Engine registration, ownership or layout failure. Native shaping returns
/// measurements and resources directly; it has no recoverable error channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeError {
    kind: ShapeErrorKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ShapeErrorKind {
    RegistryCapacityExhausted { metrics_len: usize },
    FirstLineRefinementDidNotConverge,
    ResourceGenerationMismatch,
    BorrowedDocument,
    MixedPaintOwnership,
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
            ShapeErrorKind::FirstLineRefinementDidNotConverge => {
                formatter.write_str("first-line layout did not converge")
            }
            ShapeErrorKind::BorrowedDocument => formatter.write_str(
                "repreparation requires the owning document; this stage borrows its preparation",
            ),
            ShapeErrorKind::ResourceGenerationMismatch => {
                formatter.write_str("glyph shaper resources do not belong to this document")
            }
            ShapeErrorKind::MixedPaintOwnership => formatter.write_str(
                "a native backend must consistently return retained entries or borrowed handles",
            ),
        }
    }
}

impl std::error::Error for ShapeError {}

/// Glyph metrics and native paint snapshots owned by the pipeline or caller.
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
    id: GlyphResourceId,
    generation: GlyphResourceGeneration,
    shaped_ids: FxHashMap<(GlyphPaintIdentity, char, u32, u32, u32), GlyphId>,
    paint_invalid: bool,
    native_paint_resources: Option<NativePaintResources>,
    paint_lease: OnceLock<GlyphResourceLease>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum GlyphPaintIdentity {
    Borrowed(GlyphHandle),
    Retained(usize),
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

/// Identity of one engine ledger, independent of a caller's native namespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GlyphResourceId(u64);

impl Default for GlyphResourceId {
    fn default() -> Self {
        Self(GlyphResourceGeneration::new().0)
    }
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
            id: GlyphResourceId::default(),
            generation,
            shaped_ids: FxHashMap::default(),
            paint_invalid: false,
            native_paint_resources: None,
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

    pub fn native_paint_resources(&self) -> Option<&NativePaintResources> {
        self.native_paint_resources.as_ref()
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
        if let Some(resources) = &self.native_paint_resources {
            report.extend_prefixed(
                "native_entries",
                resources.memory_usage_report_with_shared(seen),
            );
        }
        report
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn id(&self) -> GlyphResourceId {
        self.id
    }

    pub fn generation(&self) -> GlyphResourceGeneration {
        self.generation
    }

    /// Discard appended registrations, preserving the existing prefix.
    /// Callers must retain the length only while exclusively borrowing this store
    /// for append-only work; replacing or clearing it invalidates that length.
    pub fn truncate(&mut self, len: usize) {
        assert!(
            len <= self.len(),
            "glyph prefix must belong to the current store"
        );
        if len == self.len() {
            return;
        }
        self.metrics.truncate(len);
        if len == 0 {
            self.native_paint_resources = None;
        } else if let Some(resources) = &mut self.native_paint_resources {
            resources.truncate(len);
        }
        self.shaped_ids.retain(|_, id| (id.0 as usize) < len);
    }

    /// Register geometry and native entry references or borrowed paint handles.
    /// The engine assigns layout IDs and retained paint IDs; backends never
    /// receive its registry or glyph buffer.
    pub fn shape_run(
        &mut self,
        shaper: &mut (impl GlyphShaper + ?Sized),
        request: TextRunShapeRequest<'_>,
        glyphs: &mut [GlyphId],
    ) -> Result<Option<ShapedTextRun>, ShapeError> {
        let text = request.text();
        let ShapedRun {
            mut geometry,
            paint,
        } = shaper.shape_run(request);
        match paint {
            RunPaint::Character(paint) => {
                glyphs[0] =
                    self.register_shaped(text.chars().next().expect("one-character run"), paint)?;
            }
            RunPaint::Characters(paints) => {
                let mut ascii = [GlyphId(u32::MAX); 128];
                let mut non_ascii = FxHashMap::default();
                for (character, paint) in paints {
                    let id = self.register_shaped(character, paint)?;
                    if character.is_ascii() {
                        ascii[character as usize] = id;
                    } else {
                        non_ascii.insert(character, id);
                    }
                }
                for (slot, character) in glyphs.iter_mut().zip(text.chars()) {
                    *slot = if character.is_ascii() {
                        ascii[character as usize]
                    } else {
                        non_ascii[&character]
                    };
                }
            }
            RunPaint::Native { source, metrics } => {
                self.ensure_paint_ownership(matches!(&source, NativePaintSource::Retained(_)))?;
                let first_handle = match &source {
                    NativePaintSource::Borrowed(handle) => *handle,
                    NativePaintSource::Retained(_) => GlyphHandle(self.len() as u32),
                };
                // Repeated isolated runs reuse registrations as well as native cache entries.
                let mut characters = text.chars();
                if let Some(character) = characters.next()
                    && characters.next().is_none()
                    && let RunFontMetrics::Uniform(font_metrics) = &metrics
                {
                    let advance = (geometry.as_ref().expect("native run geometry")).advances()[0];
                    let shaped = match source {
                        NativePaintSource::Borrowed(handle) => {
                            ShapedGlyph::new(handle, advance, *font_metrics)
                        }
                        NativePaintSource::Retained(resource) => {
                            ShapedGlyph::retained(resource, advance, *font_metrics)
                        }
                    };
                    let glyph = self.register_shaped(character, shaped)?;
                    glyphs[0] = glyph;
                    if let Some(native) = geometry
                        .as_mut()
                        .and_then(|geometry| geometry.native.as_mut())
                        && let Some(resources) = &self.native_paint_resources
                    {
                        native.run = resources.run_for_glyph(glyph.0);
                    }
                } else {
                    let range = self
                        .metrics
                        .register_run(
                            text,
                            geometry
                                .as_ref()
                                .expect("native run geometry")
                                .shared_advances(),
                            metrics,
                            first_handle,
                        )
                        .map_err(|_| shape_error_registry_capacity_exhausted(self.len()))?;
                    if let NativePaintSource::Retained(resource) = source
                        && !range.is_empty()
                    {
                        let run = self
                            .native_paint_resources
                            .get_or_insert_with(NativePaintResources::default)
                            .push(range.clone(), resource);
                        if let Some(native) = geometry
                            .as_mut()
                            .and_then(|geometry| geometry.native.as_mut())
                        {
                            native.run = Some(run);
                        }
                    }
                    for (slot, id) in glyphs.iter_mut().zip(range) {
                        *slot = GlyphId(id);
                    }
                }
            }
        }
        Ok(geometry)
    }

    /// Intern one paint entry supplied as part of a complete run result.
    fn register_shaped(
        &mut self,
        character: char,
        shaped: ShapedGlyph,
    ) -> Result<GlyphId, ShapeError> {
        let metrics = shaped.metrics();
        let identity = match &shaped.source {
            NativePaintSource::Borrowed(handle) => GlyphPaintIdentity::Borrowed(*handle),
            NativePaintSource::Retained(resource) => {
                GlyphPaintIdentity::Retained(resource.identity())
            }
        };
        let key = (
            identity,
            character,
            shaped.advance().to_bits(),
            metrics.ascent.to_bits(),
            metrics.descent.to_bits(),
        );
        if let Some(&id) = self.shaped_ids.get(&key) {
            return Ok(id);
        }
        self.ensure_paint_ownership(matches!(&shaped.source, NativePaintSource::Retained(_)))?;
        let id = self.register(GlyphMetric::from_shaped(character, &shaped))?;
        match shaped.source {
            NativePaintSource::Borrowed(handle) => self.metrics.set_paint_handle(id, handle),
            NativePaintSource::Retained(resource) => {
                self.metrics.set_paint_handle(id, GlyphHandle(id.0));
                self.native_paint_resources
                    .get_or_insert_with(NativePaintResources::default)
                    .push(id.0..id.0 + 1, resource);
            }
        }
        self.shaped_ids.insert(key, id);
        Ok(id)
    }

    fn ensure_paint_ownership(&self, retained: bool) -> Result<(), ShapeError> {
        if self.is_empty() || retained == self.native_paint_resources.is_some() {
            Ok(())
        } else {
            Err(ShapeError {
                kind: ShapeErrorKind::MixedPaintOwnership,
            })
        }
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

/// Backend supplied by a renderer adapter for shaping styled text runs
/// and returning its native paint resources. HtmlEngine assigns layout glyph
/// IDs and reports its own registration and layout errors separately.
/// Basic adapters can use [`crate::ResourceShaper`] for native storage and
/// explicit reset support while borrowing a platform text system.
pub trait GlyphShaper {
    /// Selects the publication `@font-face` declarations and reader font
    /// policy of the document about to be shaped. The engine calls this before
    /// every document and append shaping pass, so a backend's active faces
    /// always belong to the document it shapes. Backends that register
    /// publication fonts resolve authored families through these faces;
    /// others ignore them.
    fn select_publication_fonts(&mut self, _faces: &[FontFace], _overrides: &ReaderStyleOverrides) {
    }

    /// Identity of the backend's native resource namespace. Caller-controlled
    /// invalidation must change it when borrowed handles stop being valid.
    fn resource_generation(&self) -> GlyphResourceGeneration;

    /// Begin reuse of temporary shaping results within one document/layout batch.
    fn begin_shaping(&mut self) {}

    /// Release temporary native scratch after a complete shaping/layout batch.
    /// Called on success and engine/layout errors; published glyph resources remain valid.
    fn finish_shaping(&mut self) {}

    /// Resolve metrics for the selected primary font face, applying the supplied
    /// font features to glyph advances such as `ch`. Backends without a
    /// metrics API use the CSS-defined 0.5em x-height fallback; layout derives
    /// missing vertical metrics from its shaped line geometry.
    fn font_relative_metrics(&mut self, _style: &ShapingStyle<'_>) -> FontRelativeMetrics {
        FontRelativeMetrics::default()
    }

    /// Shape a complete logical run using resolved CSS properties. This includes
    /// isolated inserted characters and isolated mandatory line breaks. Native
    /// backends return contextual geometry; basic adapters return character paint
    /// entries through [`ShapedRun::characters`]. Neither requires an engine fallback.
    fn shape_run(&mut self, request: TextRunShapeRequest<'_>) -> ShapedRun;
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
        assert_eq!(selected.line_box_extents(20.0), (20.0, 5.0));
        assert_eq!(selected.line_box_extents(0.0), (0.0, 0.0));
        assert_eq!(
            (FontRelativeMetrics::default()).line_box_extents(20.0),
            (16.0, 4.0)
        );
    }

    #[test]
    fn append_checkpoints_own_no_run_data_and_preserve_the_prefix() {
        let mut resources = GlyphResourceStore::default();
        let generation = resources.generation();
        let advances: Arc<[f32]> = [2.0, 3.0].into();
        let run = ShapedTextRun::new(advances.clone(), vec![true; 3].into());
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
        let checkpoint = resources.len();
        assert_eq!(Arc::strong_count(&advances), owners);
        let extra = GlyphMetric::new('x', 1.0, 8.0, 2.0);
        assert_eq!(resources.register(extra).unwrap(), GlyphId(2));
        resources.truncate(checkpoint);
        assert_eq!(resources.generation(), generation);
        assert_eq!(resources.len(), 2);
        assert_eq!(resources.record_count(), 1);
        assert_eq!(resources.get(GlyphId(0)).unwrap().ch(), 'a');
        assert!(resources.get(GlyphId(2)).is_none());
        assert_eq!(resources.register(extra).unwrap(), GlyphId(2));
    }

    fn fallback(len: usize, advance: f32) -> ShapedTextRun {
        ShapedTextRun::new(vec![advance; len].into(), vec![true; len + 1].into())
    }

    #[test]
    fn scattered_ranges_and_native_queries_borrow_one_stored_run() {
        let advances: Arc<[f32]> = [1.0, 2.0, 3.0, 4.0].into();
        let boundaries: Arc<[bool]> = [true, false, true, false, true].into();
        let carets: Arc<[f32]> = [0.0, 1.0, 3.0, 6.0, 10.0].into();
        let run = ShapedTextRun::with_backend_run(
            advances.clone(),
            boundaries.clone(),
            TextRunId(7),
            carets.clone().into(),
            8.0,
        );
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
        let run = ShapedTextRun::with_backend_run(
            vec![1.0; 6].into(),
            vec![true; 7].into(),
            TextRunId(3),
            carets.clone().into(),
            8.0,
        );
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
            let mut run = ShapedTextRun::with_backend_run(
                advances.clone(),
                vec![true; 3].into(),
                TextRunId(0),
                carets.clone().into(),
                8.0,
            );
            run = shaping::apply_spacing(run, [0.0, 0.0].into_iter(), right_to_left);
            assert_eq!(run.advances().as_ptr(), advances.as_ptr());
            assert_eq!(
                run.caret_stops().unwrap().to_dense().as_ptr(),
                carets.as_ptr()
            );
            run = shaping::apply_spacing(run, [1.0, -0.5].into_iter(), right_to_left);
            assert_eq!(run.advances(), [3.0, 2.5]);
            assert_eq!(run.caret_stops().unwrap().to_dense().as_ref(), expected);
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

impl ShapedRun {
    /// Geometry whose characters occupy consecutive native paint handles.
    /// The adapter supplies a representable handle range and either uniform
    /// extents or one font metric per character. Shared arrays are retained.

    pub fn painted(
        geometry: ShapedTextRun,
        first_handle: GlyphHandle,
        metrics: RunFontMetrics,
    ) -> ShapedRun {
        ShapedRun {
            geometry: Some(geometry),
            paint: RunPaint::Native {
                source: NativePaintSource::Borrowed(first_handle),
                metrics,
            },
        }
    }

    /// Return measurements together with a reference to a caller-owned native run.
    /// The engine assigns document-local paint IDs when it registers the result.

    pub fn retained(
        geometry: ShapedTextRun,
        resource: impl Into<NativePaintResource>,
        metrics: RunFontMetrics,
    ) -> ShapedRun {
        ShapedRun {
            geometry: Some(geometry),
            paint: RunPaint::Native {
                source: NativePaintSource::Retained(resource.into()),
                metrics,
            },
        }
    }

    /// A complete run for adapters that paint individual characters. The callback
    /// stays inside the adapter; repeated characters are measured once per run.
    pub fn characters(
        request: TextRunShapeRequest<'_>,
        mut shape: impl FnMut(char, &ShapingStyle<'_>) -> ShapedGlyph,
    ) -> ShapedRun {
        let mut characters = request.text.chars();
        if let Some(character) = characters.next()
            && characters.next().is_none()
        {
            return ShapedRun {
                geometry: None,
                paint: RunPaint::Character(shape(character, &request.style)),
            };
        }
        let mut ascii = [false; 128];
        let mut non_ascii = FxHashSet::default();
        let mut glyphs = Vec::new();
        for character in request.text.chars() {
            let fresh = if character.is_ascii() {
                !std::mem::replace(&mut ascii[character as usize], true)
            } else {
                non_ascii.insert(character)
            };
            if fresh {
                glyphs.push((character, shape(character, &request.style)));
            }
        }
        ShapedRun {
            geometry: None,
            paint: RunPaint::Characters(glyphs),
        }
    }

    /// Supply contextual geometry while retaining a complete run's paint entries.

    pub fn with_geometry(mut self, geometry: ShapedTextRun) -> ShapedRun {
        self.geometry = Some(geometry);
        self
    }

    pub fn into_geometry(self) -> Option<ShapedTextRun> {
        self.geometry
    }
}

impl FontSlant {
    /// The caller supplies a resolved CSS angle from -90 through 90 degrees.

    pub fn oblique(degrees: f32) -> FontSlant {
        FontSlant::Oblique {
            angle: ObliqueAngle(degrees.to_bits()),
        }
    }
}

impl<'a> TextRunShapeRequest<'a> {
    pub fn style(&self) -> &ShapingStyle<'a> {
        &self.style
    }

    pub fn text(&self) -> &'a str {
        self.text
    }
}

impl CursiveJoining {
    #[inline]
    pub const fn connects_to_next(self) -> bool {
        matches!(
            self,
            CursiveJoining::ConnectedToNext | CursiveJoining::ConnectedToBoth
        )
    }

    #[inline]
    pub const fn connects_to_previous(self) -> bool {
        matches!(
            self,
            CursiveJoining::ConnectedToPrevious | CursiveJoining::ConnectedToBoth
        )
    }
}

impl<'a> ShapingStyle<'a> {
    /// Select a character's resolved family, including the reader's CJK override.

    pub fn font_family_for_character(&self, character: char) -> Option<&'a str> {
        self.cjk_font_family
            .filter(|_| html_style_model::is_cjk_character(character))
            .or(self.font_family)
    }
}

impl FontRelativeMetrics {
    /// Font content extents at a CSS font size, before line-height leading.

    pub fn line_box_extents(self, font_size: f64) -> (f64, f64) {
        self.line_metrics.map_or_else(
            || (font_size * 0.8, font_size * 0.2),
            |metrics| {
                (
                    font_size * f64::from(metrics.ascent_ratio),
                    font_size * f64::from(metrics.descent_ratio),
                )
            },
        )
    }
}

pub(crate) fn shape_error_registry_capacity_exhausted(metrics_len: usize) -> ShapeError {
    ShapeError {
        kind: ShapeErrorKind::RegistryCapacityExhausted { metrics_len },
    }
}

impl ObliqueAngle {
    pub fn degrees(self) -> f32 {
        f32::from_bits(self.0)
    }
}
