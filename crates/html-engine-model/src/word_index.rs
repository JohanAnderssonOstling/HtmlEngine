//! Retained logical word boundaries, shared by reader queries and layout.

use html_dom::MemoryUsageReport;
use std::ops::Range;

/// One contiguous source span belonging to a word or non-whitespace grapheme.
/// A word can cover several spans when visibility filtering omitted source.
pub struct WordSpan {
    source: Range<u32>,
}

impl WordSpan {
    #[inline]
    pub fn range(&self) -> Range<u32> {
        self.source.clone()
    }
}

struct WordGap {
    source: Range<u32>,
    word: Range<u32>,
}

#[derive(Default)]
pub struct WordIndex {
    spans: Vec<WordSpan>,
    gaps: Vec<WordGap>,
}

impl WordIndex {
    pub fn push(&mut self, positions: &[u32]) {
        let Some((&first, &last)) = positions.first().zip(positions.last()) else {
            return;
        };
        let word = first..last + 1;
        let mut start = first;
        let mut end = first + 1;
        for &glyph in &positions[1..] {
            if glyph != end {
                self.spans.push(WordSpan { source: start..end });
                self.gaps.push(WordGap {
                    source: start..end,
                    word: word.clone(),
                });
                start = glyph;
            }
            end = glyph + 1;
        }
        self.spans.push(WordSpan { source: start..end });
        if start != first {
            self.gaps.push(WordGap {
                source: start..end,
                word,
            });
        }
    }

    /// Append an entry whose source characters have no visibility gaps.
    #[inline]
    pub fn push_contiguous(&mut self, source: Range<u32>) {
        if !source.is_empty() {
            self.spans.push(WordSpan { source });
        }
    }

    pub fn sort(&mut self) {
        self.spans.sort_unstable_by_key(|span| span.source.start);
        self.gaps.sort_unstable_by_key(|gap| gap.source.start);
        // Word boundaries remain immutable across reflows and restyles.
        self.spans.shrink_to_fit();
        self.gaps.shrink_to_fit();
    }

    /// Borrow only complete, contiguous entries inside this formatting context.
    /// Callers must also establish that the context contains visible DOM text;
    /// visibility-filtered or synthetic text can have different boundaries.
    pub fn contiguous_words(&self, source: Range<u32>) -> Option<&[WordSpan]> {
        let start = self
            .spans
            .partition_point(|span| span.source.end <= source.start);
        let end = self
            .spans
            .partition_point(|span| span.source.start < source.end);
        let spans = &self.spans[start..end];
        let gap = self
            .gaps
            .partition_point(|gap| gap.source.end <= source.start);
        if self
            .gaps
            .get(gap)
            .is_some_and(|gap| gap.source.start < source.end)
        {
            return None;
        }
        spans
            .iter()
            .all(|span| source.start <= span.source.start && span.source.end <= source.end)
            .then_some(spans)
    }

    pub fn range_at(&self, glyph: u32) -> Option<Range<u32>> {
        let index = self.spans.partition_point(|span| span.source.end <= glyph);
        let span = self.spans.get(index)?;
        span.source
            .contains(&glyph)
            .then(|| self.word_for_span(span))
    }

    pub fn in_range(&self, range: Range<u32>) -> impl Iterator<Item = Range<u32>> + '_ {
        let start = self
            .spans
            .partition_point(|span| span.source.end <= range.start);
        self.spans[start..]
            .iter()
            .take_while(move |span| !range.is_empty() && span.source.start < range.end)
            .map(|span| self.word_for_span(span))
    }

    fn word_for_span(&self, span: &WordSpan) -> Range<u32> {
        let gap = self
            .gaps
            .partition_point(|gap| gap.source.start < span.source.start);
        self.gaps
            .get(gap)
            .filter(|gap| gap.source.start == span.source.start)
            .map_or_else(|| span.source.clone(), |gap| gap.word.clone())
    }

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_vec_storage("spans", &self.spans);
        report.add_vec_storage("gaps", &self.gaps);
        report
    }
}
