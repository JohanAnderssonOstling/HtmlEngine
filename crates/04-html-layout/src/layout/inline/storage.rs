//! Read views into retained arenas. Mutation owns only the affected slice;
//! nested layout may append another chunk without moving a live read view.

use std::mem::{size_of, take};
use std::ops::{Deref, DerefMut, Range};
use std::sync::Arc;

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

enum BufferStorage<T> {
    Owned(Vec<T>),
    Shared {
        data: Arc<Vec<T>>,
        range: Range<usize>,
    },
}

pub(super) struct TokenBuffer<T>(BufferStorage<T>);

impl<T> Default for TokenBuffer<T> {
    fn default() -> Self {
        Self(BufferStorage::Owned(Vec::new()))
    }
}

impl<T> From<Vec<T>> for TokenBuffer<T> {
    fn from(values: Vec<T>) -> Self {
        Self(BufferStorage::Owned(values))
    }
}

impl<T> FromIterator<T> for TokenBuffer<T> {
    fn from_iter<I: IntoIterator<Item = T>>(values: I) -> Self {
        Vec::from_iter(values).into()
    }
}

impl<T> Deref for TokenBuffer<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &[T] {
        match &self.0 {
            BufferStorage::Owned(values) => values,
            BufferStorage::Shared { data, range } => &data[range.clone()],
        }
    }
}

impl<T> TokenBuffer<T> {
    /// Shared capacity is accounted for by the arena, not by working buffers.
    pub(super) fn capacity(&self) -> usize {
        match &self.0 {
            BufferStorage::Owned(values) => values.capacity(),
            BufferStorage::Shared { .. } => 0,
        }
    }

    /// Discarding a view must release its reference without copying it.
    pub(super) fn clear(&mut self) {
        match &mut self.0 {
            BufferStorage::Owned(values) => values.clear(),
            BufferStorage::Shared { .. } => *self = Self::default(),
        }
    }

    #[cfg(test)]
    pub(super) fn is_shared(&self) -> bool {
        matches!(self.0, BufferStorage::Shared { .. })
    }
}

impl<T: Clone> TokenBuffer<T> {
    #[inline]
    pub(super) fn make_mut(&mut self) -> &mut Vec<T> {
        if matches!(self.0, BufferStorage::Shared { .. }) {
            self.0 = BufferStorage::Owned(self.to_vec());
        }
        match &mut self.0 {
            BufferStorage::Owned(values) => values,
            BufferStorage::Shared { .. } => unreachable!(),
        }
    }

    pub(super) fn reserve(&mut self, additional: usize) {
        self.make_mut().reserve(additional);
    }

    pub(super) fn push(&mut self, value: T) {
        self.make_mut().push(value);
    }
}

impl<T: Clone> DerefMut for TokenBuffer<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.make_mut()
    }
}

struct ArenaChunk<T> {
    start: usize,
    data: Arc<Vec<T>>,
}

pub(super) struct TokenArena<T> {
    chunks: Vec<ArenaChunk<T>>,
}

impl<T> Default for TokenArena<T> {
    fn default() -> Self {
        Self { chunks: Vec::new() }
    }
}

impl<T> TokenArena<T> {
    pub(super) fn len(&self) -> usize {
        self.chunks
            .last()
            .map_or(0, |chunk| chunk.start + chunk.data.len())
    }

    pub(super) fn capacity(&self) -> usize {
        self.chunks.iter().map(|chunk| chunk.data.capacity()).sum()
    }

    pub(super) fn metadata_bytes(&self) -> usize {
        self.chunks.capacity() * size_of::<ArenaChunk<T>>()
            + self.chunks.len() * (2 * size_of::<usize>() + size_of::<Vec<T>>())
    }

    fn chunk(&self, range: ArenaRange) -> &ArenaChunk<T> {
        self.chunks
            .iter()
            .find(|chunk| {
                range.start as usize >= chunk.start && range.end() <= chunk.start + chunk.data.len()
            })
            .expect("inline token range must belong to one arena chunk")
    }

    pub(super) fn view(&self, range: ArenaRange) -> TokenBuffer<T> {
        if range.len == 0 {
            return TokenBuffer::default();
        }
        let chunk = self.chunk(range);
        let start = range.start as usize - chunk.start;
        TokenBuffer(BufferStorage::Shared {
            data: Arc::clone(&chunk.data),
            range: start..start + range.len as usize,
        })
    }

    pub(super) fn reserve_exact(&mut self, capacity: usize) {
        assert!(self.chunks.is_empty());
        if capacity > 0 {
            self.chunks.push(ArenaChunk {
                start: 0,
                data: Arc::new(Vec::with_capacity(capacity)),
            });
        }
    }

    pub(super) fn shrink_to_fit(&mut self) {
        for chunk in &mut self.chunks {
            if let Some(values) = Arc::get_mut(&mut chunk.data) {
                values.shrink_to_fit();
            }
        }
        self.chunks.shrink_to_fit();
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T: Clone> TokenArena<T> {
    pub(super) fn store(
        &mut self,
        previous: ArenaRange,
        values: &mut TokenBuffer<T>,
    ) -> ArenaRange {
        if values.is_empty() {
            return ArenaRange::default();
        }
        if previous.len as usize == values.len() {
            let chunk = self.chunk(previous);
            if let BufferStorage::Shared { data, range } = &values.0 {
                if Arc::ptr_eq(data, &chunk.data)
                    && range.start + chunk.start == previous.start as usize
                {
                    // An unchanged read view needs neither a copy nor a write.
                    return previous;
                }
            }
            let chunk = self
                .chunks
                .iter_mut()
                .find(|chunk| {
                    previous.start as usize >= chunk.start
                        && previous.end() <= chunk.start + chunk.data.len()
                })
                .unwrap();
            if let Some(data) = Arc::get_mut(&mut chunk.data) {
                let start = previous.start as usize - chunk.start;
                data[start..start + values.len()].clone_from_slice(values);
                return previous;
            }
        }
        let start = u32::try_from(self.len()).expect("inline token arena capacity exhausted");
        let len = u32::try_from(values.len()).expect("inline token range capacity exhausted");
        start
            .checked_add(len)
            .expect("inline token arena capacity exhausted");
        if let Some(data) = self
            .chunks
            .last_mut()
            .and_then(|chunk| Arc::get_mut(&mut chunk.data))
        {
            data.extend_from_slice(values);
        } else {
            // A live outer paragraph pins this chunk. Transfer owned changes
            // to a new chunk instead of cloning the whole retained arena.
            let data = match take(values).0 {
                BufferStorage::Owned(data) => data,
                BufferStorage::Shared { data, range } => data[range].to_vec(),
            };
            self.chunks.push(ArenaChunk {
                start: start as usize,
                data: Arc::new(data),
            });
        }
        ArenaRange { start, len }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counted {
        value: u32,
        clones: Arc<AtomicUsize>,
    }

    impl Clone for Counted {
        fn clone(&self) -> Self {
            self.clones.fetch_add(1, Ordering::Relaxed);
            Self {
                value: self.value,
                clones: Arc::clone(&self.clones),
            }
        }
    }

    #[test]
    fn mutation_copies_only_the_viewed_range_and_clear_does_not_copy() {
        let clones = Arc::new(AtomicUsize::new(0));
        let mut arena = TokenArena::default();
        let mut first = (0..10)
            .map(|value| Counted {
                value,
                clones: Arc::clone(&clones),
            })
            .collect();
        arena.store(ArenaRange::default(), &mut first);
        let mut second = (10..12)
            .map(|value| Counted {
                value,
                clones: Arc::clone(&clones),
            })
            .collect();
        let range = arena.store(ArenaRange::default(), &mut second);
        clones.store(0, Ordering::Relaxed);
        let mut view = arena.view(range);
        assert_eq!(view[0].value, 10);
        view[0].value = 42;
        assert_eq!(
            clones.load(Ordering::Relaxed),
            2,
            "copy the two-value paragraph, not the whole arena"
        );
        assert_eq!(
            arena.view(range)[0].value,
            10,
            "read views retain their original values"
        );
        clones.store(0, Ordering::Relaxed);
        let mut unchanged = arena.view(range);
        unchanged.clear();
        assert_eq!(
            clones.load(Ordering::Relaxed),
            0,
            "dropping a read view must not materialize a copy"
        );
        assert_eq!(unchanged.capacity(), 0);
    }
}
