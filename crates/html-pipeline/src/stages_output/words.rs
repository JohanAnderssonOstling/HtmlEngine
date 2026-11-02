//! Word boundaries owned by shaping, independent of width and glyph resources.

use crate::shaping::ShapedFontMetrics;
use crate::stages::lifecycle::PreparedInputs;
use crate::stages::semantics::is_block_container;
use html_engine_model::WordIndex;
use html_style_model::Visibility;
use html_text::{InlineTextKind, TextChunk, for_each_text_chunk};
use rustc_data_structures::fx::FxHashMap;
use std::iter::once;
use std::ops::Range;
use unicode_segmentation::{UNICODE_VERSION, UnicodeSegmentation};

pub(in crate::stages) fn build_word_index(
    inputs: &PreparedInputs<'_>,
    metrics: &ShapedFontMetrics,
) -> WordIndex {
    let tree = &inputs.layout_tree;
    let content = inputs.text.content();
    let mut blocks: FxHashMap<usize, Vec<Range<u32>>> = FxHashMap::default();
    for run in content.text_runs().filter(|run| {
        run.kind == InlineTextKind::Text && run.dom_text_node.is_some() && !run.glyphs.is_empty()
    }) {
        let owner = run.box_idx as usize;
        let style = tree
            .get_box_style_indices(owner)
            .unwrap_or_else(|| inputs.styles.default_indices());
        if !metrics
            .used_style(&inputs.styles, style, owner)
            .is_some_and(|style| style.visibility() == Visibility::Visible)
        {
            continue;
        }
        if let Some(block) = once(owner)
            .chain(tree.ancestors(owner))
            .find(|&index| is_block_container(tree, index) || tree.get_box_parent(index).is_none())
        {
            blocks.entry(block).or_default().push(run.glyphs.clone());
        }
    }
    let mut result = WordIndex::default();
    let mut text = String::new();
    let mut positions = Vec::new();
    let characters = content.characters();
    for runs in blocks.into_values() {
        text.clear();
        positions.clear();
        let Some((first, last)) = runs.first().zip(runs.last()) else {
            continue;
        };
        // Adjacent visible runs share source offsets even across inline styles.
        // Only omitted source requires a per-character mapping.
        let source_start = (last.end as usize <= characters.len()
            && runs.windows(2).all(|pair| pair[0].end == pair[1].start))
        .then_some(first.start);
        if source_start.is_some() {
            text.extend(
                characters[first.start as usize..last.end as usize]
                    .iter()
                    .copied(),
            );
        } else {
            for range in runs {
                for glyph in range {
                    if let Some(character) = content.character_at(glyph as usize) {
                        text.push(character);
                        positions.push(glyph);
                    }
                }
            }
        }
        let mut emit = |range: Range<usize>| {
            if let Some(start) = source_start {
                result.push_contiguous(start + range.start as u32..start + range.end as u32);
            } else {
                result.push(&positions[range]);
            }
        };
        emit_word_ranges(&text, &mut emit);
    }
    result.sort();
    result
}

fn emit_word_ranges(text: &str, emit: &mut impl FnMut(Range<usize>)) {
    let mut character_offset = 0;
    for_each_text_chunk(text, |chunk| match chunk {
        TextChunk::Ascii(text) => {
            emit_ascii_word_ranges(text, character_offset, emit);
            character_offset += text.len();
        }
        TextChunk::Unicode(text) => {
            character_offset = emit_unicode_word_ranges(text, character_offset, emit);
        }
    });
}

#[inline]
fn emit_ascii_word_ranges(
    text: &str,
    character_offset: usize,
    emit: &mut impl FnMut(Range<usize>),
) {
    // The library uses its ASCII scanner here; byte and character offsets agree.
    let bytes = text.as_bytes();
    let mut offset = 0;
    for (start, word) in text.unicode_word_indices() {
        emit_ascii_non_words(&bytes[offset..start], character_offset + offset, emit);
        offset = start + word.len();
        emit(character_offset + start..character_offset + offset);
    }
    emit_ascii_non_words(&bytes[offset..], character_offset + offset, emit);
}

fn emit_unicode_word_ranges(
    text: &str,
    mut offset: usize,
    emit: &mut impl FnMut(Range<usize>),
) -> usize {
    for segment in text.split_word_bounds() {
        let (length, word) = word_segment_properties(segment);
        let end = offset + length;
        if word {
            emit(offset..end);
        } else if !segment.chars().all(char::is_whitespace) {
            let mut grapheme_offset = offset;
            for grapheme in segment.graphemes(true) {
                let grapheme_end = grapheme_offset + grapheme.chars().count();
                if !grapheme.chars().all(char::is_whitespace) {
                    emit(grapheme_offset..grapheme_end);
                }
                grapheme_offset = grapheme_end;
            }
        }
        offset = end;
    }
    offset
}

/// Word boundaries are already known. Count UTF-8 leading bytes and detect
/// ASCII letters/numbers together, without segmenting each segment again.
#[inline]
fn word_segment_properties(segment: &str) -> (usize, bool) {
    let mut characters = 0;
    let mut word = false;
    let mut non_ascii = false;
    for byte in segment.bytes() {
        characters += usize::from(byte & 0xc0 != 0x80);
        word |= byte.is_ascii_alphanumeric();
        non_ascii |= !byte.is_ascii();
    }
    if !word && non_ascii {
        // Match unicode-segmentation's classification tables exactly. When
        // std uses another Unicode version, retain the library's own rules.
        let same_unicode_version = UNICODE_VERSION
            == (
                u64::from(char::UNICODE_VERSION.0),
                u64::from(char::UNICODE_VERSION.1),
                u64::from(char::UNICODE_VERSION.2),
            );
        word = if same_unicode_version {
            segment.chars().any(char::is_alphanumeric)
        } else {
            segment.unicode_words().next().is_some()
        };
    }
    (characters, word)
}

/// Non-word ASCII bytes are individual graphemes except CR/LF, whose bytes
/// are both whitespace. Keep vertical tab as whitespace too, matching char.
#[inline]
fn emit_ascii_non_words(bytes: &[u8], offset: usize, emit: &mut impl FnMut(Range<usize>)) {
    for (index, &byte) in bytes.iter().enumerate() {
        if !matches!(byte, b'\t'..=b'\r' | b' ') {
            let start = offset + index;
            emit(start..start + 1);
        }
    }
}
