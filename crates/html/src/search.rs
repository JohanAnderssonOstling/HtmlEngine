//! Search within one document. Hosts choose publication scope, highlights, and
//! whether moving the logical result cursor should change the visible region.

use crate::layout::RenderView;
use crate::text::TextSearchOptions;
use std::ops::Range;

use crate::content::TextStatistics;
use crate::document::{
    DocumentTextIndex, MarkupSyntax, SourceTextPosition, collapse_whitespace, parse_document,
    read_document_source,
};
use crate::engine::Error;
use crate::resources::ResourceProvider;
use crate::text::text_matches;

/// Parse-only source search. CSS visibility, generated content and layout are
/// not evaluated; use `Search` for matches in rendered visible source text.
pub struct SearchIndex {
    index: DocumentTextIndex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedMatch {
    pub range: Range<usize>,
    pub source: SourceTextPosition,
}

impl SearchIndex {
    pub fn from_index(index: DocumentTextIndex) -> Self {
        Self { index }
    }

    pub fn from_source(source: &str, syntax: MarkupSyntax) -> Result<Self, Error> {
        let parsed = parse_document(source, syntax).map_err(|error| Error(error.to_string()))?;
        let index =
            DocumentTextIndex::from_parsed(&parsed).map_err(|error| Error(error.to_string()))?;
        Ok(Self::from_index(index))
    }

    /// Decode a resource using its transport metadata without loading CSS,
    /// fonts, images, or constructing layout.
    pub fn load(provider: &dyn ResourceProvider, uri: &str) -> Result<Self, Error> {
        let (source, syntax) = read_document_source(provider, uri)
            .map_err(|error| Error(error.to_string()))?;
        Self::from_source(&source, syntax)
    }

    pub fn text(&self) -> &str {
        self.index.text()
    }

    /// Source positions are resolved as the caller consumes matches. Publication
    /// code can stop iteration without materializing every address or excerpt.
    pub fn find(
        &self,
        query: &str,
        options: TextSearchOptions,
    ) -> impl Iterator<Item = IndexedMatch> + '_ {
        text_matches(self.index.text(), query, options).filter_map(|(start, end)| {
            let range = start..end;
            Some(IndexedMatch {
                source: self.index.source_position_in(range.clone())?,
                range,
            })
        })
    }

    pub fn excerpt(&self, range: Range<usize>, before: usize, after: usize) -> Option<String> {
        if range.start >= range.end {
            return None;
        }
        let count = self.index.character_count();
        if range.end > count {
            return None;
        }
        let start = range.start.saturating_sub(before);
        let end = range.end.saturating_add(after).min(count);
        Some(self.index.text_range(start..end)?.trim().to_owned())
    }

    pub fn statistics(&self) -> TextStatistics {
        TextStatistics::from_text(self.index.text())
    }
}

#[derive(Clone, Copy)]
pub struct Search<'a> {
    view: RenderView<'a>,
}

impl<'a> Search<'a> {
    pub fn new(view: RenderView<'a>) -> Self {
        Self { view }
    }

    /// Match visible source text, retaining logical character positions through
    /// Unicode normalization and across inline elements.
    pub fn find(self, query: &str, options: TextSearchOptions) -> SearchResults {
        SearchResults::new(self.view.text().find_matches(query, options))
    }

    /// Export a preview with caller-selected character context and collapsed
    /// whitespace. Hidden text and generated markers are excluded.
    pub fn excerpt(self, range: Range<u32>, before: u32, after: u32) -> Option<String> {
        let count = self.view.text().glyph_count().min(u32::MAX as usize) as u32;
        if range.start >= range.end || range.end > count {
            return None;
        }
        let text = self.view.export().plain_text(
            range.start.saturating_sub(before)..range.end.saturating_add(after).min(count),
        )?;
        Some(collapse_whitespace(&text))
    }
}

/// An owned set of logical ranges and a result cursor, independent of layout
/// coordinates. Recompute matches when document content or visibility changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResults {
    ranges: Vec<Range<u32>>,
    current: usize,
}

/// Query, matching options and result navigation for one document. Refresh
/// against the current render view after source/style changes. The host owns
/// search UI activation, publication scope and revealing the current result.
#[derive(Clone, Debug, Default)]
pub struct DocumentSearch {
    query: String,
    options: TextSearchOptions,
    results: SearchResults,
}

impl DocumentSearch {
    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn options(&self) -> TextSearchOptions {
        self.options
    }
    pub fn results(&self) -> &SearchResults {
        &self.results
    }
    pub fn results_mut(&mut self) -> &mut SearchResults {
        &mut self.results
    }

    /// Reset the logical cursor and drop stale results when adopting a query.
    pub fn configure(&mut self, query: String, options: TextSearchOptions) {
        self.query = query;
        self.options = options;
        self.results = SearchResults::default();
    }

    pub fn set_query(&mut self, view: RenderView<'_>, query: String, options: TextSearchOptions) {
        self.configure(query, options);
        self.refresh(view);
    }

    /// Recompute using document visibility/Unicode rules, preserving the cursor
    /// where possible. This does not move a viewport or emit application events.
    pub fn refresh(&mut self, view: RenderView<'_>) {
        self.results.replace(
            Search::new(view)
                .find(&self.query, self.options)
                .into_ranges(),
        );
    }
}

impl SearchResults {
    pub fn new(ranges: Vec<Range<u32>>) -> Self {
        Self { ranges, current: 0 }
    }

    pub fn ranges(&self) -> &[Range<u32>] {
        &self.ranges
    }

    pub fn into_ranges(self) -> Vec<Range<u32>> {
        self.ranges
    }

    pub fn current_index(&self) -> usize {
        self.current
    }

    pub fn current(&self) -> Option<Range<u32>> {
        self.ranges.get(self.current).cloned()
    }

    /// Install refreshed ranges, preserving the cursor when possible.
    pub fn replace(&mut self, ranges: Vec<Range<u32>>) {
        self.ranges = ranges;
        self.focus(self.current);
    }

    /// Select a result, leaving the cursor unchanged for an invalid index.
    pub fn select(&mut self, index: usize) -> bool {
        if index >= self.ranges.len() {
            return false;
        }
        self.current = index;
        true
    }

    /// Focus an occurrence from another source, clamping it to current results.
    pub fn focus(&mut self, index: usize) {
        self.current = index.min(self.ranges.len().saturating_sub(1));
    }

    pub fn next_match(&mut self) -> bool {
        if self.ranges.is_empty() {
            return false;
        }
        self.current = (self.current + 1) % self.ranges.len();
        true
    }

    pub fn previous_match(&mut self) -> bool {
        if self.ranges.is_empty() {
            return false;
        }
        self.current = (self.current + self.ranges.len() - 1) % self.ranges.len();
        true
    }
}
