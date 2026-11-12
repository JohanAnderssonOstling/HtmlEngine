//! Values shared by engine implementations and reader interfaces.
use html_dom::DomNodeId;
use std::ops::Range;

/// A resolved page boundary in document coordinates. A cursor preserves group
/// identity when multiple groups share an offset; an offset alone is available
/// before a page is installed.
#[derive(Clone, Copy, Debug)]
pub struct PageAnchor {
    pub cursor: Option<usize>,
    pub offset_y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayStyle {
    Highlight,
    Underline,
    Squiggly,
    Strikethrough,
}

/// Text matching options; publication scope and result navigation are UI policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TextSearchOptions {
    pub match_case: bool,
    pub whole_word: bool,
    pub match_diacritics: bool,
}

/// A reading unit with private source identity and document-local character ranges.
/// Font shaping and pagination preserve its identity while text stays fixed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadingTarget {
    element: DomNodeId,
    pub glyph_ranges: Vec<Range<u32>>,
}

impl ReadingTarget {
    /// Derive a passage or speech-chunk target while preserving its source identity.
    /// Ranges belong to the same document; the caller's vector is moved without copying.
    #[inline]
    pub fn with_glyph_ranges(&self, glyph_ranges: Vec<Range<u32>>) -> ReadingTarget {
        ReadingTarget {
            element: self.element,
            glyph_ranges,
        }
    }

    /// Construct a source-addressed reading unit from engine query results.
    pub fn new(element: DomNodeId, glyph_ranges: Vec<Range<u32>>) -> Self {
        Self {
            element,
            glyph_ranges,
        }
    }

    pub fn element(&self) -> DomNodeId {
        self.element
    }
}
