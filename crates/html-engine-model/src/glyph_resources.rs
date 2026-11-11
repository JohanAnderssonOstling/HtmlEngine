//! Native glyph storage for basic renderer adapters.
use crate::{
    GlyphHandle, GlyphResourceGeneration, GlyphShaper, ShapedGlyph, ShapedRun, ShapingStyle,
    TextRunShapeRequest};

/// Native resources owned by a basic adapter. Document shaping preserves the
/// cache; explicit reset invalidates its borrowed handles. Native values need
/// not implement `Clone` or own the platform state they borrow.
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

/// Access to native resources while shaping a character.
pub struct GlyphWriter<'a, T> {
    resources: &'a mut GlyphResources<T>,
}

impl<T> GlyphWriter<'_, T> {
    /// Panics if the adapter exhausts its 32-bit native handle space.
    pub fn insert(&mut self, value: T) -> GlyphHandle {
        self.resources.insert(value)
    }

    pub fn get(&self, handle: GlyphHandle) -> Option<&T> {
        self.resources.get(handle)
    }
}

impl<T> GlyphResources<T> {
    pub fn generation(&self) -> GlyphResourceGeneration {
        self.generation
    }

    fn clear(&mut self) {
        self.values.clear();
        self.generation = GlyphResourceGeneration::default();
    }

    fn insert(&mut self, value: T) -> GlyphHandle {
        assert!(
            self.values.len() < u32::MAX as usize,
            "native glyph handle space exhausted"
        );
        let handle = GlyphHandle(self.values.len() as u32);
        self.values.push(value);
        handle
    }

    pub fn get(&self, handle: GlyphHandle) -> Option<&T> {
        self.values.get(handle.0 as usize)
    }
}

/// A basic adapter whose callback measures a character and registers its native
/// value through [`GlyphWriter`]. The callback can borrow a platform text system.
/// Its cache survives document operations; explicit reset invalidates handles.
pub struct ResourceShaper<T, F> {
    resources: GlyphResources<T>,
    shape: F,
}

impl<T, F> ResourceShaper<T, F>
where
    F: FnMut(char, &ShapingStyle<'_>, &mut GlyphWriter<'_, T>) -> ShapedGlyph,
{
    pub fn new(shape: F) -> Self {
        Self {
            resources: GlyphResources::default(),
            shape,
        }
    }

    /// Clear native values and start a fresh generation, invalidating borrowed handles.
    pub fn reset(&mut self) {
        self.resources.clear();
    }
    pub fn resources(&self) -> &GlyphResources<T> {
        &self.resources
    }
    pub fn into_resources(self) -> GlyphResources<T> {
        self.resources
    }
}

impl<T, F> GlyphShaper for ResourceShaper<T, F>
where
    F: FnMut(char, &ShapingStyle<'_>, &mut GlyphWriter<'_, T>) -> ShapedGlyph,
{
    fn resource_generation(&self) -> GlyphResourceGeneration {
        self.resources.generation()
    }
    fn shape_run(&mut self, request: TextRunShapeRequest<'_>) -> ShapedRun {
        ShapedRun::characters(request, |character, style| {
            (self.shape)(
                character,
                style,
                &mut GlyphWriter {
                    resources: &mut self.resources,
                },
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::shaping;

    use super::*;
    use crate::{FontMetrics, FontSlant, GlyphResourceStore};

    #[test]
    fn callback_borrows_native_state_and_gets_batch_shaping_and_reset_automatically() {
        let system = String::from("native text system");
        let native_system = system.as_str();
        let mut shaper = ResourceShaper::new(|character, _style, resources| {
            let handle = resources.insert((native_system, character));
            ShapedGlyph::new(
                handle,
                8.0,
                FontMetrics {
                    ascent: 12.0,
                    descent: 4.0,
                },
            )
        });
        let generation = shaper.resource_generation();
        let style = shaping::style(16.0, 400, FontSlant::Normal, None);
        let request = shaping::run("ab", style);
        let mut metrics = GlyphResourceStore::new(generation);
        let mut glyphs = [crate::GlyphId(0); 2];
        metrics
            .shape_run(&mut shaper, request, &mut glyphs)
            .unwrap();
        assert_eq!(metrics.len(), 2);
        assert_eq!(
            shaper.resources().get(GlyphHandle(0)),
            Some(&(native_system, 'a'))
        );
        assert_eq!(
            shaper.resources().get(GlyphHandle(1)),
            Some(&(native_system, 'b'))
        );
        shaper.reset();
        assert_ne!(shaper.resource_generation(), generation);
        assert!(shaper.into_resources().get(GlyphHandle(0)).is_none());
    }
}
