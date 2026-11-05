use crate::{FontMetrics, GlyphHandle, GlyphId, RunFontMetrics, ShapedGlyph};
use html_dom::MemoryUsageReport;
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use std::ops::Range;
use std::sync::Arc;

/// Immutable layout data for one shaped glyph.
///
/// Metric fields and construction are private to the engine:
///
/// ```compile_fail
/// let _ = html_engine_model::GlyphMetric {
///     ch: 'x',
///     advance: f32::NAN,
///     metrics: Default::default(),
/// };
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphMetric {
    ch: char,
    advance: f32,
    metrics: FontMetrics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlyphMetricsError {
    CapacityExceeded,
}

impl GlyphMetric {
    /// Store adapter-supplied measurements without validating their values.
    pub(crate) fn new(ch: char, advance: f32, ascent: f32, descent: f32) -> Self {
        Self {
            ch,
            advance,
            metrics: FontMetrics { ascent, descent },
        }
    }

    pub(crate) fn from_shaped(ch: char, shaped: ShapedGlyph) -> Self {
        Self {
            ch,
            advance: shaped.advance(),
            metrics: shaped.metrics(),
        }
    }

    pub fn ch(&self) -> char {
        self.ch
    }

    pub fn advance(&self) -> f32 {
        self.advance
    }

    pub fn ascent(&self) -> f32 {
        self.metrics.ascent
    }

    pub fn descent(&self) -> f32 {
        self.metrics.descent
    }
}

/// Dense IDs backed by compact run records. Character metrics are resolved on
/// demand from shared advances and vertical metrics, rather than materialized
/// and cloned for every character in a document.
///
/// Raw run registration is internal. [`crate::GlyphResourceStore`] owns run
/// registration and assigns layout IDs.
///
/// ```compile_fail,E0624
/// use html_engine_model::{FontMetrics, GlyphMetrics, RunFontMetrics};
/// let mut metrics = GlyphMetrics::default();
/// let _ = metrics.register_run(
///     "x", vec![1.0].into(), RunFontMetrics::Uniform(FontMetrics::default()), 0,
/// );
/// ```
#[derive(Debug, Default)]
pub struct GlyphMetrics {
    records: Vec<MetricRecord>,
    len: usize,
    paint_handles: FxHashMap<GlyphId, GlyphHandle>,
    // Preserve identity lookup before the first run with independent handles.
    // This boundary also lets truncation discard the slow path in constant time.
    first_offset_run: Option<u32>,
}

#[derive(Debug)]
struct MetricRecord {
    start: u32,
    end: u32,
    data: MetricData,
}

#[derive(Debug)]
enum MetricData {
    Singles(Vec<GlyphMetric>),
    AsciiRun(RunMetrics<u8>),
    BmpRun(RunMetrics<u16>),
    UnicodeRun(RunMetrics<char>),
}

#[derive(Debug)]
struct RunMetrics<T> {
    characters: Box<[T]>,
    advances: Arc<[f32]>,
    vertical: RunFontMetrics,
    first_paint_handle: GlyphHandle,
}

/// A traversal-local view of glyph advances and vertical metrics. Source
/// characters are deliberately omitted: layout already has its logical text.
pub struct GlyphDimensionsCursor<'a> {
    records: &'a [MetricRecord],
    hint: usize,
    current: MetricDimensions<'a>,
}

enum MetricDimensions<'a> {
    Singles {
        start: u32,
        values: &'a [GlyphMetric],
    },
    Run {
        start: u32,
        advances: &'a [f32],
        vertical: FontMetrics,
    },
    MixedRun {
        start: u32,
        advances: &'a [f32],
        vertical: &'a [FontMetrics],
    },
}

impl MetricDimensions<'_> {
    #[inline]
    fn get(&self, glyph: GlyphId) -> Option<(f32, FontMetrics)> {
        let glyph = glyph.0;
        match self {
            Self::Singles { start, values } => {
                let offset = glyph.checked_sub(*start)? as usize;
                values
                    .get(offset)
                    .map(|metric| (metric.advance, metric.metrics))
            }
            Self::Run {
                start,
                advances,
                vertical,
            } => {
                let offset = glyph.checked_sub(*start)? as usize;
                advances.get(offset).map(|&advance| (advance, *vertical))
            }
            Self::MixedRun {
                start,
                advances,
                vertical,
            } => {
                let offset = glyph.checked_sub(*start)? as usize;
                advances
                    .get(offset)
                    .zip(vertical.get(offset))
                    .map(|(&advance, &metrics)| (advance, metrics))
            }
        }
    }
}

impl GlyphDimensionsCursor<'_> {
    #[inline]
    pub fn get(&mut self, glyph: GlyphId) -> Option<(f32, FontMetrics)> {
        if let Some(dimensions) = self.current.get(glyph) {
            return Some(dimensions);
        }
        self.select_record(glyph)
    }

    fn select_record(&mut self, glyph: GlyphId) -> Option<(f32, FontMetrics)> {
        let next = self.hint.saturating_add(1);
        let index = if self
            .records
            .get(next)
            .is_some_and(|record| record.start <= glyph.0 && glyph.0 < record.end)
        {
            next
        } else {
            self.records
                .partition_point(|record| record.start <= glyph.0)
                .checked_sub(1)?
        };
        let record = &self.records[index];
        if glyph.0 >= record.end {
            return None;
        }
        self.hint = index;
        self.current = record.dimensions();
        self.current.get(glyph)
    }
}

impl<T> RunMetrics<T> {
    #[inline]
    fn metric(&self, offset: usize, ch: char) -> GlyphMetric {
        GlyphMetric {
            ch,
            advance: self.advances[offset],
            metrics: match &self.vertical {
                RunFontMetrics::Uniform(metrics) => *metrics,
                RunFontMetrics::PerCharacter(metrics) => metrics[offset],
            },
        }
    }

    fn dimensions(&self, start: u32, len: usize) -> MetricDimensions<'_> {
        let advances = &self.advances[..len];
        match &self.vertical {
            RunFontMetrics::Uniform(vertical) => MetricDimensions::Run {
                start,
                advances,
                vertical: *vertical,
            },
            RunFontMetrics::PerCharacter(vertical) => MetricDimensions::MixedRun {
                start,
                advances,
                vertical: &vertical[..len],
            },
        }
    }

    fn report_memory(&self, report: &mut MemoryUsageReport, seen: &mut FxHashSet<*const ()>) {
        if seen.insert(self.characters.as_ptr().cast()) {
            report.add_slice("GlyphMetrics.run.characters", &self.characters);
        }
        if seen.insert(self.advances.as_ptr().cast()) {
            report.add_slice("GlyphMetrics.run.advances.shared", &self.advances);
        }
        if let RunFontMetrics::PerCharacter(vertical) = &self.vertical
            && seen.insert(vertical.as_ptr().cast())
        {
            report.add_slice("GlyphMetrics.run.vertical.shared", vertical);
        }
    }
}

impl GlyphMetrics {
    pub fn dimensions_cursor(&self) -> GlyphDimensionsCursor<'_> {
        GlyphDimensionsCursor {
            records: &self.records,
            hint: 0,
            current: self.records.first().map_or(
                MetricDimensions::Singles {
                    start: 0,
                    values: &[],
                },
                MetricRecord::dimensions,
            ),
        }
    }

    /// Resolve a layout glyph to its adapter-owned native paint handle.
    pub fn paint_handle(&self, glyph: GlyphId) -> Option<GlyphHandle> {
        if glyph.0 as usize >= self.len {
            return None;
        }
        if self.first_offset_run.is_none_or(|start| glyph.0 < start) {
            return Some(
                self.paint_handles
                    .get(&glyph)
                    .copied()
                    .unwrap_or(GlyphHandle(glyph.0)),
            );
        }
        let record = &self.records[self
            .records
            .partition_point(|record| record.start <= glyph.0)
            - 1];
        let first_handle = match &record.data {
            MetricData::Singles(_) => {
                return Some(
                    self.paint_handles
                        .get(&glyph)
                        .copied()
                        .unwrap_or(GlyphHandle(glyph.0)),
                );
            }
            MetricData::AsciiRun(run) => run.first_paint_handle,
            MetricData::BmpRun(run) => run.first_paint_handle,
            MetricData::UnicodeRun(run) => run.first_paint_handle,
        };
        Some(GlyphHandle(first_handle.0 + (glyph.0 - record.start)))
    }
    pub(crate) fn set_paint_handle(&mut self, glyph: GlyphId, handle: GlyphHandle) {
        if glyph.0 != handle.0 {
            self.paint_handles.insert(glyph, handle);
        } else {
            self.paint_handles.remove(&glyph);
        }
    }

    #[inline]
    pub fn get(&self, glyph: GlyphId) -> GlyphMetric {
        self.get_checked(glyph).expect("registered glyph metric")
    }

    pub fn register(&mut self, metric: GlyphMetric) -> Result<GlyphId, GlyphMetricsError> {
        if self.len == u32::MAX as usize {
            return Err(GlyphMetricsError::CapacityExceeded);
        }
        let id = self.len as u32;
        if let Some(record) = self.records.last_mut()
            && let MetricData::Singles(metrics) = &mut record.data
        {
            metrics.push(metric);
            record.end += 1;
        } else {
            self.records.push(MetricRecord {
                start: id,
                end: id + 1,
                data: MetricData::Singles(vec![metric]),
            });
        }
        self.len += 1;
        Ok(GlyphId(id))
    }

    pub(crate) fn register_run(
        &mut self,
        text: &str,
        advances: Arc<[f32]>,
        vertical: RunFontMetrics,
        first_paint_handle: GlyphHandle,
    ) -> Result<Range<u32>, GlyphMetricsError> {
        // The caller validates the scalar count against these advances.
        // Keep scalar indexing for every encoding, including supplementary
        // characters, rather than indexing UTF-16 code units.
        let count = advances.len();
        debug_assert_eq!(text.chars().count(), count);
        debug_assert!(match &vertical {
            RunFontMetrics::Uniform(_) => true,
            RunFontMetrics::PerCharacter(values) => values.len() == count,
        });
        debug_assert!(u64::from(first_paint_handle.0) + count as u64 <= u64::from(u32::MAX));
        let end = self
            .len
            .checked_add(count)
            .filter(|end| *end <= u32::MAX as usize)
            .ok_or(GlyphMetricsError::CapacityExceeded)?;
        let range = self.len as u32..end as u32;
        if count != 0 {
            let data = if text.is_ascii() {
                MetricData::AsciiRun(RunMetrics {
                    characters: text.as_bytes().into(),
                    advances,
                    vertical,
                    first_paint_handle,
                })
            } else if text.chars().all(|ch| (ch as u32) <= u16::MAX as u32) {
                let mut characters = Vec::with_capacity(count);
                characters.extend(text.chars().map(|ch| ch as u16));
                MetricData::BmpRun(RunMetrics {
                    characters: characters.into_boxed_slice(),
                    advances,
                    vertical,
                    first_paint_handle,
                })
            } else {
                let mut characters = Vec::with_capacity(count);
                characters.extend(text.chars());
                MetricData::UnicodeRun(RunMetrics {
                    characters: characters.into_boxed_slice(),
                    advances,
                    vertical,
                    first_paint_handle,
                })
            };
            if first_paint_handle.0 != range.start && self.first_offset_run.is_none() {
                self.first_offset_run = Some(range.start);
            }
            self.records.push(MetricRecord {
                start: range.start,
                end: range.end,
                data,
            });
            self.len = end;
        }
        Ok(range)
    }

    #[inline]
    pub fn get_checked(&self, glyph: GlyphId) -> Option<GlyphMetric> {
        if glyph.0 as usize >= self.len {
            return None;
        }
        let record = &self.records[self
            .records
            .partition_point(|record| record.start <= glyph.0)
            - 1];
        Some(record.metric(glyph.0))
    }

    /// A caller-local cursor lets sequential layout reuse its current run.
    #[inline]
    pub fn get_with_hint(&self, glyph: GlyphId, hint: &mut usize) -> Option<GlyphMetric> {
        if glyph.0 as usize >= self.len {
            return None;
        }
        if let Some(record) = self.records.get(*hint)
            && glyph.0 >= record.start
            && glyph.0 < record.end
        {
            return Some(record.metric(glyph.0));
        }
        if let Some(record) = self.records.get(hint.saturating_add(1))
            && glyph.0 >= record.start
            && glyph.0 < record.end
        {
            *hint += 1;
            return Some(record.metric(glyph.0));
        }
        *hint = self
            .records
            .partition_point(|record| record.start <= glyph.0)
            - 1;
        Some(self.records[*hint].metric(glyph.0))
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        assert!(len <= self.len);
        self.records.truncate(
            self.records
                .partition_point(|record| (record.start as usize) < len),
        );
        if let Some(record) = self.records.last_mut() {
            record.end = len as u32;
            if let MetricData::Singles(metrics) = &mut record.data {
                metrics.truncate(len - record.start as usize);
            }
        }
        self.paint_handles.retain(|id, _| (id.0 as usize) < len);
        if self
            .first_offset_run
            .is_some_and(|start| start as usize >= len)
        {
            self.first_offset_run = None;
        }
        self.len = len;
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        self.memory_usage_report_with_shared(&mut FxHashSet::default())
    }

    pub fn memory_usage_report_with_shared(
        &self,
        seen: &mut FxHashSet<*const ()>,
    ) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_hash_map_storage("GlyphMetrics.paint_handles", &self.paint_handles);
        report.add_vec_storage("GlyphMetrics.records", &self.records);
        for record in &self.records {
            match &record.data {
                MetricData::Singles(metrics) => {
                    if seen.insert(metrics.as_ptr().cast()) {
                        report.add_vec_storage("GlyphMetrics.singles", metrics);
                    }
                }
                MetricData::AsciiRun(run) => run.report_memory(&mut report, seen),
                MetricData::BmpRun(run) => run.report_memory(&mut report, seen),
                MetricData::UnicodeRun(run) => run.report_memory(&mut report, seen),
            }
        }
        report
    }
}

impl MetricRecord {
    fn dimensions(&self) -> MetricDimensions<'_> {
        let len = (self.end - self.start) as usize;
        match &self.data {
            MetricData::Singles(values) => MetricDimensions::Singles {
                start: self.start,
                values,
            },
            MetricData::AsciiRun(run) => run.dimensions(self.start, len),
            MetricData::BmpRun(run) => run.dimensions(self.start, len),
            MetricData::UnicodeRun(run) => run.dimensions(self.start, len),
        }
    }

    #[inline]
    fn metric(&self, glyph: u32) -> GlyphMetric {
        let offset = (glyph - self.start) as usize;
        match &self.data {
            MetricData::Singles(metrics) => metrics[offset],
            MetricData::AsciiRun(run) => run.metric(offset, char::from(run.characters[offset])),
            MetricData::BmpRun(run) => run.metric(
                offset,
                char::from_u32(u32::from(run.characters[offset]))
                    .expect("BMP run contains validated source scalars"),
            ),
            MetricData::UnicodeRun(run) => run.metric(offset, run.characters[offset]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metric(ch: char) -> GlyphMetric {
        GlyphMetric::new(ch, 2.0, 8.0, 2.0)
    }

    #[test]
    fn truncation_keeps_the_prefix_and_reuses_dense_ids() {
        let mut metrics = GlyphMetrics::default();
        for ch in ['a', 'b', 'c', 'd'] {
            metrics.register(metric(ch)).unwrap();
        }
        let MetricData::Singles(values) = &metrics.records[0].data else {
            unreachable!()
        };
        let pointer = values.as_ptr();
        metrics.truncate(2);
        let MetricData::Singles(values) = &metrics.records[0].data else {
            unreachable!()
        };
        assert_eq!(pointer, values.as_ptr());
        assert_eq!(metrics.register(metric('x')).unwrap(), GlyphId(2));
        assert_eq!(metrics.get(GlyphId(0)).ch(), 'a');
        assert_eq!(metrics.get(GlyphId(1)).ch(), 'b');
        assert_eq!(metrics.get(GlyphId(2)).ch(), 'x');
        assert_eq!(metrics.len(), 3);
        assert!(metrics.get_checked(GlyphId(3)).is_none());
    }

    #[test]
    fn truncation_handles_run_records_and_releases_discarded_arrays() {
        let mut metrics = GlyphMetrics::default();
        let advances: Arc<[f32]> = [2.0, 3.0].into();
        metrics.register(metric('a')).unwrap();
        metrics
            .register_run(
                "bc",
                advances.clone(),
                RunFontMetrics::Uniform(FontMetrics {
                    ascent: 8.0,
                    descent: 2.0,
                }),
                GlyphHandle(1),
            )
            .unwrap();
        metrics.register(metric('d')).unwrap();
        metrics.register(metric('e')).unwrap();
        assert_eq!(Arc::strong_count(&advances), 2);
        metrics.truncate(4);
        assert_eq!(metrics.register(metric('x')).unwrap(), GlyphId(4));
        assert_eq!(metrics.get(GlyphId(3)).ch(), 'd');
        assert_eq!(metrics.get(GlyphId(4)).ch(), 'x');
        metrics.truncate(1);
        assert_eq!(metrics.record_count(), 1);
        assert_eq!(Arc::strong_count(&advances), 1);
        assert_eq!(metrics.register(metric('z')).unwrap(), GlyphId(1));
        assert_eq!(metrics.get(GlyphId(1)).ch(), 'z');
        metrics.truncate(0);
        assert_eq!(metrics.record_count(), 0);
        assert_eq!(metrics.register(metric('a')).unwrap(), GlyphId(0));
    }

    #[test]
    fn repeated_runs_count_shared_advances_once() {
        let mut metrics = GlyphMetrics::default();
        let advances: Arc<[f32]> = Arc::from([2.0, 3.0]);
        for _ in 0..2 {
            metrics
                .register_run(
                    "ab",
                    advances.clone(),
                    RunFontMetrics::Uniform(FontMetrics::default()),
                    GlyphHandle(metrics.len() as u32),
                )
                .unwrap();
        }
        let reported: usize = metrics
            .memory_usage_report()
            .entries
            .iter()
            .filter(|entry| entry.label == "GlyphMetrics.run.advances.shared")
            .map(|entry| entry.bytes)
            .sum();
        assert_eq!(reported, 2 * size_of::<f32>());
    }
}
