use std::cmp::Reverse;
use std::collections::HashMap;
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

    /// Count slice elements, excluding spare capacity in their backing storage.
    pub fn add_slice<T>(&mut self, label: impl Into<String>, values: &[T]) {
        self.add_slice_storage::<T>(label, values.len(), values.len());
    }

    /// Count a vector's allocated buffer, including spare capacity.
    pub fn add_vec_storage<T>(&mut self, label: impl Into<String>, values: &Vec<T>) {
        self.add_slice_storage::<T>(label, values.capacity(), values.len());
    }

    /// Estimate map entry payloads, excluding bucket metadata and nested allocations.
    pub fn add_hash_map_storage<K, V, S>(
        &mut self,
        label: impl Into<String>,
        values: &HashMap<K, V, S>,
    ) {
        self.add_slice_storage::<(K, V)>(label, values.capacity(), values.len());
    }

    /// Count a string's byte buffer, including spare capacity.
    pub fn add_string_storage(&mut self, label: impl Into<String>, value: &String) {
        self.add(label, value.capacity(), value.len());
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

#[cfg(test)]
mod tests {
    use super::MemoryUsageReport;
    use std::collections::HashMap;
    use std::mem::size_of;

    #[test]
    fn container_storage_preserves_capacity_length_and_empty_entries() {
        let mut values = Vec::with_capacity(16);
        values.extend([1u32, 2]);
        let mut report = MemoryUsageReport::new();
        report.add_vec_storage("reserved", &values);
        report.add_vec_storage("empty", &Vec::<u8>::new());
        report.add_vec_storage("zero-sized", &vec![(); 3]);
        report.add_slice("slice", &values[1..]);
        report.add_slice("empty-slice", &values[..0]);
        report.add_slice("zero-sized-slice", &[(); 3]);
        let mut map = HashMap::with_capacity(16);
        map.insert(1u16, 2u32);
        report.add_hash_map_storage("map", &map);
        report.add_hash_map_storage("empty-map", &HashMap::<u8, u8>::new());
        let mut text = String::with_capacity(32);
        text.push_str("café");
        report.add_string_storage("string", &text);
        report.add_string_storage("empty-string", &String::new());
        let entries: Vec<_> = report
            .entries
            .iter()
            .map(|entry| (entry.label.as_str(), entry.bytes, entry.count))
            .collect();
        assert_eq!(
            entries,
            [
                ("reserved", values.capacity() * size_of::<u32>(), 2),
                ("empty", 0, 0),
                ("zero-sized", 0, 3),
                ("slice", size_of::<u32>(), 1),
                ("empty-slice", 0, 0),
                ("zero-sized-slice", 0, 3),
                ("map", map.capacity() * size_of::<(u16, u32)>(), 1),
                ("empty-map", 0, 0),
                ("string", text.capacity(), text.len()),
                ("empty-string", 0, 0),
            ]
        );
    }
}
