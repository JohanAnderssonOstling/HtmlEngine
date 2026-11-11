//! Dense DOM columns. Variable-length lists are moved into shared arrays at
//! finalization; scalar columns are indexed by element or text index throughout.
use super::{DocumentBuildError, DomAttribute, DomNodeId, HtmlElementState, MemoryUsageReport};
use std::num::NonZeroU32;
use std::ops::Range;

/// All 65,536 string slots remain representable. Presence costs one bit rather
/// than a second u16 in every optional slot.
#[derive(Clone, Default)]
pub(super) struct OptionalStrings {
    values: Vec<u16>,
    present: Vec<u64>,
}

impl OptionalStrings {
    pub(super) fn push(&mut self, value: Option<u16>) {
        let index = self.values.len();
        if index % 64 == 0 {
            self.present.push(0);
        }
        self.values.push(value.unwrap_or(0));
        self.set(index, value);
    }

    pub(super) fn set(&mut self, index: usize, value: Option<u16>) {
        let bit = 1 << (index % 64);
        self.present[index / 64] &= !bit;
        if let Some(value) = value {
            self.values[index] = value;
            self.present[index / 64] |= bit;
        }
    }

    pub(super) fn get(&self, index: usize) -> Option<u16> {
        (self.present[index / 64] & (1 << (index % 64)) != 0).then(|| self.values[index])
    }

    fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_vec_storage("values", &self.values);
        report.add_vec_storage("present", &self.present);
        report
    }
}

#[derive(Clone)]
pub(super) struct ElementName {
    pub tag: u16,
    pub namespace: Option<u16>,
}

#[derive(Clone)]
pub(super) struct FilteredRange<T> {
    pub range: Range<T>,
    pub filter: u32,
}

impl<T> FilteredRange<T> {
    pub(super) fn new(range: Range<T>) -> Self {
        Self {
            range,
            filter: u32::MAX,
        }
    }
}

pub(super) trait ListRange {
    fn new(range: Range<u32>) -> Self;
    fn range(&self) -> &Range<u32>;
}

impl ListRange for Range<u32> {
    fn new(range: Range<u32>) -> Self {
        range
    }
    fn range(&self) -> &Range<u32> {
        self
    }
}

impl ListRange for FilteredRange<u32> {
    fn new(range: Range<u32>) -> Self {
        Self::new(range)
    }
    fn range(&self) -> &Range<u32> {
        &self.range
    }
}

#[derive(Clone)]
pub(super) enum Lists<T, R = Range<u32>> {
    Building(Vec<Vec<T>>),
    Flat { ranges: Vec<R>, values: Vec<T> },
}

impl<T, R> Default for Lists<T, R> {
    fn default() -> Self {
        Self::Building(Vec::new())
    }
}

impl<T, R: ListRange> Lists<T, R> {
    pub(super) fn push(&mut self, values: Vec<T>) {
        let Self::Building(lists) = self else {
            unreachable!("finalized DOM lists are immutable")
        };
        lists.push(values);
    }

    pub(super) fn get(&self, index: usize) -> &[T] {
        match self {
            Self::Building(lists) => &lists[index],
            Self::Flat { ranges, values } => {
                let range = ranges[index].range();
                &values[range.start as usize..range.end as usize]
            }
        }
    }

    pub(super) fn get_mut_for_build(&mut self, index: usize) -> &mut Vec<T> {
        let Self::Building(lists) = self else {
            unreachable!("finalized DOM lists are immutable")
        };
        &mut lists[index]
    }

    pub(super) fn finish(&mut self) -> Result<(), DocumentBuildError> {
        let Self::Building(lists) = self else {
            return Ok(());
        };
        let count = lists
            .iter()
            .try_fold(0u32, |total, list| {
                total.checked_add(u32::try_from(list.len()).ok()?)
            })
            .ok_or(DocumentBuildError::TooManyListEntries)?;
        let mut ranges = Vec::with_capacity(lists.len());
        let mut values = Vec::with_capacity(count as usize);
        for list in lists.iter_mut() {
            let start = values.len() as u32;
            values.append(list);
            ranges.push(R::new(start..values.len() as u32));
        }
        *self = Self::Flat { ranges, values };
        Ok(())
    }

    fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        match self {
            Self::Building(lists) => {
                report.add_vec_storage("pending", lists);
                for list in lists {
                    report.add_vec_storage("entries", list);
                }
            }
            Self::Flat { ranges, values } => {
                report.add_vec_storage("ranges", ranges);
                report.add_vec_storage("entries", values);
            }
        }
        report
    }
}

impl<T> Lists<T, FilteredRange<u32>> {
    pub(super) fn filter(&self, index: usize) -> u32 {
        match self {
            Self::Building(_) => u32::MAX,
            Self::Flat { ranges, .. } => ranges[index].filter,
        }
    }

    pub(super) fn set_filter(&mut self, index: usize, filter: u32) {
        let Self::Flat { ranges, .. } = self else {
            unreachable!("selector filters are computed after finalizing DOM lists")
        };
        ranges[index].filter = filter;
    }
}

#[derive(Clone, Default)]
pub(super) struct Elements {
    pub names: Vec<ElementName>,
    pub parents: Vec<Option<NonZeroU32>>,
    pub children: Lists<DomNodeId>,
    pub attributes: Lists<DomAttribute, FilteredRange<u32>>,
    pub ids: OptionalStrings,
    pub classes: Vec<FilteredRange<u16>>,
    pub states: Vec<HtmlElementState>,
    pub images: Vec<Option<NonZeroU32>>,
}

impl Elements {
    pub(super) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_vec_storage("names", &self.names);
        report.add_vec_storage("parents", &self.parents);
        report.extend_prefixed("children", self.children.memory_usage_report());
        report.extend_prefixed("attributes", self.attributes.memory_usage_report());
        report.extend_prefixed("ids", self.ids.memory_usage_report());
        report.add_vec_storage("classes", &self.classes);
        report.add_vec_storage("states", &self.states);
        report.add_vec_storage("images", &self.images);
        report
    }
}

#[derive(Clone, Default)]
pub(super) struct Texts {
    pub values: Vec<String>,
    pub parents: Vec<Option<NonZeroU32>>,
}

impl Texts {
    pub(super) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_vec_storage("values", &self.values);
        report.add_vec_storage("parents", &self.parents);
        for text in &self.values {
            report.add_string_storage("bytes", text);
        }
        report
    }
}
