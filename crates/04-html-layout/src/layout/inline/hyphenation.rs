use super::{InlineItem, InlineItemKind};
use crate::layout::LayoutEngine;
use html_engine_model::WordSpan;
use html_style_model::{
    ContentLanguage, Hyphens, LineBreak, TextAlign, TextDirection, TextWrapStyle, Visibility, WordBreak,
};
use hypher::{Lang as HyphenationLanguage, hyphenate, hyphenate_bounded};
use std::cell::RefCell;
use std::ops::Range;
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

impl<'input> WordPolicy<'input> {
    fn for_run(engine: &LayoutEngine<'input, '_>, run: &InlineItem) -> Self {
        let styles = engine.reader.styles();
        let style = engine.reader.style(run.box_idx as usize);
        let language = style.language().and_then(|id| styles.language(id));
        let fallback = styles.hyphenation_fallback_language();
        let reader_selected_auto = engine.reader.layout_tree()
            .get_box_style_indices(run.box_idx as usize)
            .and_then(|indices| styles.view(indices))
            .is_some_and(|style| style.text.reader_hyphens == Some(Hyphens::Auto));
        // An unknown web-document language must not select an arbitrary
        // dictionary. Keep the reader's explicit/default English policy.
        let dictionary = (language.is_some() || fallback.is_some()
            || reader_selected_auto || engine.config.book_optimized_text())
            .then(|| hyphenation_language(language, fallback))
            .flatten();
        Self {
            language: language.or(fallback),
            dictionary,
            enabled: style.hyphens() == Hyphens::Auto
                && style.white_space().allows_wrap()
                && style.word_break() != WordBreak::BreakAll
                && style.line_break() != LineBreak::Anywhere
                && engine.text.hyphen_glyph(run.box_idx as usize).is_some(),
        }
    }
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
    fn flush(&mut self, engine: &LayoutEngine<'_, '_>, breaks: &mut Vec<u32>) {
        let Some((first, last)) = self.source.first().zip(self.source.last()) else {
            return;
        };
        if self.indexable
            && let Some(words) = engine
                .text
                .word_index()
                .and_then(|words| words.contiguous_words(first.glyphs.start..last.glyphs.end))
        {
            self.flush_indexed(engine, words, breaks);
        } else {
            // Generated/hidden text and source gaps can give the reader index
            // different boundaries. Build UTF-8 and byte positions only here.
            for source in &mut self.source {
                self.text.extend(
                    engine.text.content().characters()
                        [source.glyphs.start as usize..source.glyphs.end as usize]
                        .iter()
                        .copied(),
                );
                source.byte_end = self.text.len();
            }
            self.flush_segmented(engine, breaks);
        }
        self.text.clear();
        self.source.clear();
    }

    fn flush_indexed(
        &mut self,
        engine: &LayoutEngine<'_, '_>,
        words: &[WordSpan],
        breaks: &mut Vec<u32>,
    ) {
        let mut first_run = 0;
        let minimum_letters = if engine.config.hyphenation_quality() { 5 } else { 4 };
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
                engine.text.content().characters()[source.start as usize..source.end as usize]
                    .iter()
                    .copied(),
            );
            for_each_word_break(engine, &self.text, source.start, language, |glyph| {
                breaks.push(glyph);
            });
        }
    }

    fn flush_segmented(&self, engine: &LayoutEngine<'_, '_>, breaks: &mut Vec<u32>) {
        let mut source_index = 0;
        let mut source_byte = 0;
        let mut source_glyph = self.source.first().map_or(0, |run| run.glyphs.start);
        for (start, word) in self.text.unicode_word_indices() {
            self.word_breaks(engine, start, word, |boundary| {
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
                if engine.text.is_cluster_boundary(glyph as usize + 1) {
                    breaks.push(glyph);
                }
            });
        }
    }

    fn word_breaks(
        &self,
        engine: &LayoutEngine<'_, '_>,
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
        for_each_cached_word_break(engine, word, language, |offset| {
            emit(start + offset);
        });
    }
}

impl<'input> TextGroup<'input> {
    fn append_run(
        &mut self,
        engine: &LayoutEngine<'input, '_>,
        run: &InlineItem,
        glyphs: Range<u32>,
    ) {
        if glyphs.is_empty() {
            return;
        }
        let visible_dom = matches!(run.kind, InlineItemKind::Text { .. })
            && run.dom_text_node.is_some()
            && engine.reader.style(run.box_idx as usize).visibility() == Visibility::Visible;
        self.indexable = if let Some(previous) = self.source.last() {
            self.indexable && visible_dom && previous.glyphs.end == glyphs.start
        } else {
            visible_dom
        };
        let policy = WordPolicy::for_run(engine, run);
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

/// Analyze adjacent text together, retaining original source positions across
/// font/color/emphasis runs. Non-text content and formatting contexts separate
/// words; inline decoration boundaries do not.
pub(super) fn automatic_hyphen_breaks(
    engine: &LayoutEngine<'_, '_>,
    runs: &[InlineItem],
    container_box_idx: usize,
) -> Vec<u32> {
    profile_scope!("tokens.hyphenation");
    let mut breaks = Vec::new();
    if !runs
        .iter()
        .any(|run| engine.reader.style(run.box_idx as usize).hyphens() == Hyphens::Auto)
    {
        return breaks;
    }
    let mut group = TextGroup::default();
    for run in runs {
        if !engine
            .reader
            .inline_item_belongs_to_context(run, container_box_idx)
        {
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
                group.flush(engine, &mut breaks);
                continue;
            }
        };
        if marker {
            group.flush(engine, &mut breaks);
        }
        group.append_run(engine, run, glyphs.clone());
        if marker {
            group.flush(engine, &mut breaks);
        }
    }
    group.flush(engine, &mut breaks);
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
    engine: &LayoutEngine<'_, '_>,
    runs: &[InlineItem],
    container: usize,
) -> bool {
    let style = engine.reader.style(container);
    if engine.config.book_optimized_text()
        || style.text_wrap_style() == TextWrapStyle::Pretty
        || engine.config.force_justify()
        || style.text_align() == TextAlign::Justify
        || style.direction() != TextDirection::Ltr
        || style.overflow_x().clips()
        || !engine.text.is_resolved_default_ltr()
    {
        return false;
    }
    let mut previous = None;
    runs.iter().all(|run| {
        if !engine.reader.inline_item_belongs_to_context(run, container) {
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

struct MatchedWord {
    source: Range<u32>,
    breaks: Range<u32>,
}

#[derive(Default)]
pub(super) struct DeferredHyphenationCache {
    words: Vec<MatchedWord>,
    breaks: Vec<u32>,
}

impl DeferredHyphenationCache {
    pub(super) fn memory_usage_bytes(&self) -> usize {
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
pub(super) struct DeferredHyphenation<'input, 'cache> {
    runs: &'input [InlineItem],
    context: Range<u32>,
    words: DeferredWords<'input>,
    word: String,
    cache: &'cache mut DeferredHyphenationCache,
    cache_cursor: usize,
}

impl<'input, 'cache> DeferredHyphenation<'input, 'cache> {
    pub(super) fn new(
        engine: &LayoutEngine<'input, '_>,
        runs: Range<u32>,
        cache: &'cache mut DeferredHyphenationCache,
    ) -> Self {
        Self {
            runs: &engine.text.content().inline_items()[runs.start as usize..runs.end as usize],
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
                    && engine.reader.style(run.box_idx as usize).visibility()
                        == Visibility::Visible;
            }
        }
        self.context.start = start.unwrap_or(self.context.end);
        if indexed
            && let Some(words) = engine
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
            engine.text.content().characters()
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
            let current = WordPolicy::for_run(engine, run);
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

    fn match_word(&mut self, engine: &LayoutEngine<'input, '_>, source: Range<u32>) {
        let Some(policy) = self.word_policy(engine, &source) else {
            return;
        };
        self.word.clear();
        self.word.extend(
            engine.text.content().characters()[source.start as usize..source.end as usize]
                .iter()
                .copied(),
        );
        for_each_word_break(
            engine,
            &self.word,
            source.start,
            policy.dictionary.unwrap(),
            |glyph| self.cache.breaks.push(glyph),
        );
    }

    pub(super) fn last_break(
        &mut self,
        engine: &LayoutEngine<'input, '_>,
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

/// Translate dictionary byte offsets for one contiguous logical word. Both
/// eager indexed matching and deferred matching keep native cluster checks.
fn for_each_word_break(
    engine: &LayoutEngine<'_, '_>,
    word: &str,
    start: u32,
    language: HyphenationLanguage,
    mut emit: impl FnMut(u32),
) {
    let ascii = word.is_ascii();
    let mut previous_byte = 0;
    let mut glyph = start;
    for_each_cached_word_break(engine, word, language, |offset| {
        let after = if ascii {
            start + offset as u32
        } else {
            glyph += word[previous_byte..offset].chars().count() as u32;
            previous_byte = offset;
            glyph
        };
        if engine.text.is_cluster_boundary(after as usize) {
            emit(after - 1);
        }
    });
}

/// Exact ASCII dictionary results shared only by one layout execution.
/// Empty slots have a zero tag; eligible word lengths are always nonzero.
#[derive(Clone, Copy, Default)]
struct DictionaryEntry {
    word: [u64; 2],
    breaks: u16,
    tag: u16,
}

#[derive(Default)]
pub(in crate::layout) struct HyphenationDictionaryCache {
    entries: RefCell<Option<Box<[DictionaryEntry; 256]>>>,
}

fn for_each_cached_word_break(
    engine: &LayoutEngine<'_, '_>,
    word: &str,
    language: HyphenationLanguage,
    mut emit: impl FnMut(usize),
) {
    let quality = engine.config.hyphenation_quality();
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
    let mut cache = engine.hyphenation_dictionary.entries.borrow_mut();
    let entries = cache.get_or_insert_with(|| Box::new([DictionaryEntry::default(); 256]));
    let entry = &mut entries[slot];
    if entry.tag == tag && entry.word == key {
        let mut mask = entry.breaks;
        // Consumers can read the engine or issue another lookup while emitting.
        drop(cache);
        while mask != 0 {
            emit(mask.trailing_zeros() as usize);
            mask &= mask - 1;
        }
        return;
    }
    let result = word_breaks(word, language, quality);
    let mask = result.iter().fold(0, |mask, &offset| mask | 1u16 << offset);
    *entry = DictionaryEntry { word: key, breaks: mask, tag };
    drop(cache);
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
