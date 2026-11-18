//! Prepare the paragraph hyphen opportunities and decide whether matching can be deferred.
use crate::inline::LayoutReadContext;
use crate::inline::hyphenation::{HyphenationDictionaryCache, for_each_cached_word_break};
use crate::inline::hyphenation::{WordPolicy, for_each_word_break};
use crate::inline::{InlineItem, InlineItemKind};
use html_engine_model::WordSpan;
use html_style_model::{TextAlign, TextDirection, TextWrapStyle, Visibility};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// Analyze adjacent text together, retaining original source positions across
/// font/color/emphasis runs. Non-text content and formatting contexts separate
/// words; inline decoration boundaries do not.
/// The caller has already established that at least one run uses automatic hyphens.
pub(super) fn automatic_hyphen_breaks(
    inputs: &LayoutReadContext<'_>,
    dictionary: &mut HyphenationDictionaryCache,
    runs: &[InlineItem],
    container_box_idx: usize,
) -> Vec<u32> {
    let mut breaks = Vec::new();
    let mut group = TextGroup::default();
    for run in runs {
        if !inputs.inline_item_belongs_to_context(run, container_box_idx) {
            // Foreign out-of-flow text does not split the parent's word.
            continue;
        }
        let marker = matches!(run.kind, InlineItemKind::Marker { .. });
        let glyphs = match &run.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => glyphs,
            InlineItemKind::InlineBoundary { .. }
            | InlineItemKind::AbsoluteAnchor { .. }
            | InlineItemKind::FloatAnchor { .. } => continue,
            _ => {
                group.flush(inputs, dictionary, &mut breaks);
                continue;
            }
        };
        if marker {
            group.flush(inputs, dictionary, &mut breaks);
        }
        group.append_run(inputs, run, glyphs.clone());
        if marker {
            group.flush(inputs, dictionary, &mut breaks);
        }
    }
    group.flush(inputs, dictionary, &mut breaks);
    // Source groups can be visited out of order or share glyphs. Consumers
    // walk each source range forward and need the same unique membership.
    breaks.sort_unstable();
    breaks.dedup();
    breaks
}

/// Mixed bidi, paragraph optimization and non-text separators retain the
/// complete opportunity stream. The deferred path has one contiguous logical
/// text group and uses greedy selection, including changing float intervals.
pub(super) fn can_defer_hyphenation(
    inputs: &LayoutReadContext<'_>,
    runs: &[InlineItem],
    container: usize,
) -> bool {
    let style = inputs.reader.style(container);
    if inputs.constraints.book_optimized_text()
        || style.text_wrap_style() == TextWrapStyle::Pretty
        || style.text_align() == TextAlign::Justify
        || style.direction() != TextDirection::Ltr
        || style.overflow_x().clips()
        || !inputs.text.is_resolved_default_ltr()
    {
        return false;
    }
    let mut previous = None;
    runs.iter().all(|run| {
        if !inputs.inline_item_belongs_to_context(run, container) {
            return false;
        }
        match &run.kind {
            InlineItemKind::Text { glyphs } => {
                if previous.is_some_and(|end| end != glyphs.start) {
                    return false;
                }
                previous = Some(glyphs.end);
                true
            }
            InlineItemKind::InlineBoundary { .. } => true,
            _ => false,
        }
    })
}

/// One logical source run can cover many characters. Glyph gaps remain
/// explicit, while equal policies on contiguous runs share one association.
struct SourceRun<'a> {
    byte_end: usize,
    glyphs: Range<u32>,
    policy: WordPolicy<'a>,
}

#[derive(Default)]
struct TextGroup<'a> {
    text: String,
    source: Vec<SourceRun<'a>>,
    indexable: bool,
}

impl TextGroup<'_> {
    fn flush(
        &mut self,
        inputs: &LayoutReadContext<'_>,
        dictionary: &mut HyphenationDictionaryCache,
        breaks: &mut Vec<u32>,
    ) {
        let Some((first, last)) = self.source.first().zip(self.source.last()) else {
            return;
        };
        if self.indexable
            && let Some(words) = inputs
                .text
                .word_index()
                .and_then(|words| words.contiguous_words(first.glyphs.start..last.glyphs.end))
        {
            self.flush_indexed(inputs, dictionary, words, breaks);
        } else {
            // Generated/hidden text and source gaps can give the reader index
            // different boundaries. Build UTF-8 and byte positions only here.
            for source in &mut self.source {
                self.text.extend(
                    inputs.text.content().characters()
                        [source.glyphs.start as usize..source.glyphs.end as usize]
                        .iter()
                        .copied(),
                );
                source.byte_end = self.text.len();
            }
            self.flush_segmented(inputs, dictionary, breaks);
        }
        self.text.clear();
        self.source.clear();
    }

    fn flush_indexed(
        &mut self,
        inputs: &LayoutReadContext<'_>,
        dictionary: &mut HyphenationDictionaryCache,
        words: &[WordSpan],
        breaks: &mut Vec<u32>,
    ) {
        let mut first_run = 0;
        let minimum_letters = if inputs.constraints.hyphenation_quality() {
            5
        } else {
            4
        };
        for word in words {
            let source = word.range();
            // Each letter needs at least one source character. Short entries,
            // including the reader's punctuation entries, cannot match.
            if source.end - source.start < minimum_letters {
                continue;
            }
            while self.source[first_run].glyphs.end <= source.start {
                first_run += 1;
            }
            let policy = &self.source[first_run].policy;
            let Some(language) = policy.dictionary.filter(|_| policy.enabled) else {
                continue;
            };
            if self.source[first_run..]
                .iter()
                .take_while(|run| run.glyphs.start < source.end)
                .any(|run| run.policy != *policy)
            {
                continue;
            }
            self.text.clear();
            self.text.extend(
                inputs.text.content().characters()[source.start as usize..source.end as usize]
                    .iter()
                    .copied(),
            );
            for_each_word_break(
                inputs,
                dictionary,
                &self.text,
                source.start,
                language,
                |glyph| {
                    breaks.push(glyph);
                },
            );
        }
    }

    fn flush_segmented(
        &self,
        inputs: &LayoutReadContext<'_>,
        dictionary: &mut HyphenationDictionaryCache,
        breaks: &mut Vec<u32>,
    ) {
        let text = inputs.text;
        let mut source_index = 0;
        let mut source_byte = 0;
        let mut source_glyph = self.source.first().map_or(0, |run| run.glyphs.start);
        for (start, word) in self.text.unicode_word_indices() {
            self.word_breaks(inputs, dictionary, start, word, |boundary| {
                // Dictionary offsets and words arrive in increasing byte order.
                // Skip whole source runs, then count only the unvisited UTF-8
                // prefix. No per-character source or policy map is needed.
                while boundary > self.source[source_index].byte_end {
                    source_byte = self.source[source_index].byte_end;
                    source_index += 1;
                    source_glyph = self.source[source_index].glyphs.start;
                }
                source_glyph += self.text[source_byte..boundary].chars().count() as u32;
                source_byte = boundary;
                let glyph = source_glyph - 1;
                if text.is_cluster_boundary(glyph as usize + 1) {
                    breaks.push(glyph);
                }
            });
        }
    }

    fn word_breaks(
        &self,
        inputs: &LayoutReadContext<'_>,
        dictionary: &mut HyphenationDictionaryCache,
        start: usize,
        word: &str,
        mut emit: impl FnMut(usize),
    ) {
        let first = self
            .source
            .partition_point(|source| source.byte_end <= start);
        let end = self
            .source
            .partition_point(|source| source.byte_end < start + word.len())
            + 1;
        let source = &self.source[first..end];
        let policy = &source[0].policy;
        let Some(language) = policy.dictionary.filter(|_| policy.enabled) else {
            return;
        };
        // Compare only policy transitions touched by the word. A policy
        // change must not turn either fragment into an independent word.
        if source.iter().any(|run| run.policy != *policy) {
            return;
        }
        for_each_cached_word_break(inputs, dictionary, word, language, |offset| {
            emit(start + offset);
        });
    }
}

impl<'input> TextGroup<'input> {
    fn append_run(
        &mut self,
        inputs: &LayoutReadContext<'input>,
        run: &InlineItem,
        glyphs: Range<u32>,
    ) {
        if glyphs.is_empty() {
            return;
        }
        let visible_dom = matches!(run.kind, InlineItemKind::Text { .. })
            && run.dom_text_node.is_some()
            && inputs.reader.style(run.box_idx as usize).visibility() == Visibility::Visible;
        self.indexable = if let Some(previous) = self.source.last() {
            self.indexable && visible_dom && previous.glyphs.end == glyphs.start
        } else {
            visible_dom
        };
        let policy = WordPolicy::for_run(inputs, run);
        if let Some(previous) = self.source.last_mut()
            && previous.glyphs.end == glyphs.start
            && previous.policy == policy
        {
            previous.glyphs.end = glyphs.end;
        } else {
            self.source.push(SourceRun {
                // Filled only when this group needs its own segmentation.
                byte_end: 0,
                glyphs,
                policy,
            });
        }
    }
}
