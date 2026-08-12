use super::*;

/// Lays out one inline-item range from start to finish.
///
/// This is the entry point used by box layout. It tokenizes the runs,
/// breaks them into logical lines, and then emits fragments from those
/// lines in a separate pass.
pub(in crate::layout) fn layout_inline_content(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    input: InlineFormattingInput,
) -> Size {
    let timing_started = engine.start_timing();
    let run_range = &input.run_range;
    if run_range.start >= run_range.end {
        engine.record_timing(|t| t.layout_runs += timing_started.elapsed());
        return Size::ZERO;
    }
    let (mut tokens, has_float_exclusions) = prepare_inline_tokens(engine, &input);
    if tokens.is_empty() {
        engine.record_timing(|t| t.layout_runs += timing_started.elapsed());
        return Size::ZERO;
    }
    let overflow = inline_overflow(engine, input.container_box_idx, &mut tokens);
    let (first_line_height, first_letter_height) =
        pseudo_line_heights(engine, input.container_box_idx);
    if has_float_exclusions {
        let size = layout_inline_content_around_float_exclusions(
            engine,
            &input,
            &tokens,
            overflow,
            first_line_height,
            first_letter_height,
        );
        engine.record_timing(|t| t.layout_runs += timing_started.elapsed());
        return size;
    }

    let ragged_knuth = engine.config.book_optimized_text()
        && input.paragraph.text_align == TextAlign::Left
        && !engine.config.force_justify()
        && !tokens.iter().any(|token| {
            token
                .run_metrics(&tokens.runs)
                .white_space
                .preserves_spaces()
        });
    let use_knuth = !tokens.iter().any(|token| token.is_tab(&tokens.runs))
        && (matches!(input.paragraph.text_align, TextAlign::Justify)
            || engine.config.force_justify()
            || ragged_knuth);
    let lines = if use_knuth {
        break_lines_knuth(
            engine,
            &tokens,
            &tokens.runs,
            input.paragraph,
            input.area.width,
            tokens.kp_plan(engine.config.book_optimized_text()),
            ragged_knuth,
        )
    } else {
        break_lines(
            engine,
            &tokens,
            &tokens.runs,
            input.paragraph,
            input.area.width,
        )
    };
    let size = emit_lines(
        engine,
        &tokens,
        &input,
        overflow,
        first_line_height,
        first_letter_height,
        &lines,
    );
    engine.record_timing(|t| t.layout_runs += timing_started.elapsed());
    size
}
