use crate::source_offsets::SourceOffsets;
use html_dom::MemoryUsageReport;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WhitespaceWrapOverride {
    #[default]
    Style,
    Allow,
    Suppress,
    /// A Unicode/CSS break before this retained character. Unlike whitespace,
    /// the character must remain on the following line.
    UnicodeAllow,
    /// Unicode/CSS evaluated this boundary and prohibited a normal break.
    UnicodeSuppress,
}

impl WhitespaceWrapOverride {
    pub fn retained(allow: bool) -> Self {
        if allow { Self::Allow } else { Self::Style }
    }
}

const _: () = assert!(std::mem::size_of::<WhitespaceWrapOverride>() == 1);

/// Canonical logical text and source correspondence, owned by preparation.
/// Shaping borrows this content and writes glyph IDs into a separate buffer;
/// it never copies or modifies the source offsets, wrap flags, or inline items.
#[derive(Default)]
#[cfg_attr(any(test, feature = "testing"), derive(Clone))]
pub struct InlineContent {
    unicode_wraps: bool,
    characters: Vec<u32>,
    /// UTF-16 source offset within `InlineItem::dom_text_node` for each glyph.
    /// Synthetic characters such as list markers have no source position.
    glyph_source_offsets: SourceOffsets,
    /// A soft-wrap opportunity retained from the pre-collapse whitespace
    /// sequence. This is separate from the surviving glyph's style because a
    /// sequence can cross inline elements with different `white-space` values.
    /// Unicode/CSS boundaries in CJK paragraphs reuse this same byte.
    whitespace_wrap_before: Vec<WhitespaceWrapOverride>,
    inline_items: Vec<InlineItem>,
}

impl InlineContent {
    pub fn has_unicode_wraps(&self) -> bool {
        self.unicode_wraps
    }
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<u32>(
            "InlineContent.characters.storage",
            self.characters.capacity(),
            self.characters.len(),
        );
        report.extend_prefixed(
            "InlineContent.glyph_source_offsets",
            self.glyph_source_offsets.memory_usage_report(),
        );
        report.add_slice_storage::<WhitespaceWrapOverride>(
            "InlineContent.whitespace_wrap_before.storage",
            self.whitespace_wrap_before.capacity(),
            self.whitespace_wrap_before.len(),
        );
        report.add_slice_storage::<InlineItem>(
            "InlineContent.inline_items.storage",
            self.inline_items.capacity(),
            self.inline_items.len(),
        );
        report
    }
}

impl InlineContent {
    pub fn characters(&self) -> &[u32] {
        self.characters.as_slice()
    }

    pub fn character_at(&self, glyph_idx: usize) -> Option<u32> {
        self.characters.get(glyph_idx).copied()
    }

    pub fn glyph_source_offset(&self, glyph_idx: usize) -> Option<u32> {
        self.glyph_source_offsets
            .get(glyph_idx)
            .filter(|offset| *offset != u32::MAX)
    }

    pub fn raw_glyph_source_offset(&self, glyph_idx: usize) -> u32 {
        self.glyph_source_offsets.get(glyph_idx).unwrap_or(u32::MAX)
    }

    pub fn whitespace_wrap_before(&self, glyph_idx: usize) -> WhitespaceWrapOverride {
        self.whitespace_wrap_before
            .get(glyph_idx)
            .copied()
            .unwrap_or_default()
    }

    pub(crate) fn set_unicode_wrap_before(&mut self, glyph_idx: usize, allow: bool) {
        self.unicode_wraps = true;
        self.whitespace_wrap_before[glyph_idx] = if allow {
            WhitespaceWrapOverride::UnicodeAllow
        } else {
            WhitespaceWrapOverride::UnicodeSuppress
        };
    }

    pub(crate) fn replace_characters(
        &mut self,
        characters: Vec<u32>,
        source_offsets: SourceOffsets,
        whitespace_wrap_before: Vec<WhitespaceWrapOverride>,
    ) {
        debug_assert_eq!(characters.len(), source_offsets.len());
        debug_assert_eq!(characters.len(), whitespace_wrap_before.len());
        self.characters = characters;
        self.glyph_source_offsets = source_offsets;
        self.whitespace_wrap_before = whitespace_wrap_before;
        self.unicode_wraps = false;
    }

    pub fn inline_items(&self) -> &[InlineItem] {
        self.inline_items.as_slice()
    }

    pub fn inline_items_mut(&mut self) -> &mut [InlineItem] {
        self.inline_items.as_mut_slice()
    }

    pub fn inline_item(&self, run_idx: usize) -> Option<&InlineItem> {
        self.inline_items.get(run_idx)
    }

    pub fn inline_items_empty(&self) -> bool {
        self.inline_items.is_empty()
    }

    pub fn push_inline_item(&mut self, run: InlineItem) {
        self.inline_items.push(run);
    }

    pub fn push_character(&mut self, character: char) {
        self.characters.push(character as u32);
        self.glyph_source_offsets.push(u32::MAX);
        self.whitespace_wrap_before
            .push(WhitespaceWrapOverride::Style);
    }

    pub fn push_source_character(&mut self, character: char, source_offset: u32) {
        self.characters.push(character as u32);
        self.glyph_source_offsets.push(source_offset);
        self.whitespace_wrap_before
            .push(WhitespaceWrapOverride::Style);
    }
}

#[derive(Clone)]
pub struct InlineItem {
    pub kind: InlineItemKind,
    pub box_idx: u32,
    pub dom_text_node: Option<u32>, // DOM text node that produced this item (None for synthetic/non-text items)
}

#[derive(Clone)]
pub enum InlineItemKind {
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
