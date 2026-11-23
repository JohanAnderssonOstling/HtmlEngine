//! Shared hyphenation policy and dictionary lookup used by preparation and composition.
use unicode_segmentation::UnicodeSegmentation;
use super::{InlineItem, LayoutReadContext};
use html_style_model::language::content_language_tag;
use html_style_model::{
    ContentLanguage, Hyphens, LineBreak, WordBreak,
};
use hypher::{Lang as HyphenationLanguage, hyphenate, hyphenate_bounded};
use unicode_categories::UnicodeCategories;
use unicode_normalization::UnicodeNormalization;

/// Explicit inherited tags take precedence over publication metadata. English
/// is used only when neither source declares a language.
pub(super) fn hyphenation_language(
    explicit: Option<&ContentLanguage>,
    publication: Option<&ContentLanguage>,
) -> Option<HyphenationLanguage> {
    let Some(language) = explicit.or(publication) else {
        return Some(HyphenationLanguage::English);
    };
    let primary = content_language_tag(language)?.primary_language();
    if primary.len() != 2 {
        return None;
    }
    let bytes = primary.as_bytes();
    HyphenationLanguage::from_iso([bytes[0].to_ascii_lowercase(), bytes[1].to_ascii_lowercase()])
}

#[derive(PartialEq)]
pub(super) struct WordPolicy<'a> {
    pub(super) language: Option<&'a ContentLanguage>,
    pub(super) dictionary: Option<HyphenationLanguage>,
    pub(super) enabled: bool,
}

impl<'input> WordPolicy<'input> {
    pub(super) fn for_run(inputs: &LayoutReadContext<'input>, run: &InlineItem) -> Self {
        let styles = inputs.reader.styles();
        let style = inputs.reader.style(run.box_idx as usize);
        let language = style.language().and_then(|id| styles.language(id));
        let fallback = styles.hyphenation_fallback_language();
        let reader_selected_auto = inputs
            .reader
            .layout_tree()
            .get_box_style_indices(run.box_idx as usize)
            .and_then(|indices| styles.view(indices))
            .is_some_and(|style| style.text.reader_hyphens == Some(Hyphens::Auto));
        // An unknown web-document language must not select an arbitrary
        // dictionary. Keep the reader's explicit/default English policy.
        let dictionary = (language.is_some()
            || fallback.is_some()
            || reader_selected_auto
            || inputs.constraints.book_optimized_text())
        .then(|| hyphenation_language(language, fallback))
        .flatten();
        Self {
            language: language.or(fallback),
            dictionary,
            enabled: style.hyphens() == Hyphens::Auto
                && style.white_space().allows_wrap()
                && style.word_break() != WordBreak::BreakAll
                && style.line_break() != LineBreak::Anywhere
                && inputs.text.hyphen_glyph(run.box_idx as usize).is_some(),
        }
    }
}

/// Translate dictionary byte offsets for one contiguous logical word. Both
/// eager indexed matching and deferred matching keep native cluster checks.
pub(super) fn for_each_word_break(
    inputs: &LayoutReadContext<'_>,
    dictionary: &mut HyphenationDictionaryCache,
    word: &str,
    start: u32,
    language: HyphenationLanguage,
    mut emit: impl FnMut(u32),
) {
    let ascii = word.is_ascii();
    let mut previous_byte = 0;
    let mut glyph = start;
    let text = inputs.text;
    for_each_cached_word_break(inputs, dictionary, word, language, |offset| {
        let after = if ascii {
            start + offset as u32
        } else {
            glyph += word[previous_byte..offset].chars().count() as u32;
            previous_byte = offset;
            glyph
        };
        if text.is_cluster_boundary(after as usize) {
            emit(after - 1);
        }
    });
}

/// Exact ASCII dictionary results retained across passes by layout scratch.
/// Empty slots have a zero tag; eligible word lengths are always nonzero.
#[derive(Clone, Copy, Default)]
struct DictionaryEntry {
    word: [u64; 2],
    breaks: u16,
    tag: u16,
}

// Layout scratch owns this bounded lazy table; its active executor updates it exclusively.
// Copy a hit's mask before emission; no table borrow crosses the callback.
#[derive(Default)]
pub(crate) struct HyphenationDictionaryCache {
    entries: Option<Box<[DictionaryEntry; 256]>>,
}

pub(super) fn for_each_cached_word_break(
    inputs: &LayoutReadContext<'_>,
    dictionary: &mut HyphenationDictionaryCache,
    word: &str,
    language: HyphenationLanguage,
    mut emit: impl FnMut(usize),
) {
    let quality = inputs.constraints.hyphenation_quality();
    if !(5..=16).contains(&word.len()) || !word.is_ascii() {
        for offset in word_breaks(word, language, quality) {
            emit(offset);
        }
        return;
    }
    let mut bytes = [0; 16];
    bytes[..word.len()].copy_from_slice(word.as_bytes());
    let key = [
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        u64::from_le_bytes(bytes[8..].try_into().unwrap()),
    ];
    let tag = word.len() as u16 | (language as u16) << 5 | u16::from(quality) << 13;
    let hash = key[0]
        ^ key[1].rotate_left(23)
        ^ (word.len() as u64) << 56
        ^ (language as u64) << 48
        ^ u64::from(quality) << 63;
    let slot = (hash.wrapping_mul(0x9e3779b97f4a7c15) >> 56) as usize;
    let entries = dictionary
        .entries
        .get_or_insert_with(|| Box::new([DictionaryEntry::default(); 256]));
    let entry = &mut entries[slot];
    if entry.tag == tag && entry.word == key {
        let mut mask = entry.breaks;
        while mask != 0 {
            emit(mask.trailing_zeros() as usize);
            mask &= mask - 1;
        }
        return;
    }
    let result = word_breaks(word, language, quality);
    let mask = result.iter().fold(0, |mask, &offset| mask | 1u16 << offset);
    *entry = DictionaryEntry {
        word: key,
        breaks: mask,
        tag,
    };
    for offset in result {
        emit(offset);
    }
}

fn is_apostrophe(character: char) -> bool {
    matches!(character, '\'' | '\u{2019}' | '\u{02bc}')
}

/// Return UTF-8 offsets in the original word, using NFC only for dictionary
/// lookup. Each offset ends a complete grapheme, so decomposed accents stay
/// attached and source text/selection offsets never change.
fn word_breaks(word: &str, language: HyphenationLanguage, book_quality: bool) -> Vec<usize> {
    // Unicode word segmentation keeps a trailing word joiner with the word.
    // It suppresses wrapping at that edge, not dictionary breaks inside the
    // preceding letters. Trimming only the suffix preserves source offsets.
    let word = word.trim_end_matches(['\u{2060}', '\u{feff}']);
    let minimum_letters = if book_quality { 5 } else { 4 };
    if word.len() < minimum_letters {
        return Vec::new();
    }
    // ASCII normalization and source-offset mapping are identities. Validate
    // and count letters together, borrowing the original word for the lookup.
    let mut letters = 0;
    for &byte in word.as_bytes() {
        if byte.is_ascii_alphabetic() {
            letters += 1;
        } else if byte == b'\'' {
            continue;
        } else if byte.is_ascii() {
            return Vec::new();
        } else {
            return unicode_word_breaks(word, language, book_quality);
        }
    }
    if letters < minimum_letters {
        return Vec::new();
    }
    // ASCII byte and character counts agree. If the language's required
    // prefix and suffix cannot both fit, no dictionary pattern can split it.
    let (left_min, right_min) = language.bounds();
    if word.len() < left_min + right_min {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut byte_offset = 0;
    let mut prefix_letters = 0;
    let mut syllables = hyphenate_bounded(word, language, left_min, right_min).peekable();
    while let Some(syllable) = syllables.next() {
        byte_offset += syllable.len();
        if syllables.peek().is_none() {
            break;
        }
        if book_quality {
            prefix_letters += syllable.bytes().filter(|&byte| byte != b'\'').count();
        }
        if word.as_bytes()[byte_offset - 1] != b'\''
            && word.as_bytes()[byte_offset] != b'\''
            && (!book_quality
                || automatic_hyphen_fragments_are_long_enough(prefix_letters, letters))
        {
            result.push(byte_offset);
        }
    }
    result
}

fn unicode_word_breaks(
    word: &str,
    language: HyphenationLanguage,
    book_quality: bool,
) -> Vec<usize> {
    let mut normalized = String::new();
    let mut positions = Vec::new();
    let mut letters = 0;
    for (start, grapheme) in word.grapheme_indices(true) {
        let mut valid = true;
        let mut has_letter = false;
        // Feed validation and letter counting through normalization's input
        // iterator, keeping the original grapheme boundaries for source offsets.
        let characters = grapheme.chars().inspect(|&ch| {
            let apostrophe = is_apostrophe(ch);
            let alphabetic = ch.is_alphabetic();
            has_letter |= alphabetic && !apostrophe;
            valid &= alphabetic || apostrophe || ch.is_mark();
        });
        normalized.extend(
            characters
                .nfc()
                .map(|ch| if is_apostrophe(ch) { '\'' } else { ch }),
        );
        if !valid {
            // Includes authored soft hyphens, whose breakpoints take precedence
            // across the entire word, including other styled runs.
            return Vec::new();
        }
        letters += usize::from(has_letter);
        positions.push((normalized.len(), start + grapheme.len(), letters));
    }
    if letters < if book_quality { 5 } else { 4 } {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut byte_offset = 0;
    let mut syllables = hyphenate(&normalized, language).peekable();
    while let Some(syllable) = syllables.next() {
        byte_offset += syllable.len();
        if syllables.peek().is_none() {
            break;
        }
        let position = positions.partition_point(|(end, _, _)| *end < byte_offset);
        if let Some(&(end, original_end, prefix_letters)) = positions.get(position)
            && end == byte_offset
            && !normalized[..end].ends_with('\'')
            && !normalized[end..].starts_with('\'')
            && (!book_quality
                || automatic_hyphen_fragments_are_long_enough(prefix_letters, letters))
        {
            result.push(original_end);
        }
    }
    result
}

pub(super) fn automatic_hyphen_fragments_are_long_enough(prefix: usize, total: usize) -> bool {
    prefix >= 2 && total.saturating_sub(prefix) >= 2
}

#[cfg(test)]
mod tests {
    use super::{HyphenationLanguage, word_breaks};
    use unicode_normalization::UnicodeNormalization;

    #[test]
    fn hyphenation_normalizes_accents_without_changing_source_offsets() {
        let composed = "électroencéphalographie";
        let decomposed = composed.nfd().collect::<String>();
        for quality in [false, true] {
            let original = word_breaks(composed, HyphenationLanguage::French, quality);
            let decomposed_breaks = word_breaks(&decomposed, HyphenationLanguage::French, quality);
            assert!(!original.is_empty());
            let prefixes = |word: &str, offsets: Vec<usize>| {
                offsets
                    .into_iter()
                    .map(|end| word[..end].nfc().collect::<String>())
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                prefixes(composed, original),
                prefixes(&decomposed, decomposed_breaks.clone())
            );
            for end in decomposed_breaks {
                assert!(
                    decomposed
                        .grapheme_indices(true)
                        .any(|(start, grapheme)| start + grapheme.len() == end)
                );
            }
        }
    }

    #[test]
    fn hyphenation_keeps_internal_apostrophes_in_the_word() {
        let word = "internationalization's";
        let expected = word_breaks(word, HyphenationLanguage::English, true);
        assert!(!expected.is_empty());
        for apostrophe in ['\'', '\u{2019}', '\u{02bc}'] {
            let variant = word.replace('\'', &apostrophe.to_string());
            let actual = word_breaks(&variant, HyphenationLanguage::English, true);
            assert_eq!(actual, expected);
            assert!(actual.into_iter().all(|offset| {
                !variant[..offset].ends_with(apostrophe)
                    && !variant[offset..].starts_with(apostrophe)
            }));
        }
    }

    #[test]
    fn hyphenation_preserves_authored_soft_hyphens_and_avoids_identifiers() {
        for word in [
            "inter\u{00ad}nationalization",
            "internationalization2",
            "internationalization_name",
        ] {
            assert!(word_breaks(word, HyphenationLanguage::English, false).is_empty());
        }
    }
}
