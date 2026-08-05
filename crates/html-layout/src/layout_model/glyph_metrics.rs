use crate::GlyphId;
use html_dom::MemoryUsageReport;

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
    pub fn try_new(ch: char, advance: f32, ascent: f32, descent: f32, baseline_offset: f32) -> Result<Self, GlyphMetricError> {
        validate_metric_value_non_negative("advance", advance)?;
        validate_metric_value_non_negative("ascent", ascent)?;
        validate_metric_value_non_negative("descent", descent)?;
        validate_metric_value("baseline_offset", baseline_offset)?;

        Ok(Self { ch, advance, ascent, descent, baseline_offset })
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

fn validate_metric_value_non_negative(field: &'static str, value: f32) -> Result<(), GlyphMetricError> {
    validate_metric_value(field, value)?;
    if value < 0.0 {
        return Err(GlyphMetricError::NegativeValue { field, value });
    }
    Ok(())
}

/// Dense glyph metrics indexed by [`GlyphId`].
#[derive(Clone, Debug, Default)]
pub(crate) struct GlyphMetrics(Vec<GlyphMetric>);

impl GlyphMetrics {
    #[inline]
    pub(crate) fn get(&self, glyph: GlyphId) -> GlyphMetric {
        self.0[glyph as usize]
    }

    #[inline]
    pub(crate) fn register(&mut self, metric: GlyphMetric) -> Result<GlyphId, GlyphMetricsError> {
        if self.0.len() == u32::MAX as usize {
            return Err(GlyphMetricsError::CapacityExceeded);
        }
        let id = self.0.len() as GlyphId;
        self.0.push(metric);
        Ok(id)
    }

    pub(crate) fn push(&mut self, metric: GlyphMetric) -> GlyphId {
        self.register(metric).expect("glyph metrics registry capacity exhausted")
    }

    pub(crate) fn get_checked(&self, glyph: GlyphId) -> Option<GlyphMetric> {
        self.0.get(glyph as usize).copied()
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<GlyphMetric>("GlyphMetrics.storage", self.0.capacity(), self.0.len());
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
