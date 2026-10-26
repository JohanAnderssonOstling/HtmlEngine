use html_dom::MemoryUsageReport;
use std::ops::Range;

/// Contiguous published groups with allocation-free interleaved construction.
/// During emission, ranges hold linked-list heads/tails. Finalization converts
/// links to a permutation and reorders items in place, publishing ordinary
/// slices. This supports an outer line continuing after nested inline layout.
#[derive(Clone)]
pub(crate) struct GroupedArena<T> {
    items: Vec<T>,
    ranges: Vec<Range<u32>>,
    links: Vec<u32>,
    frozen: bool,
}

const NONE: u32 = u32::MAX;

impl<T> Default for GroupedArena<T> {
    fn default() -> Self {
        Self { items: Vec::new(), ranges: Vec::new(), links: Vec::new(), frozen: true }
    }
}

impl<T> GroupedArena<T> {
    pub(crate) fn clear(&mut self) {
        self.items.clear();
        self.ranges.clear();
        self.links.clear();
        self.frozen = false;
    }

    pub(crate) fn reset_groups(&mut self, count: usize) {
        self.clear();
        self.ranges.resize(count, NONE..NONE);
    }

    pub(crate) fn push(&mut self, values: impl IntoIterator<Item = T>) {
        assert!(!self.frozen);
        let group = self.ranges.len();
        self.ranges.push(NONE..NONE);
        for value in values {
            self.push_to(group, value);
        }
    }

    pub(crate) fn push_to(&mut self, group: usize, value: T) {
        assert!(!self.frozen);
        let index = u32::try_from(self.items.len()).expect("grouped arena capacity exhausted");
        assert_ne!(index, NONE, "grouped arena capacity exhausted");
        let range = &mut self.ranges[group];
        if range.start == NONE {
            range.start = index;
        } else {
            self.links[range.end as usize] = index;
        }
        range.end = index;
        self.items.push(value);
        self.links.push(NONE);
    }

    pub(crate) fn last_mut(&mut self, group: usize) -> Option<&mut T> {
        assert!(!self.frozen);
        let last = self.ranges.get(group)?.end;
        (last != NONE).then(|| &mut self.items[last as usize])
    }

    pub(crate) fn freeze(&mut self) {
        if self.frozen {
            return;
        }
        let mut position = 0u32;
        for range in &mut self.ranges {
            let mut item = range.start;
            let start = position;
            while item != NONE {
                let next = self.links[item as usize];
                self.links[item as usize] = position;
                position = position.checked_add(1).expect("grouped arena capacity exhausted");
                item = next;
            }
            *range = start..position;
        }
        assert_eq!(position as usize, self.items.len());
        for index in 0..self.items.len() {
            while self.links[index] as usize != index {
                let destination = self.links[index] as usize;
                self.items.swap(index, destination);
                self.links.swap(index, destination);
            }
        }
        self.links.clear();
        self.frozen = true;
    }

    pub(crate) fn get(&self, group: usize) -> Option<&[T]> {
        assert!(self.frozen);
        let range = self.ranges.get(group)?;
        self.items.get(range.start as usize..range.end as usize)
    }

    /// Move a published group's values into the next publication. The old
    /// range remains valid for other groups; consumed values become empty.
    pub(crate) fn take_group(&mut self, group: usize) -> impl Iterator<Item = T> + '_
    where
        T: Default,
    {
        assert!(self.frozen);
        let range = self.ranges[group].clone();
        self.items[range.start as usize..range.end as usize]
            .iter_mut().map(std::mem::take)
    }

    /// Reuse the previous publication's allocation after all of its reads end.
    pub(crate) fn recycle_storage(&mut self, mut storage: Self)
    where
        T: Default,
    {
        self.freeze();
        storage.clear();
        for group in 0..self.len() {
            storage.push(self.take_group(group));
        }
        *self = storage;
    }

    pub(crate) fn get_mut(&mut self, group: usize) -> Option<&mut [T]> {
        assert!(self.frozen);
        let range = self.ranges.get(group)?;
        self.items.get_mut(range.start as usize..range.end as usize)
    }

    pub(crate) fn range(&self, group: usize) -> Range<u32> {
        assert!(self.frozen);
        self.ranges[group].clone()
    }

    pub(crate) fn clear_ranges(&mut self) {
        assert!(self.frozen);
        self.ranges.clear();
    }

    pub(crate) fn push_range(&mut self, range: Range<u32>) {
        assert!(self.frozen);
        self.ranges.push(range);
    }

    pub(crate) fn len(&self) -> usize {
        self.ranges.len()
    }

    #[cfg(test)]
    pub(crate) fn as_ptr(&self) -> *const Range<u32> {
        self.ranges.as_ptr()
    }

    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<Range<u32>>("ranges.storage", self.ranges.capacity(), self.ranges.len());
        report.add_slice_storage::<T>("items.storage", self.items.capacity(), self.items.len());
        report.add_slice_storage::<u32>("build_links.storage", self.links.capacity(), self.links.len());
        report
    }
}

#[cfg(test)]
mod tests {
    use super::GroupedArena;

    #[test]
    fn publication_transfer_moves_groups_and_recycles_the_original_allocation() {
        let mut previous = GroupedArena::<Vec<u32>>::default();
        previous.reset_groups(2);
        previous.push_to(0, vec![10, 11]);
        previous.push_to(1, vec![20, 21]);
        previous.freeze();
        let storage = previous.ranges.as_ptr();
        let payload = previous.get(1).unwrap()[0].as_ptr();
        let mut current = GroupedArena::default();
        current.clear();
        current.push(previous.take_group(1));
        current.push(previous.take_group(0));
        assert!(previous.get(1).unwrap()[0].is_empty(), "the source no longer owns the moved payload");
        current.recycle_storage(previous);
        current.freeze();
        assert_eq!(current.ranges.as_ptr(), storage);
        assert_eq!(current.get(0).unwrap(), &[vec![20, 21]]);
        assert_eq!(current.get(1).unwrap(), &[vec![10, 11]]);
        assert_eq!(current.get(0).unwrap()[0].as_ptr(), payload);
    }

    #[test]
    fn interleaved_groups_publish_contiguous_slices_and_reuse_storage() {
        let mut arena = GroupedArena::default();
        arena.reset_groups(4);
        for (group, value) in [(2, 20), (0, 1), (2, 21), (1, 10), (0, 2)] {
            arena.push_to(group, value);
        }
        *arena.last_mut(2).unwrap() = 22;
        arena.freeze();
        assert_eq!(arena.get(0), Some([1, 2].as_slice()));
        assert_eq!(arena.get(1), Some([10].as_slice()));
        assert_eq!(arena.get(2), Some([20, 22].as_slice()));
        assert_eq!(arena.get(3), Some([].as_slice()));
        assert_eq!(arena.get(4), None);
        let cloned = arena.clone();
        arena.get_mut(2).unwrap()[0] = 99;
        assert_eq!(cloned.get(2).unwrap(), &[20, 22]);
        let item_pointer = arena.items.as_ptr();
        let range_pointer = arena.as_ptr();
        arena.reset_groups(4);
        arena.push_to(3, 7);
        arena.freeze();
        assert_eq!(arena.items.as_ptr(), item_pointer);
        assert_eq!(arena.as_ptr(), range_pointer);
        assert!(arena.get(0).unwrap().is_empty());
        assert_eq!(arena.get(3).unwrap(), &[7]);
    }
}
