use super::placement::BoxPlacement;
use super::{LayoutEngine, OutputRanges};
use html_style_model::{BorderCollapseMode, PositionMode, TextDirection};
use kurbo::{Size, Vec2};

pub(super) fn finish_box_layout(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    size: Size,
    output_cursor: super::fragment_writer::OutputCursor,
    placement: BoxPlacement,
    containing_width: f64,
    containing_height: Option<f64>,
) {
    engine.geometry.set_size(box_idx, size);
    engine.placement.select(placement.box_group);
    let style = engine.reader.style(box_idx);
    if engine.reader.is_block_box(box_idx) || engine.reader.is_flex_grid_box(box_idx) {
        super::decorations::emit_block_decorations(engine, box_idx);
    } else if engine.reader.is_table_box(box_idx)
        && !matches!(style.border_collapse(), BorderCollapseMode::Collapse)
    {
        super::decorations::emit_block_decorations(engine, box_idx);
    }

    engine.placement.select(placement.content_group);
    super::absolute_positioning::layout_for_box(engine, box_idx);
    if style.position() == PositionMode::Relative {
        let offset = relative_position_offset(&style, containing_width, containing_height);
        let output = engine.fragments.output_since(output_cursor, placement);
        engine.fragments.mark_positioned_layer(
            &output,
            style.z_index().is_some_and(|z| z < 0),
            false,
        );
        translate_laid_out_output(engine, &output, offset);
    }
    // CSS block-in-inline generation makes the block a sibling in the layout
    // tree, but relative positioning on every split inline ancestor still
    // moves that block and its descendants as one painted subtree.
    let split_ancestors = engine
        .reader
        .split_inline_position_ancestors(box_idx)
        .to_vec();
    for indices in split_ancestors {
        let ancestor_style = engine.reader.used_style(indices);
        let offset = relative_position_offset(&ancestor_style, containing_width, containing_height);
        let output = engine.fragments.output_since(output_cursor, placement);
        engine.fragments.mark_positioned_layer(
            &output,
            ancestor_style.z_index().is_some_and(|z| z < 0),
            false,
        );
        translate_laid_out_output(engine, &output, offset);
    }
}

/// Retain a translation on a complete laid-out box and its content.
pub(crate) fn translate_laid_out_output(
    engine: &mut LayoutEngine<'_, '_>,
    output: &OutputRanges,
    offset: Vec2,
) {
    if offset == Vec2::ZERO {
        return;
    }
    engine
        .placement
        .translate(output.placement.box_group, offset);
}

/// Retain a translation on a box's content while keeping its own border box fixed.
pub(crate) fn translate_laid_out_content(
    engine: &mut LayoutEngine<'_, '_>,
    output: &OutputRanges,
    offset: Vec2,
) {
    if offset != Vec2::ZERO {
        engine
            .placement
            .translate(output.placement.content_group, offset);
    }
}

pub(crate) fn relative_position_offset(
    style: &html_style_model::UsedStyleView<'_>,
    containing_width: f64,
    containing_height: Option<f64>,
) -> Vec2 {
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

#[cfg(test)]
mod tests {
    use crate::LayoutConstraints;
    use crate::parser::DocumentFactory;
    use crate::test_support::TestGlyphShaper;

    #[test]
    fn table_vertical_alignment_moves_content_group_but_not_box_group() {
        let html = "<html><body style='margin:0'><table style='border-spacing:0'><tr><td style='padding:0'><div style='width:10px;height:20px'></div></td><td id='cell' style='padding:0;vertical-align:bottom;background:#ff0000'><div id='content' style='width:10px;height:5px'></div></td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let mut shaper = TestGlyphShaper::new();
        let document = factory
            .parse_with_new_pipeline(html, None)
            .shape(&mut shaper)
            .expect("test glyphs shape")
            .layout(LayoutConstraints::new(300.0, 16.0).unwrap());
        let view = document.render_view();
        let box_by_id = |id| {
            (0..view.boxes().len())
                .find(|&index| view.boxes().id(index).map(|value| view.string(value)) == Some(id))
                .expect("box id exists")
        };
        let cell = view
            .boxes()
            .point(box_by_id("cell"))
            .expect("cell geometry");
        let content = view
            .boxes()
            .point(box_by_id("content"))
            .expect("content geometry");
        let backgrounds: Vec<_> = view
            .fragments()
            .decorations()
            .iter()
            .filter(|fragment| fragment.color() == 0xff0000ff)
            .map(|fragment| fragment.rect())
            .collect();

        assert_eq!(backgrounds.len(), 1);
        assert!(
            (backgrounds[0].y0 - cell.y).abs() < 0.01,
            "cell background must remain attached to its border box"
        );
        assert!((backgrounds[0].height() - 20.0).abs() < 0.01);
        assert!((content.y - cell.y - 15.0).abs() < 0.01);
    }
}
