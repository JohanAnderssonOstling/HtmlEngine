use html_dom::MemoryUsageReport;

/// UTF-16 offsets within each originating text node. Synthetic glyphs have
/// no source position and retain the existing `u32::MAX` API sentinel.
#[derive(Clone)]
pub(crate) struct SourceOffsets {
    storage: Storage,
}

#[derive(Clone)]
enum Storage {
    Narrow(Vec<u16>),
    Wide(Vec<u32>),
}

impl Default for SourceOffsets {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl SourceOffsets {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            storage: Storage::Narrow(Vec::with_capacity(capacity)),
        }
    }

    pub(crate) fn len(&self) -> usize {
        match &self.storage {
            Storage::Narrow(values) => values.len(),
            Storage::Wide(values) => values.len(),
        }
    }

    #[inline]
    pub(crate) fn get(&self, index: usize) -> Option<u32> {
        match &self.storage {
            Storage::Narrow(values) => values.get(index).map(|&value| {
                if value == u16::MAX {
                    u32::MAX
                } else {
                    u32::from(value)
                }
            }),
            Storage::Wide(values) => values.get(index).copied(),
        }
    }

    #[inline]
    pub(crate) fn push(&mut self, offset: u32) {
        match &mut self.storage {
            Storage::Narrow(values) if offset < u32::from(u16::MAX) => values.push(offset as u16),
            Storage::Narrow(values) if offset == u32::MAX => values.push(u16::MAX),
            Storage::Narrow(_) => self.promote_and_push(offset),
            Storage::Wide(values) => values.push(offset),
        }
    }

    /// A real offset of 65,535 must not collide with the narrow sentinel.
    /// Promotion also restores every previously appended synthetic offset.
    #[cold]
    fn promote_and_push(&mut self, offset: u32) {
        let Storage::Narrow(narrow) = &self.storage else {
            unreachable!("only narrow offsets can be promoted")
        };
        let mut wide = Vec::with_capacity(narrow.capacity().max(narrow.len() + 1));
        wide.extend(narrow.iter().map(|&value| {
            if value == u16::MAX {
                u32::MAX
            } else {
                u32::from(value)
            }
        }));
        wide.push(offset);
        self.storage = Storage::Wide(wide);
    }

    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        match &self.storage {
            Storage::Narrow(values) => {
                report.add_slice_storage::<u16>("storage", values.capacity(), values.len())
            }
            Storage::Wide(values) => {
                report.add_slice_storage::<u32>("storage", values.capacity(), values.len())
            }
        }
        report
    }

    #[cfg(test)]
    pub(crate) fn is_narrow(&self) -> bool {
        matches!(self.storage, Storage::Narrow(_))
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
