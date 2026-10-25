use crate::stages::PaintColorRun;
use std::ops::Range;

use kurbo::Point;

use crate::layout::fragment_writer::FragmentWriter;
use crate::layout::inline_reader::InlineReader;
use crate::layout_model::{GlyphAdvanceRun, GlyphOffsetRun, Line, LineDetails, LineTextFragment, PreparedTextRunFragment};

pub(super) fn prepare_line_text_runs(text: &InlineReader<'_>, fragments: &mut FragmentWriter<'_>, colors: &[PaintColorRun]) {
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
    let mut offset_index =
        offsets.partition_point(|candidate| candidate.range.end <= source_range.start);
    let mut advance_index =
        advances.partition_point(|candidate| candidate.range.end <= source_range.start);
    let authoritative_runs = text.authoritative_runs();
    let first_run =
        authoritative_runs.partition_point(|run| run.source_range.end <= source_range.start);

    let mut text_cursor = text.cursor();
    for run in &authoritative_runs[first_run..] {
        if run.source_range.start >= source_range.end {
            break;
        }
        let start = source_range.start.max(run.source_range.start);
        let end = source_range.end.min(run.source_range.end);
        if start >= end {
            continue;
        }
        if start != expected {
            return None;
        }

        let local_start = start - run.source_range.start;
        let mut natural_x = f64::from(*run.caret_stops.get(local_start as usize)?);
        let mut segment: Option<(u32, u32, Point, Option<u32>)> = None;

        for index in start..end {
            let glyph = text.glyph(index)?;
            let metric = text_cursor.glyph_metric(glyph)?;

            while offsets
                .get(offset_index)
                .is_some_and(|candidate| index >= candidate.range.end)
            {
                offset_index += 1;
            }
            let offset = offsets
                .get(offset_index)
                .filter(|candidate| candidate.range.contains(&index))
                .map_or(0.0, |candidate| f64::from(candidate.offset));

            while advances
                .get(advance_index)
                .is_some_and(|candidate| index >= candidate.range.end)
            {
                advance_index += 1;
            }
            let advance_override = advances
                .get(advance_index)
                .filter(|candidate| candidate.range.contains(&index))
                .map(|candidate| f64::from(candidate.advance));

            let local_index = local_start + index - start;
            let run_origin = Point::new(
                x - natural_x,
                line.baseline - f64::from(run.ascent) - offset,
            );
            let paints = advance_override.is_none() && metric.ch() != '\u{00ad}';
            let paint_index = colors.partition_point(|run| run.glyphs.end <= index);
            let color = colors.get(paint_index).filter(|run| run.glyphs.contains(&index)).map(|run| run.color);
            if paints {
                match segment.as_mut() {
                    Some((_, segment_end, segment_origin, segment_color)) if *segment_end == local_index && *segment_origin == run_origin && (*segment_color == color || !text_cursor.is_cluster_boundary(index as usize)) => {
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

            let natural_advance = f64::from(text_cursor.text_advance(index as usize, metric.advance()));
            let tracking =
                if index + 1 < line.glyphs.end && text.is_cluster_boundary((index + 1) as usize) {
                    line.letter_spacing
                } else {
                    0.0
                };
            x += advance_override.unwrap_or(
                natural_advance
                    + tracking
                    + if metric.ch() == ' ' {
                        line.word_spacing
                    } else {
                        0.0
                    },
            );
            natural_x += natural_advance;
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

/// Refresh native paint slices after a color update, without rebuilding lines.
pub(crate) fn prepare_text_paint(inputs: crate::layout::LayoutInputs<'_>, state: &mut crate::layout_model::LayoutState, colors: &[PaintColorRun]) {
    let text = InlineReader::new(inputs.inline_content, inputs.glyph_metrics, inputs.text_geometry, inputs.ellipsis_glyphs, inputs.hyphen_glyphs);
    let mut fragments = FragmentWriter::new(state, Vec::new(), Vec::new());
    prepare_line_text_runs(&text, &mut fragments, colors);
}
