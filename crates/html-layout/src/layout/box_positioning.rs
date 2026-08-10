use super::{LayoutEngine, OutputRanges};
use html_style_model::{BorderCollapseMode, PositionMode, TextDirection};
use kurbo::{Size, Vec2};

pub(super) fn finish_box_layout(engine: &mut LayoutEngine<'_, '_>, box_idx: usize, size: Size, output_cursor: super::fragment_writer::OutputCursor, containing_width: f64, containing_height: Option<f64>) {
    engine.geometry.set_size(box_idx, size);
    let style = engine.reader.style(box_idx);
    if engine.reader.is_block_box(box_idx) || engine.reader.is_flex_grid_box(box_idx) {
        super::decorations::emit_block_decorations(engine, box_idx);
    } else if engine.reader.is_table_box(box_idx) && !matches!(style.border_collapse(), BorderCollapseMode::Collapse) {
        super::decorations::emit_block_decorations(engine, box_idx);
    }

    super::absolute_positioning::layout_for_box(engine, box_idx);
    if style.position() == PositionMode::Relative {
        let offset = relative_position_offset(&style, containing_width, containing_height);
        let output = engine.fragments.output_since(output_cursor);
        engine.fragments.mark_positioned_layer(&output, style.z_index().is_some_and(|z| z < 0), false);
        translate_laid_out_subtree_output(engine, box_idx, true, &output, offset);
    }
    // CSS block-in-inline generation makes the block a sibling in the layout
    // tree, but relative positioning on every split inline ancestor still
    // moves that block and its descendants as one painted subtree.
    let split_ancestors = engine.reader.split_inline_position_ancestors(box_idx).to_vec();
    for indices in split_ancestors {
        let ancestor_style = engine.reader.used_style(indices);
        let offset = relative_position_offset(&ancestor_style, containing_width, containing_height);
        let output = engine.fragments.output_since(output_cursor);
        engine.fragments.mark_positioned_layer(&output, ancestor_style.z_index().is_some_and(|z| z < 0), false);
        translate_laid_out_subtree_output(engine, box_idx, true, &output, offset);
    }
}

/// Translate one laid-out subtree together with the output fragments it
/// emitted. Callers aligning only descendants can leave the root fixed.
pub(crate) fn translate_laid_out_subtree_output(engine: &mut LayoutEngine<'_, '_>, root: usize, include_root: bool, output: &OutputRanges, offset: Vec2) {
    if offset == Vec2::ZERO {
        return;
    }
    for candidate in 0..engine.reader.box_count() {
        let mut current = if include_root { Some(candidate) } else { engine.reader.get_parent(candidate) };
        while let Some(box_idx) = current {
            if box_idx == root {
                engine.geometry.set_point(candidate, engine.geometry.point(candidate) + offset);
                break;
            }
            current = engine.reader.get_parent(box_idx);
        }
    }
    for line in &mut engine.fragments.state_mut().line_output.lines[output.lines.clone()] {
        line.point += offset;
    }
    for decoration in &mut engine.fragments.state_mut().fragment_output.decorations.fragments_mut()[output.decorations.clone()] {
        decoration.rect = decoration.rect + offset;
    }
    // Image offsets are line-relative. Their owning line was translated above,
    // so changing the offset as well would apply positioned movement twice.
}

pub(crate) fn relative_position_offset(style: &html_style_model::UsedStyleView<'_>, containing_width: f64, containing_height: Option<f64>) -> Vec2 {
    let resolve_x = |value: html_style_model::UsedLengthPct| value.resolve(containing_width);
    // A percentage block-axis inset computes to auto when the containing
    // block height is indefinite. That applies to the whole calc expression,
    // not just its percentage component.
    let resolve_y = |value: html_style_model::UsedLengthPct| match containing_height {
        Some(height) => value.resolve(height),
        None if value.has_percentage() => 0.0,
        None => value.resolve(0.0),
    };
    let x = match (style.inset_left(), style.inset_right()) {
        (None, None) => 0.0,
        (Some(left), None) => resolve_x(left),
        (None, Some(right)) => -resolve_x(right),
        (Some(left), Some(right)) => {
            if style.direction() == TextDirection::Rtl {
                -resolve_x(right)
            } else {
                resolve_x(left)
            }
        }
    };
    let y = match (style.inset_top(), style.inset_bottom()) {
        (None, None) => 0.0,
        (Some(top), _) => resolve_y(top),
        (None, Some(bottom)) => -resolve_y(bottom),
    };
    Vec2::new(x, y)
}
