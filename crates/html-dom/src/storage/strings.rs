use crate::MemoryUsageReport;
use rustc_data_structures::fx::FxIndexSet;

/// Insertion-ordered string storage with one allocation per distinct value.
/// Domain wrappers assign their own ID types to the stable numeric indexes.
#[derive(Default)]
pub struct StringTable {
    values: FxIndexSet<Box<str>>,
}

impl StringTable {
    pub fn intern(&mut self, value: &str) -> usize {
        if let Some(index) = self.lookup(value) {
            return index;
        }
        self.values.insert_full(value.into()).0
    }

    pub fn lookup(&self, value: &str) -> Option<usize> {
        self.values.get_index_of(value)
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.values.get_index(index).map(Box::as_ref)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Publish indexed values in insertion order, releasing the lookup index
    /// and moving each string's allocation into the immutable result.
    pub fn into_strings(self) -> Vec<Box<str>> {
        self.values.into_iter().collect()
    }

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        // Estimate entry/hash-index storage from the table's public capacity;
        // each entry stores a hash and one string, and the index stores offsets.
        report.add_slice_storage::<(usize, Box<str>)>(
            "entries.storage",
            self.values.capacity(),
            self.values.len(),
        );
        report.add_slice_storage::<usize>(
            "index.storage",
            self.values.capacity(),
            self.values.len(),
        );
        for value in &self.values {
            report.add("contents", value.len(), value.len());
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::StringTable;

    #[test]
    fn stable_ids_and_string_allocations_survive_growth_and_publication() {
        let mut table = StringTable::default();
        assert!(table.is_empty());
        let empty = table.intern("");
        let unicode = table.intern("Å😀漢");
        let original = table.get(unicode).unwrap().as_ptr();
        assert_eq!(table.intern("Å😀漢"), unicode);
        assert_eq!(table.lookup("Å😀漢"), Some(unicode));
        assert_eq!(table.lookup("missing"), None);

        for index in 0..2048 {
            let value = format!("value-{index}");
            let id = table.intern(&value);
            assert_eq!(id, index + 2);
            assert_eq!(table.get(id), Some(value.as_str()));
            assert_eq!(table.lookup(&value), Some(id));
            assert_eq!(table.intern(&value), id);
        }
        assert_eq!(table.len(), 2050);
        assert_eq!(table.get(table.len()), None);
        assert_eq!(table.get(unicode).unwrap().as_ptr(), original);

        let strings = table.into_strings();
        assert_eq!(strings[empty].as_ref(), "");
        assert_eq!(strings[unicode].as_ref(), "Å😀漢");
        assert_eq!(strings[unicode].as_ptr(), original);
        for (index, value) in strings[2..].iter().enumerate() {
            assert_eq!(value.as_ref(), format!("value-{index}"));
        }
    }
}
