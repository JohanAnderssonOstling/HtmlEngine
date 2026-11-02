//! Unicode-aware text queries. Returned offsets are logical character indices.

use super::RenderTextView;
use aho_corasick::{BuildError, Input, automaton::Automaton, nfa::contiguous::NFA};
use std::ops::Range;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
use unicode_segmentation::UnicodeSegmentation;

/// Text matching options; publication scope and result navigation are UI policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TextSearchOptions {
    pub match_case: bool,
    pub whole_word: bool,
    pub match_diacritics: bool,
}

fn normalize_into(text: &str, options: TextSearchOptions, mut emit: impl FnMut(char)) {
    let push = |character: char| {
        if options.match_case {
            emit(character);
        } else {
            character.to_lowercase().for_each(&mut emit);
        }
    };
    if options.match_diacritics {
        text.chars().for_each(push);
    } else {
        text.nfd()
            .filter(|part| !is_combining_mark(*part))
            .for_each(push);
    }
}

/// Collect Unicode-aware matches as logical character offsets.
pub fn find_text_matches(
    text: &str,
    query: &str,
    options: TextSearchOptions,
) -> Vec<(usize, usize)> {
    text_matches(text, query, options).collect()
}

/// Normalize the search input once, then yield matches without collecting them.
/// The iterator owns its normalized data and retains no input string borrows.
pub fn text_matches(
    text: &str,
    query: &str,
    options: TextSearchOptions,
) -> impl Iterator<Item = (usize, usize)> + use<> {
    let mut needle = Vec::new();
    normalize_into(query, options, |character| needle.push(character));
    let valid_query = !needle.is_empty() && !query.contains('\0');
    let mut word_boundaries = Vec::new();
    let mut haystack = Vec::new();
    if valid_query {
        // Use the same Unicode segmentation as word_range_at. Keep boundaries in
        // source coordinates so case folding and removed marks cannot split words.
        if options.whole_word {
            word_boundaries.resize(text.chars().count() + 1, false);
            let mut position = 0;
            word_boundaries[0] = true;
            for segment in text.split_word_bounds() {
                position += segment.chars().count();
                word_boundaries[position] = true;
            }
        }
        let mut offset = 0;
        haystack.reserve(text.len());
        for grapheme in text.graphemes(true) {
            let start = offset;
            offset += grapheme.chars().count();
            normalize_into(grapheme, options, |character| {
                haystack.push((character, start..offset))
            });
        }
    }
    let starts = if !valid_query || needle.len() > haystack.len() {
        0
    } else {
        haystack.len() - needle.len() + 1
    };
    (0..starts).filter_map(move |start| {
        let window = &haystack[start..start + needle.len()];
        if !window
            .iter()
            .map(|(character, _)| *character)
            .eq(needle.iter().copied())
        {
            return None;
        }
        let source_start = window.first()?.1.start;
        let source_end = window.last()?.1.end;
        if options.whole_word && (!word_boundaries[source_start] || !word_boundaries[source_end]) {
            return None;
        }
        Some((source_start, source_end))
    })
}

/// A match identifies the original query and preserves both source coordinate
/// systems through case expansion and diacritic removal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextPatternMatch {
    pub pattern: usize,
    pub range: Range<usize>,
    pub byte_range: Range<usize>,
}

/// Compile a set of queries once, then normalize each input once and search all
/// queries together. Overlapping phrases and duplicate queries remain distinct.
pub struct TextPatternMatcher {
    automaton: NFA,
    patterns: Vec<usize>,
    options: TextSearchOptions,
}

impl TextPatternMatcher {
    pub fn new<I, P>(queries: I, options: TextSearchOptions) -> Result<Self, BuildError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<str>,
    {
        let mut patterns = Vec::new();
        let mut normalized = Vec::new();
        for (index, query) in queries.into_iter().enumerate() {
            let query = query.as_ref();
            if query.contains('\0') { continue; }
            let mut needle = String::new();
            normalize_into(query, options, |character| needle.push(character));
            if needle.is_empty() { continue; }
            patterns.push(index);
            normalized.push(needle);
        }
        // Own the compact automaton directly. Disabling the optional prefilter
        // also avoids its internal shared ownership.
        let automaton = NFA::builder().prefilter(false).build(&normalized)?;
        Ok(Self { automaton, patterns, options })
    }

    pub fn find_matches(&self, text: &str) -> Vec<TextPatternMatch> {
        if self.patterns.is_empty() || text.is_empty() { return Vec::new(); }
        let mut boundaries = Vec::new();
        if self.options.whole_word {
            boundaries.resize(text.chars().count() + 1, false);
            boundaries[0] = true;
            let mut position = 0;
            for segment in text.split_word_bounds() {
                position += segment.chars().count();
                boundaries[position] = true;
            }
        }
        let mut normalized = String::with_capacity(text.len());
        let mut source = Vec::with_capacity(text.len());
        let mut position = 0;
        for (byte, grapheme) in text.grapheme_indices(true) {
            let start = position;
            position += grapheme.chars().count();
            normalize_into(grapheme, self.options, |character| {
                normalized.push(character);
                source.extend(std::iter::repeat_n((start..position, byte..byte + grapheme.len()), character.len_utf8()));
            });
        }
        self.automaton.try_find_overlapping_iter(Input::new(&normalized))
            .expect("standard unanchored overlapping search")
            .filter_map(|found| {
                let first = &source[found.start()];
                let last = &source[found.end() - 1];
                let range = first.0.start..last.0.end;
                if self.options.whole_word && (!boundaries[range.start] || !boundaries[range.end]) { return None; }
                Some(TextPatternMatch { pattern: self.patterns[found.pattern().as_usize()], range, byte_range: first.1.start..last.1.end })
            })
            .collect()
    }
}

#[cfg(test)]
mod multi_pattern_tests {
    use super::*;

    #[test]
    fn recurring_multi_pattern_matches_equal_individual_unicode_searches() {
        let queries = ["", "Elizabeth", "ELIZABETH", "Mr Darcy", "Darcy", "José", "Jose", "café", "cafe\u{301}", "İ", "i", "é", "e", "😀", "👩‍👧", "a a", "a", "語", "\0invalid"];
        let texts = ["Elizabeth ELIZABETH Elizabethan Mr Darcy Darcy", "José Jose café cafe\u{301} İ i é e 😀 👩‍👧 語語", "a a a", "\0Elizabeth\0Mr Darcy", "", "e\u{301} İ\u{301}"];
        for match_case in [false, true] {
            for whole_word in [false, true] {
                for match_diacritics in [false, true] {
                    let options = TextSearchOptions { match_case, whole_word, match_diacritics };
                    let matcher = TextPatternMatcher::new(queries, options).unwrap();
                    for text in texts {
                        let found = matcher.find_matches(text);
                        for (pattern, query) in queries.iter().enumerate() {
                            let actual = found.iter().filter(|found| found.pattern == pattern).map(|found| (found.range.start, found.range.end)).collect::<Vec<_>>();
                            assert_eq!(actual, find_text_matches(text, query, options), "{options:?}: {text:?}, {query:?}");
                        }
                        for found in found {
                            assert_eq!(text.chars().skip(found.range.start).take(found.range.len()).collect::<String>(), text[found.byte_range]);
                        }
                    }
                }
            }
        }
        assert!(TextPatternMatcher::new(Vec::<String>::new(), TextSearchOptions::default()).unwrap().find_matches("text").is_empty());
    }
}

impl<'a> RenderTextView<'a> {
    fn searchable_characters(self) -> Vec<char> {
        let content = self.doc.inputs.text.content();
        let mut characters = vec!['\0'; content.characters().len()];
        for run in self.visible_source_runs() {
            for index in run.glyphs() {
                if let Some(character) = content.character_at(index as usize) {
                    characters[index as usize] = character;
                }
            }
        }
        characters
    }

    /// Find visible source text without matching generated markers or hidden
    /// content. Case/diacritic normalization retains original range indices.
    pub fn find_matches(self, query: &str, options: TextSearchOptions) -> Vec<Range<u32>> {
        text_matches(
            &self.searchable_characters().into_iter().collect::<String>(),
            query,
            options,
        )
        .map(|(start, end)| start as u32..end as u32)
        .collect()
    }

    /// Locate a quoted passage using exact text and optional adjoining context.
    /// Publication ownership and annotation identifiers are deliberately absent.
    pub fn locate_quote(
        self,
        exact: &str,
        prefix: Option<&str>,
        suffix: Option<&str>,
    ) -> Option<Range<u32>> {
        let document = self.searchable_characters();
        let exact: Vec<char> = exact.chars().collect();
        if exact.is_empty() || exact.contains(&'\0') || exact.len() > document.len() {
            return None;
        }
        let prefix: Vec<char> = prefix.unwrap_or_default().chars().collect();
        let suffix: Vec<char> = suffix.unwrap_or_default().chars().collect();
        (0..=document.len() - exact.len()).find_map(|start| {
            let end = start + exact.len();
            (document[start..end] == exact
                && (prefix.is_empty() || document[..start].ends_with(&prefix))
                && (suffix.is_empty() || document[end..].starts_with(&suffix)))
            .then_some(start as u32..end as u32)
        })
    }

    /// A Unicode word or grapheme at a source character, across inline runs
    /// and line wraps but bounded by its block container. Lookup allocates no
    /// text and uses the index retained by the shaped document.
    pub fn word_range_at(self, glyph: u32) -> Option<Range<u32>> {
        self.doc.base_shaped.shared.words.range_at(glyph)
    }

    /// Words intersecting a logical source range. Geometry determines which
    /// candidates are painted, including when overflow clips intervening text.
    pub fn word_ranges_in(self, range: Range<u32>) -> impl Iterator<Item = Range<u32>> + 'a {
        self.doc.base_shaped.shared.words.in_range(range)
    }
}
