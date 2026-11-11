//! Bounded hash-chain indexing with admission after a signature repeats.
use rustc_data_structures::fx::{FxHashMap, FxHashSet};

#[derive(Default)]
pub(super) struct CacheIndex {
    buckets: FxHashMap<u64, usize>,
    observed: FxHashSet<u64>,
}

impl CacheIndex {
    pub(super) fn first(&self, hash: u64) -> Option<usize> {
        self.buckets.get(&hash).copied()
    }

    pub(super) fn admit(&mut self, hash: u64, entries: usize) -> bool {
        if entries >= 512 {
            return false;
        }
        if self.observed.contains(&hash) {
            return true;
        }
        if self.observed.len() < 1024 {
            self.observed.insert(hash);
        }
        false
    }

    /// Returns the previous head; entries retain exact collision comparisons.
    pub(super) fn insert(&mut self, hash: u64, index: usize) -> Option<usize> {
        self.buckets.insert(hash, index)
    }
}
