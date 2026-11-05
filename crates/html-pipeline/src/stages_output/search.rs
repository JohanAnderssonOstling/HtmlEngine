//! Unicode-aware text queries. Returned offsets are logical character indices.

use crate::stages::text_paint;
use super::RenderTextView;
use aho_corasick::{BuildError, Input, automaton::Automaton, nfa::contiguous::NFA};
use std::ops::Range;
use std::{error, fmt};
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

/// One source-backed match in caller-selected lines. Generated and hidden text
/// cannot match; context outside the selected lines preserves word boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceTextMatch<'a> {
    pub pattern: usize,
    pub glyphs: Range<u32>,
    pub source: Range<super::SourcePosition<'a>>,
    pub text: &'a str,
}

/// Compile a set of queries once, then normalize each input once and search all
/// queries together. Overlapping phrases and duplicate queries remain distinct.
pub struct TextPatternMatcher {
    automaton: NFA,
    patterns: Vec<usize>,
    options: TextSearchOptions,
}

/// A query set could not be compiled. Matcher implementation details stay private.
#[derive(Debug)]
pub struct TextPatternError {
    cause: BuildError,
}

impl fmt::Display for TextPatternError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.cause, formatter)
    }
}

impl error::Error for TextPatternError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        Some(&self.cause)
    }
}

impl TextPatternMatcher {
    pub fn new<I, P>(queries: I, options: TextSearchOptions) -> Result<Self, TextPatternError>
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
        let automaton = NFA::builder().prefilter(false).build(&normalized)
            .map_err(|cause| TextPatternError { cause })?;
        Ok(Self { automaton, patterns, options })
    }



}
/// Owns the extracted page text and native match offsets. Iteration resolves
/// source endpoints and borrows matched text; no per-match text copies or
/// intermediate vectors are needed before a caller accepts a match.
pub struct SourceTextMatches<'a> {
    view: RenderTextView<'a>,
    text: String,
    glyphs: Vec<Option<u32>>,
    matches: Vec<TextPatternMatch>,
}

impl SourceTextMatches<'_> {
    pub fn iter(&self) -> impl Iterator<Item = SourceTextMatch<'_>> + '_ {
        let addressing = self
            .view
            .doc
            .render_view(self.view.glyph_metrics)
            .addressing();
        self.matches.iter().filter_map(move |found| {
            let selected = self.glyphs.get(found.range.clone())?;
            let first = (*selected.first()?)?;
            let last = (*selected.last()?)?;
            if !selected.iter().all(Option::is_some) || last - first + 1 != selected.len() as u32 {
                return None;
            }
            Some(SourceTextMatch {
                pattern: found.pattern,
                glyphs: first..last + 1,
                source: addressing.source_position_for_glyph(first, false)?
                    ..addressing.source_position_for_glyph(last, true)?,
                text: &self.text[found.byte_range.clone()],
            })
        })
    }
}


#[cfg(test)]
mod multi_pattern_tests {
    use super::*;

    #[test]
    fn recurring_multi_pattern_matches_equal_individual_unicode_searches() {
        let queries = [
            "",
            "Elizabeth",
            "ELIZABETH",
            "Mr Darcy",
            "Darcy",
            "José",
            "Jose",
            "café",
            "cafe\u{301}",
            "İ",
            "i",
            "é",
            "e",
            "😀",
            "👩‍👧",
            "a a",
            "a",
            "語",
            "\0invalid",
        ];
        let texts = [
            "Elizabeth ELIZABETH Elizabethan Mr Darcy Darcy",
            "José Jose café cafe\u{301} İ i é e 😀 👩‍👧 語語",
            "a a a",
            "\0Elizabeth\0Mr Darcy",
            "",
            "e\u{301} İ\u{301}",
        ];
        for match_case in [false, true] {
            for whole_word in [false, true] {
                for match_diacritics in [false, true] {
                    let options = TextSearchOptions {
                        match_case,
                        whole_word,
                        match_diacritics,
                    };
                    let matcher = TextPatternMatcher::new(queries, options).unwrap();
                    for text in texts {
                        let found = text_pattern_matcher_find_matches(&matcher, text);
                        for (pattern, query) in queries.iter().enumerate() {
                            let actual = found
                                .iter()
                                .filter(|found| found.pattern == pattern)
                                .map(|found| (found.range.start, found.range.end))
                                .collect::<Vec<_>>();
                            assert_eq!(
                                actual,
                                find_text_matches(text, query, options),
                                "{options:?}: {text:?}, {query:?}"
                            );
                        }
                        for found in found {
                            assert_eq!(
                                text.chars()
                                    .skip(found.range.start)
                                    .take(found.range.len())
                                    .collect::<String>(),
                                text[found.byte_range]
                            );
                        }
                    }
                }
            }
        }
        assert!(
            text_pattern_matcher_find_matches(
                &TextPatternMatcher::new(Vec::<String>::new(), TextSearchOptions::default())
                    .unwrap(),
                "text"
            )
            .is_empty()
        );
    }
}

impl<'a> RenderTextView<'a> {
    fn searchable_characters(self) -> Vec<char> {
        let content = self.doc.inputs.text.content();
        let mut characters = vec!['\0'; content.characters().len()];
        for run in text_paint::render_text_view_visible_source_runs(self) {
            for index in run.glyphs() {
                if let Some(character) = content.character_at(index as usize) {
                    characters[index as usize] = character;
                }
            }
        }
        characters
    }

}

/// Find visible source text without matching generated markers or hidden
/// content. Case/diacritic normalization retains original range indices.
pub fn render_text_view_find_matches<'a>(
    value: RenderTextView<'a>,
    query: &str,
    options: TextSearchOptions,
) -> Vec<Range<u32>> {
    text_matches(
        &value
            .searchable_characters()
            .into_iter()
            .collect::<String>(),
        query,
        options,
    )
    .map(|(start, end)| start as u32..end as u32)
    .collect()
}

/// Words intersecting a logical source range. Geometry determines which
/// candidates are painted, including when overflow clips intervening text.
pub fn render_text_view_word_ranges_in<'a>(
    value: RenderTextView<'a>,
    range: Range<u32>,
) -> impl Iterator<Item = Range<u32>> + 'a {
    value.doc.base_shaped.shared.words.in_range(range)
}

/// A Unicode word or grapheme at a source character, across inline runs
/// and line wraps but bounded by its block container. Lookup allocates no
/// text and uses the index retained by the shaped document.
pub fn render_text_view_word_range_at<'a>(
    value: RenderTextView<'a>,
    glyph: u32,
) -> Option<Range<u32>> {
    value.doc.base_shaped.shared.words.range_at(glyph)
}

/// Locate a quoted passage using exact text and optional adjoining context.
/// Publication ownership and annotation identifiers are deliberately absent.
pub fn render_text_view_locate_quote<'a>(
    value: RenderTextView<'a>,
    exact: &str,
    prefix: Option<&str>,
    suffix: Option<&str>,
) -> Option<Range<u32>> {
    let document = value.searchable_characters();
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

pub fn text_pattern_matcher_find_matches(
    value: &TextPatternMatcher,
    text: &str,
) -> Vec<TextPatternMatch> {
    if value.patterns.is_empty() || text.is_empty() {
        return Vec::new();
    }
    let mut boundaries = Vec::new();
    if value.options.whole_word {
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
        normalize_into(grapheme, value.options, |character| {
            normalized.push(character);
            source.extend(std::iter::repeat_n(
                (start..position, byte..byte + grapheme.len()),
                character.len_utf8(),
            ));
        });
    }
    value
        .automaton
        .try_find_overlapping_iter(Input::new(&normalized))
        .expect("standard unanchored overlapping search")
        .filter_map(|found| {
            let first = &source[found.start()];
            let last = &source[found.end() - 1];
            let range = first.0.start..last.0.end;
            if value.options.whole_word && (!boundaries[range.start] || !boundaries[range.end]) {
                return None;
            }
            Some(TextPatternMatch {
                pattern: value.patterns[found.pattern().as_usize()],
                range,
                byte_range: first.1.start..last.1.end,
            })
        })
        .collect()
}

pub fn match_lines<'a>(
    matcher: &TextPatternMatcher,
    view: RenderTextView<'a>,
    lines: impl IntoIterator<Item = super::LineId>,
) -> SourceTextMatches<'a> {
    if matcher.patterns.is_empty() {
        return SourceTextMatches {
            view,
            text: String::new(),
            glyphs: Vec::new(),
            matches: Vec::new(),
        };
    }
    let mut ranges = lines
        .into_iter()
        .flat_map(|line| {
            text_paint::render_text_view_line_text_fragments(view, line)
                .into_iter()
                .flatten()
        })
        .filter(|fragment| fragment.visible())
        .map(|fragment| fragment.glyphs())
        .collect::<Vec<_>>();
    super::merge_text_ranges(&mut ranges);
    let mut text = String::new();
    let mut glyphs = Vec::new();
    let mut source_runs = text_paint::render_text_view_visible_source_runs(view).peekable();
    for range in ranges {
        text.push('\0');
        glyphs.push(None);
        if let Some(before) = range
            .start
            .checked_sub(1)
            .and_then(|glyph| text_paint::render_text_view_character_at(view, glyph as usize))
        {
            text.push(before);
            glyphs.push(None);
        }
        for glyph in range.clone() {
            let Some(character) = text_paint::render_text_view_character_at(view, glyph as usize)
            else {
                continue;
            };
            while source_runs
                .peek()
                .is_some_and(|run| run.glyphs().end <= glyph)
            {
                source_runs.next();
            }
            let source = source_runs
                .peek()
                .is_some_and(|run| run.glyphs().contains(&glyph));
            text.push(if source { character } else { '\0' });
            glyphs.push(source.then_some(glyph));
        }
        if let Some(after) = text_paint::render_text_view_character_at(view, range.end as usize) {
            text.push(after);
            glyphs.push(None);
        }
    }
    let matches = text_pattern_matcher_find_matches(matcher, &text);
    SourceTextMatches {
        view,
        text,
        glyphs,
        matches,
    }
}
