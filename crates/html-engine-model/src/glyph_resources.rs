//! Native glyph storage and lifecycle handling for basic renderer adapters.
use crate::{
    GlyphHandle, GlyphRequest, GlyphResourceGeneration, GlyphShaper, ShapeError, ShapedGlyph,
};
use std::mem::{replace, take};

/// Native paint resources owned in one place and borrowed by painters.
///
/// Document replacement moves the previous buffer aside until commit or
/// rollback. Append transactions remember only the original length. Neither
/// operation requires native resources to implement `Clone`.
pub struct GlyphResources<T> {
    values: Vec<T>,
    generation: GlyphResourceGeneration,
    previous_document: Option<PreviousDocument<T>>,
    append_checkpoint: Option<usize>,
}

struct PreviousDocument<T> {
    values: Vec<T>,
    generation: GlyphResourceGeneration,
}

impl<T> Default for GlyphResources<T> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            generation: GlyphResourceGeneration::default(),
            previous_document: None,
            append_checkpoint: None,
        }
    }
}

impl<T> GlyphResources<T> {
    pub fn generation(&self) -> GlyphResourceGeneration {
        self.generation
    }

    /// Discard the active resources and invalidate their handles.
    ///
    /// Panics if a shaping transaction is still active. Finish or roll back
    /// that transaction before resetting its resources.
    pub fn clear(&mut self) {
        assert!(
            self.previous_document.is_none() && self.append_checkpoint.is_none(),
            "cannot clear native resources during shaping"
        );
        self.values.clear();
        self.generation = GlyphResourceGeneration::default();
    }

    pub fn insert(&mut self, value: T) -> Result<GlyphHandle, ShapeError> {
        let handle = GlyphHandle::try_from(self.values.len())
            .ok()
            .filter(|handle| *handle < GlyphHandle::MAX)
            .ok_or_else(|| ShapeError::registry_capacity_exhausted(self.values.len()))?;
        self.values.push(value);
        Ok(handle)
    }

    pub fn get(&self, handle: GlyphHandle) -> Option<&T> {
        self.values.get(handle as usize)
    }

    /// Start a replacement while retaining the previous resources by moving them.
    pub fn begin_document_shaping(&mut self) {
        assert!(
            self.previous_document.is_none() && self.append_checkpoint.is_none(),
            "cannot replace native resources during another shaping transaction"
        );
        self.previous_document = Some(PreviousDocument {
            values: take(&mut self.values),
            generation: replace(&mut self.generation, GlyphResourceGeneration::default()),
        });
    }

    pub fn commit_document_shaping(&mut self) {
        assert!(
            self.append_checkpoint.is_none(),
            "finish append shaping before committing a document"
        );
        self.previous_document = None;
    }

    pub fn rollback_document_shaping(&mut self) {
        if let Some(previous) = self.previous_document.take() {
            self.append_checkpoint = None;
            self.values = previous.values;
            self.generation = previous.generation;
        }
    }

    /// Appends may be nested inside a document replacement, but not each other.
    pub fn begin_append_shaping(&mut self) {
        assert!(
            self.append_checkpoint.is_none(),
            "append shaping transactions cannot be nested"
        );
        self.append_checkpoint = Some(self.values.len());
    }

    pub fn commit_append_shaping(&mut self) {
        self.append_checkpoint = None;
    }

    pub fn rollback_append_shaping(&mut self) {
        if let Some(len) = self.append_checkpoint.take() {
            self.values.truncate(len);
        }
    }
}

/// A shaper whose native resource lifecycle is handled automatically.
///
/// Supply a callback that measures a character, stores its native paint resource
/// in the provided [`GlyphResources`], and returns its [`ShapedGlyph`]. The
/// callback may borrow a platform text system; it need not own or clone one.
/// Keep handle-indexed state in the supplied resources so rollback restores it.
/// Backends with additional document or run registries can implement
/// [`GlyphShaper`] directly instead.
pub struct ResourceShaper<T, F> {
    resources: GlyphResources<T>,
    shape: F,
}

impl<T, F> ResourceShaper<T, F>
where
    F: FnMut(GlyphRequest<'_>, &mut GlyphResources<T>) -> Result<ShapedGlyph, ShapeError>,
{
    pub fn new(shape: F) -> Self {
        Self {
            resources: GlyphResources::default(),
            shape,
        }
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
    F: FnMut(GlyphRequest<'_>, &mut GlyphResources<T>) -> Result<ShapedGlyph, ShapeError>,
{
    fn resource_generation(&self) -> GlyphResourceGeneration {
        self.resources.generation()
    }

    fn reset(&mut self) {
        self.resources.clear();
    }

    fn begin_document_shaping(&mut self) {
        self.resources.begin_document_shaping();
    }

    fn commit_document_shaping(&mut self) {
        self.resources.commit_document_shaping();
    }

    fn rollback_document_shaping(&mut self) {
        self.resources.rollback_document_shaping();
    }

    fn begin_append_shaping(&mut self) -> Result<(), ShapeError> {
        self.resources.begin_append_shaping();
        Ok(())
    }

    fn commit_append_shaping(&mut self) {
        self.resources.commit_append_shaping();
    }

    fn rollback_append_shaping(&mut self) {
        self.resources.rollback_append_shaping();
    }

    fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
        (self.shape)(request, &mut self.resources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontMetrics, FontSlant, GlyphResourceStore, TextRunShapeRequest, TextStyleSpan};
    use std::cell::Cell;

    struct Native<'a> {
        character: char,
        drops: &'a Cell<usize>,
    }

    impl Drop for Native<'_> {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    #[test]
    fn document_rollback_restores_the_buffer_and_generation_without_cloning() {
        let old_drops = Cell::new(0);
        let new_drops = Cell::new(0);
        let mut resources = GlyphResources::default();
        let old = resources
            .insert(Native {
                character: 'x',
                drops: &old_drops,
            })
            .unwrap();
        let generation = resources.generation();
        resources.begin_document_shaping();
        assert_ne!(resources.generation(), generation);
        assert_eq!(old_drops.get(), 0);
        resources
            .insert(Native {
                character: 'y',
                drops: &new_drops,
            })
            .unwrap();
        resources.begin_append_shaping();
        resources
            .insert(Native {
                character: 'z',
                drops: &new_drops,
            })
            .unwrap();
        resources.rollback_document_shaping();
        assert_eq!(resources.generation(), generation);
        assert_eq!(resources.get(old).unwrap().character, 'x');
        assert!(resources.get(1).is_none());
        assert_eq!(new_drops.get(), 2);
        assert_eq!(old_drops.get(), 0);
        // The abandoned append checkpoint must not affect the restored document.
        resources.begin_append_shaping();
        resources.rollback_append_shaping();
        resources.clear();
        assert_eq!(old_drops.get(), 1);
    }

    #[test]
    fn appends_truncate_only_uncommitted_resources_inside_and_outside_replacement() {
        let drops = Cell::new(0);
        let mut resources = GlyphResources::default();
        resources
            .insert(Native {
                character: 'x',
                drops: &drops,
            })
            .unwrap();
        resources.begin_document_shaping();
        let generation = resources.generation();
        resources
            .insert(Native {
                character: 'y',
                drops: &drops,
            })
            .unwrap();
        resources.begin_append_shaping();
        resources
            .insert(Native {
                character: 'z',
                drops: &drops,
            })
            .unwrap();
        resources.rollback_append_shaping();
        assert_eq!(resources.get(0).unwrap().character, 'y');
        assert!(resources.get(1).is_none());
        assert_eq!(drops.get(), 1);
        resources.begin_append_shaping();
        assert_eq!(
            resources
                .insert(Native {
                    character: 'a',
                    drops: &drops
                })
                .unwrap(),
            1
        );
        resources.commit_append_shaping();
        resources.commit_document_shaping();
        assert_eq!(drops.get(), 2);
        resources.begin_append_shaping();
        resources
            .insert(Native {
                character: 'b',
                drops: &drops,
            })
            .unwrap();
        resources.rollback_append_shaping();
        assert_eq!(resources.generation(), generation);
        assert_eq!(resources.get(1).unwrap().character, 'a');
        assert!(resources.get(2).is_none());
        assert_eq!(drops.get(), 3);
    }

    #[test]
    fn callback_borrows_native_state_and_gets_batch_shaping_and_reset_automatically() {
        let system = String::from("native text system");
        let native_system = system.as_str();
        let mut shaper = ResourceShaper::new(|request, resources| {
            let handle = resources.insert((native_system, request.character))?;
            Ok(ShapedGlyph::new(handle, 8.0, FontMetrics::new(12.0, 4.0)?)?)
        });
        let generation = shaper.resource_generation();
        let style = TextStyleSpan::new(0..2, 16.0, 400, FontSlant::Normal, 0, None).unwrap();
        let request = TextRunShapeRequest::new("ab", style).unwrap();
        let mut metrics = GlyphResourceStore::new(generation);
        let mut glyphs = [0; 2];
        shaper
            .shape_glyph_run(request, &mut glyphs, &mut metrics)
            .unwrap();
        assert_eq!(metrics.len(), 2);
        assert_eq!(shaper.resources().get(0), Some(&(native_system, 'a')));
        assert_eq!(shaper.resources().get(1), Some(&(native_system, 'b')));
        shaper.reset();
        assert_ne!(shaper.resource_generation(), generation);
        assert!(shaper.into_resources().get(0).is_none());
    }
}
