//! Paragraph bidi resolution over logical text and CSS inline boundaries.
//!
//! Synthetic directional controls participate in UAX #9 without entering the
//! source glyph arena, so addressing continues to use the original text order.

use crate::{InlineContent, InlineItem, InlineItemKind, LayoutMode, LayoutTree};
use html_style_model::{ComputedStyles, TextDirection, UnicodeBidi};
use html_text::get_style;
use html_text::{is_bidi_control};
use rustc_data_structures::fx::FxHashMap;
use std::ops::Range;
use std::sync::Arc;
use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, Default)]
pub struct ResolvedBidi {
    levels: Vec<u8>,
    items: FxHashMap<(u32, u8), u8>,
}

#[derive(Default)]
struct Paragraph {
    text: String,
    glyphs: Vec<(u32, usize)>,
    items: Vec<((u32, u8), usize)>,
    ancestors: Vec<usize>,
}

impl ResolvedBidi {
    pub fn new(styles: &ComputedStyles, tree: &LayoutTree, content: &InlineContent) -> Self {
        let rtl_text = content
            .characters()
            .iter()
            .copied()
            .any(|character| {
                character > '\u{007f}'
                    && (matches!(
                        bidi_class(character),
                        BidiClass::R | BidiClass::AL | BidiClass::AN
                    ) || is_bidi_control(character))
            });
        if !rtl_text
            && !(0..tree.box_count())
                .any(|owner| get_style(styles, tree, owner).direction() == TextDirection::Rtl)
        {
            return Self::default();
        }
        let mut paragraphs = FxHashMap::<usize, Paragraph>::default();
        for item in content.inline_items() {
            if matches!(
                item.kind,
                InlineItemKind::FloatAnchor { .. } | InlineItemKind::AbsoluteAnchor { .. }
            ) {
                continue;
            }
            let context_owner = item.context_owner(tree);
            let root = item.paragraph_root(tree);
            let paragraph = paragraphs.entry(root).or_default();
            let mut ancestors = vec![context_owner];
            while *ancestors.last().expect("bidi owner") != root {
                let Some(parent) = tree.get_box_parent(*ancestors.last().expect("bidi owner"))
                else {
                    break;
                };
                ancestors.push(parent);
            }
            ancestors.reverse();
            paragraph.enter(&ancestors, styles, tree);
            match &item.kind {
                InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => {
                    for index in glyphs.clone() {
                        let character = content
                            .character_at(index as usize)
                            .unwrap_or('\u{fffd}');
                        if character == '\n' {
                            paragraph.close(styles, tree);
                        }
                        paragraph.glyphs.push((index, paragraph.text.len()));
                        paragraph.text.push(character);
                        if character == '\n' {
                            paragraph.open(styles, tree);
                        }
                    }
                }
                InlineItemKind::Break { .. } => {
                    paragraph.close(styles, tree);
                    paragraph.text.push('\n');
                    paragraph.open(styles, tree);
                }
                _ => {
                    paragraph.items.push((item_key(item), paragraph.text.len()));
                    // Empty inline edges must not become strong/neutral text.
                    paragraph.text.push(
                        if matches!(item.kind, InlineItemKind::InlineBoundary { .. }) {
                            '\u{2060}'
                        } else {
                            '\u{fffc}'
                        },
                    );
                }
            }
        }
        let mut result = Self {
            levels: vec![0; content.characters().len()],
            items: FxHashMap::default(),
        };
        for (root, mut paragraph) in paragraphs {
            paragraph.close(styles, tree);
            let style = get_style(styles, tree, root);
            let base = (paragraph_unicode_bidi(styles, tree, root) != UnicodeBidi::Plaintext)
                .then(|| direction_level(style.direction()));
            let info = BidiInfo::new(&paragraph.text, base);
            for (glyph, byte) in paragraph.glyphs {
                let paragraph = info
                    .paragraphs
                    .partition_point(|paragraph| paragraph.range.end <= byte);
                let base = info.paragraphs[paragraph].level.number();
                result.levels[glyph as usize] = info.levels[byte].number() | (base << 7);
            }
            for (item, byte) in paragraph.items {
                if info.levels[byte].number() != 0 {
                    result.items.insert(item, info.levels[byte].number());
                }
            }
        }
        // Ordinary LTR documents retain no per-character bidi allocation.
        if result.levels.iter().all(|&level| level == 0) {
            result.levels = Vec::new();
        }
        result
    }

    pub fn level(&self, index: u32) -> u8 {
        self.levels.get(index as usize).copied().unwrap_or(0) & 0x7f
    }

    /// Consecutive logical character ranges with the same embedding level.
    /// Unstored levels retain the default LTR value without scanning the tail.
    pub fn level_runs(
        &self,
        mut range: Range<u32>,
    ) -> impl Iterator<Item = (Range<u32>, u8)> + Clone + '_ {
        let stored_end = range
            .end
            .min(u32::try_from(self.levels.len()).unwrap_or(u32::MAX));
        std::iter::from_fn(move || {
            if range.is_empty() {
                return None;
            }
            let start = range.start;
            let level = self.level(start);
            let end = (start + 1..stored_end)
                .find(|&index| self.level(index) != level)
                .unwrap_or(if level == 0 { range.end } else { stored_end });
            range.start = end;
            Some((start..end, level))
        })
    }

    pub fn paragraph_direction(&self, index: u32) -> TextDirection {
        if self.levels.get(index as usize).copied().unwrap_or(0) & 0x80 != 0 {
            TextDirection::Rtl
        } else {
            TextDirection::Ltr
        }
    }

    pub fn item_level(&self, item: &InlineItem) -> u8 {
        self.items.get(&item_key(item)).copied().unwrap_or(0)
    }

    pub fn memory_usage_bytes(&self) -> usize {
        self.levels.capacity() + self.items.capacity() * std::mem::size_of::<((u32, u8), u8)>()
    }
}

fn item_key(item: &InlineItem) -> (u32, u8) {
    (
        item.box_idx,
        match item.kind {
            InlineItemKind::InlineBoundary {
                inline_start,
                inline_end,
            } => 1 + u8::from(inline_start) + 2 * u8::from(inline_end),
            _ => 0,
        },
    )
}

pub fn grapheme_boundaries(text: &str, character_count: usize) -> Arc<[bool]> {
    profile_scope!("shape.grapheme_boundaries");
    // Shaping already owns one entry per scalar; reuse its count rather than
    // scanning the UTF-8 text again just to size this array.
    debug_assert_eq!(text.chars().count(), character_count);
    let mut boundaries = vec![true; character_count + 1];
    if text.is_ascii() {
        // Every ASCII scalar stands alone except the CR/LF pair. This also
        // covers control characters without entering the Unicode iterator.
        for (index, pair) in text.as_bytes().windows(2).enumerate() {
            if pair == b"\r\n" {
                boundaries[index + 1] = false;
            }
        }
        return boundaries.into();
    }

    let mut index = 0;
    for grapheme in text.graphemes(true) {
        if grapheme.len() == 1 {
            // Most graphemes in mixed Latin text are single-byte ASCII.
            index += 1;
        } else {
            let scalars = grapheme.chars().count();
            boundaries[index + 1..index + scalars].fill(false);
            index += scalars;
        }
    }
    debug_assert_eq!(index, character_count);
    boundaries.into()
}

fn direction_level(direction: TextDirection) -> Level {
    match direction {
        TextDirection::Ltr => Level::ltr(),
        TextDirection::Rtl => Level::rtl(),
    }
}

/// Anonymous blocks are fragments of their parent's immediate inline content.
/// They keep its paragraph override/plaintext context without making the CSS
/// property inherit onto actual block descendants.
pub fn paragraph_unicode_bidi(
    styles: &ComputedStyles,
    tree: &LayoutTree,
    mut owner: usize,
) -> UnicodeBidi {
    let mut anonymous = false;
    while matches!(
        tree.box_at(owner).map(|box_| box_.layout_mode()),
        Some(LayoutMode::Anonymous(_))
    ) {
        let Some(parent) = tree.get_box_parent(owner) else {
            break;
        };
        anonymous = true;
        owner = parent;
    }
    let value = get_style(styles, tree, owner).unicode_bidi();
    if anonymous
        && !matches!(
            value,
            UnicodeBidi::Plaintext | UnicodeBidi::BidiOverride | UnicodeBidi::IsolateOverride
        )
    {
        UnicodeBidi::Normal
    } else {
        value
    }
}

impl Paragraph {
    fn enter(&mut self, ancestors: &[usize], styles: &ComputedStyles, tree: &LayoutTree) {
        let shared = self
            .ancestors
            .iter()
            .zip(ancestors)
            .take_while(|(a, b)| a == b)
            .count();
        for &owner in self.ancestors[shared..].iter().rev() {
            self.text.push_str(
                controls(
                    paragraph_unicode_bidi(styles, tree, owner),
                    get_style(styles, tree, owner).direction(),
                    self.ancestors.first() == Some(&owner),
                )
                .1,
            );
        }
        self.ancestors.truncate(shared);
        for &owner in &ancestors[shared..] {
            let style = get_style(styles, tree, owner);
            self.text.push_str(
                controls(
                    paragraph_unicode_bidi(styles, tree, owner),
                    style.direction(),
                    ancestors.first() == Some(&owner),
                )
                .0,
            );
            self.ancestors.push(owner);
        }
    }

    fn close(&mut self, styles: &ComputedStyles, tree: &LayoutTree) {
        for &owner in self.ancestors.iter().rev() {
            let style = get_style(styles, tree, owner);
            self.text.push_str(
                controls(
                    paragraph_unicode_bidi(styles, tree, owner),
                    style.direction(),
                    self.ancestors.first() == Some(&owner),
                )
                .1,
            );
        }
    }

    fn open(&mut self, styles: &ComputedStyles, tree: &LayoutTree) {
        for &owner in &self.ancestors {
            let style = get_style(styles, tree, owner);
            self.text.push_str(
                controls(
                    paragraph_unicode_bidi(styles, tree, owner),
                    style.direction(),
                    self.ancestors.first() == Some(&owner),
                )
                .0,
            );
        }
    }
}

fn controls(
    bidi: UnicodeBidi,
    direction: TextDirection,
    root: bool,
) -> (&'static str, &'static str) {
    let bidi = if root {
        match bidi {
            UnicodeBidi::BidiOverride | UnicodeBidi::IsolateOverride => UnicodeBidi::BidiOverride,
            _ => UnicodeBidi::Normal,
        }
    } else {
        bidi
    };
    match (bidi, direction) {
        (UnicodeBidi::Normal, _) => ("", ""),
        (UnicodeBidi::Embed, TextDirection::Ltr) => ("\u{202a}", "\u{202c}"),
        (UnicodeBidi::Embed, TextDirection::Rtl) => ("\u{202b}", "\u{202c}"),
        (UnicodeBidi::Isolate, TextDirection::Ltr) => ("\u{2066}", "\u{2069}"),
        (UnicodeBidi::Isolate, TextDirection::Rtl) => ("\u{2067}", "\u{2069}"),
        (UnicodeBidi::BidiOverride, TextDirection::Ltr) => ("\u{202d}", "\u{202c}"),
        (UnicodeBidi::BidiOverride, TextDirection::Rtl) => ("\u{202e}", "\u{202c}"),
        (UnicodeBidi::IsolateOverride, TextDirection::Ltr) => {
            ("\u{2068}\u{202d}", "\u{202c}\u{2069}")
        }
        (UnicodeBidi::IsolateOverride, TextDirection::Rtl) => {
            ("\u{2068}\u{202e}", "\u{202c}\u{2069}")
        }
        (UnicodeBidi::Plaintext, _) => ("\u{2068}", "\u{2069}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{ResolvedBidi, grapheme_boundaries};
    use std::iter::once;
    use std::ops::Range;
    use unicode_segmentation::UnicodeSegmentation;

    #[test]
    fn level_runs_preserve_embedding_levels_subranges_and_unstored_ltr_tails() {
        let bidi = ResolvedBidi {
            levels: vec![0, 0x80, 1, 0x81, 2, 0x82, 0],
            ..Default::default()
        };
        assert_eq!(
            bidi.level_runs(0..7).collect::<Vec<_>>(),
            [(0..2, 0), (2..4, 1), (4..6, 2), (6..7, 0)]
        );
        assert_eq!(
            bidi.level_runs(1..5).collect::<Vec<_>>(),
            [(1..2, 0), (2..4, 1), (4..5, 2)]
        );
        assert_eq!(
            bidi.level_runs(5..u32::MAX).collect::<Vec<_>>(),
            [(5..6, 2), (6..u32::MAX, 0)]
        );
        assert_eq!(
            ResolvedBidi::default()
                .level_runs(0..u32::MAX)
                .collect::<Vec<_>>(),
            [(0..u32::MAX, 0)]
        );
        assert!(bidi.level_runs(2..2).next().is_none());
        assert!(bidi.level_runs(Range { start: 6, end: 5 }).next().is_none());
    }

    fn assert_unicode_boundaries(text: &str) {
        // Compare scalar-indexed output with the library's byte-indexed
        // boundaries, including the terminal boundary for empty input.
        let bytes: Vec<_> = text
            .grapheme_indices(true)
            .map(|(byte, _)| byte)
            .chain(once(text.len()))
            .collect();
        let expected: Vec<_> = text
            .char_indices()
            .map(|(byte, _)| bytes.binary_search(&byte).is_ok())
            .chain(once(true))
            .collect();
        assert_eq!(
            grapheme_boundaries(text, text.chars().count()).as_ref(),
            expected,
            "{text:?}"
        );
    }

    #[test]
    fn ascii_boundaries_match_unicode_for_every_pair_including_crlf() {
        assert_unicode_boundaries("");
        for first in 0..=127u8 {
            for second in 0..=127u8 {
                let pair = [first, second];
                assert_unicode_boundaries(std::str::from_utf8(&pair).unwrap());
            }
        }
        for text in ["plain ASCII", "a\r\nb", "\r\r\n\n", "\r\n\r\n"] {
            assert_unicode_boundaries(text);
        }
    }

    #[test]
    fn mixed_unicode_boundaries_keep_scalar_indices_and_context() {
        for text in [
            "e\u{301}fi 👩\u{200d}💻 🇸🇪",
            "🇸🇪🇫🇮🇳",
            "👩🏽\u{200d}💻",
            "क्\u{200d}ष",
            "각",
            "\u{600}a\u{903}",
            "é\r\ne\u{301}",
        ] {
            assert_unicode_boundaries(text);
        }
        let parts = [
            "a", "\r", "\n", "\u{301}", "\u{600}", "\u{903}", "é", "क", "्", "ष", "👩", "\u{200d}",
            "💻", "🇸", "🇪", "ᄀ", "ᅡ", "ᆨ",
        ];
        for first in parts {
            for second in parts {
                for third in parts {
                    assert_unicode_boundaries(&format!("{first}{second}{third}"));
                }
            }
        }
    }
}
