//! Shared token and width views. Mutations copy only the borrowed slice they change.
//! Retained arenas are owned by stage 00.

use super::token_span::TokenSpan;
use super::tokens::InlineToken;
use std::borrow::Cow;
use std::ops::{Deref, DerefMut};

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

impl<'a, T: Clone> From<&'a [T]> for TokenBuffer<'a, T> {
    fn from(values: &'a [T]) -> Self {
        Self(Cow::Borrowed(values))
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

/// The same columns are used while preparing, retaining and rewriting tokens.
#[derive(Default)]
pub(super) struct TokenData<'a> {
    pub(super) metadata: TokenBuffer<'a, u64>,
    pub(super) widths: TokenBuffer<'a, f32>,
}
impl TokenData<'_> {
    pub(super) fn span(&self) -> TokenSpan<'_> {
        TokenSpan {
            metadata: &self.metadata,
            widths: &self.widths[..self.metadata.len()],
        }
    }
    pub(super) fn len(&self) -> usize {
        self.metadata.len()
    }
    pub(super) fn is_empty(&self) -> bool {
        self.metadata.is_empty()
    }
    pub(super) fn reserve(&mut self, capacity: usize) {
        self.metadata.reserve(capacity);
        self.widths.reserve(capacity);
    }
    pub(super) fn clear(&mut self) {
        self.metadata.clear();
        self.widths.clear();
    }
    pub(super) fn push(&mut self, token: InlineToken) {
        self.metadata.push(token.header);
        self.widths.make_mut().push(token.width);
    }
    pub(super) fn update_header(&mut self, index: usize, update: impl FnOnce(&mut InlineToken)) {
        let mut token = InlineToken {
            header: self.metadata[index],
            width: 0.0,
        };
        update(&mut token);
        self.metadata[index] = token.header;
    }
    pub(super) fn into_working(self) -> TokenData<'static> {
        TokenData {
            metadata: self.metadata.into_working(),
            widths: self.widths.into_working(),
        }
    }
    pub(super) fn storage_bytes(&self) -> usize {
        self.metadata.capacity() * size_of::<u64>() + self.widths.capacity() * size_of::<f32>()
    }
}
impl FromIterator<InlineToken> for TokenData<'_> {
    fn from_iter<I: IntoIterator<Item = InlineToken>>(values: I) -> Self {
        let values = values.into_iter();
        let mut result = Self::default();
        result.reserve(values.size_hint().0);
        for token in values {
            result.push(token);
        }
        result
    }
}
