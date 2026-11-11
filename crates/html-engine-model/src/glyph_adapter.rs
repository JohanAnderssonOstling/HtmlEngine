//! Renderer-facing paint entries, measurements, and native resource storage.
use crate::TextRunId;
use html_dom::MemoryUsageReport;
use rustc_data_structures::fx::FxHashSet;
use std::any::{Any, TypeId};
use std::fmt;
use std::mem::size_of;
use std::ops::Range;
use std::sync::Arc;

/// A reference to one caller-owned native glyph or run cache entry.
/// The backend chooses cache storage and eviction; retained layouts keep only
/// the entries they register alive. The engine never inspects native data.
#[derive(Clone)]
pub struct NativePaintResource(Arc<dyn Any + Send + Sync>);

impl NativePaintResource {
    pub fn new<T: Any + Send + Sync>(entry: Arc<T>) -> Self {
        Self(entry)
    }

    pub fn get<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }

    /// Recover the caller's original shared entry without copying native data.
    pub fn downcast<T: Any + Send + Sync>(self) -> Result<Arc<T>, Self> {
        self.0.downcast::<T>().map_err(Self)
    }

    pub(crate) fn identity(&self) -> usize {
        Arc::as_ptr(&self.0) as *const () as usize
    }
}

impl<T: Any + Send + Sync> From<Arc<T>> for NativePaintResource {
    fn from(entry: Arc<T>) -> Self {
        Self::new(entry)
    }
}

impl fmt::Debug for NativePaintResource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NativePaintResource")
            .field(&self.0.as_ref().type_id())
            .finish()
    }
}

impl PartialEq for NativePaintResource {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Compact document-local IDs mapped to references supplied by the backend.
/// This table contains registered entries, never a snapshot of a caller's cache.
#[derive(Clone, Default)]
pub struct NativePaintResources {
    table: Arc<NativePaintTable>,
}

#[derive(Clone, Default)]
struct NativePaintTable {
    entries: Vec<NativePaintEntry>,
    types: Vec<TypeId>,
}

#[derive(Clone)]
struct NativePaintEntry {
    glyphs: Range<u32>,
    resource: NativePaintResource,
}

impl NativePaintResources {
    pub(crate) fn memory_usage_report_with_shared(
        &self,
        seen: &mut FxHashSet<*const ()>,
    ) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        if seen.insert(Arc::as_ptr(&self.table).cast()) {
            report.add("NativePaintTable", size_of::<NativePaintTable>(), 1);
            report.add_vec_storage("entries", &self.table.entries);
            report.add_vec_storage("types", &self.table.types);
        }
        // Native payloads are opaque and remain the adapter's accounting responsibility.
        report
    }

    /// Check backend compatibility once per binding, using distinct payload types
    /// rather than scanning every registered glyph/run.
    pub fn contains_only(&self, types: &[TypeId]) -> bool {
        self.table.types.iter().all(|kind| types.contains(kind))
    }

    /// Resolve an individual character to its native entry and run-local index.
    #[inline]
    pub fn glyph(&self, glyph: GlyphHandle) -> Option<(&NativePaintResource, u32)> {
        let index = self
            .table
            .entries
            .partition_point(|entry| entry.glyphs.start <= glyph.0)
            .checked_sub(1)?;
        let entry = &self.table.entries[index];
        entry
            .glyphs
            .contains(&glyph.0)
            .then_some((&entry.resource, glyph.0 - entry.glyphs.start))
    }

    #[inline]
    pub fn run(&self, run: TextRunId) -> Option<&NativePaintResource> {
        self.table
            .entries
            .get(run.0 as usize)
            .map(|entry| &entry.resource)
    }

    pub(crate) fn run_for_glyph(&self, glyph: u32) -> Option<TextRunId> {
        let index = self
            .table
            .entries
            .partition_point(|entry| entry.glyphs.start <= glyph)
            .checked_sub(1)?;
        self.table.entries[index]
            .glyphs
            .contains(&glyph)
            .then_some(TextRunId(index as u32))
    }

    pub(crate) fn push(&mut self, glyphs: Range<u32>, resource: NativePaintResource) -> TextRunId {
        debug_assert!(!glyphs.is_empty());
        let table = Arc::make_mut(&mut self.table);
        let run = TextRunId(table.entries.len() as u32);
        let entry_type = resource.0.as_ref().type_id();
        if !table.types.contains(&entry_type) {
            table.types.push(entry_type);
        }
        debug_assert!(
            table
                .entries
                .last()
                .is_none_or(|entry| entry.glyphs.end <= glyphs.start)
        );
        table.entries.push(NativePaintEntry { glyphs, resource });
        run
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        if self
            .table
            .entries
            .last()
            .is_none_or(|entry| entry.glyphs.end as usize <= len)
        {
            return;
        }
        let table = Arc::make_mut(&mut self.table);
        table.entries.truncate(
            table
                .entries
                .partition_point(|entry| (entry.glyphs.start as usize) < len),
        );
        if let Some(last) = table.entries.last_mut() {
            last.glyphs.end = last.glyphs.end.min(len as u32);
        }
        table.types.retain(|kind| {
            table
                .entries
                .iter()
                .any(|entry| entry.resource.0.as_ref().type_id() == *kind)
        });
    }
}

/// A native paint key. Borrowed results use backend handles; retained results
/// receive document-local keys when the engine registers their entry references.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GlyphHandle(pub u32);

/// Font/run extents measured by the native text system, in layout pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FontMetrics {
    /// Finite, non-negative ascent in layout pixels.
    pub ascent: f32,
    /// Finite, non-negative descent in layout pixels.
    pub descent: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NativePaintSource {
    Borrowed(GlyphHandle),
    Retained(NativePaintResource),
}

/// A native paint source and its measurements. Registration belongs to the
/// engine; adapters never need to assign layout glyph IDs.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapedGlyph {
    pub(crate) source: NativePaintSource,
    advance: f32,
    metrics: FontMetrics,
}

impl ShapedGlyph {
    /// The adapter supplies a valid paint handle and finite, non-negative advance.
    pub fn new(handle: GlyphHandle, advance: f32, metrics: FontMetrics) -> Self {
        Self {
            source: NativePaintSource::Borrowed(handle),
            advance,
            metrics,
        }
    }
    /// Return a caller-owned entry. The engine assigns its document-local ID.
    pub fn retained(
        resource: impl Into<NativePaintResource>,
        advance: f32,
        metrics: FontMetrics,
    ) -> Self {
        Self {
            source: NativePaintSource::Retained(resource.into()),
            advance,
            metrics,
        }
    }

    pub(crate) fn advance(&self) -> f32 {
        self.advance
    }
    pub(crate) fn metrics(&self) -> FontMetrics {
        self.metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FontSlant, GlyphId, GlyphResourceGeneration, GlyphResourceStore, ResourceShaper,
        GlyphWriter, GlyphShaper, shaping, ShapedRun, TextRunShapeRequest};

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
            fn shape_run(&mut self, request: TextRunShapeRequest<'_>) -> ShapedRun {
                ShapedRun::characters(request, |_, _| self.0.clone())
            }
        }
        let mut shaper = Fixed(native, resources.generation());
        let style = shaping::style(16.0, 400, FontSlant::Normal, None);
        let request = |text| shaping::run(text, style);
        let id = {
            let mut run_glyphs = [GlyphId(0)];
            resources
                .shape_run(&mut shaper, request("x"), &mut run_glyphs)
                .map(|_| run_glyphs[0])
        }
        .unwrap();
        assert_eq!(id, GlyphId(0));
        assert_eq!(resources.metrics().paint_handle(id), Some(GlyphHandle(700)));
        assert_eq!(
            {
                let mut glyphs = [GlyphId(0)];
                resources
                    .shape_run(&mut shaper, request("x"), &mut glyphs)
                    .unwrap();
                glyphs[0]
            },
            id
        );
        let checkpoint = resources.len();
        shaper.0 = ShapedGlyph::new(GlyphHandle(900), 9.0, metrics);
        assert_eq!(
            {
                let mut glyphs = [GlyphId(0)];
                resources
                    .shape_run(&mut shaper, request("y"), &mut glyphs)
                    .unwrap();
                glyphs[0]
            },
            GlyphId(1)
        );
        resources.truncate(checkpoint);
        assert_eq!(resources.metrics().paint_handle(GlyphId(1)), None);
        assert_eq!(
            {
                let mut glyphs = [GlyphId(0)];
                resources
                    .shape_run(&mut shaper, request("y"), &mut glyphs)
                    .unwrap();
                glyphs[0]
            },
            GlyphId(1)
        );
        assert_eq!(
            resources.metrics().paint_handle(GlyphId(1)),
            Some(GlyphHandle(900))
        );
    }

    #[test]
    fn native_storage_reset_invalidates_its_generation() {
        let mut shaper = ResourceShaper::new(|_, _, resources: &mut GlyphWriter<'_, &str>| {
            let handle = resources.insert("old");
            ShapedGlyph::new(handle, 1.0, FontMetrics::default())
        });
        let style = shaping::style(16.0, 400, FontSlant::Normal, None);
        shaper.shape_run(shaping::run("x", style));
        let handle = GlyphHandle(0);
        let generation = shaper.resource_generation();
        assert_eq!(shaper.resources().get(handle), Some(&"old"));
        shaper.reset();
        assert_ne!(shaper.resource_generation(), generation);
        assert!(shaper.resources().get(handle).is_none());
        shaper.shape_run(shaping::run("x", style));
        assert_eq!(shaper.resources().get(handle), Some(&"old"));
    }
}
