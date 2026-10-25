use crate::GlyphId;
use html_dom::MemoryUsageReport;
use rustc_data_structures::fx::FxHashSet;
use std::ops::Range;
use std::sync::Arc;

/// Immutable validated layout data for one shaped glyph.
///
/// Metric fields cannot be populated without validation:
///
/// ```compile_fail
/// let _ = html_layout::GlyphMetric {
///     ch: 'x',
///     advance: f32::NAN,
///     ascent: 1.0,
///     descent: 1.0,
///     baseline_offset: 0.0,
/// };
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlyphMetric {
    ch: char,
    advance: f32,
    ascent: f32,
    descent: f32,
    baseline_offset: f32,
}

/// Error returned when glyph metric values violate boundary invariants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlyphMetricError {
    NonFiniteValue { field: &'static str, value: f32 },
    NegativeValue { field: &'static str, value: f32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GlyphMetricsError {
    CapacityExceeded,
}

impl GlyphMetric {
    pub fn try_new(
        ch: char,
        advance: f32,
        ascent: f32,
        descent: f32,
        baseline_offset: f32,
    ) -> Result<Self, GlyphMetricError> {
        validate_metric_value_non_negative("advance", advance)?;
        validate_metric_value_non_negative("ascent", ascent)?;
        validate_metric_value_non_negative("descent", descent)?;
        validate_metric_value("baseline_offset", baseline_offset)?;

        Ok(Self {
            ch,
            advance,
            ascent,
            descent,
            baseline_offset,
        })
    }

    pub fn ch(&self) -> char {
        self.ch
    }

    pub fn advance(&self) -> f32 {
        self.advance
    }

    pub fn ascent(&self) -> f32 {
        self.ascent
    }

    pub fn descent(&self) -> f32 {
        self.descent
    }

    pub fn baseline_offset(&self) -> f32 {
        self.baseline_offset
    }
}

fn validate_metric_value(field: &'static str, value: f32) -> Result<(), GlyphMetricError> {
    if !value.is_finite() {
        return Err(GlyphMetricError::NonFiniteValue { field, value });
    }
    Ok(())
}

fn validate_metric_value_non_negative(
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
#[derive(Clone, Debug, Default)]
pub(crate) struct GlyphMetrics {
    records: Vec<MetricRecord>,
    len: usize,
}

#[derive(Clone, Debug)]
struct MetricRecord {
    start: GlyphId,
    end: GlyphId,
    data: MetricData,
}

#[derive(Clone, Debug)]
enum MetricData {
    Singles(Arc<Vec<GlyphMetric>>),
    Run {
        characters: Arc<[char]>,
        advances: Arc<[f32]>,
        vertical: GlyphMetric,
    },
}

impl GlyphMetrics {
    #[inline]
    pub(crate) fn get(&self, glyph: GlyphId) -> GlyphMetric {
        self.get_checked(glyph).expect("registered glyph metric")
    }

    pub(crate) fn register(&mut self, metric: GlyphMetric) -> Result<GlyphId, GlyphMetricsError> {
        if self.len == u32::MAX as usize {
            return Err(GlyphMetricsError::CapacityExceeded);
        }
        let id = self.len as GlyphId;
        if let Some(MetricRecord {
            data: MetricData::Singles(metrics),
            ..
        }) = self.records.last_mut()
        {
            Arc::make_mut(metrics).push(metric);
            self.records.last_mut().unwrap().end += 1;
        } else {
            self.records.push(MetricRecord {
                start: id,
                end: id + 1,
                data: MetricData::Singles(Arc::new(vec![metric])),
            });
        }
        self.len += 1;
        Ok(id)
    }

    pub(crate) fn register_run(
        &mut self,
        characters: Arc<[char]>,
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
    pub(crate) fn get_checked(&self, glyph: GlyphId) -> Option<GlyphMetric> {
        if glyph as usize >= self.len {
            return None;
        }
        let record =
            &self.records[self.records.partition_point(|record| record.start <= glyph) - 1];
        Some(record.metric(glyph))
    }

    /// A caller-local cursor lets sequential layout reuse its current run.
    #[inline]
    pub(crate) fn get_with_hint(&self, glyph: GlyphId, hint: &mut usize) -> Option<GlyphMetric> {
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
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn record_count(&self) -> usize {
        self.records.len()
    }

    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        let mut shared_advances = FxHashSet::default();
        report.add_slice_storage::<MetricRecord>(
            "GlyphMetrics.records",
            self.records.capacity(),
            self.records.len(),
        );
        for record in &self.records {
            match &record.data {
                MetricData::Singles(metrics) => report.add_slice_storage::<GlyphMetric>(
                    "GlyphMetrics.singles",
                    metrics.capacity(),
                    metrics.len(),
                ),
                MetricData::Run {
                    characters,
                    advances,
                    ..
                } => {
                    report.add_slice_storage::<char>(
                        "GlyphMetrics.run.characters",
                        characters.len(),
                        characters.len(),
                    );
                    if shared_advances.insert(advances.as_ptr()) {
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

    #[test]
    fn repeated_runs_count_shared_advances_once() {
        let mut metrics = GlyphMetrics::default();
        let advances: Arc<[f32]> = Arc::from([2.0, 3.0]);
        for _ in 0..2 {
            metrics.register_run(Arc::from(['a', 'b']), advances.clone(), GlyphMetric::default()).unwrap();
        }
        let reported: usize = metrics.memory_usage_report().entries.iter()
            .filter(|entry| entry.label == "GlyphMetrics.run.advances.shared")
            .map(|entry| entry.bytes)
            .sum();
        assert_eq!(reported, 2 * size_of::<f32>());
    }

    #[test]
    fn glyph_metric_rejects_negative_advance() {
        assert!(GlyphMetric::try_new('x', -1.0, 0.0, 0.0, 0.0).is_err());
    }

    #[test]
    fn glyph_metric_rejects_negative_ascent() {
        assert!(GlyphMetric::try_new('x', 1.0, -0.25, 0.0, 0.0).is_err());
    }

    #[test]
    fn glyph_metric_rejects_negative_descent() {
        assert!(GlyphMetric::try_new('x', 1.0, 0.0, -0.25, 0.0).is_err());
    }

    #[test]
    fn glyph_metric_rejects_nan_advance() {
        assert!(GlyphMetric::try_new('x', f32::NAN, 0.0, 0.0, 0.0).is_err());
    }

    #[test]
    fn glyph_metric_rejects_nan_ascent() {
        assert!(GlyphMetric::try_new('x', 1.0, f32::NAN, 0.0, 0.0).is_err());
    }

    #[test]
    fn glyph_metric_rejects_nan_descent() {
        assert!(GlyphMetric::try_new('x', 1.0, 0.0, f32::NAN, 0.0).is_err());
    }

    #[test]
    fn glyph_metric_rejects_nan_baseline_offset() {
        assert!(GlyphMetric::try_new('x', 1.0, 0.0, 0.0, f32::NAN).is_err());
    }

    #[test]
    fn glyph_metric_rejects_infinite_baseline_offset() {
        assert!(GlyphMetric::try_new('x', 1.0, 0.0, 0.0, f32::INFINITY).is_err());
    }
}
