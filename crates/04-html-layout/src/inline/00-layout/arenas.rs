//! Retained token and width arenas owned by the layout cache.
use crate::inline::storage::TokenBuffer;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ArenaRange {
    pub(super) start: u32,
    pub(super) len: u32,
}

impl ArenaRange {
    fn end(self) -> usize {
        self.start as usize + self.len as usize
    }
}

pub(super) struct TokenArena<T> {
    data: Vec<T>,
}

impl<T> Default for TokenArena<T> {
    fn default() -> Self {
        Self { data: Vec::new() }
    }
}

impl<T: Clone> TokenArena<T> {
    pub(super) fn len(&self) -> usize {
        self.data.len()
    }
    pub(super) fn capacity(&self) -> usize {
        self.data.capacity()
    }
    pub(super) fn view(&self, range: ArenaRange) -> TokenBuffer<'_, T> {
        TokenBuffer::from(&self.data[range.start as usize..range.end()])
    }
    pub(super) fn reserve_exact(&mut self, capacity: usize) {
        assert!(self.data.is_empty());
        self.data.reserve_exact(capacity);
    }
    #[cfg(any(test, feature = "testing"))]
    pub(super) fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    pub(super) fn store(
        &mut self,
        previous: ArenaRange,
        values: &TokenBuffer<'_, T>,
        changed_only: bool,
    ) -> ArenaRange {
        if changed_only && values.is_borrowed() {
            return previous;
        }
        if values.is_empty() {
            return ArenaRange::default();
        }
        if previous.len as usize == values.len() {
            self.data[previous.start as usize..previous.end()].clone_from_slice(values);
            return previous;
        }
        let start = u32::try_from(self.len()).expect("inline token arena capacity exhausted");
        let len = u32::try_from(values.len()).expect("inline token range capacity exhausted");
        start
            .checked_add(len)
            .expect("inline token arena capacity exhausted");
        self.data.extend_from_slice(values);
        ArenaRange { start, len }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Counted<'a> {
        value: u32,
        clones: &'a Cell<usize>,
    }
    impl Clone for Counted<'_> {
        fn clone(&self) -> Self {
            self.clones.set(self.clones.get() + 1);
            Self {
                value: self.value,
                clones: self.clones,
            }
        }
    }

    #[test]
    fn mutation_copies_only_the_viewed_range_and_clear_does_not_copy() {
        let clones = Cell::new(0);
        let mut arena = TokenArena::default();
        let values: Vec<_> = (0..12)
            .map(|value| Counted {
                value,
                clones: &clones,
            })
            .collect();
        arena.store(ArenaRange::default(), &values.into(), false);
        let range = ArenaRange { start: 10, len: 2 };
        clones.set(0);
        let mut view = arena.view(range);
        assert_eq!(view[0].value, 10);
        view[0].value = 42;
        assert_eq!(clones.get(), 2, "copy only the transformed paragraph");
        assert_eq!(arena.view(range)[0].value, 10);
        clones.set(0);
        let mut unchanged = arena.view(range);
        unchanged.clear();
        assert_eq!(clones.get(), 0, "clearing a borrowed view must not copy it");
        assert_eq!(unchanged.capacity(), 0);
    }
}
