use std::cmp::Reverse;
use std::mem::size_of;

#[derive(Clone, Debug)]
pub struct MemoryUsageEntry {
    pub label: String,
    pub bytes: usize,
    pub count: usize,
}

#[derive(Default, Clone, Debug)]
pub struct MemoryUsageReport {
    pub entries: Vec<MemoryUsageEntry>,
}

impl MemoryUsageReport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, label: impl Into<String>, bytes: usize, count: usize) {
        self.entries.push(MemoryUsageEntry { label: label.into(), bytes, count });
    }

    pub fn add_struct<T>(&mut self, label: impl Into<String>, count: usize) {
        self.add(label, size_of::<T>() * count, count);
    }

    pub fn add_slice_storage<T>(&mut self, label: impl Into<String>, capacity: usize, len: usize) {
        self.add(label, size_of::<T>() * capacity, len);
    }

    pub fn add_value<T>(&mut self, label: impl Into<String>, count: usize) {
        self.add_struct::<T>(label, count);
    }

    pub fn extend_prefixed(&mut self, prefix: impl AsRef<str>, report: MemoryUsageReport) {
        let prefix = prefix.as_ref();
        for entry in report.entries {
            let label = if prefix.is_empty() { entry.label } else { format!("{prefix}.{}", entry.label) };
            self.entries.push(MemoryUsageEntry { label, bytes: entry.bytes, count: entry.count });
        }
    }

    pub fn total_bytes(&self) -> usize {
        self.entries.iter().map(|entry| entry.bytes).sum()
    }

    pub fn sort_by_bytes_desc(&mut self) {
        self.entries.sort_by_key(|entry| Reverse(entry.bytes));
    }
}

pub trait MemoryUsage {
    fn memory_usage_report(&self) -> MemoryUsageReport;

    fn memory_usage_bytes(&self) -> usize {
        self.memory_usage_report().total_bytes()
    }
}
