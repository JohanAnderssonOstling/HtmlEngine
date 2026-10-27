//! Renderer-neutral shaping contracts and validated font metrics.

use crate::*;
use html_dom::MemoryUsageReport;
use std::sync::atomic::{AtomicU64, Ordering};

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
    pub advances: Arc<[f32]>,
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
            || advances
                .iter()
                .any(|advance| !advance.is_finite() || *advance < 0.0)
        {
            return None;
        }
        Some(Self {
            advances,
            cluster_boundaries,
            backend_run: None,
            caret_stops: None,
            ascent: 0.0,
        })
    }

    pub fn with_backend_run(
        advances: Arc<[f32]>,
        cluster_boundaries: Arc<[bool]>,
        backend_run: TextRunId,
        caret_stops: Arc<[f32]>,
        ascent: f32,
    ) -> Option<Self> {
        let run = Self::new(advances, cluster_boundaries)?;
        (caret_stops.len() == run.advances.len() + 1
            && caret_stops.iter().all(|stop| stop.is_finite())
            && ascent.is_finite())
        .then_some(Self {
            backend_run: Some(backend_run),
            caret_stops: Some(caret_stops),
            ascent,
            ..run
        })
    }

    pub fn advances(&self) -> &[f32] {
        &self.advances
    }

    /// Share validated geometry with resource ledgers and layout without
    /// copying character arrays at adapter boundaries.
    pub fn shared_advances(&self) -> Arc<[f32]> {
        self.advances.clone()
    }

    pub fn cluster_boundaries(&self) -> &[bool] {
        &self.cluster_boundaries
    }

    /// Apply range-local CSS spacing to both layout widths and native caret
    /// positions. Backend cache geometry remains unchanged.
    pub fn apply_spacing(&mut self, deltas: &[f32], right_to_left: bool) {
        assert_eq!(deltas.len(), self.advances.len());
        if !deltas.iter().any(|delta| *delta != 0.0) {
            return;
        }
        for (advance, delta) in Arc::make_mut(&mut self.advances).iter_mut().zip(deltas) {
            *advance += *delta;
        }
        if let Some(stops) = &mut self.caret_stops {
            let mut cumulative = 0.0;
            for (offset, stop) in Arc::make_mut(stops).iter_mut().enumerate() {
                *stop += if right_to_left {
                    -cumulative
                } else {
                    cumulative
                };
                if let Some(delta) = deltas.get(offset) {
                    cumulative += delta;
                }
            }
        }
    }

    pub fn preserve_graphemes(&mut self, text: &str) {
        profile_scope!("shape.preserve_graphemes");
        if text.is_ascii() && !text.as_bytes().contains(&b'\r') {
            return;
        }
        let graphemes = super::bidi::grapheme_boundaries(text, self.advances.len());
        if self
            .cluster_boundaries
            .iter()
            .zip(graphemes.iter())
            .any(|(&native, &grapheme)| native && !grapheme)
        {
            for (native, &grapheme) in Arc::make_mut(&mut self.cluster_boundaries)
                .iter_mut()
                .zip(graphemes.iter())
            {
                *native &= grapheme;
            }
        }
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
        (style.byte_range() == (0..text.len()) && !text.is_empty()).then_some(Self {
            text,
            style,
            paint_colors: None,
        })
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
        Some(NativeRunView {
            source_range: range.source.clone(),
            run_offset: range.offset as u32,
            backend_run: run.geometry.backend_run()?,
            caret_stops: run.geometry.caret_stops()?,
            ascent: run.geometry.ascent(),
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
        if indices.is_empty() {
            return;
        }
        let run = self.store_run(geometry, placement_required, 0);
        let mut range_count = 0;
        let mut start = 0;
        while start < indices.len() {
            let mut end = start + 1;
            while end < indices.len() && indices[end] == indices[end - 1] + 1 {
                end += 1;
            }
            self.ranges.push(TextGeometryRange {
                source: indices[start]..indices[end - 1] + 1,
                offset: start,
                run,
                apply_start_boundary: true,
            });
            range_count += 1;
            start = end;
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
        ranges.push(TextGeometryRange {
            source: range,
            offset: 0,
            run: self.store_run(geometry, placement_required, 1),
            apply_start_boundary: true,
        });
        ranges.sort_by_key(|run| run.source.start);
        self.ranges = ranges;
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
                        + run
                            .geometry
                            .caret_stops
                            .as_ref()
                            .map_or(0, |stops| allocation_bytes(stops, seen))
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
    language: Option<&'a ContentLanguage>,
}

impl<'a> FontMetricsRequest<'a> {
    pub fn new(
        font_size: f32,
        font_weight: u16,
        font_slant: FontSlant,
        font_family: Option<&'a str>,
    ) -> Option<Self> {
        (font_size.is_finite() && font_size > 0.0 && font_weight > 0).then_some(Self {
            font_size,
            font_weight,
            font_slant,
            font_family,
            language: None,
        })
    }

    /// Content language used when selecting the face whose metrics are measured.
    pub fn with_language(mut self, language: Option<&'a ContentLanguage>) -> Self {
        self.language = language;
        self
    }

    pub fn language(self) -> Option<&'a ContentLanguage> {
        self.language
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

#[derive(Clone)]
pub enum RequiredFontMetrics<T> {
    NotRequired,
    Required(T),
}

impl ShapedFontMetrics {
    pub fn new(
        box_metrics: BoxFontMetrics,
        non_box_metrics: RequiredFontMetrics<FxHashMap<StyleIndices, FontRelativeMetrics>>,
        expected_box_count: usize,
        root_ch_px: f32,
        root_cap_height_px: f32,
        root_line_height_px: f32,
    ) -> Self {
        match &box_metrics {
            BoxFontMetrics::NotRequired => {}
            BoxFontMetrics::Dense(values) => assert_eq!(
                values.len(),
                expected_box_count,
                "dense shaping metrics must provide one entry per layout box"
            ),
            BoxFontMetrics::Indexed {
                metric_ids,
                metrics,
            } => {
                assert_eq!(
                    metric_ids.len(),
                    expected_box_count,
                    "indexed shaping metrics must provide one ID per layout box"
                );
                assert!(
                    metric_ids
                        .iter()
                        .all(|id| *id == 0 || (*id as usize) <= metrics.len()),
                    "indexed shaping metric ID is out of range"
                );
            }
        }
        Self {
            box_metrics,
            non_box_metrics,
            root_ch_px,
            root_cap_height_px,
            root_line_height_px,
        }
    }

    pub fn for_box(&self, box_idx: usize) -> FontRelativeMetrics {
        match &self.box_metrics {
            BoxFontMetrics::NotRequired => FontRelativeMetrics::fallback(),
            BoxFontMetrics::Dense(values) => values[box_idx],
            BoxFontMetrics::Indexed {
                metric_ids,
                metrics,
            } => metric_ids[box_idx]
                .checked_sub(1)
                .map_or_else(FontRelativeMetrics::fallback, |id| metrics[id as usize]),
        }
    }

    pub fn for_non_box_style(&self, indices: StyleIndices) -> FontRelativeMetrics {
        match &self.non_box_metrics {
            RequiredFontMetrics::NotRequired => FontRelativeMetrics::fallback(),
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
                metrics.x_height_ratio(),
                metrics.ch_advance_ratio(),
                metrics.cap_height_ratio(),
                self.root_ch_px,
                self.root_cap_height_px,
                self.root_line_height_px,
            )
            .unwrap_or_else(|| style.font_size())
    }

    fn used_style_with_metrics<'styles>(
        &self,
        styles: &'styles ComputedStyles,
        indices: StyleIndices,
        metrics: FontRelativeMetrics,
    ) -> Option<UsedStyleView<'styles>> {
        styles.used_view_with_root(
            indices,
            metrics.x_height_ratio(),
            metrics.ch_advance_ratio(),
            metrics.cap_height_ratio(),
            self.root_ch_px,
            self.root_cap_height_px,
            self.root_line_height_px,
        )
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

impl FontRelativeMetrics {
    pub fn new(x_height_ratio: f32, ch_advance_ratio: f32, cap_height_ratio: f32) -> Option<Self> {
        (x_height_ratio.is_finite()
            && x_height_ratio > 0.0
            && ch_advance_ratio.is_finite()
            && ch_advance_ratio >= 0.0
            && cap_height_ratio.is_finite()
            && cap_height_ratio > 0.0)
            .then_some(Self {
                x_height_ratio,
                ch_advance_ratio,
                cap_height_ratio,
                ascent_ratio: 0.0,
                descent_ratio: 0.0,
            })
    }

    pub fn from_ratios(
        x_height_ratio: f32,
        ch_advance_ratio: f32,
        cap_height_ratio: f32,
        ascent_ratio: f32,
    ) -> Option<Self> {
        (x_height_ratio.is_finite()
            && x_height_ratio > 0.0
            && ch_advance_ratio.is_finite()
            && ch_advance_ratio >= 0.0
            && cap_height_ratio.is_finite()
            && cap_height_ratio > 0.0
            && ascent_ratio.is_finite()
            && ascent_ratio > 0.0)
            .then_some(Self {
                x_height_ratio,
                ch_advance_ratio,
                cap_height_ratio,
                ascent_ratio,
                descent_ratio: (1.0 - ascent_ratio).max(0.0),
            })
    }

    pub fn from_line_ratios(
        x_height_ratio: f32,
        ch_advance_ratio: f32,
        cap_height_ratio: f32,
        ascent_ratio: f32,
        descent_ratio: f32,
    ) -> Option<Self> {
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
            .then_some(Self {
                x_height_ratio,
                ch_advance_ratio,
                cap_height_ratio,
                ascent_ratio,
                descent_ratio,
            })
    }

    pub fn fallback() -> Self {
        Self {
            x_height_ratio: 0.5,
            ch_advance_ratio: 0.5,
            cap_height_ratio: 0.8,
            ascent_ratio: 0.0,
            descent_ratio: 0.0,
        }
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
    direction: TextDirection,
    font_size: f32,
    font_weight: u16,
    font_slant: FontSlant,
    color: u32,
    font_family: Option<&'a str>,
    cjk_font_family: Option<&'a str>,
    language: Option<&'a ContentLanguage>,
    font_features: &'a [OpenTypeFeature],
}

impl<'a> TextStyleSpan<'a> {
    pub fn new(
        byte_range: Range<usize>,
        font_size: f32,
        font_weight: u16,
        font_slant: FontSlant,
        color: u32,
        font_family: Option<&'a str>,
    ) -> Option<Self> {
        // CSS permits `font-size: 0`. It still needs a real shaping request so
        // character identities and backend paint resources stay registered;
        // the resulting advances and ink geometry are simply zero-sized.
        if byte_range.start >= byte_range.end || !font_size.is_finite() || font_size < 0.0 {
            return None;
        }
        Some(Self {
            byte_range,
            direction: TextDirection::Ltr,
            font_size,
            font_weight,
            font_slant,
            color,
            font_family,
            cjk_font_family: None,
            language: None,
            font_features: &[],
        })
    }

    pub fn with_font_features(mut self, font_features: &'a [OpenTypeFeature]) -> Self {
        self.font_features = font_features;
        self
    }

    /// Inherited BCP 47 content language, for font selection and localized glyphs.
    pub fn with_language(mut self, language: Option<&'a ContentLanguage>) -> Self {
        self.language = language;
        self
    }

    pub fn language(&self) -> Option<&'a ContentLanguage> {
        self.language
    }

    /// Resolved direction of this shaping run, after paragraph bidi analysis.
    pub fn with_direction(mut self, direction: TextDirection) -> Self {
        self.direction = direction;
        self
    }

    pub fn direction(&self) -> TextDirection {
        self.direction
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

    pub fn with_cjk_font_family(mut self, family: Option<&'a str>) -> Self {
        self.cjk_font_family = family;
        self
    }

    pub fn cjk_font_family(&self) -> Option<&'a str> {
        self.cjk_font_family
    }

    pub fn font_family_for_character(&self, character: char) -> Option<&'a str> {
        self.cjk_font_family
            .filter(|_| html_style_model::is_cjk_character(character))
            .or(self.font_family)
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

    pub fn with_baseline_shift(
        extra_advance: f32,
        advance_override: Option<f32>,
        baseline_shift: f32,
        paint: bool,
    ) -> Option<Self> {
        if !extra_advance.is_finite()
            || !baseline_shift.is_finite()
            || advance_override.is_some_and(|advance| !advance.is_finite() || advance < 0.0)
        {
            return None;
        }
        Some(Self {
            extra_advance,
            advance_override,
            baseline_shift,
            paint,
        })
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
        Self {
            extra_advance: 0.0,
            advance_override: None,
            baseline_shift: 0.0,
            paint: true,
        }
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
    pub fn new(
        line_index: usize,
        text_range: Range<u32>,
        text: &'a str,
        styles: &'a [TextStyleSpan<'a>],
        placements: &'a [CharacterPlacement],
    ) -> Option<Self> {
        let character_count = text.chars().count();
        if text.is_empty()
            || styles.is_empty()
            || character_count != text_range.end.checked_sub(text_range.start)? as usize
            || placements.len() != character_count
        {
            return None;
        }
        let mut next_byte = 0;
        for style in styles {
            let range = style.byte_range();
            if range.start != next_byte
                || range.end > text.len()
                || !text.is_char_boundary(range.start)
                || !text.is_char_boundary(range.end)
            {
                return None;
            }
            next_byte = range.end;
        }
        if next_byte != text.len() {
            return None;
        }
        Some(Self {
            line_index,
            text_range,
            text,
            styles,
            placements,
        })
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
        if raw.len() != self.placements.len().checked_add(1)?
            || raw.iter().any(|value| !value.is_finite())
        {
            return None;
        }
        let mut adjusted = Vec::with_capacity(raw.len());
        adjusted.push(raw[0]);
        for (index, placement) in self.placements.iter().enumerate() {
            let natural_advance = raw[index + 1] - raw[index];
            let direction = if natural_advance < 0.0 { -1.0 } else { 1.0 };
            let advance = placement.advance_override().map_or(
                natural_advance + direction * placement.extra_advance(),
                |advance| direction * advance,
            );
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
    RunLengthMismatch {
        characters: usize,
        advances: usize,
    },
    UnregisteredGlyphId {
        glyph_id: GlyphId,
        metrics_len: usize,
    },
    RejectedMetric {
        reason: GlyphMetricError,
    },
    RegistryCapacityExhausted {
        metrics_len: usize,
    },
    FirstLineRefinementDidNotConverge,
    FirstLineShaperRequired,
    AppendTransactionUnsupported,
    ResourceGenerationMismatch,
    BorrowedDocument,
}

impl ShapeError {
    pub fn first_line_shaper_required() -> Self {
        Self {
            kind: ShapeErrorKind::FirstLineShaperRequired,
        }
    }

    pub fn requires_first_line_shaper(&self) -> bool {
        matches!(self.kind, ShapeErrorKind::FirstLineShaperRequired)
    }

    pub fn unregistered_glyph_id(glyph_id: GlyphId, metrics_len: usize) -> Self {
        Self {
            kind: ShapeErrorKind::UnregisteredGlyphId {
                glyph_id,
                metrics_len,
            },
        }
    }

    pub fn rejected_metric(reason: GlyphMetricError) -> Self {
        Self {
            kind: ShapeErrorKind::RejectedMetric { reason },
        }
    }

    pub fn registry_capacity_exhausted(metrics_len: usize) -> Self {
        Self {
            kind: ShapeErrorKind::RegistryCapacityExhausted { metrics_len },
        }
    }

    pub fn first_line_refinement_did_not_converge() -> Self {
        Self {
            kind: ShapeErrorKind::FirstLineRefinementDidNotConverge,
        }
    }

    pub fn append_transaction_unsupported() -> Self {
        Self {
            kind: ShapeErrorKind::AppendTransactionUnsupported,
        }
    }

    pub fn resource_generation_mismatch() -> Self {
        Self {
            kind: ShapeErrorKind::ResourceGenerationMismatch,
        }
    }

    pub fn borrowed_document() -> Self {
        Self {
            kind: ShapeErrorKind::BorrowedDocument,
        }
    }

    fn run_length_mismatch(characters: usize, advances: usize) -> Self {
        Self {
            kind: ShapeErrorKind::RunLengthMismatch {
                characters,
                advances,
            },
        }
    }

    pub fn glyph_id(self) -> Option<GlyphId> {
        match self.kind {
            ShapeErrorKind::UnregisteredGlyphId { glyph_id, .. } => Some(glyph_id),
            ShapeErrorKind::RunLengthMismatch { .. }
            | ShapeErrorKind::RejectedMetric { .. }
            | ShapeErrorKind::RegistryCapacityExhausted { .. }
            | ShapeErrorKind::FirstLineRefinementDidNotConverge
            | ShapeErrorKind::FirstLineShaperRequired
            | ShapeErrorKind::AppendTransactionUnsupported
            | ShapeErrorKind::ResourceGenerationMismatch
            | ShapeErrorKind::BorrowedDocument => None,
        }
    }

    pub fn metrics_len(self) -> Option<usize> {
        match self.kind {
            ShapeErrorKind::UnregisteredGlyphId { metrics_len, .. }
            | ShapeErrorKind::RegistryCapacityExhausted { metrics_len } => Some(metrics_len),
            ShapeErrorKind::RunLengthMismatch { .. }
            | ShapeErrorKind::RejectedMetric { .. }
            | ShapeErrorKind::FirstLineRefinementDidNotConverge
            | ShapeErrorKind::FirstLineShaperRequired
            | ShapeErrorKind::AppendTransactionUnsupported
            | ShapeErrorKind::ResourceGenerationMismatch
            | ShapeErrorKind::BorrowedDocument => None,
        }
    }

    pub fn reason(self) -> Option<GlyphMetricError> {
        match self.kind {
            ShapeErrorKind::RejectedMetric { reason } => Some(reason),
            ShapeErrorKind::RunLengthMismatch { .. }
            | ShapeErrorKind::UnregisteredGlyphId { .. }
            | ShapeErrorKind::RegistryCapacityExhausted { .. } => None,
            ShapeErrorKind::FirstLineRefinementDidNotConverge
            | ShapeErrorKind::FirstLineShaperRequired
            | ShapeErrorKind::AppendTransactionUnsupported
            | ShapeErrorKind::ResourceGenerationMismatch
            | ShapeErrorKind::BorrowedDocument => None,
        }
    }
}

impl fmt::Display for ShapeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ShapeErrorKind::RunLengthMismatch {
                characters,
                advances,
            } => write!(
                formatter,
                "shaped run has {characters} characters but {advances} advances"
            ),
            ShapeErrorKind::UnregisteredGlyphId {
                glyph_id,
                metrics_len,
            } => {
                write!(
                    formatter,
                    "glyph shaper returned ID {} with only {} registered metrics",
                    glyph_id, metrics_len
                )
            }
            ShapeErrorKind::RejectedMetric { reason } => write!(
                formatter,
                "glyph shaper returned invalid glyph metric: {reason:?}"
            ),
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
pub struct GlyphResourceStore {
    metrics: GlyphMetrics,
    generation: GlyphResourceGeneration,
    shaped_ids: FxHashMap<(GlyphHandle, char, u32, u32, u32), GlyphId>,
}

/// An append-only ledger checkpoint. It owns no glyph data and can be consumed
/// once to discard registrations made after it. Use only while this store has
/// been appended to; document replacement and branching rollback invalidate it.
#[derive(Debug)]
pub struct GlyphResourceCheckpoint {
    generation: GlyphResourceGeneration,
    len: usize,
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

impl Default for GlyphResourceStore {
    fn default() -> Self {
        Self {
            metrics: GlyphMetrics::default(),
            generation: GlyphResourceGeneration::new(),
            shaped_ids: FxHashMap::default(),
        }
    }
}

impl GlyphResourceStore {
    pub fn new(generation: GlyphResourceGeneration) -> Self {
        Self {
            metrics: GlyphMetrics::default(),
            generation,
            shaped_ids: FxHashMap::default(),
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
        report.add(
            "GlyphResourceStore.shaped_ids",
            self.shaped_ids.capacity()
                * std::mem::size_of::<((GlyphHandle, char, u32, u32, u32), GlyphId)>(),
            self.shaped_ids.len(),
        );
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
            .retain(|_, id| (*id as usize) < checkpoint.len);
        true
    }

    /// Shape and intern a native result, assigning its layout ID here.
    pub fn shape_glyph(
        &mut self,
        shaper: &mut (impl GlyphShaper + ?Sized),
        request: GlyphRequest<'_>,
    ) -> Result<GlyphId, ShapeError> {
        let shaped = shaper.shape_glyph(request)?;
        let metric = shaped.metric(request.character);
        let key = (
            shaped.handle(),
            request.character,
            metric.advance().to_bits(),
            metric.ascent().to_bits(),
            metric.descent().to_bits(),
        );
        if let Some(&id) = self.shaped_ids.get(&key) {
            return Ok(id);
        }
        let id = self.register_shaped(request.character, shaped)?;
        self.shaped_ids.insert(key, id);
        Ok(id)
    }

    /// Register a newly produced result without interning. Batch backends use
    /// this when each native resource is already unique to its shaped run.
    pub fn register_shaped(
        &mut self,
        character: char,
        shaped: ShapedGlyph,
    ) -> Result<GlyphId, ShapeError> {
        let id = self.register(shaped.metric(character))?;
        self.metrics.set_paint_handle(id, shaped.handle());
        Ok(id)
    }

    pub fn register(&mut self, metric: GlyphMetric) -> Result<GlyphId, ShapeError> {
        self.metrics
            .register(metric)
            .map_err(|_| ShapeError::registry_capacity_exhausted(self.len()))
    }

    /// Register a whole run using its existing validated advance allocation.
    /// IDs remain dense for compatibility with individual-glyph paint APIs;
    /// no per-character GlyphMetric records are created.
    pub fn register_run(
        &mut self,
        text: &str,
        shaped: &ShapedTextRun,
        ascent: f32,
        descent: f32,
    ) -> Result<Range<GlyphId>, ShapeError> {
        let characters: Box<[char]> = text.chars().collect();
        if characters.len() != shaped.advances.len() {
            return Err(ShapeError::run_length_mismatch(
                characters.len(),
                shaped.advances.len(),
            ));
        }
        let vertical = GlyphMetric::try_new('\0', 0.0, ascent, descent)
            .map_err(ShapeError::rejected_metric)?;
        self.metrics
            .register_run(characters, shaped.shared_advances(), vertical)
            .map_err(|_| ShapeError::registry_capacity_exhausted(self.len()))
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
        (glyph as usize) < self.len()
    }
}

/// Backend supplied by a renderer adapter for measuring styled characters
/// and returning handles to its native paint resources. HtmlEngine assigns
/// layout glyph IDs and stores the returned measurements.
pub trait GlyphShaper {
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
        _request: FontMetricsRequest<'_>,
    ) -> Result<FontRelativeMetrics, ShapeError> {
        Ok(FontRelativeMetrics::fallback())
    }

    /// Shape one character using the native text system. Return its paint
    /// handle, advance, and measured font extents; no metric registration is needed.
    fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError>;

    /// Shapes one normalized, single-style logical run into a caller-owned
    /// dense glyph-ID buffer. Backends can override this to resolve shared
    /// style state once and batch character lookup; the default preserves the
    /// established per-character contract.
    fn shape_glyph_run(
        &mut self,
        request: TextRunShapeRequest<'_>,
        glyphs: &mut [GlyphId],
        resources: &mut GlyphResourceStore,
    ) -> Result<Option<ShapedTextRun>, ShapeError> {
        debug_assert_eq!(request.text().chars().count(), glyphs.len());
        let style = request.style();
        for (slot, character) in glyphs.iter_mut().zip(request.text().chars()) {
            *slot = resources.shape_glyph(
                self,
                GlyphRequest {
                    character,
                    font_size: style.font_size(),
                    font_weight: style.font_weight(),
                    font_slant: style.font_slant(),
                    color: style.color(),
                    font_family: style.font_family_for_character(character),
                },
            )?;
        }
        self.shape_text_run(request)
    }

    /// Optionally supplies native cluster geometry for a logical style run.
    /// Backends without run shaping retain the per-character metric fallback.
    fn shape_text_run(
        &mut self,
        _request: TextRunShapeRequest<'_>,
    ) -> Result<Option<ShapedTextRun>, ShapeError> {
        Ok(None)
    }

    /// Optionally measures a complete candidate line at its actual start and
    /// end boundaries without creating a paint resource.
    fn measure_line(
        &mut self,
        _request: TextShapeRequest<'_>,
    ) -> Result<Option<ShapedTextRun>, ShapeError> {
        Ok(None)
    }

    /// Optionally shapes a final visual line as one platform text layout. The
    /// default keeps non-native backends on the established glyph path.
    fn shape_line(
        &mut self,
        _request: TextShapeRequest<'_>,
    ) -> Result<Option<ShapedLine>, ShapeError> {
        Ok(None)
    }

    fn begin_line_shaping(&mut self) {}
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    #[test]
    fn append_checkpoints_own_no_run_data_and_preserve_the_prefix() {
        let mut resources = GlyphResourceStore::default();
        let generation = resources.generation();
        let advances: Arc<[f32]> = [2.0, 3.0].into();
        let run = ShapedTextRun::new(advances.clone(), vec![true; 3].into()).unwrap();
        resources.register_run("ab", &run, 8.0, 2.0).unwrap();
        let owners = Arc::strong_count(&advances);
        let checkpoint = resources.checkpoint();
        assert_eq!(Arc::strong_count(&advances), owners);
        let extra = GlyphMetric::try_new('x', 1.0, 8.0, 2.0).unwrap();
        assert_eq!(resources.register(extra).unwrap(), 2);
        assert!(resources.rollback_to(checkpoint));
        assert_eq!(resources.generation(), generation);
        assert_eq!(resources.len(), 2);
        assert_eq!(resources.record_count(), 1);
        assert_eq!(resources.get(0).unwrap().ch(), 'a');
        assert!(resources.get(2).is_none());
        assert_eq!(resources.register(extra).unwrap(), 2);
    }

    #[test]
    fn invalid_append_checkpoints_leave_the_active_ledger_unchanged() {
        let mut resources = GlyphResourceStore::default();
        let other = GlyphResourceStore::default().checkpoint();
        let empty = resources.checkpoint();
        let extra = GlyphMetric::try_new('x', 1.0, 8.0, 2.0).unwrap();
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
        ShapedTextRun::new(vec![advance; len].into(), vec![true; len + 1].into()).unwrap()
    }

    #[test]
    fn scattered_ranges_and_native_queries_borrow_one_stored_run() {
        let advances: Arc<[f32]> = [1.0, 2.0, 3.0, 4.0].into();
        let boundaries: Arc<[bool]> = [true, false, true, false, true].into();
        let carets: Arc<[f32]> = [0.0, 1.0, 3.0, 6.0, 10.0].into();
        let run = ShapedTextRun::with_backend_run(
            advances.clone(),
            boundaries.clone(),
            7,
            carets.clone(),
            8.0,
        )
        .unwrap();
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
            assert_eq!(run.backend_run, 7);
            assert_eq!(run.caret_stops.as_ptr(), carets.as_ptr());
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
            3,
            carets.clone(),
            8.0,
        )
        .unwrap();
        let mut geometry = ShapedTextGeometry::with_len(6);
        geometry.push_run(&[0, 1, 2, 3, 4, 5], run, false);
        geometry.replace_range(2..4, fallback(2, 2.0), false);
        let native = geometry.authoritative_runs().collect::<Vec<_>>();
        assert_eq!(native.len(), 2);
        assert_eq!(native[0].source_range, 0..2);
        assert_eq!(native[1].source_range, 4..6);
        assert_eq!(native[1].run_offset, 4);
        assert_eq!(
            native[0].caret_stops.as_ptr(),
            native[1].caret_stops.as_ptr()
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
                0,
                carets.clone(),
                8.0,
            )
            .unwrap();
            run.apply_spacing(&[0.0, 0.0], right_to_left);
            assert_eq!(run.advances().as_ptr(), advances.as_ptr());
            assert_eq!(run.caret_stops().unwrap().as_ptr(), carets.as_ptr());
            run.apply_spacing(&[1.0, -0.5], right_to_left);
            assert_eq!(run.advances(), [3.0, 2.5]);
            assert_eq!(run.caret_stops().unwrap(), expected);
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
