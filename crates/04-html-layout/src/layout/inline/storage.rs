//! Packed token storage with scoped read views. Arena writes occur after
//! borrowed views end; transformations copy only the slice they change.

use super::token_span::WidthsView;
use html_dom::MemoryUsageReport;
use rustc_data_structures::fx::FxHashMap;
use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::mem::size_of;
use std::ops::Range;
use std::ops::{Deref, DerefMut};

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

pub(super) struct TokenBuffer<'a, T: Clone>(Cow<'a, [T]>);

impl<T: Clone> Default for TokenBuffer<'_, T> {
    fn default() -> Self {
        Self(Cow::Owned(Vec::new()))
    }
}

impl<T: Clone> From<Vec<T>> for TokenBuffer<'_, T> {
    fn from(values: Vec<T>) -> Self {
        Self(Cow::Owned(values))
    }
}

impl<T: Clone> FromIterator<T> for TokenBuffer<'_, T> {
    fn from_iter<I: IntoIterator<Item = T>>(values: I) -> Self {
        Vec::from_iter(values).into()
    }
}

impl<T: Clone> Deref for TokenBuffer<'_, T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T: Clone> TokenBuffer<'_, T> {
    /// Arena storage is accounted for by its owner, not by borrowed views.
    pub(super) fn capacity(&self) -> usize {
        match &self.0 {
            Cow::Owned(values) => values.capacity(),
            Cow::Borrowed(_) => 0,
        }
    }

    pub(super) fn clear(&mut self) {
        match &mut self.0 {
            Cow::Owned(values) => values.clear(),
            Cow::Borrowed(_) => *self = Self::default(),
        }
    }

    /// Keep changed working storage; untouched arena views become empty borrows.
    pub(super) fn into_working(self) -> TokenBuffer<'static, T>
    where
        T: 'static,
    {
        TokenBuffer(match self.0 {
            Cow::Owned(values) => Cow::Owned(values),
            Cow::Borrowed(_) => Cow::Borrowed(&[]),
        })
    }

    pub(super) fn is_borrowed(&self) -> bool {
        matches!(self.0, Cow::Borrowed(_))
    }

    #[inline]
    pub(super) fn make_mut(&mut self) -> &mut Vec<T> {
        self.0.to_mut()
    }
    pub(super) fn reserve(&mut self, additional: usize) {
        self.make_mut().reserve(additional);
    }
    pub(super) fn push(&mut self, value: T) {
        self.make_mut().push(value);
    }
}

impl<T: Clone> DerefMut for TokenBuffer<'_, T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.make_mut()
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
        TokenBuffer(Cow::Borrowed(&self.data[range.start as usize..range.end()]))
    }
    pub(super) fn reserve_exact(&mut self, capacity: usize) {
        assert!(self.data.is_empty());
        self.data.reserve_exact(capacity);
    }
    pub(super) fn shrink_to_fit(&mut self) {
        self.data.shrink_to_fit();
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

/// A byte index always addresses an initialized entry. Fixed-size tables let
/// readers omit the second bounds check and keep traversal descriptors small.
pub(super) type WidthTable = [f32; 256];
const EMPTY_WIDTHS: WidthTable = [0.0; 256];

/// Construction keeps writable f32 widths. Retained plans can borrow exact
/// values through byte indexes without expanding a paragraph on every reflow.
pub(super) enum WidthBuffer<'a> {
    Direct(TokenBuffer<'a, f32>),
    Indexed {
        indexes: &'a [u8],
        values: &'a WidthTable,
    },
}
impl Default for WidthBuffer<'_> {
    fn default() -> Self {
        Self::Direct(TokenBuffer::default())
    }
}
impl From<Vec<f32>> for WidthBuffer<'_> {
    fn from(values: Vec<f32>) -> Self {
        Self::Direct(values.into())
    }
}
impl<'a> WidthBuffer<'a> {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Direct(values) => values.len(),
            Self::Indexed { indexes, .. } => indexes.len(),
        }
    }
    pub(super) fn capacity(&self) -> usize {
        match self {
            Self::Direct(values) => values.capacity(),
            Self::Indexed { .. } => 0,
        }
    }
    pub(super) fn is_borrowed(&self) -> bool {
        match self {
            Self::Direct(values) => values.is_borrowed(),
            Self::Indexed { .. } => true,
        }
    }
    pub(super) fn clear(&mut self) {
        match self {
            Self::Direct(values) => values.clear(),
            Self::Indexed { .. } => *self = Self::default(),
        }
    }
    pub(super) fn reserve(&mut self, capacity: usize) {
        self.make_mut().reserve(capacity);
    }
    pub(super) fn make_mut(&mut self) -> &mut Vec<f32> {
        if let Self::Indexed { indexes, values } = self {
            *self = Self::Direct(
                indexes
                    .iter()
                    .map(|&index| values[index as usize])
                    .collect(),
            );
        }
        let Self::Direct(values) = self else {
            unreachable!()
        };
        values.make_mut()
    }
    pub(super) fn into_working(self) -> WidthBuffer<'static> {
        match self {
            Self::Direct(values) => WidthBuffer::Direct(values.into_working()),
            Self::Indexed { .. } => WidthBuffer::Indexed {
                indexes: &[],
                values: &EMPTY_WIDTHS,
            },
        }
    }
    #[inline]
    pub(super) fn view(&self, range: Range<usize>) -> WidthsView<'_> {
        match self {
            Self::Direct(values) => WidthsView::Compact(&values[range]),
            Self::Indexed { indexes, values } => WidthsView::Indexed {
                indexes: &indexes[range],
                values,
            },
        }
    }
}

enum WidthStorage {
    Indexed {
        indexes: Vec<u8>,
        values: Option<Box<WidthTable>>,
        value_count: u16,
        lookup: FxHashMap<u32, u8>,
    },
    Direct(Vec<f32>),
}

pub(super) struct WidthArena {
    storage: WidthStorage,
}
impl Default for WidthArena {
    fn default() -> Self {
        Self {
            storage: WidthStorage::Indexed {
                indexes: Vec::new(),
                values: None,
                value_count: 0,
                lookup: Default::default(),
            },
        }
    }
}
impl WidthArena {
    pub(super) fn capacity(&self) -> usize {
        match &self.storage {
            WidthStorage::Indexed { indexes, .. } => indexes.capacity(),
            WidthStorage::Direct(values) => values.capacity(),
        }
    }
    pub(super) fn reserve_exact(&mut self, capacity: usize) {
        match &mut self.storage {
            WidthStorage::Indexed { indexes, .. } => {
                assert!(indexes.is_empty());
                indexes.reserve_exact(capacity);
            }
            WidthStorage::Direct(values) => {
                assert!(values.is_empty());
                values.reserve_exact(capacity);
            }
        }
    }
    pub(super) fn shrink_to_fit(&mut self) {
        match &mut self.storage {
            WidthStorage::Indexed {
                indexes, lookup, ..
            } => {
                indexes.shrink_to_fit();
                lookup.shrink_to_fit();
            }
            WidthStorage::Direct(values) => values.shrink_to_fit(),
        }
    }
    pub(super) fn storage_bytes(&self) -> usize {
        match &self.storage {
            WidthStorage::Indexed {
                indexes,
                values,
                lookup,
                ..
            } => {
                let mut report = MemoryUsageReport::new();
                report.add_vec_storage("indexes", indexes);
                if values.is_some() {
                    report.add("values", size_of::<WidthTable>(), 1);
                }
                report.add_hash_map_storage("lookup", lookup);
                report.total_bytes()
            }
            WidthStorage::Direct(values) => values.capacity() * size_of::<f32>(),
        }
    }
    pub(super) fn view(&self, range: ArenaRange) -> WidthBuffer<'_> {
        match &self.storage {
            WidthStorage::Indexed {
                indexes, values, ..
            } => WidthBuffer::Indexed {
                indexes: &indexes[range.start as usize..range.end()],
                values: values.as_deref().unwrap_or(&EMPTY_WIDTHS),
            },
            WidthStorage::Direct(values) => WidthBuffer::Direct(TokenBuffer(Cow::Borrowed(
                &values[range.start as usize..range.end()],
            ))),
        }
    }
    pub(super) fn store(
        &mut self,
        previous: ArenaRange,
        widths: &WidthBuffer<'_>,
        changed_only: bool,
    ) -> ArenaRange {
        if changed_only && widths.is_borrowed() {
            return previous;
        }
        let len = widths.len();
        if len == 0 {
            return ArenaRange::default();
        }
        let view = widths.view(0..len);
        let existing = match &self.storage {
            WidthStorage::Indexed { indexes, .. } => indexes.len(),
            WidthStorage::Direct(values) => values.len(),
        };
        let range = if previous.len as usize == len {
            previous
        } else {
            ArenaRange {
                start: u32::try_from(existing).expect("inline width arena capacity exhausted"),
                len: u32::try_from(len).expect("inline width range capacity exhausted"),
            }
        };
        range
            .start
            .checked_add(range.len)
            .expect("inline width range capacity exhausted");
        // Intern and write each exact width in one pass. Repeated adjacent
        // widths reuse their index without hashing or looking up the table.
        let mut direct_start = 0;
        if let WidthStorage::Indexed {
            indexes,
            values,
            lookup,
            value_count,
        } = &mut self.storage
        {
            if previous.len as usize != len {
                indexes.resize(range.end(), 0);
            }
            let values = values.get_or_insert_with(|| Box::new(EMPTY_WIDTHS));
            let mut recent = None;
            for index in 0..len {
                let width = view.at(index);
                let bits = width.to_bits();
                let code = if let Some((previous, code)) = recent
                    && previous == bits
                {
                    code
                } else {
                    match lookup.entry(bits) {
                        Entry::Occupied(entry) => *entry.get(),
                        Entry::Vacant(entry) => {
                            if *value_count >= 256 {
                                direct_start = index;
                                break;
                            }
                            let code = *value_count as u8;
                            values[*value_count as usize] = width;
                            *value_count += 1;
                            entry.insert(code);
                            code
                        }
                    }
                };
                indexes[range.start as usize + index] = code;
                recent = Some((bits, code));
                if index + 1 == len {
                    return range;
                }
            }
            // Diverse shaping keeps the original raw column. The encoded
            // prefix is already complete; only the remaining widths need reads.
            let mut direct = Vec::with_capacity(indexes.capacity());
            direct.extend(indexes.iter().map(|&code| values[code as usize]));
            self.storage = WidthStorage::Direct(direct);
        }
        let WidthStorage::Direct(values) = &mut self.storage else {
            unreachable!()
        };
        if previous.len as usize != len {
            values.resize(range.end(), 0.0);
        }
        for (index, target) in values[range.start as usize + direct_start..range.end()]
            .iter_mut()
            .enumerate()
        {
            *target = view.at(direct_start + index);
        }
        range
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
