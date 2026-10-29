//! Indexed storage shared by document preparation and published stage data.

mod strings;
pub use strings::StringTable;

use crate::MemoryUsageReport;
use std::ops::Range;

/// A scalar buffer that reserves `u32::MAX` for missing values. Narrow storage
/// encodes that sentinel as `u16::MAX`; a real value of 65,535 requires widening.
/// Domain wrappers own the meaning of each value and their mutation policy.
#[derive(Clone)]
pub struct CompactU32Buffer {
    storage: Storage,
}

#[derive(Clone)]
enum Storage {
    Narrow(Vec<u16>),
    Wide(Vec<u32>),
}

impl Default for CompactU32Buffer {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl CompactU32Buffer {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            storage: Storage::Narrow(Vec::with_capacity(capacity)),
        }
    }

    pub fn len(&self) -> usize {
        match &self.storage {
            Storage::Narrow(values) => values.len(),
            Storage::Wide(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        match &mut self.storage {
            Storage::Narrow(values) => values.clear(),
            Storage::Wide(values) => values.clear(),
        }
    }

    #[inline]
    pub fn get(&self, index: usize) -> Option<u32> {
        match &self.storage {
            Storage::Narrow(values) => values.get(index).map(|&value| decode(value)),
            Storage::Wide(values) => values.get(index).copied(),
        }
    }

    /// Append a value, promoting existing values when narrow storage is full.
    #[inline]
    pub fn push(&mut self, value: u32) {
        match &mut self.storage {
            Storage::Narrow(values) if value < u32::from(u16::MAX) => values.push(value as u16),
            Storage::Narrow(values) if value == u32::MAX => values.push(u16::MAX),
            Storage::Narrow(_) => self.promote_and_push(value),
            Storage::Wide(values) => values.push(value),
        }
    }

    #[cold]
    fn promote_and_push(&mut self, value: u32) {
        let Storage::Narrow(narrow) = &self.storage else {
            unreachable!("only narrow storage can be promoted")
        };
        let mut wide = Vec::with_capacity(narrow.capacity().max(narrow.len() + 1));
        wide.extend(narrow.iter().copied().map(decode));
        wide.push(value);
        self.storage = Storage::Wide(wide);
    }

    /// Reset to missing values with room for indexes in `0..value_count`.
    /// Reuses matching storage; switching widths releases the previous buffer.
    pub fn reset(&mut self, len: usize, value_count: usize) {
        assert!(
            value_count <= u32::MAX as usize,
            "compact index capacity exceeded"
        );
        let narrow = value_count <= u16::MAX as usize;
        match (&self.storage, narrow) {
            (Storage::Narrow(_), false) => self.storage = Storage::Wide(Vec::new()),
            (Storage::Wide(_), true) => self.storage = Storage::Narrow(Vec::new()),
            _ => {}
        }
        match &mut self.storage {
            Storage::Narrow(values) => {
                values.resize(len, u16::MAX);
                values.fill(u16::MAX);
            }
            Storage::Wide(values) => {
                values.resize(len, u32::MAX);
                values.fill(u32::MAX);
            }
        }
    }

    /// Fill an existing range. The value must fit the width chosen by `reset`.
    pub fn fill(&mut self, range: Range<usize>, value: u32) {
        match &mut self.storage {
            Storage::Narrow(values) => {
                let encoded = if value == u32::MAX {
                    u16::MAX
                } else {
                    assert!(
                        value < u32::from(u16::MAX),
                        "value exceeds narrow storage capacity"
                    );
                    value as u16
                };
                values[range].fill(encoded);
            }
            Storage::Wide(values) => values[range].fill(value),
        }
    }

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
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

    pub fn is_narrow(&self) -> bool {
        matches!(self.storage, Storage::Narrow(_))
    }

    /// Storage identity for allocation-reuse diagnostics; never dereferenced.
    pub fn as_ptr(&self) -> *const () {
        match &self.storage {
            Storage::Narrow(values) => values.as_ptr().cast(),
            Storage::Wide(values) => values.as_ptr().cast(),
        }
    }
}

fn decode(value: u16) -> u32 {
    if value == u16::MAX {
        u32::MAX
    } else {
        u32::from(value)
    }
}
