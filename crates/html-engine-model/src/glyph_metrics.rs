use crate::{FontMetrics, GlyphHandle, GlyphId, ShapedGlyph};
use html_dom::MemoryUsageReport;
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use std::ops::Range;
use std::sync::Arc;

/// Immutable validated layout data for one shaped glyph.
///
/// Metric fields cannot be populated without validation:
///
/// ```compile_fail
/// let _ = html_engine_model::GlyphMetric {
///     ch: 'x',
///     advance: f32::NAN,
///     metrics: Default::default(),
/// };
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlyphMetric {
    ch: char,
    advance: f32,
    metrics: FontMetrics,
}

/// Error returned when glyph metric values violate boundary invariants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlyphMetricError {
    NonFiniteValue { field: &'static str, value: f32 },
    NegativeValue { field: &'static str, value: f32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlyphMetricsError {
    CapacityExceeded,
}

impl GlyphMetric {
    pub fn try_new(
        ch: char,
        advance: f32,
        ascent: f32,
        descent: f32,
    ) -> Result<Self, GlyphMetricError> {
        validate_metric_value_non_negative("advance", advance)?;
        let metrics = FontMetrics::new(ascent, descent)?;

        Ok(Self {
            ch,
            advance,
            metrics,
        })
    }

    pub(crate) fn from_shaped(ch: char, shaped: ShapedGlyph) -> Self {
        Self {
            ch,
            advance: shaped.advance(),
            metrics: shaped.metrics(),
        }
    }

    pub(crate) fn font_metrics(self) -> FontMetrics {
        self.metrics
    }

    pub fn ch(&self) -> char {
        self.ch
    }

    pub fn advance(&self) -> f32 {
        self.advance
    }

    pub fn ascent(&self) -> f32 {
        self.metrics.ascent()
    }

    pub fn descent(&self) -> f32 {
        self.metrics.descent()
    }
}

fn validate_metric_value(field: &'static str, value: f32) -> Result<(), GlyphMetricError> {
    if !value.is_finite() {
        return Err(GlyphMetricError::NonFiniteValue { field, value });
    }
    Ok(())
}

pub(crate) fn validate_metric_value_non_negative(
    field: &'static str,
    value: f32,
) -> Result<(), GlyphMetricError> {
    validate_metric_value(field, value)?;
    if value < 0.0 {
        return Err(GlyphMetricError::NegativeValue { field, value });
    }
    Ok(())
}

/// Dense IDs backed by compact run records. Character metrics are resolved on
/// demand from shared advances and uniform vertical metrics, rather than
/// materialized and cloned for every character in a document.
///
/// Raw run registration is internal. Public callers register validated runs
/// through [`crate::GlyphResourceStore`].
///
/// ```compile_fail,E0624
/// use html_engine_model::{GlyphMetric, GlyphMetrics};
/// let mut metrics = GlyphMetrics::default();
/// let _ = metrics.register_run(vec!['x'].into(), vec![1.0].into(), GlyphMetric::default());
/// ```
#[derive(Debug, Default)]
pub struct GlyphMetrics {
    records: Vec<MetricRecord>,
    len: usize,
    paint_handles: FxHashMap<GlyphId, GlyphHandle>,
}

#[derive(Debug)]
struct MetricRecord {
    start: GlyphId,
    end: GlyphId,
    data: MetricData,
}

#[derive(Debug)]
enum MetricData {
    Singles(Vec<GlyphMetric>),
    Run {
        characters: Box<[char]>,
        advances: Arc<[f32]>,
        vertical: GlyphMetric,
    },
}

impl GlyphMetrics {
    /// Resolve a layout glyph to its adapter-owned native paint handle.
    pub fn paint_handle(&self, glyph: GlyphId) -> Option<GlyphHandle> {
        ((glyph as usize) < self.len)
            .then(|| self.paint_handles.get(&glyph).copied().unwrap_or(glyph))
    }
    pub(crate) fn set_paint_handle(&mut self, glyph: GlyphId, handle: GlyphHandle) {
        if glyph != handle {
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
        let id = self.len as GlyphId;
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
        Ok(id)
    }

    pub(crate) fn register_run(
        &mut self,
        characters: Box<[char]>,
        advances: Arc<[f32]>,
        vertical: GlyphMetric,
    ) -> Result<Range<GlyphId>, GlyphMetricsError> {
        let end = self
            .len
            .checked_add(characters.len())
            .filter(|end| *end <= u32::MAX as usize)
            .ok_or(GlyphMetricsError::CapacityExceeded)?;
        let range = self.len as GlyphId..end as GlyphId;
        if !characters.is_empty() {
            debug_assert_eq!(characters.len(), advances.len());
            self.records.push(MetricRecord {
                start: range.start,
                end: range.end,
                data: MetricData::Run {
                    characters,
                    advances,
                    vertical,
                },
            });
            self.len = end;
        }
        Ok(range)
    }

    #[inline]
    pub fn get_checked(&self, glyph: GlyphId) -> Option<GlyphMetric> {
        if glyph as usize >= self.len {
            return None;
        }
        let record =
            &self.records[self.records.partition_point(|record| record.start <= glyph) - 1];
        Some(record.metric(glyph))
    }

    /// A caller-local cursor lets sequential layout reuse its current run.
    #[inline]
    pub fn get_with_hint(&self, glyph: GlyphId, hint: &mut usize) -> Option<GlyphMetric> {
        if glyph as usize >= self.len {
            return None;
        }
        if let Some(record) = self.records.get(*hint)
            && glyph >= record.start
            && glyph < record.end
        {
            return Some(record.metric(glyph));
        }
        if let Some(record) = self.records.get(hint.saturating_add(1))
            && glyph >= record.start
            && glyph < record.end
        {
            *hint += 1;
            return Some(record.metric(glyph));
        }
        *hint = self.records.partition_point(|record| record.start <= glyph) - 1;
        Some(self.records[*hint].metric(glyph))
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
            record.end = len as GlyphId;
            if let MetricData::Singles(metrics) = &mut record.data {
                metrics.truncate(len - record.start as usize);
            }
        }
        self.paint_handles.retain(|id, _| (*id as usize) < len);
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
        report.add(
            "GlyphMetrics.paint_handles",
            self.paint_handles.capacity() * std::mem::size_of::<(GlyphId, GlyphHandle)>(),
            self.paint_handles.len(),
        );
        report.add_slice_storage::<MetricRecord>(
            "GlyphMetrics.records",
            self.records.capacity(),
            self.records.len(),
        );
        for record in &self.records {
            match &record.data {
                MetricData::Singles(metrics) => {
                    if seen.insert(metrics.as_ptr().cast()) {
                        report.add_slice_storage::<GlyphMetric>(
                            "GlyphMetrics.singles",
                            metrics.capacity(),
                            metrics.len(),
                        );
                    }
                }
                MetricData::Run {
                    characters,
                    advances,
                    ..
                } => {
                    if seen.insert(characters.as_ptr().cast()) {
                        report.add_slice_storage::<char>(
                            "GlyphMetrics.run.characters",
                            characters.len(),
                            characters.len(),
                        );
                    }
                    if seen.insert(advances.as_ptr().cast()) {
                        report.add_slice_storage::<f32>(
                            "GlyphMetrics.run.advances.shared",
                            advances.len(),
                            advances.len(),
                        );
                    }
                }
            }
        }
        report
    }
}

impl MetricRecord {
    #[inline]
    fn metric(&self, glyph: GlyphId) -> GlyphMetric {
        let offset = (glyph - self.start) as usize;
        match &self.data {
            MetricData::Singles(metrics) => metrics[offset],
            MetricData::Run {
                characters,
                advances,
                vertical,
            } => GlyphMetric {
                ch: characters[offset],
                advance: advances[offset],
                ..*vertical
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metric(ch: char) -> GlyphMetric {
        GlyphMetric::try_new(ch, 2.0, 8.0, 2.0).unwrap()
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
        assert_eq!(metrics.register(metric('x')).unwrap(), 2);
        assert_eq!(metrics.get(0).ch(), 'a');
        assert_eq!(metrics.get(1).ch(), 'b');
        assert_eq!(metrics.get(2).ch(), 'x');
        assert_eq!(metrics.len(), 3);
        assert!(metrics.get_checked(3).is_none());
    }

    #[test]
    fn truncation_handles_run_records_and_releases_discarded_arrays() {
        let mut metrics = GlyphMetrics::default();
        let advances: Arc<[f32]> = [2.0, 3.0].into();
        metrics.register(metric('a')).unwrap();
        metrics
            .register_run(['b', 'c'].into(), advances.clone(), metric('\0'))
            .unwrap();
        metrics.register(metric('d')).unwrap();
        metrics.register(metric('e')).unwrap();
        assert_eq!(Arc::strong_count(&advances), 2);
        metrics.truncate(4);
        assert_eq!(metrics.register(metric('x')).unwrap(), 4);
        assert_eq!(metrics.get(3).ch(), 'd');
        assert_eq!(metrics.get(4).ch(), 'x');
        metrics.truncate(1);
        assert_eq!(metrics.record_count(), 1);
        assert_eq!(Arc::strong_count(&advances), 1);
        assert_eq!(metrics.register(metric('z')).unwrap(), 1);
        assert_eq!(metrics.get(1).ch(), 'z');
        metrics.truncate(0);
        assert_eq!(metrics.record_count(), 0);
        assert_eq!(metrics.register(metric('a')).unwrap(), 0);
    }

    #[test]
    fn repeated_runs_count_shared_advances_once() {
        let mut metrics = GlyphMetrics::default();
        let advances: Arc<[f32]> = Arc::from([2.0, 3.0]);
        for _ in 0..2 {
            metrics
                .register_run(
                    Box::from(['a', 'b']),
                    advances.clone(),
                    GlyphMetric::default(),
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

    #[test]
    fn glyph_metric_rejects_negative_advance() {
        assert!(GlyphMetric::try_new('x', -1.0, 0.0, 0.0).is_err());
    }

    #[test]
    fn glyph_metric_rejects_negative_ascent() {
        assert!(GlyphMetric::try_new('x', 1.0, -0.25, 0.0).is_err());
    }

    #[test]
    fn glyph_metric_rejects_negative_descent() {
        assert!(GlyphMetric::try_new('x', 1.0, 0.0, -0.25).is_err());
    }

    #[test]
    fn glyph_metric_rejects_nan_advance() {
        assert!(GlyphMetric::try_new('x', f32::NAN, 0.0, 0.0).is_err());
    }

    #[test]
    fn glyph_metric_rejects_nan_ascent() {
        assert!(GlyphMetric::try_new('x', 1.0, f32::NAN, 0.0).is_err());
    }

    #[test]
    fn glyph_metric_rejects_nan_descent() {
        assert!(GlyphMetric::try_new('x', 1.0, 0.0, f32::NAN).is_err());
    }
}
