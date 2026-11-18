use html_engine_model::{
    LayoutState, LineOutput, LineTextPlacement, PaintColorRun, PreparedTextRunFragment,
    TextDirection, paint_color_at,
};
use std::ops::Range;

use kurbo::Point;

use crate::inline::reader::InlineReader;

/// Prepare native paint slices after final geometry and palette resolution.
/// Palette-only updates reuse the same path without rebuilding lines.
pub fn prepare_text_paint(
    inputs: crate::LayoutInputs<'_>,
    state: &mut LayoutState,
    colors: &[PaintColorRun],
) {
    let text = InlineReader::from_inputs(&inputs);
    prepare_line_text_runs(&text, &mut state.line_output, colors);
}

fn prepare_line_text_runs(
    text: &InlineReader<'_>,
    output: &mut LineOutput,
    colors: &[PaintColorRun],
) {
    output.prepared_text_run_fragments.clear();
    for line_idx in 0..output.lines.len() {
        let start = output.prepared_text_run_fragments.len();
        let complete = prepare_line(text, output, line_idx, colors).is_some();
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
    output: &mut LineOutput,
    line_idx: usize,
    colors: &[PaintColorRun],
) -> Option<()> {
    let line = &output.lines[line_idx];
    for fragment in line
        .text_fragments(&output.line_details, &output.text_fragments)
        .filter(|fragment| fragment.visible)
    {
        let mut placement = text.line_placement(
            line,
            output.line_glyph_offsets.get(line_idx).unwrap_or(&[]),
            output.line_glyph_advances.get(line_idx).unwrap_or(&[]),
        );
        prepare_fragment(
            text,
            &mut placement,
            fragment.glyphs,
            Point::new(
                line.optical_offset_x(&output.line_details) + fragment.offset_x,
                line.baseline,
            ),
            &mut output.prepared_text_run_fragments,
            colors,
        )?;
    }
    Some(())
}

fn prepare_fragment(
    text: &InlineReader<'_>,
    placement: &mut LineTextPlacement<'_>,
    source_range: Range<u32>,
    origin: Point,
    prepared: &mut Vec<PreparedTextRunFragment>,
    colors: &[PaintColorRun],
) -> Option<()> {
    let mut expected = source_range.start;
    let mut x = origin.x;
    let authoritative_runs = text.authoritative_runs(source_range.clone())?;

    for run in authoritative_runs {
        let start = run.source_range.start;
        let end = run.source_range.end;
        if start < expected {
            return None;
        }
        // Source-only controls can have no native character slot. They do
        // not make paint coverage incomplete when they paint no glyph; retain
        // their layout advance before preparing the following native slice.
        for index in expected..start {
            let character = placement.at(index)?;
            if character.paint_glyph.is_some() {
                return None;
            }
            x += character.advance;
        }

        let local_start = run.run_range().start;
        let mut natural_x = f64::from(run.caret_stops.get(local_start as usize)?);
        let rtl_origin_x = if text.bidi_level(start) % 2 == 1 {
            Some(x - f64::from(run.natural_offset(TextDirection::Rtl)?))
        } else {
            None
        };
        let mut segment: Option<PreparedTextRunFragment> = None;

        for index in start..end {
            let character = placement.at(index)?;

            let local_index = local_start + index - start;
            if text.bidi_level(index) % 2 == 1 {
                natural_x = f64::from(run.caret_stops.get(local_index as usize)?)
                    .min(f64::from(run.caret_stops.get(local_index as usize + 1)?));
            }
            let run_origin = Point::new(
                rtl_origin_x.unwrap_or(x - natural_x),
                origin.y - character.offset_y,
            );
            let color = paint_color_at(colors, index);
            if character.paint_glyph.is_some() {
                match segment.as_mut() {
                    Some(segment)
                        if segment.range.end == local_index
                            && segment.offset == run_origin
                            && (segment.color == color
                                || !text.is_cluster_boundary(index as usize)) =>
                    {
                        segment.range.end = local_index + 1;
                    }
                    _ => {
                        prepared.extend(segment.take());
                        segment = Some(PreparedTextRunFragment {
                            run: run.backend_run,
                            range: local_index..local_index + 1,
                            offset: run_origin,
                            color,
                        });
                    }
                }
            } else {
                prepared.extend(segment.take());
            }

            x += character.advance;
            natural_x += character.natural_advance;
        }
        prepared.extend(segment.take());
        expected = end;
    }

    for index in expected..source_range.end {
        if placement.at(index)?.paint_glyph.is_some() {
            return None;
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use html_engine_model::{
        FontMetrics, FontSlant, GlyphId, GlyphResourceStore, GlyphSlice, GlyphWriter,
        InlineContent, LayoutState, Line, ResourceShaper, ShapedGlyph, ShapedTextGeometry,
        ShapedTextRun, TextGeometryView, TextRunId, shaping,
    };

    #[test]
    fn native_slices_keep_tracking_positions_around_conditional_characters() {
        for (source, positions) in [
            ("AB\u{00ad}CD", [0.0, 10.0, 18.0, 28.0]),
            ("AB\u{200e}CD", [0.0, 10.0, 20.0, 30.0]),
        ] {
            let mut content = InlineContent::default();
            let mut resources = GlyphResourceStore::default();
            let mut shaper =
                ResourceShaper::new(|character, style, native: &mut GlyphWriter<'_, ()>| {
                    let handle = native.insert(());
                    ShapedGlyph::new(
                        handle,
                        if matches!(character, '\u{00ad}' | '\u{200e}') {
                            0.0
                        } else {
                            8.0
                        },
                        FontMetrics {
                            ascent: 12.0,
                            descent: 4.0,
                        },
                    )
                });
            let style = shaping::style(16.0, 400, FontSlant::Normal, None);
            let mut glyphs = Vec::new();
            for character in source.chars() {
                content.push_character(character);
                glyphs.push(
                    {
                        let mut run_glyphs = [GlyphId(0)];
                        resources
                            .shape_run(
                                &mut shaper,
                                shaping::run(character.encode_utf8(&mut [0; 4]), style),
                                &mut run_glyphs,
                            )
                            .map(|_| run_glyphs[0])
                    }
                    .unwrap(),
                );
            }
            let metrics = resources.metrics();
            let native = ShapedTextRun::with_backend_run(
                vec![8.0, 8.0, 0.0, 8.0, 8.0].into(),
                vec![true; 6].into(),
                TextRunId(0),
                vec![0.0, 8.0, 16.0, 16.0, 24.0, 32.0].into(),
                12.0,
            );
            let mut geometry = ShapedTextGeometry::with_len(5);
            geometry.push_run(&[0, 1, 2, 3, 4], native, true);
            geometry.finish_runs();
            let empty = Default::default();
            let first_letter_styles = html_text::FirstLetterStyles::default();
            let text = InlineReader::new(
                &content,
                &first_letter_styles,
                GlyphSlice::new(&glyphs, &[]),
                metrics,
                Some(TextGeometryView::new(&geometry, None)),
                &empty,
                &empty,
            );
            let mut state = LayoutState::default();
            state.line_output.lines.push(Line {
                glyphs: 0..5,
                baseline: 12.0,
                letter_spacing: 2.0,
                ..Default::default()
            });
            prepare_line_text_runs(
                &text,
                &mut state.line_output,
                &[PaintColorRun {
                    glyphs: 0..5,
                    color: 0x008000ff,
                }],
            );
            assert!(state.line_output.lines[0].native_text_runs_complete);
            let carets = [0.0, 8.0, 16.0, 16.0, 24.0];
            let positions_from_slices: Vec<_> = state
                .line_output
                .prepared_text_run_fragments
                .iter()
                .flat_map(|fragment| {
                    assert_eq!(fragment.color, Some(0x008000ff));
                    fragment
                        .range
                        .clone()
                        .map(|index| fragment.offset.x + carets[index as usize])
                })
                .collect();
            assert_eq!(positions_from_slices, positions);
        }
    }
}
