use std::ops::Range;

use kurbo::Point;

use crate::layout::fragment_writer::FragmentWriter;
use crate::layout::inline_reader::InlineReader;
use crate::layout_model::{GlyphAdvanceRun, GlyphOffsetRun, Line, PreparedTextRunFragment};

pub(super) fn prepare_line_text_runs(text: &InlineReader<'_>, fragments: &mut FragmentWriter<'_>) {
    let output = &mut fragments.state_mut().line_output;
    output.prepared_text_run_fragments.clear();
    for line_idx in 0..output.lines.len() {
        let start = output.prepared_text_run_fragments.len();
        let complete = prepare_line(
            text,
            &output.lines[line_idx],
            output
                .line_glyph_offsets
                .get(line_idx)
                .map_or(&[], Vec::as_slice),
            output
                .line_glyph_advances
                .get(line_idx)
                .map_or(&[], Vec::as_slice),
            &mut output.prepared_text_run_fragments,
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

fn prepare_line(
    text: &InlineReader<'_>,
    line: &Line,
    offsets: &[GlyphOffsetRun],
    advances: &[GlyphAdvanceRun],
    prepared: &mut Vec<PreparedTextRunFragment>,
) -> Option<()> {
    let mut offset_index = 0usize;
    let mut advance_index = 0usize;

    if let Some(fragments) = &line.text_fragments {
        for fragment in fragments.iter().filter(|fragment| fragment.visible) {
            prepare_fragment(
                text,
                line,
                fragment.glyphs.clone(),
                fragment.offset_x,
                offsets,
                advances,
                &mut offset_index,
                &mut advance_index,
                prepared,
            )?;
        }
    } else if !line.glyphs.is_empty() {
        prepare_fragment(
            text,
            line,
            line.glyphs.clone(),
            0.0,
            offsets,
            advances,
            &mut offset_index,
            &mut advance_index,
            prepared,
        )?;
    }

    Some(())
}

#[allow(clippy::too_many_arguments)]
fn prepare_fragment(
    text: &InlineReader<'_>,
    line: &Line,
    source_range: Range<u32>,
    fragment_offset_x: f64,
    offsets: &[GlyphOffsetRun],
    advances: &[GlyphAdvanceRun],
    offset_index: &mut usize,
    advance_index: &mut usize,
    prepared: &mut Vec<PreparedTextRunFragment>,
) -> Option<()> {
    let mut expected = source_range.start;
    let mut x = line.optical_offset_x + fragment_offset_x;

    for run in text.authoritative_runs() {
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
        let mut segment: Option<(u32, u32, Point)> = None;

        for index in start..end {
            let glyph = text.glyph(index)?;
            let metric = text.glyph_metric_checked(glyph)?;

            while offsets
                .get(*offset_index)
                .is_some_and(|candidate| index >= candidate.range.end)
            {
                *offset_index += 1;
            }
            let offset = offsets
                .get(*offset_index)
                .filter(|candidate| candidate.range.contains(&index))
                .map_or(0.0, |candidate| f64::from(candidate.offset));

            while advances
                .get(*advance_index)
                .is_some_and(|candidate| index >= candidate.range.end)
            {
                *advance_index += 1;
            }
            let advance_override = advances
                .get(*advance_index)
                .filter(|candidate| candidate.range.contains(&index))
                .map(|candidate| f64::from(candidate.advance));

            let local_index = local_start + index - start;
            let run_origin = Point::new(
                x - natural_x,
                line.baseline - f64::from(run.ascent) - offset,
            );
            let paints = advance_override.is_none() && metric.ch() != '\u{00ad}';
            if paints {
                match segment.as_mut() {
                    Some((_, segment_end, segment_origin))
                        if *segment_end == local_index && *segment_origin == run_origin =>
                    {
                        *segment_end = local_index + 1;
                    }
                    _ => {
                        flush_segment(run.backend_run, &mut segment, prepared);
                        segment = Some((local_index, local_index + 1, run_origin));
                    }
                }
            } else {
                flush_segment(run.backend_run, &mut segment, prepared);
            }

            let natural_advance = f64::from(text.text_advance(index as usize, metric.advance()));
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

fn flush_segment(
    run: crate::TextRunId,
    segment: &mut Option<(u32, u32, Point)>,
    prepared: &mut Vec<PreparedTextRunFragment>,
) {
    if let Some((start, end, offset)) = segment.take() {
        prepared.push(PreparedTextRunFragment {
            run,
            range: start..end,
            offset,
        });
    }
}
