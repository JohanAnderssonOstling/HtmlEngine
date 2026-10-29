use html_dom::{MemoryUsageReport, storage::CompactU32Buffer};

/// UTF-16 offsets within each originating text node. Synthetic glyphs have
/// no source position and retain the existing `u32::MAX` API sentinel.
#[derive(Clone, Default)]
pub(crate) struct SourceOffsets(CompactU32Buffer);

impl SourceOffsets {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self(CompactU32Buffer::with_capacity(capacity))
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub(crate) fn get(&self, index: usize) -> Option<u32> {
        self.0.get(index)
    }

    #[inline]
    pub(crate) fn push(&mut self, offset: u32) {
        self.0.push(offset);
    }

    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        self.0.memory_usage_report()
    }

    #[cfg(test)]
    pub(crate) fn is_narrow(&self) -> bool {
        self.0.is_narrow()
    }
}

#[cfg(test)]
mod tests {
    use super::SourceOffsets;

    #[test]
    fn narrow_offsets_distinguish_last_valid_value_from_synthetic_text() {
        let mut offsets = SourceOffsets::with_capacity(4);
        for offset in [0, 65_534, u32::MAX] {
            offsets.push(offset);
        }
        assert!(offsets.is_narrow());
        assert_eq!(offsets.len(), 3);
        assert_eq!(offsets.get(0), Some(0));
        assert_eq!(offsets.get(1), Some(65_534));
        assert_eq!(offsets.get(2), Some(u32::MAX));
        assert_eq!(offsets.get(3), None);
        assert_eq!(offsets.memory_usage_report().total_bytes(), 8);
    }

    #[test]
    fn source_offset_65535_promotes_without_changing_existing_offsets() {
        let mut offsets = SourceOffsets::with_capacity(4);
        for offset in [0, u32::MAX, 65_534, 65_535] {
            offsets.push(offset);
        }
        assert!(!offsets.is_narrow());
        assert_eq!(offsets.get(1), Some(u32::MAX));
        assert_eq!(offsets.get(2), Some(65_534));
        assert_eq!(offsets.get(3), Some(65_535));
        assert_eq!(offsets.memory_usage_report().total_bytes(), 16);
        offsets.push(65_536);
        offsets.push(u32::MAX - 1);
        offsets.push(u32::MAX);
        assert_eq!(offsets.get(4), Some(65_536));
        assert_eq!(offsets.get(5), Some(u32::MAX - 1));
        assert_eq!(offsets.get(6), Some(u32::MAX));
    }

    #[test]
    fn promotion_does_not_modify_cloned_snapshots() {
        let mut original = SourceOffsets::default();
        for offset in [0, u32::MAX, 65_534] {
            original.push(offset);
        }
        let mut changed = original.clone();
        changed.push(65_535);
        assert!(original.is_narrow());
        assert!(!changed.is_narrow());
        assert_eq!(original.len(), 3);
        assert_eq!(original.get(1), Some(u32::MAX));
        assert_eq!(original.get(3), None);
        assert_eq!(changed.get(3), Some(65_535));
    }
}
