pub mod cfi;
mod dom;
mod memory;
mod resources;
mod toc;

pub use dom::*;
pub use memory::*;
pub use resources::*;
pub use toc::*;

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone)]
pub struct DocumentLineage(NonZeroU64);

static NEXT_DOCUMENT_LINEAGE: AtomicU64 = AtomicU64::new(1);

impl DocumentLineage {
    fn new() -> Self {
        let raw = NEXT_DOCUMENT_LINEAGE.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add(1)).expect("document lineage identity space exhausted");
        Self(NonZeroU64::new(raw).expect("document lineage IDs start at one"))
    }

    pub fn matches(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentMode {
    Html,
    Xml,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(transparent)]
pub struct RootFontSize(f32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidRootFontSize;

impl RootFontSize {
    pub fn new(value: f32) -> Result<Self, InvalidRootFontSize> {
        if value.is_finite() && value > 0.0 { Ok(Self(value)) } else { Err(InvalidRootFontSize) }
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

impl Default for RootFontSize {
    fn default() -> Self {
        Self(16.0)
    }
}

impl std::fmt::Display for InvalidRootFontSize {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("root font size must be finite and positive")
    }
}

impl std::error::Error for InvalidRootFontSize {}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode(&self) -> DocumentMode {
        self.mode
    }

    pub fn lineage(&self) -> &DocumentLineage {
        &self.lineage
    }

    pub fn set_mode(&mut self, mode: DocumentMode) {
        self.mode = mode;
    }

    pub fn root_font_size(&self) -> f32 {
        self.root_font_size.get()
    }

    pub fn set_root_font_size(&mut self, root_font_size: RootFontSize) {
        self.root_font_size = root_font_size;
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn set_title(&mut self, title: Option<String>) {
        self.title = title;
    }

    pub fn intern_string(&mut self, value: &str) -> u16 {
        self.strings.intern(value)
    }

    pub fn string(&self, idx: u16) -> &str {
        self.strings.get(idx)
    }

    pub fn lookup_string(&self, value: &str) -> Option<u16> {
        self.strings.lookup(value)
    }

    pub fn images(&self) -> &[ImageResource] {
        self.images.as_slice()
    }

    pub fn images_mut(&mut self) -> &mut [ImageResource] {
        self.images.as_mut_slice()
    }

    pub fn image(&self, image_idx: u32) -> Option<&ImageResource> {
        self.images.get(image_idx as usize)
    }

    pub fn push_image(&mut self, image: ImageResource) -> u32 {
        let image_idx = self.images.len() as u32;
        self.images.push(image);
        image_idx
    }

    pub fn toc_entries(&self) -> &[DocumentTocEntry] {
        self.toc_entries.as_slice()
    }

    pub fn document_toc_entries(&self) -> &[DocumentTocNode] {
        self.document_toc_entries.as_slice()
    }

    pub fn rebuild_document_toc_entries(&mut self) {
        self.document_toc_entries = build_document_toc_tree(self.toc_entries.as_slice());
    }

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<DomNode>("Document.dom_nodes.storage", self.dom_nodes.capacity(), self.dom_nodes.len());
        for node in &self.dom_nodes {
            report.extend_prefixed("Document.dom_nodes", node.memory_usage_report());
        }
        report.add_slice_storage::<ImageResource>("Document.images.storage", self.images.capacity(), self.images.len());
        for image in &self.images {
            report.extend_prefixed("Document.images", image.memory_usage_report());
        }
        if let Some(title) = &self.title {
            report.add_slice_storage::<u8>("Document.title.storage", title.capacity(), title.len());
        }
        report.extend_prefixed("Document.strings", self.strings.memory_usage_report());
        report.add_slice_storage::<u16>("Document.class_list.storage", self.class_list.capacity(), self.class_list.len());
        report.add_slice_storage::<DocumentTocEntry>("Document.toc_entries.storage", self.toc_entries.capacity(), self.toc_entries.len());
        report.add_slice_storage::<DocumentTocNode>("Document.document_toc_entries.storage", self.document_toc_entries.capacity(), self.document_toc_entries.len());
        for node in &self.document_toc_entries {
            report.extend_prefixed("Document.document_toc_entries", node.memory_usage_report());
        }
        report
    }
}

pub struct Document {
    // Stable identity shared only with derived stage data. This makes it
    // impossible to accidentally pair styles from another parsed document.
    lineage: DocumentLineage,

    // DOM tree (phase 1: parsing)
    dom_nodes: Vec<DomNode>,
    dom_root: Option<DomNodeId>,
    mode: DocumentMode,

    // Resources
    images: Vec<ImageResource>,
    title: Option<String>,

    // Interning
    strings: StringInterner,
    class_list: Vec<u16>, // interned class indices

    // Document-level navigation
    toc_entries: Vec<DocumentTocEntry>,
    document_toc_entries: Vec<DocumentTocNode>,

    // Style configuration input; computed styles live in the style-stage output.
    root_font_size: RootFontSize,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            lineage: DocumentLineage::new(),
            dom_nodes: Vec::new(),
            dom_root: None,
            mode: DocumentMode::Html,
            images: Vec::new(),
            title: None,
            strings: StringInterner::new(),
            class_list: Vec::new(),
            toc_entries: Vec::new(),
            document_toc_entries: Vec::new(),
            root_font_size: RootFontSize::default(),
        }
    }
}

#[cfg(test)]
mod invariant_tests {
    use super::RootFontSize;

    #[test]
    fn root_font_size_rejects_values_that_poison_style_math() {
        assert!(RootFontSize::new(12.0).is_ok());
        assert!(RootFontSize::new(0.0).is_err());
        assert!(RootFontSize::new(-1.0).is_err());
        assert!(RootFontSize::new(f32::NAN).is_err());
        assert!(RootFontSize::new(f32::INFINITY).is_err());
    }
}
