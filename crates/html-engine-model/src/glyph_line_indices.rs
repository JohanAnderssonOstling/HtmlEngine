use html_dom::{MemoryUsageReport, storage::CompactU32Buffer};
use std::ops::Range;

/// Source-glyph ownership by stable line ID, independent of spatial rank.
/// One value is reserved for unowned glyphs;
/// larger layouts use wide indexes rather than truncating a valid line ID.
#[derive(Clone, Default)]
pub struct GlyphLineIndices(CompactU32Buffer);

impl GlyphLineIndices {
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Reuse the current buffer when its width still fits. Switching widths
    /// releases the old buffer before allocating its replacement.
    pub fn reset(&mut self, glyph_count: usize, line_count: usize) {
        self.0.reset(glyph_count, line_count);
    }

    pub fn assign(&mut self, glyphs: Range<usize>, line_index: usize) {
        let line = u32::try_from(line_index).expect("line index exceeds glyph-line capacity");
        assert_ne!(
            line,
            u32::MAX,
            "line index collides with unowned-glyph sentinel"
        );
        self.0.fill(glyphs, line);
    }

    #[inline]
    pub fn get(&self, glyph_index: usize) -> Option<usize> {
        self.0
            .get(glyph_index)
            .filter(|&line| line != u32::MAX)
            .map(|line| line as usize)
    }

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        self.0.memory_usage_report()
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn as_ptr(&self) -> *const () {
        self.0.as_ptr()
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn is_narrow(&self) -> bool {
        self.0.is_narrow()
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
