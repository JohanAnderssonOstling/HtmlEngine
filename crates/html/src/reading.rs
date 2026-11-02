//! Document-local reading units. Playback, region splitting, and following the
//! active passage belong to the embedding application.
use crate::layout::RenderView;
pub use crate::layout::{ReadingFragment, ReadingTarget};
use std::ops::Range;

#[derive(Clone, Copy)]
pub struct Reading<'a> {
    view: RenderView<'a>,
}

/// Logical position among document reading units. Exhaustion is represented by
/// `index == targets.len()`; advancing never wraps or initiates navigation.
pub struct ReadingCursor {
    targets: Vec<ReadingTarget>,
    index: usize,
}

impl ReadingCursor {
    pub fn targets(&self) -> &[ReadingTarget] {
        &self.targets
    }
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn current(&self) -> Option<&ReadingTarget> {
        self.targets.get(self.index)
    }
    pub fn seek(&mut self, glyph: u32) {
        self.index = self
            .targets
            .iter()
            .position(|target| target.glyph_ranges.iter().any(|range| range.end > glyph))
            .unwrap_or(self.targets.len());
    }
    pub fn advance(&mut self) -> bool {
        self.index = self.index.saturating_add(1).min(self.targets.len());
        self.current().is_some()
    }
    pub fn previous(&mut self) -> bool {
        if self.targets.is_empty() || self.index == 0 {
            return false;
        }
        self.index -= 1;
        true
    }
    pub fn into_parts(self) -> (usize, Vec<ReadingTarget>) {
        (self.index, self.targets)
    }
}

impl<'a> Reading<'a> {
    pub fn cursor(self, glyph: u32) -> ReadingCursor {
        let mut cursor = ReadingCursor {
            targets: self.targets(0..u32::MAX),
            index: 0,
        };
        cursor.seek(glyph);
        cursor
    }
    pub fn new(view: RenderView<'a>) -> Self {
        Self { view }
    }
    pub fn targets(self, range: Range<u32>) -> Vec<ReadingTarget> {
        self.view.addressing().reading_targets_in(range)
    }
    pub fn fragments(self, range: Range<u32>) -> Vec<ReadingFragment> {
        self.view.addressing().reading_fragments_in(range)
    }
    pub fn text(self, target: &ReadingTarget) -> String {
        self.view.addressing().reading_text(target)
    }
    pub fn boundary_after_word(self, end: u32) -> u32 {
        self.view.addressing().reading_boundary_after_word(end)
    }
    pub fn chunk(
        self,
        target: &ReadingTarget,
        start: u32,
        max_chars: usize,
        select: impl FnOnce(&str) -> Option<Range<usize>>,
    ) -> Option<(ReadingFragment, Option<u32>)> {
        self.view
            .addressing()
            .reading_chunk(target, start, max_chars, select)
    }
}
