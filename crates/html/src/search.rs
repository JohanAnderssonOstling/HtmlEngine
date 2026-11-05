//! Search within one document. Hosts choose publication scope, highlights, and
//! whether moving the logical result cursor should change the visible region.

use html_pipeline::engine::search_queries;
use crate::layout::RenderView;
use crate::text::TextSearchOptions;
use std::ops::Range;

use html_parse::MarkupSyntax;
use crate::document::SourceTextPosition;
use html_parse::{DocumentTextIndex, parse_document, read_document_source};
use crate::Error;
use crate::resources::ResourceProvider;
use html_pipeline::engine::text_matches;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextStatistics {
    pub characters: u64,
    pub non_whitespace_characters: u64,
}

impl TextStatistics {
    pub(crate) fn from_text(text: &str) -> Self {
        text.chars().fold(Self::default(), |mut stats, character| {
            stats.characters += 1;
            stats.non_whitespace_characters += u64::from(!character.is_whitespace());
            stats
        })
    }
}

/// Parse-only source search. CSS visibility, generated content and layout are
/// not evaluated; use `DocumentSearch` for matches in rendered visible source text.
pub struct SearchIndex {
    index: DocumentTextIndex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedMatch {
    pub range: Range<usize>,
    pub source: SourceTextPosition,
}

impl SearchIndex {
    pub(crate) fn from_source(source: &str, syntax: MarkupSyntax) -> Result<Self, Error> {
        let parsed = parse_document(source, syntax).map_err(Error::from)?;
        let index = DocumentTextIndex::from_parsed(&parsed).map_err(Error::from)?;
        Ok(Self { index })
    }

    /// Decode a resource using its transport metadata without loading CSS,
    /// fonts, images, or constructing layout.
    pub fn load(provider: &dyn ResourceProvider, uri: &str) -> Result<Self, Error> {
        let (source, syntax) = read_document_source(provider, uri)
            .map_err(Error::from)?;
        Self::from_source(&source, syntax)
    }

    /// Resolve a character offset in this exact extracted text to its DOM source.
    pub(crate) fn source_at_character(&self, offset: usize) -> Option<SourceTextPosition> {
        self.index
            .source_position_in(offset..offset.checked_add(1)?)
    }

    pub(crate) fn text(&self) -> &str {
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

/// Query, matching options and result navigation for one document. Refresh
/// against the current render view after source/style changes. The host owns
/// search UI activation, publication scope and revealing the current result.
#[derive(Clone, Debug, Default)]
pub struct DocumentSearch {
    query: String,
    options: TextSearchOptions,
    ranges: Vec<Range<u32>>,
    current: usize,
}

impl DocumentSearch {
    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn options(&self) -> TextSearchOptions {
        self.options
    }

    /// Logical matches in the last refreshed view, independent of layout coordinates.
    pub fn ranges(&self) -> &[Range<u32>] {
        &self.ranges
    }

    /// The focused occurrence and its range, or `None` when there are no matches.
    pub fn current(&self) -> Option<(usize, Range<u32>)> {
        self.ranges
            .get(self.current)
            .cloned()
            .map(|range| (self.current, range))
    }

    /// Reset the logical cursor and drop stale results when adopting a query.
    pub fn configure(&mut self, query: String, options: TextSearchOptions) {
        self.query = query;
        self.options = options;
        self.ranges.clear();
        self.current = 0;
    }

    /// Focus a result, clamping the occurrence to the current matches.
    pub fn focus(&mut self, occurrence: usize) {
        self.current = occurrence.min(self.ranges.len().saturating_sub(1));
    }

    /// Recompute using document visibility/Unicode rules, preserving the cursor
    /// where possible. This does not move a viewport or emit application events.
    pub fn refresh(&mut self, view: RenderView<'_>) {
        if self.query.is_empty() {
            return;
        }
        self.ranges = search_queries::render_text_view_find_matches(view.text(), &self.query, self.options);
        self.focus(self.current);
    }
}

/// Compiled multi-pattern search, including source-backed matches in selected lines.
pub use html_pipeline::engine::{
    SourceTextMatch, SourceTextMatches, TextPatternError, TextPatternMatcher,
};
