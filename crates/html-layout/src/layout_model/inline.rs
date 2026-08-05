use html_dom::MemoryUsageReport;
use std::ops::Range;

/// Width of a single cell in the inline glyph buffer.
///
/// Each cell holds a Unicode scalar value before shaping and a glyph-cache
/// index after shaping (see [`crate::text::glyph_cache`]). Both roles share
/// this one type so the buffer width can be changed in a single place.
///
/// `u32` so the buffer holds full Unicode scalar values (no BMP truncation of
/// emoji / supplementary-plane CJK) and the glyph-cache index has no 65 536-entry
/// ceiling.
pub type GlyphId = u32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WhitespaceWrapOverride {
    #[default]
    Style,
    Allow,
    Suppress,
}

impl WhitespaceWrapOverride {
    pub(crate) fn retained(allow: bool) -> Self {
        if allow { Self::Allow } else { Self::Style }
    }
}

#[derive(Default, Clone)]
pub(crate) struct InlineContent {
    pub(super) glyphs: Vec<GlyphId>,
    /// UTF-16 source offset within `InlineItem::dom_text_node` for each glyph.
    /// Synthetic glyphs such as list markers use `u32::MAX`.
    pub(super) glyph_source_offsets: Vec<u32>,
    /// A soft-wrap opportunity retained from the pre-collapse whitespace
    /// sequence. This is separate from the surviving glyph's style because a
    /// sequence can cross inline elements with different `white-space` values.
    pub(super) whitespace_wrap_before: Vec<WhitespaceWrapOverride>,
    pub(super) inline_items: Vec<InlineItem>,
}

impl InlineContent {
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<GlyphId>("InlineContent.glyphs.storage", self.glyphs.capacity(), self.glyphs.len());
        report.add_slice_storage::<u32>("InlineContent.glyph_source_offsets.storage", self.glyph_source_offsets.capacity(), self.glyph_source_offsets.len());
        report.add_slice_storage::<WhitespaceWrapOverride>("InlineContent.whitespace_wrap_before.storage", self.whitespace_wrap_before.capacity(), self.whitespace_wrap_before.len());
        report.add_slice_storage::<InlineItem>("InlineContent.inline_items.storage", self.inline_items.capacity(), self.inline_items.len());
        report
    }
}

impl InlineContent {
    pub(crate) fn glyphs(&self) -> &[GlyphId] {
        self.glyphs.as_slice()
    }

    pub(crate) fn glyphs_mut(&mut self) -> &mut [GlyphId] {
        self.glyphs.as_mut_slice()
    }

    pub(crate) fn glyph_at(&self, glyph_idx: usize) -> Option<GlyphId> {
        self.glyphs.get(glyph_idx).copied()
    }

    pub(crate) fn glyph_source_offset(&self, glyph_idx: usize) -> Option<u32> {
        self.glyph_source_offsets.get(glyph_idx).copied().filter(|offset| *offset != u32::MAX)
    }

    pub(crate) fn raw_glyph_source_offset(&self, glyph_idx: usize) -> u32 {
        self.glyph_source_offsets.get(glyph_idx).copied().unwrap_or(u32::MAX)
    }

    pub(crate) fn whitespace_wrap_before(&self, glyph_idx: usize) -> WhitespaceWrapOverride {
        self.whitespace_wrap_before.get(glyph_idx).copied().unwrap_or_default()
    }

    pub(crate) fn replace_glyphs(&mut self, glyphs: Vec<GlyphId>, source_offsets: Vec<u32>, whitespace_wrap_before: Vec<WhitespaceWrapOverride>) {
        debug_assert_eq!(glyphs.len(), source_offsets.len());
        debug_assert_eq!(glyphs.len(), whitespace_wrap_before.len());
        self.glyphs = glyphs;
        self.glyph_source_offsets = source_offsets;
        self.whitespace_wrap_before = whitespace_wrap_before;
    }

    pub(crate) fn inline_items(&self) -> &[InlineItem] {
        self.inline_items.as_slice()
    }

    pub(crate) fn inline_items_mut(&mut self) -> &mut [InlineItem] {
        self.inline_items.as_mut_slice()
    }

    pub(crate) fn inline_item(&self, run_idx: usize) -> Option<&InlineItem> {
        self.inline_items.get(run_idx)
    }

    pub(crate) fn inline_items_empty(&self) -> bool {
        self.inline_items.is_empty()
    }

    pub(crate) fn push_inline_item(&mut self, run: InlineItem) {
        self.inline_items.push(run);
    }

    pub(crate) fn push_glyph(&mut self, glyph: GlyphId) {
        self.glyphs.push(glyph);
        self.glyph_source_offsets.push(u32::MAX);
        self.whitespace_wrap_before.push(WhitespaceWrapOverride::Style);
    }

    pub(crate) fn push_source_glyph(&mut self, glyph: GlyphId, source_offset: u32) {
        self.glyphs.push(glyph);
        self.glyph_source_offsets.push(source_offset);
        self.whitespace_wrap_before.push(WhitespaceWrapOverride::Style);
    }
}

#[derive(Clone)]
pub(crate) struct InlineItem {
    pub kind: InlineItemKind,
    pub box_idx: u32,
    pub dom_text_node: Option<u32>, // DOM text node that produced this item (None for synthetic/non-text items)
}

impl InlineItem {
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        MemoryUsageReport::new()
    }
}

#[derive(Clone)]
pub(crate) enum InlineItemKind {
    Text {
        glyphs: Range<u32>,
    },
    Image {
        image_idx: u32,
    },
    Break {
        clear: html_style_model::Clear,
    },
    FloatAnchor {
        box_idx: u32,
    },
    /// Static-position anchor for an absolutely positioned descendant. Unlike
    /// a float this never contributes an intrinsic width or an exclusion.
    AbsoluteAnchor {
        box_idx: u32,
    },
    /// An otherwise empty fragment created when a block descendant splits an
    /// inline box. The marker keeps the fragment's start/end edge geometry in
    /// inline layout without inventing selectable text.
    InlineBoundary {
        inline_start: bool,
        inline_end: bool,
    },
    /// An inline-level formatting context (`inline-flex`, `inline-grid`, or
    /// `inline-table`). Its descendants are laid out by their own algorithm
    /// and the resulting border box participates in the surrounding line as
    /// one indivisible inline item.
    AtomicBox {
        box_idx: u32,
    },
    /// Generated list-item marker text (the `::marker` content). Holds glyphs
    /// like `Text`, but is synthetic: it is excluded from selection, search, and
    /// CFI so it never appears in the document's addressable text, matching how
    /// browsers keep marker content out of the DOM.
    Marker {
        glyphs: Range<u32>,
    },
}

impl InlineItemKind {
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        MemoryUsageReport::new()
    }
}
