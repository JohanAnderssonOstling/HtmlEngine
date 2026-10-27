//! Unicode-aware text queries. Returned offsets are logical character indices.

use super::{RenderTextRun, RenderTextView};
use html_style_model::Visibility;
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

pub fn find_text_matches(
    text: &str,
    query: &str,
    options: TextSearchOptions,
) -> Vec<(usize, usize)> {
    let mut needle = Vec::new();
    normalize_into(query, options, |character| needle.push(character));
    if needle.is_empty() || query.contains('\0') {
        return Vec::new();
    }
    // Use the same Unicode segmentation as word_range_at. Keep boundaries in
    // source coordinates so case folding and removed marks cannot split words.
    let mut word_boundaries = Vec::new();
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
    let mut haystack = Vec::with_capacity(text.len());
    for grapheme in text.graphemes(true) {
        let start = offset;
        offset += grapheme.chars().count();
        normalize_into(grapheme, options, |character| {
            haystack.push((character, start..offset))
        });
    }
    haystack
        .windows(needle.len())
        .filter_map(|window| {
            if !window
                .iter()
                .map(|(character, _)| *character)
                .eq(needle.iter().copied())
            {
                return None;
            }
            let source_start = window.first()?.1.start;
            let source_end = window.last()?.1.end;
            if options.whole_word
                && (!word_boundaries[source_start] || !word_boundaries[source_end])
            {
                return None;
            }
            Some((source_start, source_end))
        })
        .collect()
}

impl<'a> RenderTextView<'a> {
    fn visible_source_runs(self) -> impl Iterator<Item = RenderTextRun> + 'a {
        self.text_runs().filter(move |run| {
            run.dom_text_node().is_some()
                && self
                    .doc
                    .box_used_style(run.box_idx())
                    .is_some_and(|style| style.visibility() == Visibility::Visible)
        })
    }

    fn searchable_characters(self) -> Vec<char> {
        let content = self.doc.inputs.text.content();
        let mut characters = vec!['\0'; content.characters().len()];
        for run in self.visible_source_runs() {
            for index in run.glyphs() {
                if let Some(character) = content
                    .character_at(index as usize)
                    .and_then(char::from_u32)
                {
                    characters[index as usize] = character;
                }
            }
        }
        characters
    }

    /// Find visible source text without matching generated markers or hidden
    /// content. Case/diacritic normalization retains original range indices.
    pub fn find_matches(self, query: &str, options: TextSearchOptions) -> Vec<Range<u32>> {
        find_text_matches(
            &self.searchable_characters().into_iter().collect::<String>(),
            query,
            options,
        )
        .into_iter()
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

    /// A Unicode word or grapheme at a character, across inline runs and line
    /// wraps but bounded by its block container.
    pub fn word_range_at(self, glyph: u32) -> Option<Range<u32>> {
        let semantics = self.doc.render_view(self.glyph_metrics).semantics();
        let hit = self
            .visible_source_runs()
            .find(|run| run.glyphs().contains(&glyph))?;
        let block = semantics.block_container(hit.box_idx())?;
        let mut text = String::new();
        let mut positions = Vec::new();
        let mut target_byte = None;
        let content = self.doc.inputs.text.content();
        for run in self
            .visible_source_runs()
            .filter(|run| semantics.block_container(run.box_idx()) == Some(block))
        {
            for index in run.glyphs() {
                if index == glyph {
                    target_byte = Some(text.len());
                }
                let character = content
                    .character_at(index as usize)
                    .and_then(char::from_u32)?;
                positions.push(index);
                text.push(character);
            }
        }
        let target = target_byte?;
        let segment = text
            .unicode_word_indices()
            .find(|(offset, word)| *offset <= target && target < *offset + word.len())
            .or_else(|| {
                text.grapheme_indices(true).find(|(offset, cluster)| {
                    *offset <= target
                        && target < *offset + cluster.len()
                        && !cluster.chars().all(char::is_whitespace)
                })
            })?;
        let first = text[..segment.0].chars().count();
        let count = segment.1.chars().count();
        Some(positions[first]..positions[first + count - 1] + 1)
    }
}
