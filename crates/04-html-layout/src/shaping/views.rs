//! Borrowed views over canonical shaping and range-local first-line changes.

use super::{AuthoritativeShapedRun, ResolvedBidi, ShapedTextGeometry, TextRunId};
use crate::GlyphId;
use std::ops::{Index, Range};

pub(crate) struct GlyphPatch {
    pub(crate) source: Range<u32>,
    pub(crate) glyphs: Vec<GlyphId>,
}

/// A logical slice borrowing glyph IDs from the base shape and affected ranges.
/// Iteration and indexing never flatten or copy the backing buffers.
#[derive(Clone, Copy)]
pub struct GlyphSlice<'a> {
    base: &'a [GlyphId],
    patches: &'a [GlyphPatch],
    start: usize,
    end: usize,
}

impl<'a> GlyphSlice<'a> {
    pub(crate) fn new(base: &'a [GlyphId], patches: &'a [GlyphPatch]) -> Self {
        Self {
            base,
            patches,
            start: 0,
            end: base.len(),
        }
    }

    pub fn len(self) -> usize {
        self.end - self.start
    }
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn get(self, index: usize) -> Option<&'a GlyphId> {
        if index >= self.len() {
            return None;
        }
        let source = self.start + index;
        for patch in self.patches.iter().rev() {
            if patch.source.contains(&(source as u32)) {
                return patch.glyphs.get(source - patch.source.start as usize);
            }
        }
        self.base.get(source)
    }

    pub fn slice(self, range: Range<usize>) -> Option<Self> {
        (range.start <= range.end && range.end <= self.len()).then_some(Self {
            start: self.start + range.start,
            end: self.start + range.end,
            ..self
        })
    }

    pub fn iter(self) -> impl ExactSizeIterator<Item = &'a GlyphId> + DoubleEndedIterator + 'a {
        (0..self.len()).map(move |index| self.get(index).expect("validated glyph slice"))
    }

    /// Returns storage only when this entire view lies in one backing buffer.
    pub fn as_contiguous(self) -> Option<&'a [GlyphId]> {
        for patch in self.patches.iter().rev() {
            let start = patch.source.start as usize;
            let end = patch.source.end as usize;
            if start < self.end && self.start < end {
                return (start <= self.start && self.end <= end)
                    .then(|| &patch.glyphs[self.start - start..self.end - start]);
            }
        }
        self.base.get(self.start..self.end)
    }
}

impl Index<usize> for GlyphSlice<'_> {
    type Output = GlyphId;
    fn index(&self, index: usize) -> &GlyphId {
        self.get(index).expect("glyph index out of bounds")
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TextGeometryView<'a> {
    base: &'a ShapedTextGeometry,
    refined: Option<&'a ShapedTextGeometry>,
}

impl<'a> TextGeometryView<'a> {
    pub(crate) fn new(
        base: &'a ShapedTextGeometry,
        refined: Option<&'a ShapedTextGeometry>,
    ) -> Self {
        Self { base, refined }
    }

    pub(crate) fn bidi(self) -> &'a ResolvedBidi {
        &self.base.bidi
    }

    fn geometry_at(self, index: usize) -> &'a ShapedTextGeometry {
        self.refined
            .filter(|geometry| geometry.has_character_geometry(index))
            .unwrap_or(self.base)
    }

    pub(crate) fn advance(self, index: usize) -> Option<f32> {
        self.geometry_at(index).advance(index)
    }
    pub(crate) fn advance_with_hint(self, index: usize, hint: &mut usize) -> Option<f32> {
        self.geometry_at(index).advance_with_hint(index, hint)
    }
    #[inline]
    pub(crate) fn character_geometry_with_hint(self, index: usize, hint: &mut usize) -> (Option<f32>, bool) {
        if let Some(refined) = self.refined
            && refined.has_character_geometry(index)
        {
            return refined.character_geometry_with_hint(index, hint);
        }
        let (advance, boundary) = self.base.character_geometry_with_hint(index, hint);
        let refinement_end = !boundary && index > 0
            && self.refined.is_some_and(|refined| refined.has_character_geometry(index - 1));
        (advance, boundary || refinement_end)
    }

    pub(crate) fn is_cluster_boundary(self, index: usize) -> bool {
        self.boundary_with_hint(index, &mut 0)
    }
    pub(crate) fn boundary_with_hint(self, index: usize, hint: &mut usize) -> bool {
        if let Some(refined) = self.refined {
            if refined.has_character_geometry(index) {
                return refined.boundary_with_hint(index, hint);
            }
            // A replacement ends on a validated boundary, even if it split
            // a cluster in the base run. An adjacent replacement wins above.
            if index > 0 && refined.has_character_geometry(index - 1) {
                return true;
            }
        }
        self.base.boundary_with_hint(index, hint)
    }
    pub(crate) fn paint_glyph(self, index: u32, fallback: GlyphId) -> GlyphId {
        self.geometry_at(index as usize)
            .paint_glyph(index, fallback)
    }

    pub(crate) fn authoritative_runs(self, range: Range<u32>) -> NativeRuns<'a> {
        let base = self.base.authoritative_runs();
        let refined = self
            .refined
            .map_or(&[][..], ShapedTextGeometry::authoritative_runs);
        NativeRuns {
            geometry: self,
            base,
            refined,
            base_index: base.partition_point(|run| run.source_range.end <= range.start),
            refined_index: refined.partition_point(|run| run.source_range.end <= range.start),
            cursor: range.start,
            end: range.end,
        }
    }
}

pub(crate) struct NativeRunView<'a> {
    pub(crate) source_range: Range<u32>,
    pub(crate) run_offset: u32,
    pub(crate) backend_run: TextRunId,
    pub(crate) caret_stops: &'a [f32],
    pub(crate) ascent: f32,
    pub(crate) placement_required: bool,
}

pub(crate) struct NativeRuns<'a> {
    geometry: TextGeometryView<'a>,
    base: &'a [AuthoritativeShapedRun],
    refined: &'a [AuthoritativeShapedRun],
    base_index: usize,
    refined_index: usize,
    cursor: u32,
    end: u32,
}

impl<'a> Iterator for NativeRuns<'a> {
    type Item = NativeRunView<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        while self.cursor < self.end {
            let mask = self
                .geometry
                .refined
                .and_then(|geometry| geometry.next_geometry_range(self.cursor));
            let (use_refined, end) = match mask {
                Some(range) if range.start <= self.cursor => (true, self.end.min(range.end)),
                Some(range) => (false, self.end.min(range.start)),
                None => (false, self.end),
            };
            let (runs, index) = if use_refined {
                (self.refined, &mut self.refined_index)
            } else {
                (self.base, &mut self.base_index)
            };
            while runs
                .get(*index)
                .is_some_and(|run| run.source_range.end <= self.cursor)
            {
                *index += 1;
            }
            let Some(run) = runs.get(*index).filter(|run| run.source_range.start < end) else {
                self.cursor = end;
                continue;
            };
            let start = self.cursor.max(run.source_range.start);
            let end = end.min(run.source_range.end);
            self.cursor = end;
            return Some(NativeRunView {
                source_range: start..end,
                run_offset: run.run_offset + start - run.source_range.start,
                backend_run: run.backend_run,
                caret_stops: &run.caret_stops,
                ascent: run.ascent,
                placement_required: run.placement_required,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShapedTextRun;
    use std::sync::Arc;

    #[test]
    fn glyph_slices_borrow_base_and_overlapping_patches_without_flattening() {
        let base = [0, 1, 2, 3, 4, 5];
        let patches = [
            GlyphPatch {
                source: 1..4,
                glyphs: vec![10, 20, 30],
            },
            GlyphPatch {
                source: 3..5,
                glyphs: vec![300, 400],
            },
        ];
        let view = GlyphSlice::new(&base, &patches);
        assert_eq!(
            view.iter().copied().collect::<Vec<_>>(),
            [0, 10, 20, 300, 400, 5]
        );
        assert_eq!(
            view.iter().rev().copied().collect::<Vec<_>>(),
            [5, 400, 300, 20, 10, 0]
        );
        assert!(view.as_contiguous().is_none());
        assert!(view.slice(2..5).unwrap().as_contiguous().is_none());
        let patch = view.slice(3..5).unwrap();
        assert_eq!(
            patch.as_contiguous().unwrap().as_ptr(),
            patches[1].glyphs.as_ptr()
        );
        assert!(std::ptr::eq(view.get(0).unwrap(), &base[0]));
        assert!(std::ptr::eq(view.get(2).unwrap(), &patches[0].glyphs[1]));
        assert!(view.get(6).is_none());
        assert!(view.slice(0..7).is_none());
    }

    #[test]
    fn geometry_overlays_split_native_runs_and_preserve_replacement_boundaries() {
        let base_run = ShapedTextRun::with_backend_run(
            Arc::from([1.0; 6]),
            Arc::from([true, false, true, false, false, true, true]),
            10,
            Arc::from([0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            8.0,
        )
        .unwrap();
        let patch_run = ShapedTextRun::with_backend_run(
            Arc::from([4.0; 2]),
            Arc::from([true, false, true]),
            20,
            Arc::from([0.0, 4.0, 8.0]),
            12.0,
        )
        .unwrap();
        let mut base = ShapedTextGeometry::with_len(6);
        base.retain_native_run(&[0, 1, 2, 3, 4, 5], &base_run, true);
        base.push_run(&[0, 1, 2, 3, 4, 5], base_run);
        let mut patch = ShapedTextGeometry::with_len(6);
        patch.retain_native_run(&[2, 3], &patch_run, true);
        patch.replace_range(2..4, patch_run);
        let view = TextGeometryView::new(&base, Some(&patch));
        assert_eq!(
            (0..6)
                .map(|index| view.advance(index).unwrap())
                .collect::<Vec<_>>(),
            [1.0, 1.0, 4.0, 4.0, 1.0, 1.0]
        );
        assert!(!view.is_cluster_boundary(1));
        assert!(!view.is_cluster_boundary(3));
        assert!(
            view.is_cluster_boundary(4),
            "the replacement ends inside a base cluster"
        );
        let runs = view.authoritative_runs(1..5).collect::<Vec<_>>();
        assert_eq!(
            runs.iter()
                .map(|run| (run.source_range.clone(), run.backend_run, run.run_offset))
                .collect::<Vec<_>>(),
            [(1..2, 10, 1), (2..4, 20, 0), (4..5, 10, 4)]
        );
        assert_eq!(
            runs[0].caret_stops.as_ptr(),
            base.authoritative_runs()[0].caret_stops.as_ptr()
        );
        assert_eq!(
            runs[1].caret_stops.as_ptr(),
            patch.authoritative_runs()[0].caret_stops.as_ptr()
        );
        let mut fallback = ShapedTextGeometry::with_len(6);
        fallback.replace_range(
            2..4,
            ShapedTextRun::new(Arc::from([2.0; 2]), Arc::from([true; 3])).unwrap(),
        );
        let runs = TextGeometryView::new(&base, Some(&fallback))
            .authoritative_runs(1..5)
            .collect::<Vec<_>>();
        assert_eq!(
            runs.iter()
                .map(|run| run.source_range.clone())
                .collect::<Vec<_>>(),
            [1..2, 4..5],
            "fallback geometry masks the old native run"
        );
    }
}
