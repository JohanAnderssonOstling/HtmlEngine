//! Renderer-facing glyph requests, measurements, and native resource storage.
use crate::ShapingStyle;

/// Opaque key into an adapter's native paint resources. This is independent
/// of the dense glyph IDs assigned by HtmlEngine for layout.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GlyphHandle(pub u32);

/// One character and the resolved properties borrowed from its shaping span.
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub struct GlyphRequest<'a> {
    pub character: char,
    pub style: &'a ShapingStyle<'a>,
}

/// Font/run extents measured by the native text system, in layout pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FontMetrics {
    /// Finite, non-negative ascent in layout pixels.
    pub ascent: f32,
    /// Finite, non-negative descent in layout pixels.
    pub descent: f32,
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
    /// The adapter supplies a valid paint handle and finite, non-negative advance.
    pub fn new(handle: GlyphHandle, advance: f32, metrics: FontMetrics) -> Self {
        Self {
            handle,
            advance,
            metrics,
        }
    }
    pub(crate) fn handle(self) -> GlyphHandle {
        self.handle
    }
    pub(crate) fn advance(self) -> f32 {
        self.advance
    }
    pub(crate) fn metrics(self) -> FontMetrics {
        self.metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FontSlant, GlyphId, GlyphResourceGeneration, GlyphResourceStore, ResourceShaper,
        GlyphWriter, GlyphShaper, ShapeError, shaping,
    };

    #[test]
    fn engine_ids_are_independent_of_native_handles_and_survive_rollback() {
        let mut resources = GlyphResourceStore::default();
        let metrics = FontMetrics {
            ascent: 12.0,
            descent: 4.0,
        };
        let native = ShapedGlyph::new(GlyphHandle(700), 8.0, metrics);
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
        let style = shaping::style(16.0, 400, FontSlant::Normal, 0, None);
        let request = |character| shaping::glyph(character, &style);
        let id = resources.shape_glyph(&mut shaper, request('x')).unwrap();
        assert_eq!(id, GlyphId(0));
        assert_eq!(resources.metrics().paint_handle(id), Some(GlyphHandle(700)));
        assert_eq!(
            resources.shape_glyph(&mut shaper, request('x')).unwrap(),
            id
        );
        let checkpoint = resources.checkpoint();
        shaper.0 = ShapedGlyph::new(GlyphHandle(900), 9.0, metrics);
        assert_eq!(
            resources.shape_glyph(&mut shaper, request('y')).unwrap(),
            GlyphId(1)
        );
        assert!(resources.rollback_to(checkpoint));
        assert_eq!(resources.metrics().paint_handle(GlyphId(1)), None);
        assert_eq!(
            resources.shape_glyph(&mut shaper, request('y')).unwrap(),
            GlyphId(1)
        );
        assert_eq!(
            resources.metrics().paint_handle(GlyphId(1)),
            Some(GlyphHandle(900))
        );
    }

    #[test]
    fn native_storage_reset_invalidates_its_generation() {
        let mut shaper = ResourceShaper::new(|_, resources: &mut GlyphWriter<'_, &str>| {
            let handle = resources.insert("old")?;
            Ok(ShapedGlyph::new(handle, 1.0, FontMetrics::default()))
        });
        let style = shaping::style(16.0, 400, FontSlant::Normal, 0, None);
        let handle = shaper
            .shape_glyph(shaping::glyph('x', &style))
            .unwrap()
            .handle();
        let generation = shaper.resource_generation();
        assert_eq!(shaper.resources().get(handle), Some(&"old"));
        shaper.reset();
        assert_ne!(shaper.resource_generation(), generation);
        assert!(shaper.resources().get(handle).is_none());
        assert_eq!(
            shaper
                .shape_glyph(shaping::glyph('x', &style))
                .unwrap()
                .handle(),
            handle
        );
    }
}
