use html_engine_model::{PaintColorRun, TextDirection};
use std::ops::Range;

use kurbo::Point;

use crate::layout::fragment_writer::FragmentWriter;
use crate::layout::inline_reader::InlineReader;
use crate::layout_model::{GlyphAdvanceRun, GlyphOffsetRun, Line, LineDetails, LineTextFragment, PreparedTextRunFragment};

fn prepare_line_text_runs(text: &InlineReader<'_>, fragments: &mut FragmentWriter<'_>, colors: &[PaintColorRun]) {
    profile_scope!("finalize.text_paint_runs");
    let output = &mut fragments.state_mut().line_output;
    output.prepared_text_run_fragments.clear();
    for line_idx in 0..output.lines.len() {
        let start = output.prepared_text_run_fragments.len();
        let complete = prepare_line(
            text,
            &output.lines[line_idx],
            &output.line_details,
            &output.text_fragments,
            output
                .line_glyph_offsets
                .get(line_idx)
                .unwrap_or(&[]),
            output
                .line_glyph_advances
                .get(line_idx)
                .unwrap_or(&[]),
            &mut output.prepared_text_run_fragments,
            colors,
        )
        .is_some();
        if !complete {
            output.prepared_text_run_fragments.truncate(start);
        }
        let end = output.prepared_text_run_fragments.len();
        output.lines[line_idx].prepared_text_runs = u32::try_from(start)
            .expect("prepared text fragment index exceeds layout capacity")
            ..u32::try_from(end).expect("prepared text fragment index exceeds layout capacity");
        output.lines[line_idx].native_text_runs_complete = complete;
    }
}

fn prepare_line(text: &InlineReader<'_>, line: &Line, details: &[LineDetails], text_fragments: &[LineTextFragment], offsets: &[GlyphOffsetRun], advances: &[GlyphAdvanceRun], prepared: &mut Vec<PreparedTextRunFragment>, colors: &[PaintColorRun]) -> Option<()> {
    let optical_offset_x = line.optical_offset_x(details);
    if let Some(range) = line.text_fragment_range(details) {
        for fragment in text_fragments[range.start as usize..range.end as usize].iter().filter(|fragment| fragment.visible) {
            prepare_fragment(text, line, fragment.glyphs.clone(), fragment.offset_x, offsets, advances, prepared, colors, optical_offset_x)?;
        }
    } else if !line.glyphs.is_empty() {
        prepare_fragment(text, line, line.glyphs.clone(), 0.0, offsets, advances, prepared, colors, optical_offset_x)?;
    }

    Some(())
}

#[allow(clippy::too_many_arguments)]
fn prepare_fragment(
    text: &InlineReader<'_>, line: &Line, source_range: Range<u32>, fragment_offset_x: f64, offsets: &[GlyphOffsetRun], advances: &[GlyphAdvanceRun], prepared: &mut Vec<PreparedTextRunFragment>, colors: &[PaintColorRun], optical_offset_x: f64,
) -> Option<()> {
    let mut expected = source_range.start;
    let mut x = optical_offset_x + fragment_offset_x;
    let mut placement = text.line_placement(line, offsets, advances);
    let authoritative_runs = text.authoritative_runs(source_range.clone())?;

    for run in authoritative_runs {
        let start = run.source_range.start;
        let end = run.source_range.end;
        if start != expected {
            return None;
        }

        let local_start = run.run_range().start;
        let mut natural_x = f64::from(*run.caret_stops.get(local_start as usize)?);
        let rtl_origin_x = if text.bidi_level(start) % 2 == 1 {
            Some(x - f64::from(run.natural_offset(TextDirection::Rtl)?))
        } else {
            None
        };
        let mut segment: Option<(u32, u32, Point, Option<u32>)> = None;

        for index in start..end {
            let character = placement.at(index)?;

            let local_index = local_start + index - start;
            if text.bidi_level(index) % 2 == 1 {
                natural_x = f64::from(*run.caret_stops.get(local_index as usize)?).min(f64::from(*run.caret_stops.get(local_index as usize + 1)?));
            }
            let run_origin = Point::new(
                rtl_origin_x.unwrap_or(x - natural_x),
                line.baseline - character.offset_y,
            );
            let paint_index = colors.partition_point(|run| run.glyphs.end <= index);
            let color = colors.get(paint_index).filter(|run| run.glyphs.contains(&index)).map(|run| run.color);
            if character.paint_glyph.is_some() {
                match segment.as_mut() {
                    Some((_, segment_end, segment_origin, segment_color)) if *segment_end == local_index && *segment_origin == run_origin && (*segment_color == color || !text.is_cluster_boundary(index as usize)) => {
                        *segment_end = local_index + 1;
                    }
                    _ => {
                        flush_segment(run.backend_run, &mut segment, prepared);
                        segment = Some((local_index, local_index + 1, run_origin, color));
                    }
                }
            } else {
                flush_segment(run.backend_run, &mut segment, prepared);
            }

            x += character.advance;
            natural_x += character.natural_advance;
        }
        flush_segment(run.backend_run, &mut segment, prepared);
        expected = end;
    }

    (expected == source_range.end).then_some(())
}

fn flush_segment(run: crate::TextRunId, segment: &mut Option<(u32, u32, Point, Option<u32>)>, prepared: &mut Vec<PreparedTextRunFragment>) {
    if let Some((start, end, offset, color)) = segment.take() {
        prepared.push(PreparedTextRunFragment { run, range: start..end, offset, color });
    }
}

/// Prepare native paint slices after final geometry and palette resolution.
/// Palette-only updates reuse the same path without rebuilding lines.
pub fn prepare_text_paint(inputs: crate::layout::LayoutInputs<'_>, state: &mut crate::layout_model::LayoutState, colors: &[PaintColorRun]) {
    let text = InlineReader::new(inputs.inline_content, inputs.first_letter_styles, inputs.glyphs, inputs.glyph_metrics, inputs.text_geometry, inputs.ellipsis_glyphs, inputs.hyphen_glyphs);
    let mut fragments = FragmentWriter::new(state, Vec::new());
    prepare_line_text_runs(&text, &mut fragments, colors);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GlyphMetric, GlyphMetrics, GlyphSlice, InlineContent, LayoutState, ShapedTextGeometry, ShapedTextRun, TextGeometryView};

    #[test]
    fn native_slices_keep_tracking_positions_around_conditional_characters() {
        for (source, positions) in [
            ("AB\u{00ad}CD", [0.0, 10.0, 18.0, 28.0]),
            ("AB\u{200e}CD", [0.0, 10.0, 20.0, 30.0]),
        ] {
            let mut content = InlineContent::default();
            let mut metrics = GlyphMetrics::default();
            let mut glyphs = Vec::new();
            for (index, character) in source.chars().enumerate() {
                content.push_character(character);
                glyphs.push(metrics.register(GlyphMetric::try_new(character,
                    if index == 2 { 0.0 } else { 8.0 }, 12.0, 4.0).unwrap()).unwrap());
            }
            let native = ShapedTextRun::with_backend_run(
                vec![8.0, 8.0, 0.0, 8.0, 8.0].into(), vec![true; 6].into(), 0,
                vec![0.0, 8.0, 16.0, 16.0, 24.0, 32.0].into(), 12.0,
            ).unwrap();
            let mut geometry = ShapedTextGeometry::with_len(5);
            geometry.push_run(&[0, 1, 2, 3, 4], native, true);
            geometry.finish_runs();
            let empty = Default::default();
            let first_letter_styles = html_text::FirstLetterStyles::default();
            let text = InlineReader::new(&content, &first_letter_styles, GlyphSlice::new(&glyphs, &[]), &metrics,
                Some(TextGeometryView::new(&geometry, None)), &empty, &empty);
            let mut state = LayoutState::default();
            state.line_output.lines.push(Line { glyphs: 0..5, baseline: 12.0, letter_spacing: 2.0, ..Default::default() });
            let mut writer = FragmentWriter::new(&mut state, Vec::new());
            prepare_line_text_runs(&text, &mut writer, &[PaintColorRun { glyphs: 0..5, color: 0x008000ff }]);
            assert!(state.line_output.lines[0].native_text_runs_complete);
            let carets = [0.0, 8.0, 16.0, 16.0, 24.0];
            let positions_from_slices: Vec<_> = state.line_output.prepared_text_run_fragments.iter()
                .flat_map(|fragment| {
                    assert_eq!(fragment.color, Some(0x008000ff));
                    fragment.range.clone().map(|index| fragment.offset.x + carets[index as usize])
                }).collect();
            assert_eq!(positions_from_slices, positions);
        }
    }
}
