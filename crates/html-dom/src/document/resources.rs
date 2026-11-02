use super::{MemoryUsage, MemoryUsageReport};
use crate::storage::StringTable;
use std::sync::Arc;

#[derive(Clone, PartialEq)]
pub enum ImageSource {
    Uri(String),
    /// Original MathML retained beside its generated, self-contained SVG.
    /// The baseline is a fraction of the SVG height, measured from its top.
    MathMl {
        markup: Arc<str>,
        svg: Arc<[u8]>,
        baseline: f32,
    },
    /// Serialized inline SVG plus the effective base URI needed to resolve
    /// resources referenced from inside the SVG.
    InlineSvg {
        bytes: Arc<[u8]>,
        base_uri: String,
    },
}

impl MemoryUsage for ImageSource {
    fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        match self {
            ImageSource::Uri(uri) => {
                report.add_string_storage("ImageSource::Uri.storage", uri);
            }
            ImageSource::MathMl { markup, svg, .. } => {
                report.add_slice("ImageSource::MathMl.markup", markup.as_bytes());
                report.add_slice("ImageSource::MathMl.svg", svg);
            }
            ImageSource::InlineSvg { bytes, base_uri } => {
                report.add_slice("ImageSource::InlineSvg.bytes", bytes);
                report.add_string_storage("ImageSource::InlineSvg.base_uri", base_uri);
            }
        }
        report
    }
}

#[derive(Clone, PartialEq)]
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

#[derive(Default)]
pub struct StringInterner {
    strings: StringTable,
}

impl MemoryUsage for StringInterner {
    fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.extend_prefixed("StringInterner", self.strings.memory_usage_report());
        report
    }
}

impl StringInterner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, s: &str) -> u16 {
        self.strings.intern(s) as u16
    }

    pub fn get(&self, idx: u16) -> &str {
        self.strings.get(idx as usize).expect("registered DOM string")
    }

    pub fn lookup(&self, s: &str) -> Option<u16> {
        self.strings.lookup(s).map(|index| index as u16)
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }
}
