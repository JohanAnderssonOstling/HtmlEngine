use rustc_data_structures::fx::FxHashSet;
use std::mem::{size_of, size_of_val};
use std::sync::Arc;

const CHECKPOINT_STRIDE: usize = 32;

/// Exact native text-boundary positions. Ordinary runs reuse their advances;
/// checkpoints bound random access, and exceptions preserve native rounding,
/// ligature geometry and visual direction changes without approximation.
#[derive(Clone, Debug)]
pub enum CaretPositions {
    Dense(Arc<[f32]>),
    Derived(Arc<DerivedCarets>),
}

#[derive(Debug)]
pub struct DerivedCarets {
    advances: Arc<[f32]>,
    checkpoints: Box<[f32]>,
    exceptions: Box<[CaretException]>,
}

#[derive(Clone, Copy, Debug)]
struct CaretException {
    index: u32,
    position: f32,
}

impl CaretPositions {
    /// Build advances and exact compact carets together. The producer pushes
    /// each character's starting position and advance, then returns the final
    /// boundary. Return `None` to let the caller use its dense fallback when
    /// the input is unordered, invalid, or does not compress economically.
    pub fn build_compact(
        character_count: usize,
        produce: impl FnOnce(&mut CaretPositionsBuilder<'_>) -> Option<f32>,
    ) -> Option<(Arc<[f32]>, Self)> {
        let len = character_count.checked_add(1)?;
        if len < CHECKPOINT_STRIDE * 2 || u32::try_from(len).is_err() {
            return None;
        }
        let checkpoint_count = len.div_ceil(CHECKPOINT_STRIDE);
        let fixed_bytes = size_of::<DerivedCarets>() + checkpoint_count * size_of::<f32>();
        let dense_bytes = len.checked_mul(size_of::<f32>())?;
        if fixed_bytes >= dense_bytes {
            return None;
        }
        let mut advances: Arc<[f32]> = std::iter::repeat_n(0.0, character_count).collect();
        let (checkpoints, exceptions) = {
            let mut builder = CaretPositionsBuilder {
                advances: Arc::make_mut(&mut advances),
                checkpoints: Vec::with_capacity(checkpoint_count),
                exceptions: Vec::new(),
                index: 0,
                position: 0.0,
                previous_advance: 0.0,
                fixed_bytes,
                dense_bytes,
            };
            let terminal = produce(&mut builder)?;
            if builder.index != character_count {
                return None;
            }
            builder.push_boundary(terminal)?;
            (
                builder.checkpoints.into_boxed_slice(),
                builder.exceptions.into_boxed_slice(),
            )
        };
        let positions = Self::Derived(Arc::new(DerivedCarets {
            advances: advances.clone(),
            checkpoints,
            exceptions,
        }));
        Some((advances, positions))
    }

    /// Retain the dense representation whenever deriving exact positions would
    /// cost more space. The advances allocation is already owned by layout.
    pub fn from_advances(advances: Arc<[f32]>, positions: Arc<[f32]>) -> Self {
        let len = positions.len();
        if len < CHECKPOINT_STRIDE * 2
            || len.checked_sub(1) != Some(advances.len())
            || u32::try_from(len).is_err()
            || advances
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
            || positions.iter().any(|value| !value.is_finite())
        {
            return Self::Dense(positions);
        }
        let checkpoint_count = len.div_ceil(CHECKPOINT_STRIDE);
        let fixed_bytes = size_of::<DerivedCarets>() + checkpoint_count * size_of::<f32>();
        let dense_bytes = size_of_val(positions.as_ref());
        let mut exception_count = 0;
        let mut value = positions[0];
        for index in 1..len {
            if index % CHECKPOINT_STRIDE == 0 {
                value = positions[index];
            } else {
                value += advances[index - 1];
                if value.to_bits() != positions[index].to_bits() {
                    exception_count += 1;
                    value = positions[index];
                    if fixed_bytes + exception_count * size_of::<CaretException>() >= dense_bytes {
                        return Self::Dense(positions);
                    }
                }
            }
        }
        if fixed_bytes + exception_count * size_of::<CaretException>() >= dense_bytes {
            return Self::Dense(positions);
        }
        let mut checkpoints = Vec::with_capacity(checkpoint_count);
        let mut exceptions = Vec::with_capacity(exception_count);
        for index in 0..len {
            if index % CHECKPOINT_STRIDE == 0 {
                value = positions[index];
                checkpoints.push(value);
            } else {
                value += advances[index - 1];
                if value.to_bits() != positions[index].to_bits() {
                    value = positions[index];
                    exceptions.push(CaretException {
                        index: index as u32,
                        position: value,
                    });
                }
            }
        }
        Self::Derived(Arc::new(DerivedCarets {
            advances,
            checkpoints: checkpoints.into_boxed_slice(),
            exceptions: exceptions.into_boxed_slice(),
        }))
    }

    #[inline]
    pub fn len(&self) -> usize {
        match self {
            Self::Dense(values) => values.len(),
            Self::Derived(values) => values.advances.len() + 1,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Compact storage only has validating constructors, so its positions do
    /// not need to be reconstructed for another finite-value check.
    pub fn is_finite(&self) -> bool {
        match self {
            Self::Dense(values) => values.iter().all(|position| position.is_finite()),
            Self::Derived(_) => true,
        }
    }

    #[inline]
    pub fn get(&self, index: usize) -> Option<f32> {
        self.cursor().get(index)
    }

    /// Repeated forward queries avoid resumming each checkpoint block.
    #[inline]
    pub fn cursor(&self) -> CaretCursor<'_> {
        CaretCursor {
            positions: self,
            index: 0,
            value: match self {
                Self::Dense(_) => 0.0,
                Self::Derived(values) => values.checkpoints[0],
            },
            next_exception: 0,
        }
    }

    #[inline]
    pub fn iter(&self) -> CaretIter<'_> {
        CaretIter {
            cursor: self.cursor(),
            index: 0,
        }
    }

    pub fn to_dense(&self) -> Arc<[f32]> {
        match self {
            Self::Dense(values) => values.clone(),
            Self::Derived(_) => self.iter().collect(),
        }
    }

    /// Materialize only when indexed mutation is required. Uniquely owned
    /// dense storage can be updated without copying its allocation.
    pub fn make_dense(&mut self) -> &mut [f32] {
        if matches!(self, Self::Derived(_)) {
            *self = Self::Dense(self.to_dense());
        }
        let Self::Dense(values) = self else {
            unreachable!()
        };
        Arc::make_mut(values)
    }

    pub(crate) fn memory_usage_bytes_with_shared(&self, seen: &mut FxHashSet<*const ()>) -> usize {
        match self {
            Self::Dense(values) => {
                if seen.insert(values.as_ptr().cast()) {
                    size_of_val(values.as_ref())
                } else {
                    0
                }
            }
            Self::Derived(values) => {
                if !seen.insert(Arc::as_ptr(values).cast()) {
                    return 0;
                }
                size_of::<DerivedCarets>()
                    + size_of_val(values.checkpoints.as_ref())
                    + size_of_val(values.exceptions.as_ref())
                    + if seen.insert(values.advances.as_ptr().cast()) {
                        size_of_val(values.advances.as_ref())
                    } else {
                        0
                    }
            }
        }
    }
}

/// Receives logically ordered character geometry without a dense caret array.
/// Created by [`CaretPositions::build_compact`].
pub struct CaretPositionsBuilder<'a> {
    advances: &'a mut [f32],
    checkpoints: Vec<f32>,
    exceptions: Vec<CaretException>,
    index: usize,
    position: f32,
    previous_advance: f32,
    fixed_bytes: usize,
    dense_bytes: usize,
}

impl CaretPositionsBuilder<'_> {
    /// Append one character's exact native starting position and advance.
    /// A failed push requests the producer's existing dense fallback.
    #[inline]
    pub fn push(&mut self, position: f32, advance: f32) -> Option<()> {
        if self.index >= self.advances.len() || !advance.is_finite() || advance < 0.0 {
            return None;
        }
        let index = self.index;
        self.push_boundary(position)?;
        self.advances[index] = advance;
        self.previous_advance = advance;
        Some(())
    }

    #[inline]
    fn push_boundary(&mut self, position: f32) -> Option<()> {
        if !position.is_finite() {
            return None;
        }
        if self.index % CHECKPOINT_STRIDE == 0 {
            self.checkpoints.push(position);
        } else if (self.position + self.previous_advance).to_bits() != position.to_bits() {
            if self.fixed_bytes + (self.exceptions.len() + 1) * size_of::<CaretException>()
                >= self.dense_bytes
            {
                return None;
            }
            self.exceptions.push(CaretException {
                index: self.index as u32,
                position,
            });
        }
        self.position = position;
        self.index += 1;
        Some(())
    }
}

impl From<Arc<[f32]>> for CaretPositions {
    fn from(values: Arc<[f32]>) -> Self {
        Self::Dense(values)
    }
}

impl From<Vec<f32>> for CaretPositions {
    fn from(values: Vec<f32>) -> Self {
        Self::Dense(values.into())
    }
}

impl<const N: usize> From<[f32; N]> for CaretPositions {
    fn from(values: [f32; N]) -> Self {
        Self::Dense(values.into())
    }
}

impl PartialEq for CaretPositions {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

pub struct CaretCursor<'a> {
    positions: &'a CaretPositions,
    index: usize,
    value: f32,
    next_exception: usize,
}

impl CaretCursor<'_> {
    #[inline]
    pub fn get(&mut self, index: usize) -> Option<f32> {
        let values = match self.positions {
            CaretPositions::Dense(values) => return values.get(index).copied(),
            CaretPositions::Derived(values) => values,
        };
        if index > values.advances.len() {
            return None;
        }
        if index < self.index || index - self.index >= CHECKPOINT_STRIDE {
            self.index = index / CHECKPOINT_STRIDE * CHECKPOINT_STRIDE;
            self.value = values.checkpoints[self.index / CHECKPOINT_STRIDE];
            self.next_exception = values
                .exceptions
                .partition_point(|exception| exception.index as usize <= self.index);
        }
        while self.index < index {
            self.index += 1;
            if self.index % CHECKPOINT_STRIDE == 0 {
                self.value = values.checkpoints[self.index / CHECKPOINT_STRIDE];
            } else {
                self.value += values.advances[self.index - 1];
                if let Some(exception) = values.exceptions.get(self.next_exception)
                    && exception.index as usize == self.index
                {
                    self.value = exception.position;
                    self.next_exception += 1;
                }
            }
        }
        Some(self.value)
    }
}

pub struct CaretIter<'a> {
    cursor: CaretCursor<'a>,
    index: usize,
}

impl Iterator for CaretIter<'_> {
    type Item = f32;
    #[inline]
    fn next(&mut self) -> Option<f32> {
        let value = self.cursor.get(self.index)?;
        self.index += 1;
        Some(value)
    }
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.cursor.positions.len() - self.index;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for CaretIter<'_> {}
