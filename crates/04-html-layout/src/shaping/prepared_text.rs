//! Immutable logical text prepared independently of renderer font resources.

use super::{collapse_whitespace, first_letter_style_overrides, transform_text};
use crate::layout_model::{InlineContent, LayoutTree};
use html_dom::{Document, MemoryUsageReport};
use html_style_model::{ComputedStyles, StyleIndices};
use std::ops::Range;

/// Text transforms, whitespace and source correspondence depend on publication
/// styles, not the lifetime of a renderer's glyph cache. Keep their result with
/// preparation so replacing font resources only repeats native shaping.
pub(crate) struct PreparedText {
    content: InlineContent,
    first_letter_styles: FirstLetterStyles,
}

impl PreparedText {
    pub(crate) fn new(
        document: &Document,
        styles: &ComputedStyles,
        tree: &LayoutTree,
        mut content: InlineContent,
    ) -> Self {
        profile_scope!("prepare.logical_text");
        let unicode_roots = if content.characters().is_empty() {
            Default::default()
        } else {
            profile!("prepare.whitespace", collapse_whitespace(styles, tree, &mut content))
        };
        let mut first_letter_styles =
            profile!("prepare.first_letter", first_letter_style_overrides(document, styles, tree, &content));
        if !content.characters().is_empty() {
            profile!("prepare.text_transform", transform_text(styles, tree, &mut content, &mut first_letter_styles));
        }
        profile!("prepare.unicode_line_breaks", super::line_breaks::resolve(styles, tree, &mut content, &unicode_roots));
        Self {
            content,
            first_letter_styles: profile!("prepare.compact_first_letter", FirstLetterStyles::new(&first_letter_styles)),
        }
    }

    pub(crate) fn content(&self) -> &InlineContent {
        &self.content
    }

    pub(crate) fn first_letter_styles(&self) -> &FirstLetterStyles {
        &self.first_letter_styles
    }

    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = self.content.memory_usage_report();
        report.add_slice_storage::<StyleOverrideRun>(
            "PreparedText.first_letter_styles.storage",
            self.first_letter_styles.runs.len(),
            self.first_letter_styles.runs.len(),
        );
        report
    }
}

/// Read one contiguous style segment without scanning all unstyled characters.
/// Dense slices remain useful to normalization tests; prepared text keeps only
/// the sparse first-letter ranges that survive whitespace and case conversion.
pub(super) trait StyleOverrides {
    fn segment_at(&self, start: u32, end: u32) -> (Option<StyleIndices>, u32);
}

pub(crate) struct FirstLetterStyles {
    runs: Box<[StyleOverrideRun]>,
}

struct StyleOverrideRun {
    glyphs: Range<u32>,
    style: StyleIndices,
}

impl FirstLetterStyles {
    pub(crate) fn segment(&self, start: u32, end: u32) -> (Option<StyleIndices>, u32) {
        self.segment_at(start, end)
    }

    fn new(dense: &[Option<StyleIndices>]) -> Self {
        let mut runs: Vec<StyleOverrideRun> = Vec::new();
        for (index, &style) in dense.iter().enumerate() {
            let Some(style) = style else {
                continue;
            };
            let index = index as u32;
            if let Some(last) = runs
                .last_mut()
                .filter(|last| last.glyphs.end == index && last.style == style)
            {
                last.glyphs.end += 1;
            } else {
                runs.push(StyleOverrideRun {
                    glyphs: index..index + 1,
                    style,
                });
            }
        }
        Self { runs: runs.into() }
    }

    #[cfg(test)]
    pub(crate) fn style_at(&self, glyph: u32) -> Option<StyleIndices> {
        self.segment_at(glyph, glyph + 1).0
    }
}

impl StyleOverrides for FirstLetterStyles {
    fn segment_at(&self, start: u32, end: u32) -> (Option<StyleIndices>, u32) {
        let index = self.runs.partition_point(|run| run.glyphs.end <= start);
        match self.runs.get(index) {
            Some(run) if run.glyphs.start <= start => (Some(run.style), end.min(run.glyphs.end)),
            Some(run) => (None, end.min(run.glyphs.start)),
            None => (None, end),
        }
    }
}

#[cfg(test)]
impl StyleOverrides for [Option<StyleIndices>] {
    fn segment_at(&self, start: u32, end: u32) -> (Option<StyleIndices>, u32) {
        if self.is_empty() {
            return (None, end);
        }
        let style = self[start as usize];
        let mut boundary = start + 1;
        while boundary < end && self[boundary as usize] == style {
            boundary += 1;
        }
        (style, boundary)
    }
}

#[cfg(test)]
impl StyleOverrides for Vec<Option<StyleIndices>> {
    fn segment_at(&self, start: u32, end: u32) -> (Option<StyleIndices>, u32) {
        self.as_slice().segment_at(start, end)
    }
}

#[cfg(test)]
impl<const N: usize> StyleOverrides for [Option<StyleIndices>; N] {
    fn segment_at(&self, start: u32, end: u32) -> (Option<StyleIndices>, u32) {
        self.as_slice().segment_at(start, end)
    }
}
