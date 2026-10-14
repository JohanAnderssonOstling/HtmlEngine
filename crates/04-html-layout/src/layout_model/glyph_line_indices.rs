use html_dom::MemoryUsageReport;
use std::ops::Range;

/// Dense source-glyph ownership. One value is reserved for unowned glyphs;
/// larger layouts use wide indexes rather than truncating a valid line ID.
#[derive(Clone)]
pub(crate) struct GlyphLineIndices {
    storage: Storage,
}

#[derive(Clone)]
enum Storage {
    Narrow(Vec<u16>),
    Wide(Vec<u32>),
}

impl Default for GlyphLineIndices {
    fn default() -> Self {
        Self { storage: Storage::Narrow(Vec::new()) }
    }
}

impl GlyphLineIndices {
    pub(crate) fn clear(&mut self) {
        match &mut self.storage {
            Storage::Narrow(values) => values.clear(),
            Storage::Wide(values) => values.clear(),
        }
    }

    /// Reuse the current buffer when its width still fits. Switching widths
    /// releases the old buffer before allocating its replacement.
    pub(crate) fn reset(&mut self, glyph_count: usize, line_count: usize) {
        assert!(line_count <= u32::MAX as usize, "layout line count exceeds glyph-line index capacity");
        let narrow = line_count <= u16::MAX as usize;
        match (&self.storage, narrow) {
            (Storage::Narrow(_), false) => self.storage = Storage::Wide(Vec::new()),
            (Storage::Wide(_), true) => self.storage = Storage::Narrow(Vec::new()),
            _ => {}
        }
        match &mut self.storage {
            Storage::Narrow(values) => {
                values.resize(glyph_count, u16::MAX);
                values.fill(u16::MAX);
            }
            Storage::Wide(values) => {
                values.resize(glyph_count, u32::MAX);
                values.fill(u32::MAX);
            }
        }
    }

    pub(crate) fn assign(&mut self, glyphs: Range<usize>, line_index: usize) {
        match &mut self.storage {
            Storage::Narrow(values) => {
                let line = u16::try_from(line_index).expect("line index exceeds narrow glyph-line capacity");
                assert!(line != u16::MAX, "line index collides with unowned-glyph sentinel");
                values[glyphs].fill(line);
            }
            Storage::Wide(values) => {
                let line = u32::try_from(line_index).expect("line index exceeds wide glyph-line capacity");
                assert!(line != u32::MAX, "line index collides with unowned-glyph sentinel");
                values[glyphs].fill(line);
            }
        }
    }

    #[inline]
    pub(crate) fn get(&self, glyph_index: usize) -> Option<usize> {
        match &self.storage {
            Storage::Narrow(values) => {
                let line = *values.get(glyph_index)?;
                (line != u16::MAX).then_some(line as usize)
            }
            Storage::Wide(values) => {
                let line = *values.get(glyph_index)?;
                (line != u32::MAX).then_some(line as usize)
            }
        }
    }

    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        match &self.storage {
            Storage::Narrow(values) => report.add_slice_storage::<u16>("storage", values.capacity(), values.len()),
            Storage::Wide(values) => report.add_slice_storage::<u32>("storage", values.capacity(), values.len()),
        }
        report
    }

    #[cfg(test)]
    pub(crate) fn as_ptr(&self) -> *const () {
        match &self.storage {
            Storage::Narrow(values) => values.as_ptr().cast(),
            Storage::Wide(values) => values.as_ptr().cast(),
        }
    }

    #[cfg(test)]
    pub(crate) fn is_narrow(&self) -> bool {
        matches!(self.storage, Storage::Narrow(_))
    }
}

#[cfg(test)]
mod tests {
    use super::GlyphLineIndices;

    #[test]
    fn narrow_capacity_keeps_the_last_valid_line_distinct_from_unowned_glyphs() {
        let mut index = GlyphLineIndices::default();
        index.reset(8, u16::MAX as usize);
        index.assign(1..3, u16::MAX as usize - 1);
        assert!(index.is_narrow());
        assert_eq!(index.get(0), None);
        assert_eq!(index.get(1), Some(65_534));
        assert_eq!(index.get(2), Some(65_534));
        assert_eq!(index.get(3), None);
        assert_eq!(index.get(8), None);
        assert_eq!(index.memory_usage_report().total_bytes(), 16);
    }

    #[test]
    fn wide_capacity_keeps_line_65535_valid_and_returns_to_narrow_storage() {
        let mut index = GlyphLineIndices::default();
        index.reset(8, 65_536);
        index.assign(0..2, 65_535);
        assert!(!index.is_narrow());
        assert_eq!(index.get(0), Some(65_535));
        assert_eq!(index.get(2), None);
        assert_eq!(index.memory_usage_report().total_bytes(), 32);
        index.reset(8, 65_537);
        index.assign(4..5, 65_536);
        assert_eq!(index.get(0), None);
        assert_eq!(index.get(4), Some(65_536));
        index.reset(8, 65_535);
        assert!(index.is_narrow());
        assert_eq!(index.get(4), None);
        assert_eq!(index.memory_usage_report().total_bytes(), 16);
    }

    #[test]
    fn rebuilding_reuses_matching_storage_and_preserves_cloned_snapshots() {
        let mut index = GlyphLineIndices::default();
        for count in [12, 65_536] {
            index.reset(8, count);
            index.assign(2..4, count - 1);
            let snapshot = index.clone();
            let pointer = index.as_ptr();
            index.clear();
            assert_eq!(index.get(2), None);
            index.reset(4, count);
            assert_eq!(index.as_ptr(), pointer);
            assert_eq!(index.get(2), None);
            index.assign(0..1, 0);
            assert_eq!(snapshot.get(2), Some(count - 1));
            assert_eq!(snapshot.get(0), None);
            assert_eq!(index.get(0), Some(0));
        }
    }
}
