use super::{InlineItem, InlineItemKind};
use crate::layout::LayoutEngine;
use html_style_model::{ContentLanguage, Hyphens, LineBreak, WordBreak};
use hypher::Lang as HyphenationLanguage;
use rustc_data_structures::fx::FxHashSet;
use unicode_categories::UnicodeCategories;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// Explicit inherited tags take precedence over publication metadata. English
/// is used only when neither source declares a language.
pub(super) fn hyphenation_language(
    explicit: Option<&ContentLanguage>,
    publication: Option<&ContentLanguage>,
) -> Option<HyphenationLanguage> {
    let Some(language) = explicit.or(publication) else {
        return Some(HyphenationLanguage::English);
    };
    let primary = language.tag()?.primary_language();
    if primary.len() != 2 {
        return None;
    }
    let bytes = primary.as_bytes();
    HyphenationLanguage::from_iso([bytes[0].to_ascii_lowercase(), bytes[1].to_ascii_lowercase()])
}

#[derive(PartialEq)]
struct WordPolicy<'a> {
    language: Option<&'a ContentLanguage>,
    dictionary: Option<HyphenationLanguage>,
    enabled: bool,
}

struct SourceCharacter {
    byte_end: usize,
    glyph: u32,
    policy: usize,
}

#[derive(Default)]
struct TextGroup {
    text: String,
    source: Vec<SourceCharacter>,
}

impl TextGroup {
    fn flush(
        &mut self,
        engine: &LayoutEngine<'_, '_>,
        policies: &[WordPolicy<'_>],
        breaks: &mut FxHashSet<u32>,
    ) {
        for (start, word) in self.text.unicode_word_indices() {
            let first = self
                .source
                .partition_point(|source| source.byte_end <= start);
            let end = self
                .source
                .partition_point(|source| source.byte_end <= start + word.len());
            let source = &self.source[first..end];
            let Some(first) = source.first() else {
                continue;
            };
            let policy = &policies[first.policy];
            let Some(language) = policy.dictionary.filter(|_| policy.enabled) else {
                continue;
            };
            // A policy change inside a word must not turn either fragment into
            // an independent dictionary word.
            if source
                .iter()
                .any(|character| &policies[character.policy] != policy)
            {
                continue;
            }
            for offset in word_breaks(word, language, engine.config.hyphenation_quality()) {
                let index = source.partition_point(|source| source.byte_end < start + offset);
                let glyph = source[index].glyph;
                if engine.text.is_cluster_boundary(glyph as usize + 1) {
                    breaks.insert(glyph);
                }
            }
        }
        self.text.clear();
        self.source.clear();
    }
}

/// Analyze adjacent text together, retaining original source positions across
/// font/color/emphasis runs. Non-text content and formatting contexts separate
/// words; inline decoration boundaries do not.
pub(super) fn automatic_hyphen_breaks(
    engine: &LayoutEngine<'_, '_>,
    runs: &[InlineItem],
    container_box_idx: usize,
) -> FxHashSet<u32> {
    profile_scope!("tokens.hyphenation");
    let mut breaks = FxHashSet::default();
    if !runs
        .iter()
        .any(|run| engine.reader.style(run.box_idx as usize).hyphens() == Hyphens::Auto)
    {
        return breaks;
    }
    let styles = engine.reader.styles();
    let mut group = TextGroup::default();
    let mut policies = Vec::new();
    for run in runs {
        if !engine.reader.inline_item_belongs_to_context(run, container_box_idx) {
            group.flush(engine, &policies, &mut breaks);
            continue;
        }
        let marker = matches!(run.kind, InlineItemKind::Marker { .. });
        let glyphs = match &run.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => glyphs,
            InlineItemKind::InlineBoundary { .. } => continue,
            _ => {
                group.flush(engine, &policies, &mut breaks);
                continue;
            }
        };
        if marker
            || group
                .source
                .last()
                .is_some_and(|previous| previous.glyph + 1 != glyphs.start)
        {
            group.flush(engine, &policies, &mut breaks);
        }
        let style = engine.reader.style(run.box_idx as usize);
        let language = style.language().and_then(|id| styles.language(id));
        let policy = policies.len();
        policies.push(WordPolicy {
            language: language.or(styles.hyphenation_fallback_language()),
            dictionary: hyphenation_language(language, styles.hyphenation_fallback_language()),
            enabled: style.hyphens() == Hyphens::Auto
                && style.white_space().allows_wrap()
                && style.word_break() != WordBreak::BreakAll
                && style.line_break() != LineBreak::Anywhere
                && engine.text.hyphen_glyph(run.box_idx as usize).is_some(),
        });
        for glyph in glyphs.clone() {
            group.text.push(engine.text.character(glyph));
            group.source.push(SourceCharacter {
                byte_end: group.text.len(),
                glyph,
                policy,
            });
        }
        if marker {
            group.flush(engine, &policies, &mut breaks);
        }
    }
    group.flush(engine, &policies, &mut breaks);
    breaks
}

fn is_apostrophe(character: char) -> bool {
    matches!(character, '\'' | '\u{2019}' | '\u{02bc}')
}

/// Return UTF-8 offsets in the original word, using NFC only for dictionary
/// lookup. Each offset ends a complete grapheme, so decomposed accents stay
/// attached and source text/selection offsets never change.
fn word_breaks(word: &str, language: HyphenationLanguage, book_quality: bool) -> Vec<usize> {
    if word.chars().any(|character| {
        !(character.is_alphabetic() || character.is_mark() || is_apostrophe(character))
    }) {
        // Includes authored soft hyphens: their breakpoints take precedence
        // across the entire word, including other styled runs.
        return Vec::new();
    }
    let mut normalized = String::new();
    let mut positions = Vec::new();
    let mut letters = 0;
    for (start, grapheme) in word.grapheme_indices(true) {
        letters += usize::from(
            grapheme
                .chars()
                .any(|ch| ch.is_alphabetic() && !is_apostrophe(ch)),
        );
        normalized.extend(
            grapheme
                .nfc()
                .map(|ch| if is_apostrophe(ch) { '\'' } else { ch }),
        );
        positions.push((normalized.len(), start + grapheme.len(), letters));
    }
    if letters < if book_quality { 5 } else { 4 } {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut byte_offset = 0;
    let mut syllables = hypher::hyphenate(&normalized, language).peekable();
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
    use unicode_segmentation::UnicodeSegmentation;

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
