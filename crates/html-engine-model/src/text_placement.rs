//! Shared used advances and paint offsets over a published logical line.

use crate::{GlyphAdvanceRun, GlyphOffsetRun, InlineContent, Line};
use crate::{GlyphId, GlyphMetric, GlyphMetrics, GlyphSlice, TextGeometryView};
use html_text::{spaced_advance, suppresses_glyph, tracking_boundary_after};

/// A character's resolved spacing and paint identity. Coordinates remain
/// relative to the containing text fragment; painters own device projection.
pub struct LineCharacterPlacement {
    pub metric: GlyphMetric,
    pub paint_glyph: Option<GlyphId>,
    pub natural_advance: f64,
    pub advance: f64,
    pub offset_y: f64,
}

/// Allocation-free cursor over a line's shaping and sparse placement data.
/// Forward reads retain lookup hints; backward reads also remain valid.
pub struct LineTextPlacement<'a> {
    content: &'a InlineContent,
    glyphs: GlyphSlice<'a>,
    metrics: &'a GlyphMetrics,
    geometry: Option<TextGeometryView<'a>>,
    line: &'a Line,
    offsets: &'a [GlyphOffsetRun],
    advances: &'a [GlyphAdvanceRun],
    metric_hint: usize,
    geometry_hint: usize,
    offset_index: usize,
    advance_index: usize,
    previous: Option<u32>,
}

impl<'a> LineTextPlacement<'a> {
    pub fn new(
        content: &'a InlineContent,
        glyphs: GlyphSlice<'a>,
        metrics: &'a GlyphMetrics,
        geometry: Option<TextGeometryView<'a>>,
        line: &'a Line,
        offsets: &'a [GlyphOffsetRun],
        advances: &'a [GlyphAdvanceRun],
    ) -> Self {
        Self {
            content,
            glyphs,
            metrics,
            geometry,
            line,
            offsets,
            advances,
            metric_hint: 0,
            geometry_hint: 0,
            offset_index: 0,
            advance_index: 0,
            previous: None,
        }
    }

    pub fn at(&mut self, index: u32) -> Option<LineCharacterPlacement> {
        if !self.line.glyphs.contains(&index) {
            return None;
        }
        let glyph = *self.glyphs.get(index as usize)?;
        let metric = self.metrics.get_with_hint(glyph, &mut self.metric_hint)?;
        let character = self.content.character_at(index as usize)?;
        let natural_advance = f64::from(
            self.geometry
                .and_then(|geometry| {
                    geometry.advance_with_hint(index as usize, &mut self.geometry_hint)
                })
                .unwrap_or_else(|| metric.advance()),
        );
        let boundary = self.geometry.is_none_or(|geometry| {
            geometry.boundary_with_hint(index as usize + 1, &mut self.geometry_hint)
        });

        if self.previous.is_none_or(|previous| index < previous) {
            self.offset_index = self.offsets.partition_point(|run| run.range.end <= index);
            self.advance_index = self.advances.partition_point(|run| run.range.end <= index);
        } else {
            while self
                .offsets
                .get(self.offset_index)
                .is_some_and(|run| run.range.end <= index)
            {
                self.offset_index += 1;
            }
            while self
                .advances
                .get(self.advance_index)
                .is_some_and(|run| run.range.end <= index)
            {
                self.advance_index += 1;
            }
        }
        self.previous = Some(index);
        let offset_y = self
            .offsets
            .get(self.offset_index)
            .filter(|run| run.range.contains(&index))
            .map_or(0.0, |run| f64::from(run.offset));
        let advance_override = self
            .advances
            .get(self.advance_index)
            .filter(|run| run.range.contains(&index))
            .map(|run| f64::from(run.advance));
        let tracking = tracking_boundary_after(self.content, index, self.line.glyphs.end, boundary);
        let advance = advance_override.unwrap_or_else(|| {
            spaced_advance(
                natural_advance,
                character,
                tracking,
                self.line.word_spacing,
                self.line.letter_spacing,
            )
        });
        let paint_glyph = (advance_override.is_none() && !suppresses_glyph(character)).then(|| {
            self.geometry
                .map_or(glyph, |geometry| geometry.paint_glyph(index, glyph))
        });
        Some(LineCharacterPlacement {
            metric,
            paint_glyph,
            natural_advance,
            advance,
            offset_y,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ShapedTextGeometry, ShapedTextRun};

    fn fixture(text: &str) -> (InlineContent, Vec<GlyphId>, GlyphMetrics, Line) {
        let mut content = InlineContent::default();
        let mut metrics = GlyphMetrics::default();
        let mut glyphs = Vec::new();
        for character in text.chars() {
            content.push_character(character);
            let advance = if suppresses_glyph(character) {
                0.0
            } else {
                8.0
            };
            glyphs.push(
                metrics
                    .register(GlyphMetric::try_new(character, advance, 12.0, 4.0).unwrap())
                    .unwrap(),
            );
        }
        let line = Line {
            glyphs: 0..glyphs.len() as u32,
            word_spacing: 3.0,
            letter_spacing: 2.0,
            ..Default::default()
        };
        (content, glyphs, metrics, line)
    }

    #[test]
    fn controls_soft_hyphens_and_sparse_overrides_preserve_used_advances() {
        let (content, glyphs, metrics, line) = fixture("A\u{00ad}B\u{200e}C \tD");
        let offsets = [GlyphOffsetRun {
            range: 6..7,
            offset: 4.0,
        }];
        let advances = [GlyphAdvanceRun {
            range: 6..7,
            advance: 32.0,
        }];
        let mut cursor = LineTextPlacement::new(
            &content,
            GlyphSlice::new(&glyphs, &[]),
            &metrics,
            None,
            &line,
            &offsets,
            &advances,
        );
        let expected = [8.0, 0.0, 10.0, 0.0, 10.0, 13.0, 32.0, 8.0];
        for (index, expected) in expected.into_iter().enumerate() {
            let character = cursor.at(index as u32).unwrap();
            assert_eq!(character.advance, expected, "character {index}");
            assert_eq!(character.paint_glyph.is_some(), ![1, 3, 6].contains(&index));
        }
        assert!(cursor.at(8).is_none());
        // Visual bidi fragments may visit logical characters in reverse order.
        assert_eq!(cursor.at(6).unwrap().offset_y, 4.0);
        assert_eq!(cursor.at(2).unwrap().advance, 10.0);
        assert_eq!(cursor.at(6).unwrap().advance, 32.0);
        assert_eq!(cursor.at(0).unwrap().offset_y, 0.0);
    }

    #[test]
    fn native_cluster_boundaries_control_tracking_and_override_metric_widths() {
        let (content, glyphs, metrics, line) = fixture("e\u{301}x");
        let mut geometry = ShapedTextGeometry::with_len(glyphs.len());
        geometry.push_run(
            &[0, 1, 2],
            ShapedTextRun::new(
                vec![5.0, 0.0, 9.0].into(),
                vec![true, false, true, true].into(),
            )
            .unwrap(),
            false,
        );
        geometry.finish_runs();
        let mut cursor = LineTextPlacement::new(
            &content,
            GlyphSlice::new(&glyphs, &[]),
            &metrics,
            Some(TextGeometryView::new(&geometry, None)),
            &line,
            &[],
            &[],
        );
        assert_eq!(
            cursor.at(0).unwrap().advance,
            5.0,
            "no tracking inside a shaped cluster"
        );
        assert_eq!(
            cursor.at(1).unwrap().advance,
            2.0,
            "tracking follows the complete cluster"
        );
        assert_eq!(
            cursor.at(2).unwrap().advance,
            9.0,
            "no tracking beyond the line end"
        );
    }
}
