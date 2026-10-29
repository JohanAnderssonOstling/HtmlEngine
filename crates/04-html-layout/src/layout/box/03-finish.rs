use super::box_constraints::constrain_size;
use super::placement::BoxPlacement;
use super::{BoxLayoutResult, ContentLayoutResult, ResolvedBoxSizing};
use super::{LayoutEngine, OutputRanges};
use crate::layout_model::{Children, LayoutMode};
use html_style_model::{BorderCollapseMode, Float, PositionMode, TextDirection};
use html_style_model::{BoxSizing, UsedPreferredSize as PreferredSize};
use kurbo::{Size, Vec2};
use std::iter;

pub(super) fn finish_resolved_box(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    content: ContentLayoutResult,
    cursor: super::fragment_writer::OutputCursor,
    placement: BoxPlacement,
) -> BoxLayoutResult {
    let request = resolved.request;
    let size = resolve_used_border_size(engine, resolved, content.size);
    finish_box_layout(
        engine,
        request.box_idx,
        size,
        cursor,
        placement,
        request.available_width,
        request.parent_content_height,
    );
    let output = engine.fragments.output_since(cursor, placement);
    engine.placement.select(placement.previous_group());
    let origin_y = engine.geometry.point(request.box_idx).y;
    let (first_baseline, last_baseline) = engine
        .fragments
        .baseline_offsets(output.lines.clone(), origin_y);
    let first_baseline = first_baseline
        .or(content.first_baseline)
        .or_else(|| inline_table_first_row_bottom(engine, request.box_idx));
    let last_baseline =
        last_baseline.or_else(|| last_in_flow_block_child_bottom(engine, request.box_idx));
    let after_margin = content.after_margin.unwrap_or_else(|| {
        super::block::MarginStrut::from_margin(
            engine
                .reader
                .vertical_margins(request.box_idx, request.available_width)
                .1,
        )
    });
    BoxLayoutResult {
        size,
        natural_content_size: content.size,
        first_baseline,
        last_baseline,
        after_margin,
        output,
        float_context_bottom: engine
            .floats
            .current_float_context()
            .map_or(0.0, |context| context.max_bottom() - origin_y),
    }
}

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
    if engine.reader.is_block_box(box_idx)
        || engine.reader.is_flex_grid_box(box_idx)
        || (engine.reader.is_table_box(box_idx)
            && !matches!(style.border_collapse(), BorderCollapseMode::Collapse))
    {
        super::decorations::record_block_decorations(engine, box_idx);
    }

    engine.placement.select(placement.content_group);
    if style.position() != PositionMode::Static {
        super::absolute_positioning::layout_pending(engine, Some(box_idx));
    }
    // A block and its split inline ancestors move the same painted subtree.
    // Apply their translations in order and mark the combined paint layer once.
    let reader = engine.reader;
    let styles = iter::once(style)
        .filter(|style| style.position() == PositionMode::Relative)
        .chain(
            reader
                .split_inline_position_ancestors(box_idx)
                .iter()
                .map(|&indices| reader.used_style(indices)),
        );
    let mut negative_layer = None;
    for style in styles {
        let offset = relative_position_offset(&style, containing_width, containing_height);
        engine.placement.translate(placement.box_group, offset);
        negative_layer =
            Some(negative_layer.unwrap_or(false) || style.z_index().is_some_and(|z| z < 0));
    }
    if let Some(negative) = negative_layer {
        let output = engine.fragments.output_since(output_cursor, placement);
        engine
            .fragments
            .mark_positioned_layer(&output, negative, false);
    }
}

/// Retain a translation on a complete laid-out box and its content.
pub fn translate_laid_out_output(
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
pub fn translate_laid_out_content(
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

pub fn relative_position_offset(
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
    use crate::GlyphResourceStore;

    use crate::LayoutConstraints;
    use crate::parser::DocumentFactory;
    use crate::test_support::TestGlyphShaper;

    #[test]
    fn table_vertical_alignment_moves_content_group_but_not_box_group() {
        let html = "<html><body style='margin:0'><table style='border-spacing:0'><tr><td style='padding:0'><div style='width:10px;height:20px'></div></td><td id='cell' style='padding:0;vertical-align:bottom;background:#ff0000'><div id='content' style='width:10px;height:5px'></div></td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let mut shaper = TestGlyphShaper::new();
        let mut document_resources = GlyphResourceStore::default();
        let document = factory
            .parse_with_new_pipeline(html, None)
            .shape(&mut shaper, &mut document_resources)
            .expect("test glyphs shape")
            .layout(
                LayoutConstraints::new(300.0, 16.0).unwrap(),
                &document_resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles");
        let view = document.render_view(&document_resources);
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

fn resolve_used_border_size(
    engine: &LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    content_size: Size,
) -> Size {
    let request = resolved.request;
    let horizontal_padding = resolved.box_model.horizontal_padding();
    let vertical_padding = resolved.box_model.vertical_padding();
    let horizontal_border = resolved.box_model.horizontal_border();
    let vertical_border = resolved.box_model.vertical_border();
    let table_width_is_minimum = engine.reader.is_table_box(request.box_idx);
    let table_height_is_minimum = matches!(
        engine.reader.box_layout_mode(request.box_idx),
        Some(LayoutMode::Table(_) | LayoutMode::TableRow(_) | LayoutMode::TableCell(_))
    );
    let ratio_auto_height = resolved
        .used_aspect_ratio
        .map(|ratio| match resolved.box_sizing {
            BoxSizing::ContentBox => resolved.horizontal.content / ratio,
            BoxSizing::BorderBox => {
                ((resolved.horizontal.content + horizontal_padding + horizontal_border) / ratio
                    - vertical_padding
                    - vertical_border)
                    .max(0.0)
            }
        });
    let mut content_height = match (resolved.replaced_size, resolved.vertical.explicit) {
        (Some(size), _) => size.height,
        (None, Some(explicit)) if table_height_is_minimum => content_size.height.max(explicit),
        (None, Some(explicit)) => explicit,
        (None, None) => ratio_auto_height.unwrap_or(content_size.height),
    };
    if resolved.replaced_size.is_none() {
        let ratio_automatic_minimum = (resolved.used_aspect_ratio.is_some()
            && resolved.vertical.explicit.is_none()
            && resolved.vertical.ratio_auto_min_uses_content)
            .then_some(content_size.height);
        content_height = constrain_size(
            content_height,
            resolved
                .vertical
                .min
                .resolve(Some(content_size.height))
                .or(ratio_automatic_minimum),
            resolved.vertical.max.resolve(Some(content_size.height)),
        );
    }
    content_height = content_height.max(0.0);

    let used_content_width = match (resolved.replaced_size, resolved.horizontal.preferred) {
        (Some(size), _) => size.width,
        // Table layout owns its final grid width. In particular, a 100%
        // column can make an auto-width floated table consume the available
        // inline size instead of the generic float shrink-to-fit width.
        (None, PreferredSize::Auto) if table_width_is_minimum => content_size.width,
        (None, PreferredSize::Auto) if resolved.fills_available_width => {
            resolved.horizontal.content
        }
        // Overflow does not enlarge a shrink-to-fit box. In particular, an
        // unbreakable text run may paint beyond a float constrained by
        // max-width while the float's used border width stays constrained.
        (None, PreferredSize::Auto) if resolved.horizontal.shrink_to_fit => {
            resolved.horizontal.content
        }
        (None, PreferredSize::Auto) => content_size.width,
        // CSS table width is a minimum: an over-constrained grid is allowed
        // to enlarge the table instead of squeezing tracks below their
        // minimum inline sizes.
        (None, _) if table_width_is_minimum => content_size.width.max(resolved.horizontal.content),
        (None, _) => resolved.horizontal.content,
    };
    let natural_width = used_content_width + horizontal_padding + horizontal_border;
    let natural_height = content_height + vertical_padding + vertical_border;
    request
        .assigned_border_size
        .map(|assigned| {
            let width = assigned.width.map_or(natural_width, |width| {
                if table_width_is_minimum {
                    width.max(natural_width)
                } else {
                    width
                }
            });
            let height = assigned.height.map_or(natural_height, |height| {
                if table_height_is_minimum {
                    height.max(natural_height)
                } else {
                    height
                }
            });
            Size::new(width, height)
        })
        .unwrap_or_else(|| Size::new(natural_width, natural_height))
}

/// An empty terminal block child publishes no line fragment, but it still
/// supplies the synthesized baseline used by an enclosing inline-block. The
/// baseline is its bottom border edge; padding after the child belongs to the
/// enclosing box and must not move it.
fn last_in_flow_block_child_bottom(session: &LayoutEngine<'_, '_>, box_idx: usize) -> Option<f64> {
    let LayoutMode::Block(block) = session.reader.box_layout_mode(box_idx)? else {
        return None;
    };
    let Children::Blocks(children) = &block.children else {
        return None;
    };
    let child_idx = children
        .iter()
        .rev()
        .map(|child| *child as usize)
        .find(|child| {
            let style = session.reader.style(*child);
            style.position() != PositionMode::Absolute && style.float() == Float::None
        })?;
    let parent_y = session.geometry.point(box_idx).y;
    let child_point = session.geometry.point(child_idx);
    let child_size = session.geometry.size(child_idx);
    Some(child_point.y + child_size.height - parent_y)
}

fn inline_table_first_row_bottom(engine: &LayoutEngine<'_, '_>, table_idx: usize) -> Option<f64> {
    let LayoutMode::Table(table) = engine.reader.box_layout_mode(table_idx)? else {
        return None;
    };
    let row_idx = *table.rows.first()? as usize;
    let table_y = engine.geometry.point(table_idx).y;
    let row_point = engine.geometry.point(row_idx);
    let row_size = engine.geometry.size(row_idx);
    Some(row_point.y + row_size.height - table_y)
}
