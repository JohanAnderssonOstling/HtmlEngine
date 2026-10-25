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
    let dependencies = ParagraphDependencies::new(engine, &input, overflow, first_line_height, first_letter_height, has_float_exclusions);
    let slot = ParagraphSlot::new(&input, !engine.placement.retains_output());
    let (line_storage, adjustment_storage) = match engine.paragraph_layouts.take(slot, &dependencies, &tokens) {
        ParagraphLookup::Hit(paragraph) => {
            let size = paragraph.size;
            emit_prepared_paragraph(engine, &input, &tokens, &paragraph);
            engine.paragraph_layouts.insert(slot, paragraph);
            engine.record_timing(|t| t.layout_runs += timing_started.elapsed());
            return size;
        }
        ParagraphLookup::Miss { lines, adjustments } => (lines, adjustments),
    };
    if has_float_exclusions {
        let (lines, adjustments, size) = prepare_lines_around_float_exclusions(engine, &input, &tokens, overflow, first_line_height, first_letter_height, line_storage, adjustment_storage);
        let paragraph = PreparedParagraph::new(dependencies, &tokens, lines, adjustments, size);
        emit_prepared_paragraph(engine, &input, &tokens, &paragraph);
        engine.paragraph_layouts.insert(slot, paragraph);
        engine.record_timing(|t| t.layout_runs += timing_started.elapsed());
        return size;
    }

    let ragged_knuth = engine.config.book_optimized_text() && input.paragraph.text_align == TextAlign::Left && !engine.config.force_justify() && !tokens.iter().any(|token| token.run_metrics(&tokens.runs).white_space.preserves_spaces());
    let use_knuth = !tokens.iter().any(|token| token.is_tab(&tokens.runs)) && (matches!(input.paragraph.text_align, TextAlign::Justify) || engine.config.force_justify() || ragged_knuth);
    // Detach storage while emitting lines: inline layout can recurse for
    // atomic content, and nested paragraphs must own separate scratch.
    let mut scratch = engine.paragraphs.pop().unwrap_or_default();
    let break_storage = std::mem::take(&mut scratch.lines);
    let lines = if use_knuth {
        break_lines_knuth(engine, &tokens, &tokens.runs, input.paragraph, input.area.width, tokens.kp_plan(engine.config.book_optimized_text()), ragged_knuth, break_storage, &mut scratch.solver)
    } else {
        wrapping::greedy::break_lines_into(engine, &tokens, &tokens.runs, input.paragraph, input.area.width, break_storage)
    };
    let context = LineEmissionContext::new(&tokens, &input, overflow, first_line_height, first_letter_height);
    let (prepared, adjustments, size) = prepare_lines(engine, &context, &lines, line_storage, adjustment_storage);
    let paragraph = PreparedParagraph::new(dependencies, &tokens, prepared, adjustments, size);
    emit_prepared_paragraph(engine, &input, &tokens, &paragraph);
    engine.paragraph_layouts.insert(slot, paragraph);
    scratch.lines = lines;
    scratch.lines.clear();
    engine.paragraphs.push(scratch);
    engine.record_timing(|t| t.layout_runs += timing_started.elapsed());
    size
}
