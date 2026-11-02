//! Source text and resource dependencies for caller-selected document content.

use crate::layout::RenderView;
use kurbo::Rect;
use std::collections::HashSet;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextStatistics {
    pub characters: u64,
    pub non_whitespace_characters: u64,
}

impl TextStatistics {
    pub fn from_text(text: &str) -> Self {
        text.chars().fold(Self::default(), |mut stats, character| {
            stats.characters += 1;
            stats.non_whitespace_characters += u64::from(!character.is_whitespace());
            stats
        })
    }
}

#[derive(Clone, Copy)]
pub struct Content<'a> {
    view: RenderView<'a>,
}

impl<'a> Content<'a> {
    pub fn new(view: RenderView<'a>) -> Self {
        Self { view }
    }

    /// Export selected lines in source order, preserving document structure.
    /// Repeated lines are included once. Hosts choose visibility and ordering
    /// of regions; synthetic markers and hidden text never become source text.
    pub fn text_for_lines(self, lines: impl IntoIterator<Item = usize>) -> Option<String> {
        self.text_for_ranges(lines.into_iter().flat_map(|line| {
            self.view
                .text()
                .line_text_fragments(line)
                .into_iter()
                .flatten()
                .filter(|fragment| fragment.visible())
                .map(|fragment| fragment.glyphs())
        }))
    }

    pub fn text_for_ranges(self, ranges: impl IntoIterator<Item = Range<u32>>) -> Option<String> {
        self.view.export().plain_text_ranges(ranges)
    }

    /// Counts visible exported source characters, independent of wrapping.
    /// Unicode scalar values are counted; bytes, glyph shapes and grapheme
    /// clusters deliberately do not affect this convention.
    pub fn statistics(self) -> TextStatistics {
        let count = self.view.text().glyph_count().min(u32::MAX as usize) as u32;
        TextStatistics::from_text(&self.view.export().plain_text(0..count).unwrap_or_default())
    }

    pub fn image_ids_for_lines(self, lines: impl IntoIterator<Item = usize>) -> HashSet<u32> {
        let mut images = HashSet::new();
        for line in lines {
            images.extend(
                self.view
                    .fragments()
                    .images_for_line(line)
                    .iter()
                    .map(|image| image.image_idx()),
            );
        }
        images
    }

    pub fn image_ids_in(self, region: Rect) -> HashSet<u32> {
        let text = self.view.text();
        self.view
            .fragments()
            .images()
            .iter()
            .filter_map(|image| {
                let line = text.line(image.line_idx())?;
                let visible = (image.visible_rect() + line.point().to_vec2()).intersect(region);
                (visible.width() > 0.0 && visible.height() > 0.0).then_some(image.image_idx())
            })
            .collect()
    }
}
