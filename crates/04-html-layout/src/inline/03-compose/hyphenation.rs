//! Match hyphen opportunities on demand while selecting lines.
use crate::LayoutEngine;
use crate::inline::hyphenation::{WordPolicy, for_each_word_break};
use crate::inline::{InlineItem, InlineItemKind};
use html_engine_model::WordSpan;
use html_style_model::Visibility;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

struct MatchedWord {
    source: Range<u32>,
    breaks: Range<u32>,
}

#[derive(Default)]
pub(in crate::inline) struct DeferredHyphenationCache {
    words: Vec<MatchedWord>,
    breaks: Vec<u32>,
}

impl DeferredHyphenationCache {
    pub(in crate::inline) fn memory_usage_bytes(&self) -> usize {
        size_of::<Self>()
            + self.words.capacity() * size_of::<MatchedWord>()
            + self.breaks.capacity() * size_of::<u32>()
    }

    // A cached word also covers the punctuation/whitespace gaps immediately
    // around it. Coverage must include the whole requested source interval;
    // finding no breakpoint in an uncovered interval is not a cache hit.
    fn lookup(&self, source: Range<u32>, cursor: &mut usize) -> Option<Option<u32>> {
        // Greedy queries normally move forward. A backward float reprobe or a
        // changed insertion position falls back to the same lower-bound search.
        let mut first = (*cursor).min(self.words.len());
        if first > 0 && self.words[first - 1].source.end > source.start {
            first = self
                .words
                .partition_point(|word| word.source.end <= source.start);
        } else {
            while first < self.words.len() && self.words[first].source.end <= source.start {
                first += 1;
            }
        }
        *cursor = first;
        let mut covered = source.start;
        let mut last = None;
        for word in &self.words[first..] {
            if word.source.start > covered {
                return None;
            }
            for &glyph in &self.breaks[word.breaks.start as usize..word.breaks.end as usize] {
                if source.contains(&glyph) {
                    last = Some(last.map_or(glyph, |previous: u32| previous.max(glyph)));
                }
            }
            covered = covered.max(word.source.end);
            if covered >= source.end {
                return Some(last);
            }
        }
        None
    }
}

enum DeferredWords<'input> {
    Unprepared,
    Indexed(&'input [WordSpan]),
    Segmented(Vec<Range<u32>>),
}

impl DeferredWords<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Unprepared => 0,
            Self::Indexed(words) => words.len(),
            Self::Segmented(words) => words.len(),
        }
    }

    fn range(&self, index: usize) -> Range<u32> {
        match self {
            Self::Unprepared => unreachable!("word boundaries prepared before lookup"),
            Self::Indexed(words) => words[index].range(),
            Self::Segmented(words) => words[index].clone(),
        }
    }

    fn first_relevant(&self, glyph: u32) -> usize {
        let first = match self {
            Self::Unprepared => 0,
            Self::Indexed(words) => words.partition_point(|word| word.range().end <= glyph),
            Self::Segmented(words) => words.partition_point(|word| word.end <= glyph),
        };
        first.saturating_sub(1)
    }
}

/// Borrow precomputed word boundaries when they cover the same visible DOM
/// text. Only queried words need UTF-8 dictionary input; policies stay on runs
/// and contiguous glyph positions follow directly from character offsets.
pub(in crate::inline) struct DeferredHyphenation<'input, 'cache> {
    runs: &'input [InlineItem],
    context: Range<u32>,
    words: DeferredWords<'input>,
    word: String,
    cache: &'cache mut DeferredHyphenationCache,
    cache_cursor: usize,
}

impl<'input, 'cache> DeferredHyphenation<'input, 'cache> {
    pub(in crate::inline) fn new(
        engine: &LayoutEngine<'input, '_>,
        runs: Range<u32>,
        cache: &'cache mut DeferredHyphenationCache,
    ) -> Self {
        Self {
            runs: &engine.read.text.content().inline_items()
                [runs.start as usize..runs.end as usize],
            context: 0..0,
            words: DeferredWords::Unprepared,
            word: String::new(),
            cache,
            cache_cursor: 0,
        }
    }

    fn prepare_words(&mut self, engine: &LayoutEngine<'input, '_>) {
        let mut start = None;
        let mut indexed = true;
        for run in self.runs {
            if let InlineItemKind::Text { glyphs } = &run.kind
                && !glyphs.is_empty()
            {
                start.get_or_insert(glyphs.start);
                self.context.end = glyphs.end;
                indexed &= run.dom_text_node.is_some()
                    && engine.read.reader.style(run.box_idx as usize).visibility()
                        == Visibility::Visible;
            }
        }
        self.context.start = start.unwrap_or(self.context.end);
        if indexed
            && let Some(words) = engine
                .read
                .text
                .word_index()
                .and_then(|words| words.contiguous_words(self.context.clone()))
        {
            self.words = DeferredWords::Indexed(words);
            return;
        }

        // Visibility filtering, generated text and context-spanning words can
        // make the reader index unsuitable. Keep the original Unicode rules,
        // translating ordered byte boundaries in one forward character walk.
        self.word.extend(
            engine.read.text.content().characters()
                [self.context.start as usize..self.context.end as usize]
                .iter()
                .copied(),
        );
        let mut words = Vec::new();
        let mut previous_byte = 0;
        let mut glyph = self.context.start;
        for (start, word) in self.word.unicode_word_indices() {
            glyph += self.word[previous_byte..start].chars().count() as u32;
            let end = glyph + word.chars().count() as u32;
            words.push(glyph..end);
            glyph = end;
            previous_byte = start + word.len();
        }
        self.word.clear();
        self.words = DeferredWords::Segmented(words);
    }

    fn word_policy(
        &self,
        engine: &LayoutEngine<'input, '_>,
        source: &Range<u32>,
    ) -> Option<WordPolicy<'input>> {
        let mut policy = None;
        for run in self.runs {
            let InlineItemKind::Text { glyphs } = &run.kind else {
                continue;
            };
            if glyphs.start >= source.end {
                break;
            }
            if glyphs.end <= source.start || glyphs.is_empty() {
                continue;
            }
            let current = WordPolicy::for_run(&engine.read, run);
            if !current.enabled
                || current.dictionary.is_none()
                || policy.as_ref().is_some_and(|previous| *previous != current)
            {
                return None;
            }
            policy = Some(current);
        }
        policy
    }

    fn match_word(&mut self, engine: &mut LayoutEngine<'input, '_>, source: Range<u32>) {
        let Some(policy) = self.word_policy(engine, &source) else {
            return;
        };
        self.word.clear();
        self.word.extend(
            engine.read.text.content().characters()[source.start as usize..source.end as usize]
                .iter()
                .copied(),
        );
        for_each_word_break(
            &engine.read,
            &mut engine.scratch.hyphenation_dictionary,
            &self.word,
            source.start,
            policy.dictionary.unwrap(),
            |glyph| self.cache.breaks.push(glyph),
        );
    }

    pub(in crate::inline) fn last_break(
        &mut self,
        engine: &mut LayoutEngine<'input, '_>,
        source: Range<u32>,
    ) -> Option<u32> {
        if let Some(result) = self.cache.lookup(source.clone(), &mut self.cache_cursor) {
            return result;
        }
        if matches!(self.words, DeferredWords::Unprepared) {
            self.prepare_words(engine);
        }
        if self.words.len() == 0 {
            self.cache.words.push(MatchedWord {
                source: self.context.clone(),
                breaks: 0..0,
            });
            return None;
        }
        let mut last = None;
        for index in self.words.first_relevant(source.start)..self.words.len() {
            let start = index.checked_sub(1).map_or(self.context.start, |previous| {
                self.words.range(previous).end
            });
            if start >= source.end {
                break;
            }
            let end = if index + 1 < self.words.len() {
                self.words.range(index + 1).start
            } else {
                self.context.end
            };
            if end <= source.start {
                continue;
            }
            let insertion = if self
                .cache
                .words
                .last()
                .is_none_or(|word| word.source.start < start)
            {
                self.cache.words.len()
            } else {
                self.cache
                    .words
                    .partition_point(|word| word.source.start < start)
            };
            let breaks = if let Some(word) = self.cache.words.get(insertion)
                && word.source == (start..end)
            {
                word.breaks.clone()
            } else {
                let first_break = self.cache.breaks.len() as u32;
                self.match_word(engine, self.words.range(index));
                let breaks = first_break..self.cache.breaks.len() as u32;
                self.cache.words.insert(
                    insertion,
                    MatchedWord {
                        source: start..end,
                        breaks: breaks.clone(),
                    },
                );
                breaks
            };
            if let Some(glyph) = self.cache.breaks[breaks.start as usize..breaks.end as usize]
                .iter()
                .rev()
                .copied()
                .find(|glyph| source.contains(glyph))
            {
                last = Some(last.map_or(glyph, |previous: u32| previous.max(glyph)));
            }
        }
        last
    }
}
