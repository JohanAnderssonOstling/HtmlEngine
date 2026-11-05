use crate::source_offsets::SourceOffsets;
use crate::{PseudoStyles, TextStructure, whitespace_context_root};
use html_dom::MemoryUsageReport;
use std::ops::Range;
use std::sync::OnceLock;

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

/// Two policies share a byte; trailing default policies need no bytes.
/// The legacy slice API expands lazily, while engine queries stay packed.
#[derive(Default, Clone)]
struct WrapPolicies {
    bytes: Vec<u8>,
    len: usize,
    expanded: OnceLock<Vec<WhitespaceWrapOverride>>,
}

impl WrapPolicies {
    #[inline]
    fn get(&self, index: usize) -> WhitespaceWrapOverride {
        if index >= self.len {
            return WhitespaceWrapOverride::Style;
        }
        let value = (self.bytes.get(index / 2).copied().unwrap_or(0) >> ((index % 2) * 4)) & 15;
        match value {
            0 => WhitespaceWrapOverride::Style,
            1 => WhitespaceWrapOverride::Allow,
            2 => WhitespaceWrapOverride::Suppress,
            3 => WhitespaceWrapOverride::UnicodeAllow,
            4 => WhitespaceWrapOverride::UnicodeSuppress,
            _ => unreachable!("validated wrap policy"),
        }
    }
    fn push_default(&mut self) {
        self.expanded.take();
        self.len += 1;
    }
    fn from_values(values: Vec<WhitespaceWrapOverride>) -> Self {
        let mut result = Self {
            len: values.len(),
            ..Self::default()
        };
        let Some(last) = values
            .iter()
            .rposition(|&value| value != WhitespaceWrapOverride::Style)
        else {
            return result;
        };
        result.bytes = values[..=last]
            .chunks(2)
            .map(|chunk| (chunk[0] as u8) | (chunk.get(1).copied().unwrap_or_default() as u8) << 4)
            .collect();
        result
    }
    fn as_slice(&self) -> &[WhitespaceWrapOverride] {
        self.expanded
            .get_or_init(|| (0..self.len).map(|index| self.get(index)).collect())
    }
}

/// One preparation pass owns the policy buffer. Allocate and invalidate its
/// expanded view once, then update byte-aligned fields directly.
pub(crate) struct UnicodeWrapWriter<'a> {
    policies: &'a mut WrapPolicies,
    unicode_wraps: &'a mut bool,
}

impl UnicodeWrapWriter<'_> {
    #[inline]
    pub(crate) fn get(&self, index: usize) -> WhitespaceWrapOverride {
        self.policies.get(index)
    }
    #[inline]
    pub(crate) fn set(&mut self, index: usize, allow: bool) {
        assert!(index < self.policies.len);
        *self.unicode_wraps = true;
        let shift = (index % 2) * 4;
        let value = if allow { 3 } else { 4 };
        let byte = &mut self.policies.bytes[index / 2];
        *byte = (*byte & !(15 << shift)) | (value << shift);
    }
}

/// Canonical logical text and source correspondence, owned by preparation.
/// Characters are stored as validated Unicode scalar values.
/// Shaping borrows this content and writes glyph IDs into a separate buffer;
/// it never copies or modifies the source offsets, wrap flags, or inline items.
#[derive(Default)]
#[cfg_attr(any(test, feature = "testing"), derive(Clone))]
pub struct InlineContent {
    unicode_wraps: bool,
    pub(crate) pseudo_styles: PseudoStyles,
    characters: Vec<char>,
    /// UTF-16 source offset within `InlineItem::dom_text_node` for each glyph.
    /// Synthetic characters such as list markers have no source position.
    glyph_source_offsets: SourceOffsets,
    /// A soft-wrap opportunity retained from the pre-collapse whitespace
    /// sequence. This is separate from the surviving glyph's style because a
    /// sequence can cross inline elements with different `white-space` values.
    /// Unicode/CSS boundaries in CJK paragraphs reuse this same byte.
    whitespace_wrap_before: WrapPolicies,
    inline_items: Vec<InlineItem>,
}

impl InlineContent {
    pub fn pseudo_styles(&self) -> &PseudoStyles {
        &self.pseudo_styles
    }

    pub fn has_unicode_wraps(&self) -> bool {
        self.unicode_wraps
    }
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = self.pseudo_styles.memory_usage_report();
        report.add_vec_storage("InlineContent.characters.storage", &self.characters);
        report.extend_prefixed(
            "InlineContent.glyph_source_offsets",
            self.glyph_source_offsets.memory_usage_report(),
        );
        report.add(
            "InlineContent.whitespace_wrap_before.storage",
            self.whitespace_wrap_before.bytes.capacity(),
            self.whitespace_wrap_before.len,
        );
        if let Some(expanded) = self.whitespace_wrap_before.expanded.get() {
            report.add_vec_storage("InlineContent.whitespace_wrap_before.expanded", expanded);
        }
        report.add_vec_storage("InlineContent.inline_items.storage", &self.inline_items);
        report
    }
}

impl InlineContent {
    pub fn characters(&self) -> &[char] {
        self.characters.as_slice()
    }

    pub fn character_at(&self, glyph_idx: usize) -> Option<char> {
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

    /// Prepared break policy in logical character order.
    pub fn whitespace_wrap_overrides(&self) -> &[WhitespaceWrapOverride] {
        self.whitespace_wrap_before.as_slice()
    }

    #[inline]
    pub fn whitespace_wrap_before(&self, glyph_idx: usize) -> WhitespaceWrapOverride {
        self.whitespace_wrap_before.get(glyph_idx)
    }

    /// Scan a validated source range without expanding the legacy slice view.
    pub fn has_unicode_wrap_opportunity(&self, source: Range<usize>) -> bool {
        let policies = &self.whitespace_wrap_before;
        if source.start >= source.end || source.end > policies.len {
            return false;
        }
        let mut start = source.start;
        if start % 2 != 0 {
            if policies.get(start) == WhitespaceWrapOverride::UnicodeAllow {
                return true;
            }
            start += 1;
        }
        let end = source.end & !1;
        let byte_start = start / 2;
        if byte_start >= policies.bytes.len() {
            return false;
        }
        let byte_end = (end / 2).min(policies.bytes.len());
        if policies.bytes[byte_start..byte_end]
            .iter()
            .any(|&byte| byte & 15 == 3 || byte >> 4 == 3)
        {
            return true;
        }
        end < source.end && policies.get(end) == WhitespaceWrapOverride::UnicodeAllow
    }

    pub(crate) fn unicode_wrap_writer(&mut self) -> UnicodeWrapWriter<'_> {
        let policies = &mut self.whitespace_wrap_before;
        policies.expanded.take();
        let bytes = policies.len.div_ceil(2);
        if policies.bytes.capacity() < bytes {
            policies.bytes.reserve_exact(bytes - policies.bytes.len());
        }
        policies.bytes.resize(bytes, 0);
        UnicodeWrapWriter {
            policies,
            unicode_wraps: &mut self.unicode_wraps,
        }
    }

    pub(crate) fn replace_characters(
        &mut self,
        characters: Vec<char>,
        source_offsets: SourceOffsets,
        whitespace_wrap_before: Vec<WhitespaceWrapOverride>,
    ) {
        debug_assert_eq!(characters.len(), source_offsets.len());
        debug_assert_eq!(characters.len(), whitespace_wrap_before.len());
        self.characters = characters;
        self.glyph_source_offsets = source_offsets;
        self.whitespace_wrap_before = WrapPolicies::from_values(whitespace_wrap_before);
        self.unicode_wraps = false;
    }

    pub fn inline_items(&self) -> &[InlineItem] {
        self.inline_items.as_slice()
    }

    /// Items that affect logical whitespace and word boundaries, in source order.
    /// Formatting edges and out-of-flow anchors are transparent to these passes.
    pub(crate) fn logical_flow_items(&self) -> impl Iterator<Item = &InlineItem> {
        self.inline_items.iter().filter(|item| {
            !matches!(
                item.kind,
                InlineItemKind::FloatAnchor { .. }
                    | InlineItemKind::AbsoluteAnchor { .. }
                    | InlineItemKind::InlineBoundary { .. }
            )
        })
    }

    /// Borrow ordinary text and generated marker runs in source order,
    /// including empty ranges. Structural events stay in `inline_items`.
    pub fn text_runs(&self) -> impl DoubleEndedIterator<Item = InlineTextRun<'_>> + Clone {
        self.inline_items.iter().filter_map(InlineItem::text_run)
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
        self.push_source_character(character, u32::MAX);
    }

    pub fn push_source_character(&mut self, character: char, source_offset: u32) {
        self.characters.push(character);
        self.glyph_source_offsets.push(source_offset);
        self.whitespace_wrap_before.push_default();
    }
}

#[derive(Clone)]
pub struct InlineItem {
    pub kind: InlineItemKind,
    pub box_idx: u32,
    pub dom_text_node: Option<u32>, // DOM text node that produced this item (None for synthetic/non-text items)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InlineTextKind {
    Text,
    Marker,
}

/// Borrowed logical text metadata, independent of shaping and layout output.
#[derive(Clone, Copy, Debug)]
pub struct InlineTextRun<'a> {
    pub kind: InlineTextKind,
    pub glyphs: &'a Range<u32>,
    pub box_idx: u32,
    pub dom_text_node: Option<u32>,
}

impl InlineItem {
    /// Atomic content belongs to the surrounding flow, not its own descendants.
    pub fn context_owner(&self, tree: &(impl TextStructure + ?Sized)) -> usize {
        let owner = self.box_idx as usize;
        match self.kind {
            InlineItemKind::Image { .. } | InlineItemKind::AtomicBox { .. } => {
                tree.get_box_parent(owner).unwrap_or(owner)
            }
            _ => owner,
        }
    }

    pub fn flow_root(&self, tree: &(impl TextStructure + ?Sized)) -> usize {
        whitespace_context_root(tree, self.context_owner(tree))
    }

    /// Markers form independent Unicode paragraphs beside their principal box.
    pub fn paragraph_root(&self, tree: &(impl TextStructure + ?Sized)) -> usize {
        if matches!(self.kind, InlineItemKind::Marker { .. }) {
            self.box_idx as usize
        } else {
            self.flow_root(tree)
        }
    }

    pub fn text_run(&self) -> Option<InlineTextRun<'_>> {
        let (kind, glyphs) = match &self.kind {
            InlineItemKind::Text { glyphs } => (InlineTextKind::Text, glyphs),
            InlineItemKind::Marker { glyphs } => (InlineTextKind::Marker, glyphs),
            _ => return None,
        };
        Some(InlineTextRun {
            kind,
            glyphs,
            box_idx: self.box_idx,
            dom_text_node: self.dom_text_node,
        })
    }
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

#[cfg(test)]
mod tests {
    use super::{InlineContent, InlineItem, InlineItemKind, InlineTextKind};
    use html_style_model::Clear;
    use std::ptr;

    #[test]
    fn text_views_borrow_ranges_and_keep_empty_runs_markers_and_source_order() {
        let mut content = InlineContent::default();
        for character in "ab•".chars() {
            content.push_character(character);
        }
        for (kind, box_idx, dom_text_node) in [
            (InlineItemKind::Text { glyphs: 0..0 }, 1, Some(9)),
            (InlineItemKind::Image { image_idx: 0 }, 2, None),
            (InlineItemKind::Text { glyphs: 0..2 }, 1, Some(9)),
            (InlineItemKind::Break { clear: Clear::None }, 1, None),
            (InlineItemKind::Marker { glyphs: 2..3 }, 3, None),
        ] {
            content.push_inline_item(InlineItem {
                kind,
                box_idx,
                dom_text_node,
            });
        }
        let runs = content.text_runs().collect::<Vec<_>>();
        assert_eq!(
            runs.iter()
                .map(|run| (run.kind, run.glyphs.clone(), run.box_idx, run.dom_text_node))
                .collect::<Vec<_>>(),
            [
                (InlineTextKind::Text, 0..0, 1, Some(9)),
                (InlineTextKind::Text, 0..2, 1, Some(9)),
                (InlineTextKind::Marker, 2..3, 3, None),
            ]
        );
        assert_eq!(
            content
                .text_runs()
                .rev()
                .map(|run| run.glyphs.clone())
                .collect::<Vec<_>>(),
            [2..3, 0..2, 0..0]
        );
        let InlineItemKind::Text { glyphs } = &content.inline_items()[2].kind else {
            panic!("text item")
        };
        assert!(ptr::eq(runs[1].glyphs, glyphs));
    }
}
