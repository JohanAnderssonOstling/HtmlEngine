use super::{MemoryUsage, MemoryUsageReport};
use rustc_data_structures::fx::FxHashMap;
use std::sync::Arc;

#[derive(Clone)]
pub enum ImageSource {
    Uri(String),
    Inline(Arc<[u8]>),
}

impl MemoryUsage for ImageSource {
    fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        match self {
            ImageSource::Uri(uri) => {
                report.add_slice_storage::<u8>("ImageSource::Uri.storage", uri.capacity(), uri.len());
            }
            ImageSource::Inline(bytes) => {
                report.add_slice_storage::<u8>("ImageSource::Inline.storage", bytes.len(), bytes.len());
            }
        }
        report
    }
}

#[derive(Clone)]
pub struct ImageResource {
    pub source: ImageSource,
    pub width: u32,
    pub height: u32,
    pub width_attr: Option<u32>,
    pub height_attr: Option<u32>,
}

impl MemoryUsage for ImageResource {
    fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.extend_prefixed("ImageResource.source", self.source.memory_usage_report());
        report
    }
}

impl ImageResource {
    pub fn display_size(&self) -> (f64, f64) {
        let width = self.width_attr.unwrap_or(self.width) as f64;
        let height = self.height_attr.unwrap_or(self.height) as f64;
        (width.max(1.0), height.max(1.0))
    }
}

#[derive(Default)]
pub struct StringInterner {
    map: FxHashMap<String, u16>,
    strings: Vec<String>,
}

impl MemoryUsage for StringInterner {
    fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<(String, u16)>("StringInterner.map.storage", self.map.capacity(), self.map.len());
        for value in &self.strings {
            report.add_slice_storage::<u8>("StringInterner.strings.content", value.capacity(), value.len());
        }
        report.add_slice_storage::<String>("StringInterner.strings.storage", self.strings.capacity(), self.strings.len());
        report
    }
}

impl StringInterner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, s: &str) -> u16 {
        if let Some(&idx) = self.map.get(s) {
            return idx;
        }
        let idx = self.strings.len() as u16;
        self.strings.push(s.to_string());
        self.map.insert(s.to_string(), idx);
        idx
    }

    pub fn get(&self, idx: u16) -> &str {
        &self.strings[idx as usize]
    }

    pub fn lookup(&self, s: &str) -> Option<u16> {
        self.map.get(s).copied()
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }
}
