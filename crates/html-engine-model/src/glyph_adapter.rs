//! Small renderer-facing glyph requests and validated shaping results.
use crate::{
    FontSlant, GlyphId, GlyphMetric, GlyphMetricError, GlyphResourceGeneration, ShapeError,
};

/// Opaque key into an adapter's native paint resources. This is independent
/// of the dense glyph IDs assigned by HtmlEngine for layout.
pub type GlyphHandle = u32;

#[derive(Clone, Copy, Debug)]
pub struct GlyphRequest<'a> {
    pub character: char,
    pub font_size: f32,
    pub font_weight: u16,
    pub font_slant: FontSlant,
    pub color: u32,
    pub font_family: Option<&'a str>,
}

/// Font/run extents measured by the native text system, in layout pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FontMetrics {
    ascent: f32,
    descent: f32,
}

impl FontMetrics {
    pub fn new(ascent: f32, descent: f32) -> Result<Self, GlyphMetricError> {
        GlyphMetric::try_new('\0', 0.0, ascent, descent)?;
        Ok(Self { ascent, descent })
    }
    pub fn ascent(self) -> f32 {
        self.ascent
    }
    pub fn descent(self) -> f32 {
        self.descent
    }
}

/// A native paint handle and its measurements. Registration belongs to the
/// engine; adapters never need to assign layout glyph IDs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    handle: GlyphHandle,
    advance: f32,
    metrics: FontMetrics,
}

impl ShapedGlyph {
    pub fn new(
        handle: GlyphHandle,
        advance: f32,
        metrics: FontMetrics,
    ) -> Result<Self, GlyphMetricError> {
        GlyphMetric::try_new('\0', advance, metrics.ascent, metrics.descent)?;
        Ok(Self {
            handle,
            advance,
            metrics,
        })
    }
    pub fn from_metric(handle: GlyphHandle, metric: GlyphMetric) -> Self {
        Self {
            handle,
            advance: metric.advance(),
            metrics: FontMetrics {
                ascent: metric.ascent(),
                descent: metric.descent(),
            },
        }
    }
    pub fn handle(self) -> GlyphHandle {
        self.handle
    }
    pub fn advance(self) -> f32 {
        self.advance
    }
    pub fn metrics(self) -> FontMetrics {
        self.metrics
    }
    pub(crate) fn metric(self, character: char) -> GlyphMetric {
        GlyphMetric::try_new(
            character,
            self.advance,
            self.metrics.ascent,
            self.metrics.descent,
        )
        .expect("validated shaping result")
    }
}

/// Optional storage for simple adapters. Clearing native glyphs also changes
/// their generation, keeping invalidation bookkeeping in one place.
pub struct GlyphResources<T> {
    values: Vec<T>,
    generation: GlyphResourceGeneration,
}
impl<T> Default for GlyphResources<T> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            generation: GlyphResourceGeneration::default(),
        }
    }
}
impl<T> GlyphResources<T> {
    pub fn generation(&self) -> GlyphResourceGeneration {
        self.generation
    }
    pub fn clear(&mut self) {
        self.values.clear();
        self.generation = GlyphResourceGeneration::default();
    }
    pub fn insert(&mut self, value: T) -> Result<GlyphHandle, ShapeError> {
        let id = GlyphId::try_from(self.values.len())
            .ok()
            .filter(|id| *id < GlyphId::MAX)
            .ok_or_else(|| ShapeError::registry_capacity_exhausted(self.values.len()))?;
        self.values.push(value);
        Ok(id)
    }
    pub fn get(&self, handle: GlyphHandle) -> Option<&T> {
        self.values.get(handle as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GlyphResourceStore, GlyphShaper};

    #[test]
    fn engine_ids_are_independent_of_native_handles_and_survive_rollback() {
        let mut resources = GlyphResourceStore::default();
        let metrics = FontMetrics::new(12.0, 4.0).unwrap();
        let native = ShapedGlyph::new(700, 8.0, metrics).unwrap();
        struct Fixed(ShapedGlyph, GlyphResourceGeneration);
        impl GlyphShaper for Fixed {
            fn resource_generation(&self) -> GlyphResourceGeneration {
                self.1
            }
            fn reset(&mut self) {
                self.1 = GlyphResourceGeneration::default();
            }
            fn shape_glyph(&mut self, _: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
                Ok(self.0)
            }
        }
        let mut shaper = Fixed(native, resources.generation());
        let request = |character| GlyphRequest {
            character,
            font_size: 16.0,
            font_weight: 400,
            font_slant: FontSlant::Normal,
            color: 0,
            font_family: None,
        };
        let id = resources.shape_glyph(&mut shaper, request('x')).unwrap();
        assert_eq!(id, 0);
        assert_eq!(resources.metrics().paint_handle(id), Some(700));
        assert_eq!(
            resources.shape_glyph(&mut shaper, request('x')).unwrap(),
            id
        );
        let checkpoint = resources.checkpoint();
        shaper.0 = ShapedGlyph::new(900, 9.0, metrics).unwrap();
        assert_eq!(resources.shape_glyph(&mut shaper, request('y')).unwrap(), 1);
        assert!(resources.rollback_to(checkpoint));
        assert_eq!(resources.metrics().paint_handle(1), None);
        assert_eq!(resources.shape_glyph(&mut shaper, request('y')).unwrap(), 1);
        assert_eq!(resources.metrics().paint_handle(1), Some(900));
    }

    #[test]
    fn native_storage_reset_invalidates_its_generation() {
        let mut resources = GlyphResources::default();
        let generation = resources.generation();
        let handle = resources.insert("old").unwrap();
        assert_eq!(resources.get(handle), Some(&"old"));
        resources.clear();
        assert_ne!(resources.generation(), generation);
        assert!(resources.get(handle).is_none());
        assert_eq!(resources.insert("new").unwrap(), handle);
    }

    #[test]
    fn shaping_measurements_reject_invalid_values() {
        assert!(FontMetrics::new(f32::NAN, 2.0).is_err());
        assert!(FontMetrics::new(8.0, -1.0).is_err());
        assert!(FontMetrics::new(8.0, f32::INFINITY).is_err());
        assert!(ShapedGlyph::new(7, -1.0, FontMetrics::default()).is_err());
    }
}
