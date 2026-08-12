use crate::layout::LayoutEngine;
use crate::layout_model::BoxType;
use html_style_model::{Float, PositionMode};
use kurbo::{Point, Size, Vec2};

use super::floats::{clearance_offset, layout_float};
use super::fragmentation::{PreviousBlock, make_previous_block, offset_after_layout, offset_before_layout, offset_for_forced_break_after};
use super::margins::{FlowParticipation, MarginProfile, MarginStrut, clear_matches_float_sides, collapse_before_child, parent_collapse_context, remaining_parent_margin};

struct BlockFlowItem {
    profile: MarginProfile,
    is_anonymous: bool,
}

/// `available_width` is the content width of `parent_idx` — i.e. the containing
/// block inline-size for the children, against which their margin percentages
/// resolve. `parent_cb_width` is the parent's own containing-block width.
pub(crate) fn layout_block_children(engine: &mut LayoutEngine<'_, '_>, parent_idx: usize, children: &[u32], available_width: f64, parent_cb_width: f64, parent_content_height: Option<f64>, first_line_indent: f64) -> Size {
    let timing_started = engine.start_timing();
    let mut y_offset = 0.0;
    let mut max_width = 0.0f64;
    let mut pending_margin = MarginStrut::default();
    // Actual clearance on a zero-height collapse-through box separates that
    // adjoining margin chain from both open parent edges. Keep the barrier
    // until real in-flow content consumes the chain.
    let mut clearance_margin_barrier = false;
    let mut clearance_consumed_margin = 0.0;
    let mut collapsed_parent_top_margin = MarginStrut::default();
    let mut indent_remaining = first_line_indent;
    let mut started_flow = false;
    let mut previous_block: Option<PreviousBlock> = None;
    // Only the latest float source adjoining through the open margin chain
    // may trigger the negative-clearance case. An intervening float source or
    // ordinary in-flow box ends that relationship even though older floats
    // remain active in the exclusion context.
    let mut latest_adjoining_float_sides = 0;

    let parent_style = engine.reader.style(parent_idx);
    // Children start inside the parent's border and padding (the parent's box
    // point is its border-box origin).
    let parent_padding = parent_style.get_top_left_padding(parent_cb_width) + Vec2::new(parent_style.border_left_width() as f64, parent_style.border_top_width() as f64);
    let parent_ctx = parent_collapse_context(&engine.reader, parent_idx);
    let position = engine.geometry.point(parent_idx) + parent_padding;

    for &child_handle in children {
        let child_idx = child_handle as usize;
        if engine.reader.style(child_idx).position() == PositionMode::Absolute {
            // The hypothetical static position lies after the adjoining
            // margin accumulated from preceding in-flow content. Resolving it
            // here does not consume that margin; the next in-flow sibling
            // still collapses against the same strut.
            engine.defer_absolute_box(child_idx, position + Vec2::new(0.0, y_offset + pending_margin.resolve()));
            continue;
        }
        if engine.reader.is_inline_box(child_idx) {
            continue;
        }

        let mut item = block_flow_item(engine, child_idx, available_width);
        let child_style = engine.reader.style(child_idx);
        let child_float = child_style.float();
        let child_clear = child_style.clear();
        debug_assert_eq!(matches!(item.profile.participation, FlowParticipation::Float), matches!(child_float, Float::Left | Float::Right));

        // A descendant clear behind open before edges exposes a different
        // margin when a matching float is active. Leave the clear candidate's
        // own margin inside the child so its runtime clearance contributes to
        // the child's used height instead of shifting the whole child.
        let leading_clear = item.profile.leading_clear();
        let mut runtime_leading_clearance = false;
        let mut runtime_leading_clearance_rewind = 0.0;
        if child_clear == html_style_model::Clear::None && leading_clear != html_style_model::Clear::None {
            let leading_clearance = clearance_offset(
                &engine.floats,
                position.y,
                y_offset,
                pending_margin,
                item.profile.before,
                0.0,
                leading_clear,
                !started_flow && parent_ctx.top_open && !item.is_anonymous,
                clear_matches_float_sides(leading_clear, latest_adjoining_float_sides),
            );
            if leading_clearance.is_some() {
                // Keep the adjoining margins that occur before the clear
                // candidate exposed at the wrapper's open top edge. The
                // candidate's own top margin remains inside: clearance is
                // inserted between that margin and the preceding margin
                // chain (CSS 2.1 8.3.1 and 9.5.2).
                runtime_leading_clearance_rewind = item.profile.before_clear.resolve() - MarginStrut::from_margin(item.profile.own_before).resolve();
                item.profile.before = item.profile.before_clear;
                // The cached profile describes the no-clearance case. Once a
                // leading descendant actually clears an external float this
                // wrapper acquires used height and can no longer collapse
                // through as a zero-height margin-only box.
                item.profile.collapses_through = false;
                item.profile.after = MarginStrut::from_margin(item.profile.own_after);
                runtime_leading_clearance = true;
            }
        }

        // A leading BFC whose margin remains adjoining to an earlier nested
        // float behaves like it has clearance when its border box cannot fit
        // in any band beside that float. Resolve the propagated candidate at
        // the real containing width before allowing its margin to escape
        // through the wrapper.
        let has_active_float = engine.floats.current_float_context().is_some_and(|context| !context.is_empty());
        if let Some(leading_bfc_idx) = item.profile.leading_bfc_to_probe(has_active_float).filter(|leading_bfc_idx| *leading_bfc_idx != child_idx) {
            let candidate = engine.resolve_box_sizing(crate::layout::BoxLayoutRequest::normal(leading_bfc_idx, available_width, parent_content_height));
            let horizontal = candidate.horizontal_margins();
            let measured = crate::layout::measure_resolved_box_isolated(engine, candidate);
            if measured.width + horizontal.total >= available_width - 0.001 {
                item.profile.before = item.profile.before_clear;
                item.profile.collapses_through = false;
            }
        }

        let margin_adjoins_parent_top = !started_flow && parent_ctx.top_open && !item.is_anonymous;
        // A self-collapsing box exposes its combined top/bottom strut to
        // siblings, but clearance is calculated at its top border edge. Its
        // bottom margin must therefore not move the hypothetical clear
        // position.
        let clearance_before = if child_clear != html_style_model::Clear::None && item.profile.collapses_through { MarginStrut::from_margin(item.profile.own_before) } else { item.profile.before };
        let cleared_offset =
            clearance_offset(&engine.floats, position.y, y_offset, pending_margin, clearance_before, item.profile.own_before, child_clear, margin_adjoins_parent_top, clear_matches_float_sides(child_clear, latest_adjoining_float_sides));

        if matches!(item.profile.participation, FlowParticipation::Float) {
            // `clear` changes this float's hypothetical position, but a
            // float remains out of flow: its clearance must not advance the
            // parent's normal-flow cursor or consume the adjoining margin
            // that a later in-flow sibling still sees.
            let float_margin_y = cleared_offset.map(|clearance| clearance.offset).unwrap_or_else(|| y_offset + pending_margin.resolve());
            let resolved = engine.resolve_box_sizing(crate::layout::BoxLayoutRequest::normal(child_idx, available_width, parent_content_height));
            let horizontal = resolved.horizontal_margins();
            let child_size = layout_float(engine, resolved, child_float, position, float_margin_y, horizontal.left, horizontal.total, item.profile.own_before, item.profile.own_after);
            max_width = max_width.max(child_size.width + horizontal.total);
            latest_adjoining_float_sides = item.profile.adjoining_float_sides();
            continue;
        }

        let mut has_clearance = false;
        let mut clearance_consumed_before_margin = false;
        let clearance_on_collapsed_through_box = cleared_offset.is_some() && item.profile.collapses_through;
        let clearance_chain_leading_margin = if !collapsed_parent_top_margin.is_empty() {
            collapsed_parent_top_margin.resolve()
        } else if item.profile.own_after.abs() > 0.001 {
            MarginStrut::from_margin(item.profile.own_before).resolve()
        } else {
            0.0
        };
        if let Some(clearance) = cleared_offset {
            y_offset = clearance.offset;
            pending_margin = MarginStrut::default();
            has_clearance = true;
            clearance_consumed_before_margin = true;
            if clearance_on_collapsed_through_box {
                // The box's adjoining margins still collapse with each other,
                // but its top component lies above the cleared border edge.
                // Carry that consumed component until the chain is resolved.
                clearance_margin_barrier = true;
                let cleared_border_y = position.y + clearance.offset;
                clearance_consumed_margin = if clearance.hypothetical_y > cleared_border_y + 0.001 { 0.0 } else { clearance_chain_leading_margin - engine.floats.hypothetical_clearance_rewind() };
                item.profile.before = MarginStrut::from_margin(item.profile.own_before);
                item.profile.after = MarginStrut::from_margin(item.profile.own_after);
                item.profile.collapses_through = false;
            }
        }

        let (top_collapses_with_parent, mut margin_offset) = collapse_before_child(&mut pending_margin, item.profile, started_flow || clearance_margin_barrier, has_clearance, clearance_consumed_before_margin, parent_ctx, item.is_anonymous);
        if clearance_margin_barrier && !item.profile.collapses_through && !clearance_on_collapsed_through_box {
            margin_offset -= clearance_consumed_margin;
            clearance_margin_barrier = false;
            clearance_consumed_margin = 0.0;
        }
        y_offset += margin_offset;

        // Preserve the child's hypothetical normal-flow margin position for
        // descendant floats. A collapse-through wrapper does not consume the
        // pending adjoining strut into `y_offset`, but floats sourced inside
        // it must still start after that strut. For ordinary children the
        // strut was consumed and cleared above, so this expression reduces to
        // the already-resolved border position.
        if let Some(context) = engine.floats.current_float_context_mut() {
            context.note_source_position(position.y + y_offset + pending_margin.resolve());
        }

        let fragment_height = engine.fragment_height();
        y_offset += offset_before_layout(engine, child_idx, position.y + y_offset, fragment_height);

        let child_indent = engine.reader.box_layout_mode(child_idx).map(|mode| calculate_child_indent(mode, &mut indent_remaining)).unwrap_or(0.0);
        let mut resolved = engine.resolve_box_sizing(crate::layout::BoxLayoutRequest::normal(child_idx, available_width, parent_content_height)).with_first_line_indent(child_indent);
        let mut horizontal = resolved.horizontal_margins();
        // A collapse-through box still has a hypothetical border position.
        // When it establishes a containing block via `position: relative`,
        // that origin includes the adjoining margin even though the margin
        // remains pending in normal flow. Floats and absolute descendants
        // must therefore agree on the same source position.
        let relative_collapse_through_offset = if item.profile.collapses_through && child_style.position() == PositionMode::Relative { pending_margin.resolve() } else { 0.0 };
        let inline_offset = crate::layout::box_sizing::block_inline_alignment_offset(engine, &resolved);
        let mut child_pos = position + Vec2::new(inline_offset, y_offset + relative_collapse_through_offset);
        if engine.reader.box_uses_float_context(child_idx)
            && let Some(float_context) = engine.floats.current_float_context().filter(|context| !context.is_empty()).cloned()
        {
            let original_y = child_pos.y;
            let mut candidate_y = original_y;
            let mut probe_height = crate::layout::measure_resolved_box_isolated(engine, resolved.clone()).height.max(1.0);
            let mut selected = false;
            for _ in 0..float_context.band_count() + 2 {
                let (band_left, band_right) = float_context.available(candidate_y, probe_height, position.x, position.x + available_width);
                let band_width = (band_right - band_left).max(0.0);
                let candidate = engine.resolve_box_sizing(crate::layout::BoxLayoutRequest::normal(child_idx, available_width, parent_content_height).with_auto_width_limit(band_width)).with_first_line_indent(child_indent);
                let candidate_horizontal = candidate.horizontal_margins();
                let measured = crate::layout::measure_resolved_box_isolated(engine, candidate.clone());
                let candidate_height = measured.height.max(1.0);
                let (stable_left, stable_right) = float_context.available(candidate_y, candidate_height, position.x, position.x + available_width);
                if (stable_left - band_left).abs() > 0.001 || (stable_right - band_right).abs() > 0.001 {
                    probe_height = candidate_height;
                    continue;
                }

                // The margin-box edge adjoining a float must stay outside that
                // float. A trailing margin at the containing block edge may
                // overflow that edge, though, and must not force the BFC below
                // an otherwise fitting float (CSS2 new-fc-beside-float-with-margin).
                let trailing_float_margin = if stable_right < position.x + available_width - 0.001 { candidate_horizontal.total - candidate_horizontal.left } else { 0.0 };
                let required_width = (measured.width + candidate_horizontal.left + trailing_float_margin).max(0.0);
                let stable_width = (stable_right - stable_left).max(0.0);
                if stable_left <= stable_right + 0.001 && required_width <= stable_width + 0.001 {
                    resolved = candidate;
                    horizontal = candidate_horizontal;
                    child_pos = Point::new(stable_left + horizontal.left, candidate_y);
                    selected = true;
                    break;
                }

                let next_y = float_context.next_y_fitting(candidate_y, required_width, candidate_height, position.x, position.x + available_width);
                if next_y <= candidate_y + 0.001 {
                    resolved = candidate;
                    horizontal = candidate_horizontal;
                    child_pos = Point::new(stable_left + horizontal.left, candidate_y);
                    selected = true;
                    break;
                }
                candidate_y = next_y;
                probe_height = candidate_height;
            }
            if !selected {
                let measured = crate::layout::measure_resolved_box_isolated(engine, resolved.clone());
                let placement = float_context.place_margin_box(original_y, measured.width + horizontal.total, measured.height.max(1.0), position.x, position.x + available_width);
                child_pos = Point::new(placement.left + horizontal.left, placement.y);
            }
            y_offset += child_pos.y - original_y;
        }
        engine.geometry.set_point(child_idx, child_pos);
        let inherited_hypothetical_margin = (item.profile.collapses_through && !pending_margin.is_empty()).then_some(position.y + y_offset + pending_margin.resolve());
        if let Some(floor) = inherited_hypothetical_margin {
            engine.floats.push_hypothetical_clearance_floor(floor);
        }
        if runtime_leading_clearance_rewind.abs() > 0.001 {
            engine.floats.push_hypothetical_clearance_rewind(runtime_leading_clearance_rewind);
        }
        let child_layout = engine.layout_resolved_box(resolved);
        if runtime_leading_clearance_rewind.abs() > 0.001 {
            engine.floats.pop_hypothetical_clearance_rewind();
        }
        if inherited_hypothetical_margin.is_some() {
            engine.floats.pop_hypothetical_clearance_floor();
        }
        let child_size = child_layout.size;
        let output = child_layout.output;
        let used_after_margin = if runtime_leading_clearance { item.profile.after } else { engine.margins.used_after_margin(&engine.reader, child_idx, available_width, parent_content_height) };

        y_offset += offset_after_layout(engine, child_idx, child_size, &output, previous_block.as_ref(), fragment_height, &mut child_pos);

        max_width = max_width.max(child_size.width + horizontal.total);

        if item.profile.collapses_through {
            if !top_collapses_with_parent {
                pending_margin.merge(item.profile.after);
            } else {
                collapsed_parent_top_margin.merge(item.profile.before);
                collapsed_parent_top_margin.merge(item.profile.after);
            }
            if item.profile.adjoining_float_sides() != 0 {
                latest_adjoining_float_sides = item.profile.adjoining_float_sides();
            }
            continue;
        }

        if clearance_on_collapsed_through_box {
            y_offset += child_size.height;
            pending_margin.merge(item.profile.after);
            continue;
        }

        y_offset += child_size.height;
        y_offset += offset_for_forced_break_after(child_style.break_after(), position.y + y_offset, fragment_height, engine.config.book_optimized_text());
        pending_margin.merge(used_after_margin);
        clearance_margin_barrier = false;
        clearance_consumed_margin = 0.0;
        started_flow = true;
        collapsed_parent_top_margin = MarginStrut::default();
        latest_adjoining_float_sides = 0;
        previous_block = Some(make_previous_block(engine, child_idx, child_pos, output));
    }

    y_offset += remaining_parent_margin(pending_margin, parent_ctx, clearance_margin_barrier, clearance_consumed_margin);
    if clearance_margin_barrier {
        // The clearance-separated chain was consumed into this box's used
        // height. Do not let the static trailing-margin analysis expose it a
        // second time to the parent.
        let own_after = MarginStrut::from_margin(parent_style.margin_bottom().resolve(parent_cb_width));
        engine.margins.record_used_after_margin(parent_idx, parent_cb_width, parent_content_height, own_after);
    }

    let size = Size::new(max_width, y_offset);
    engine.record_timing(|t| t.layout_block_children += timing_started.elapsed());
    size
}

/// `cb_width` is the containing-block inline-size for `box_idx` (its parent's
/// content width), against which this box's margin percentages resolve.
fn block_flow_item(engine: &mut LayoutEngine<'_, '_>, box_idx: usize, cb_width: f64) -> BlockFlowItem {
    let is_anonymous = matches!(engine.reader.box_layout_mode(box_idx), Some(BoxType::Anonymous(_)));
    let profile = engine.margins.margin_profile(&engine.reader, box_idx, cb_width);
    BlockFlowItem { profile, is_anonymous }
}

fn calculate_child_indent(box_type: &BoxType, indent_remaining: &mut f64) -> f64 {
    match box_type {
        BoxType::Anonymous(_) => {
            let indent = *indent_remaining;
            *indent_remaining = 0.0;
            indent
        }
        _ => {
            *indent_remaining = 0.0;
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::LaidOutDocument;
    use crate::parser::DocumentFactory;
    use crate::test_support::TestGlyphShaper;
    use kurbo::{Point, Size};

    fn layout_html(body: &str) -> LaidOutDocument {
        let html = format!("<html><body style='margin:0'>{body}</body></html>");
        layout_document(&html)
    }

    fn layout_document(html: &str) -> LaidOutDocument {
        layout_document_with_css(html, None)
    }

    fn layout_document_with_css(html: &str, css: Option<&str>) -> LaidOutDocument {
        let mut factory = DocumentFactory::new();
        let mut glyphs = TestGlyphShaper::new();
        factory.parse_with_new_pipeline(html, css).shape(&mut glyphs).expect("test glyphs shape").layout(crate::LayoutConstraints::new(300.0, 16.0).unwrap())
    }

    fn geometry(document: &LaidOutDocument, id: &str) -> (Point, Size) {
        let view = document.render_view();
        let box_idx = (0..view.boxes().len()).find(|idx| view.boxes().id(*idx).map(|value| view.string(value)) == Some(id)).unwrap_or_else(|| panic!("missing box #{id}"));
        (view.boxes().point(box_idx).expect("box index in range"), view.boxes().size(box_idx).expect("box index in range"))
    }

    #[test]
    fn adjoining_sibling_margins_use_the_largest_positive_value() {
        let document = layout_html("<div id='first' style='height:10px;margin-bottom:30px'></div><div id='second' style='height:10px;margin-top:20px'></div>");
        let (first, first_size) = geometry(&document, "first");
        let (second, _) = geometry(&document, "second");

        assert_eq!(second.y - (first.y + first_size.height), 30.0);
    }

    #[test]
    fn adjoining_positive_and_negative_margins_sum_their_extremes() {
        let document = layout_html("<div id='first' style='height:10px;margin-bottom:30px'></div><div id='second' style='height:10px;margin-top:-10px'></div>");
        let (first, first_size) = geometry(&document, "first");
        let (second, _) = geometry(&document, "second");

        assert_eq!(second.y - (first.y + first_size.height), 20.0);
    }

    #[test]
    fn first_child_margin_collapses_with_parent_even_when_child_has_a_border() {
        let document = layout_html("<div id='anchor' style='height:10px'></div><div id='parent' style='margin-top:30px'><div id='child' style='height:10px;margin-top:50px;border-top:1px solid'></div></div>");
        let (anchor, anchor_size) = geometry(&document, "anchor");
        let (parent, _) = geometry(&document, "parent");
        let (child, _) = geometry(&document, "child");

        assert_eq!(parent.y - (anchor.y + anchor_size.height), 50.0);
        assert_eq!(child.y, parent.y);
    }

    #[test]
    fn nested_empty_blocks_collapse_through_one_adjoining_set() {
        let document = layout_html("<div id='first' style='height:10px;margin-bottom:10px'></div><div id='empty' style='height:0;margin-top:30px;margin-bottom:40px'></div><div id='last' style='height:10px;margin-top:20px'></div>");
        let (first, first_size) = geometry(&document, "first");
        let (last, _) = geometry(&document, "last");

        assert_eq!(last.y - (first.y + first_size.height), 40.0);
    }

    #[test]
    fn parent_border_prevents_child_margin_from_escaping() {
        let document = layout_html("<div id='parent' style='border-top:5px solid'><div id='child' style='height:10px;margin-top:30px'></div></div>");
        let (parent, _) = geometry(&document, "parent");
        let (child, _) = geometry(&document, "child");

        assert_eq!(child.y - parent.y, 35.0);
    }

    #[test]
    fn overflow_formatting_context_contains_child_margin() {
        let document = layout_html("<div id='parent' style='overflow:hidden'><div id='child' style='height:10px;margin-top:30px'></div></div>");
        let (parent, _) = geometry(&document, "parent");
        let (child, _) = geometry(&document, "child");

        assert_eq!(child.y - parent.y, 30.0);
    }

    #[test]
    fn float_margins_do_not_collapse_with_the_preceding_block_margin() {
        let document = layout_html("<div id='first' style='height:10px;margin-bottom:20px'></div><div id='float' style='float:left;width:10px;height:10px;margin-top:15px'></div>");
        let (first, first_size) = geometry(&document, "first");
        let (float, _) = geometry(&document, "float");

        assert_eq!(float.y - (first.y + first_size.height), 35.0);
    }

    #[test]
    fn nested_float_keeps_the_preceding_adjoining_margin_position() {
        let document = layout_html("<div id='first' style='height:10px;margin-bottom:20px'></div><div><div id='float' style='float:left;width:10px;height:10px;margin-top:15px'></div></div>");
        let (first, first_size) = geometry(&document, "first");
        let (float, _) = geometry(&document, "float");

        assert_eq!(float.y - (first.y + first_size.height), 35.0);
    }

    #[test]
    fn inline_absolute_anchor_shares_a_nested_floats_collapsed_source_position() {
        let document = layout_html(
            "<div id='first' style='height:10px;margin-bottom:20px'></div>\
             <div id='wrapper' style='position:relative'>\
                 <div id='absolute' style='position:absolute;width:10px;height:10px'></div>\
                 <div id='float' style='float:left;width:10px;height:10px'></div>\
             </div>",
        );
        let (wrapper, _) = geometry(&document, "wrapper");
        let (absolute, _) = geometry(&document, "absolute");
        let (float, _) = geometry(&document, "float");

        assert_eq!(wrapper.y, float.y);
        assert_eq!(absolute.y, float.y);
    }

    #[test]
    fn inline_image_prevents_its_wrapper_from_collapsing_through() {
        let document = layout_html(
            "<div id='first' style='height:10px;margin-bottom:20px'></div>\
             <div id='wrapper'><svg id='image' width='96' height='96'></svg></div>",
        );
        let (first, first_size) = geometry(&document, "first");
        let (wrapper, _) = geometry(&document, "wrapper");
        let (image, _) = geometry(&document, "image");

        assert_eq!(wrapper.y, first.y + first_size.height + 20.0);
        assert!((image.y - wrapper.y).abs() < 1.0, "wrapper={wrapper:?} image={image:?}");
    }

    #[test]
    fn floated_box_contents_do_not_move_the_outer_float_source_position() {
        let document = layout_html(
            "<div id='first' style='height:10px;margin-bottom:20px'></div><div><span id='float' style='display:inline-block;float:right;width:20px;height:20px'><span style='display:block'>a</span><span style='display:block'>b</span></span></div>",
        );
        let (first, first_size) = geometry(&document, "first");
        let (float, _) = geometry(&document, "float");

        assert_eq!(float.y - (first.y + first_size.height), 20.0);
    }

    #[test]
    fn nonzero_min_height_prevents_empty_block_collapse_through() {
        let document = layout_html("<div id='first' style='height:10px'></div><div id='empty' style='min-height:1px;margin-top:20px;margin-bottom:30px'></div><div id='last' style='height:10px'></div>");
        let (first, first_size) = geometry(&document, "first");
        let (last, _) = geometry(&document, "last");

        assert_eq!(last.y - (first.y + first_size.height), 51.0);
    }

    #[test]
    fn nonzero_min_height_does_not_close_the_parent_last_child_edge() {
        let document = layout_html("<div id='parent' style='min-height:10px;margin-bottom:20px'><div id='child' style='height:10px;margin-bottom:30px'></div></div><div id='next' style='height:10px'></div>");
        let (parent, parent_size) = geometry(&document, "parent");
        let (next, _) = geometry(&document, "next");

        assert_eq!(parent_size.height, 10.0);
        assert_eq!(next.y - (parent.y + parent_size.height), 30.0);
    }

    #[test]
    fn binding_min_height_closes_the_parent_last_child_edge() {
        let document = layout_html("<div id='parent' style='min-height:50px;margin-bottom:20px'><div id='child' style='margin-bottom:30px'></div></div><div id='next' style='height:10px'></div>");
        let (parent, parent_size) = geometry(&document, "parent");
        let (next, _) = geometry(&document, "next");

        assert_eq!(parent_size.height, 50.0);
        assert_eq!(next.y - (parent.y + parent_size.height), 20.0);
    }

    #[test]
    fn nonzero_min_height_prevents_a_descendant_margin_from_escaping_both_edges() {
        let document = layout_document_with_css(
            "<html><body>\
                 <div id='parent' class='parent'>\
                     <div></div><div></div>\
                     <div class='last-child'><div id='float' class='float'>floating text</div></div>\
                 </div><div id='next' style='height:1px'></div>\
             </body></html>",
            Some(
                "body { margin: 0; }\
                 div.parent { min-height: 2em; height: auto; margin: 1em 0; }\
                 div.last-child { margin-bottom: 5em; }\
                 div.float { float: left; width: 10px; height: 10px; }",
            ),
        );
        let (parent, parent_size) = geometry(&document, "parent");
        let (float, _) = geometry(&document, "float");
        let (next, _) = geometry(&document, "next");

        assert_eq!(parent.y, 80.0);
        assert_eq!(parent_size.height, 32.0);
        assert_eq!(float.y, parent.y);
        assert_eq!(next.y, parent.y + parent_size.height + 16.0);
    }

    #[test]
    fn binding_max_height_closes_the_parent_last_child_edge() {
        let document = layout_html("<div id='parent' style='max-height:50px'><div id='child' style='height:51px;margin-bottom:10px'></div></div><div id='next' style='height:50px'></div>");
        let (parent, parent_size) = geometry(&document, "parent");
        let (next, _) = geometry(&document, "next");

        assert_eq!(parent_size.height, 50.0);
        assert_eq!(next.y, parent.y + parent_size.height);
    }

    #[test]
    fn root_margin_does_not_collapse_with_the_first_descendant_margin() {
        let document = layout_document("<html style='margin-top:20px'><body style='margin:0'><div id='child' style='height:10px;margin-top:20px'></div></body></html>");
        let (child, _) = geometry(&document, "child");

        assert_eq!(child.y, 40.0);
    }

    #[test]
    fn body_and_first_child_top_margins_collapse_normally() {
        let document = layout_document("<html style='margin:0'><body style='margin-top:40px'><div id='child' style='height:10px;margin-top:40px'></div></body></html>");
        let (child, _) = geometry(&document, "child");

        assert_eq!(child.y, 40.0);
    }

    #[test]
    fn collapsed_child_margin_is_considered_before_clearance() {
        let document = layout_html("<div id='float' style='float:left;width:10px;height:50px'></div><div id='cleared' style='clear:left'><div id='child' style='height:50px;margin-top:50px'></div></div>");
        let (float, _) = geometry(&document, "float");
        let (child, _) = geometry(&document, "child");

        assert_eq!(child.y, float.y, "the already-sufficient collapsed margin must not receive extra clearance");
    }

    #[test]
    fn actual_clearance_places_the_border_edge_at_the_float_bottom_once() {
        let document = layout_html("<div id='float' style='float:left;width:10px;height:50px'></div><div id='cleared' style='clear:left;height:10px;margin-top:10px'></div>");
        let (float, float_size) = geometry(&document, "float");
        let (cleared, _) = geometry(&document, "cleared");

        assert_eq!(cleared.y, float.y + float_size.height);
    }

    #[test]
    fn clear_without_a_matching_float_still_collapses_through() {
        let document = layout_html(
            "<div id='parent' style='height:100px'></div>\
             <div id='empty' style='clear:both;margin-top:100px'></div>",
        );
        let (parent, parent_size) = geometry(&document, "parent");
        let (empty, empty_size) = geometry(&document, "empty");

        assert_eq!(empty.y, parent.y + parent_size.height);
        assert_eq!(empty_size.height, 0.0);
    }

    #[test]
    fn following_margin_remains_after_clearance_on_a_collapsed_through_box() {
        let document = layout_html(
            "<div id='parent'>\
                 <div style='float:left;width:0;height:1px'></div>\
                 <div style='clear:left'></div>\
                 <div style='margin-top:99px'></div>\
             </div>",
        );
        let (_, parent_size) = geometry(&document, "parent");

        assert_eq!(parent_size.height, 100.0);
    }

    #[test]
    fn collapsed_margin_after_actual_clearance_cannot_escape_parent_bottom() {
        let document = layout_html(
            "<div id='parent' style='border-top:1px solid black'>\
                 <div style='float:left;width:10px;height:100px'></div>\
                 <div id='cleared-chain' style='clear:left;margin-top:40px;margin-bottom:140px'></div>\
             </div>\
             <div id='next' style='height:1px'></div>",
        );
        let (parent, parent_size) = geometry(&document, "parent");
        let (cleared, cleared_size) = geometry(&document, "cleared-chain");
        let (next, _) = geometry(&document, "next");

        assert_eq!(parent_size.height, 201.0, "parent={parent:?} cleared={cleared:?}/{cleared_size:?} next={next:?}");
        assert_eq!(next.y, parent.y + parent_size.height);
    }

    #[test]
    fn clearance_stops_a_later_margin_from_escaping_past_a_nested_inline_float_anchor() {
        let document = layout_html(
            "<div id='parent' style='width:100px'>\
                 <div><div id='float' style='float:left;width:100px;height:50px'></div></div>\
                 <div id='cleared' style='clear:left;height:50px;margin-top:400px'></div>\
             </div>",
        );
        let (parent, parent_size) = geometry(&document, "parent");
        let (float, float_size) = geometry(&document, "float");
        let (cleared, cleared_size) = geometry(&document, "cleared");

        assert_eq!(float.y, parent.y);
        assert_eq!(cleared.y, float.y + float_size.height);
        assert_eq!(parent_size.height, float_size.height + cleared_size.height);
    }

    #[test]
    fn negative_clearance_after_an_empty_bottom_margin_contributes_parent_height() {
        let document = layout_html(
            "<div id='outer'>\
                 <div id='float' style='float:left;width:50px;height:50px;border-top:50px solid white'></div>\
                 <div id='padded' style='padding-top:1px'>\
                     <div id='green' style='width:100px'>\
                         <div id='zero' style='margin-bottom:49px'></div>\
                         <div id='cleared' style='clear:left;margin-top:98px'></div>\
                     </div>\
                     <div id='lower' style='width:100px;height:50px'></div>\
                 </div>\
             </div>",
        );
        let (green, green_size) = geometry(&document, "green");
        let (lower, _) = geometry(&document, "lower");
        let (zero, zero_size) = geometry(&document, "zero");
        let (cleared, cleared_size) = geometry(&document, "cleared");

        assert_eq!(green_size.height, 50.0, "green={green:?} zero={zero:?}/{zero_size:?} cleared={cleared:?}/{cleared_size:?} lower={lower:?}");
        assert_eq!(lower.y, green.y + green_size.height);
    }

    #[test]
    fn cleared_descendant_margin_does_not_shift_its_relative_wrapper() {
        let document = layout_html(
            "<p>Test passes if there is a filled green square.</p>\
             <div id='outer' style='position:relative;top:-50px'>\
                 <div id='float' style='float:left;width:50px;height:50px;border-top:50px solid white'></div>\
                 <div style='padding-top:1px'>\
                     <div id='green' style='width:100px'>\
                         <div style='margin-bottom:49px'></div>\
                         <div style='clear:left;margin-top:98px'></div>\
                     </div>\
                     <div id='lower' style='width:100px;height:50px'></div>\
                 </div>\
             </div>",
        );
        let (outer, _) = geometry(&document, "outer");
        let (float, float_size) = geometry(&document, "float");
        let (green, green_size) = geometry(&document, "green");
        let (lower, _) = geometry(&document, "lower");

        assert_eq!(float.y, outer.y);
        assert_eq!(green.y, float.y + float_size.height - 50.0, "outer={outer:?} float={float:?}/{float_size:?} green={green:?}/{green_size:?} lower={lower:?}");
        assert_eq!(lower.y, green.y + green_size.height);
    }

    #[test]
    fn clearance_is_inserted_above_a_floats_negative_top_margin() {
        let document = layout_html(
            "<div style='height:192px;width:192px'>\
             <div id='first' style='float:right;width:96px;height:96px'></div>\
             <div id='second' style='float:right;clear:right;width:96px;height:96px;margin-top:-96px'></div>\
             </div>",
        );
        let (first, _) = geometry(&document, "first");
        let (second, _) = geometry(&document, "second");

        assert_eq!(second.y, first.y, "clearance belongs above the top margin, so the negative margin pulls the border box back over the first float");
    }

    #[test]
    fn inline_float_starts_at_its_paragraphs_first_line() {
        let document = layout_html("<p id='paragraph'><span id='float' style='float:left'>PA</span>SS</p>");
        let (paragraph, _) = geometry(&document, "paragraph");
        let (float, _) = geometry(&document, "float");

        assert_eq!(float.y, paragraph.y, "an inline float at the start of a paragraph must use the first line's source position");
    }

    #[test]
    fn full_width_inline_float_after_text_starts_below_that_line() {
        let document = layout_html("<p id='paragraph' style='width:100px;font-size:5px'>H<span id='float' style='float:left;width:100px;height:20px'></span></p>");
        let (paragraph, _) = geometry(&document, "paragraph");
        let (float, _) = geometry(&document, "float");

        assert!(float.y > paragraph.y, "paragraph={paragraph:?} float={float:?}");
    }

    #[test]
    fn inline_float_after_text_shares_the_line_when_both_fit() {
        let document = layout_html("<p id='paragraph' style='width:200px;font-size:5px'>H<span id='float' style='float:right;width:100px;height:20px'></span></p>");
        let (paragraph, _) = geometry(&document, "paragraph");
        let (float, _) = geometry(&document, "float");

        assert_eq!(float.y, paragraph.y, "paragraph={paragraph:?} float={float:?}");
    }

    #[test]
    fn clearing_empty_wrappers_advance_between_their_floated_children() {
        let document = layout_html(
            "<div style='clear:both'><p id='one' style='float:left;margin:0'>one</p></div>\
             <div style='clear:both;width:300px'><p id='two' style='float:left;margin:0'>two</p></div>\
             <div style='clear:both;height:0'><p id='three' style='float:left;margin:0'>three</p></div>",
        );
        let (one, one_size) = geometry(&document, "one");
        let (two, two_size) = geometry(&document, "two");
        let (three, _) = geometry(&document, "three");

        assert_eq!(two.y, one.y + one_size.height);
        assert_eq!(three.y, two.y + two_size.height);
        let text = document.render_view().text();
        let painted_lines = text.lines().iter().filter(|line| !line.glyphs().is_empty()).collect::<Vec<_>>();
        assert_eq!(painted_lines.len(), 3, "each floated paragraph must retain its painted line");
        let line_bottoms = text.lines().iter().map(|line| line.point().y + line.height()).collect::<Vec<_>>();
        assert!(line_bottoms.windows(2).all(|pair| pair[0] <= pair[1]), "physical line storage must remain spatially searchable: {line_bottoms:?}");
        let paint_order = text.paint_order_indices();
        let mut covered = paint_order.to_vec();
        covered.sort_unstable();
        assert_eq!(covered, (0..text.line_count() as u32).collect::<Vec<_>>(), "paint traversal must cover every spatial line exactly once");
        // These floats are both sourced and positioned top-to-bottom, so the
        // valid paint traversal may equal spatial order. Non-spatial stacking
        // is covered by the positioned z-index and overlapping-inline tests.
    }

    #[test]
    fn clearing_break_starts_the_next_float_row_below_collapsed_tables() {
        let document = layout_document(
            "<html><body style='margin:0'>\
             <table id='a' style='border-collapse:collapse;float:left'><tr style='border:25px solid red'><td style='padding:0;border:25px solid green'></td></tr></table>\
             <table id='b' style='border-collapse:collapse;float:left'><tbody style='border:25px solid red'><td style='padding:0;border:25px solid green'></td></tbody></table>\
             <table id='c' style='border-collapse:collapse;float:left'><col style='border:25px solid red'><td style='padding:0;border:25px solid green'></td></table>\
             <table id='d' style='border-collapse:collapse;float:left'><colgroup style='border:25px solid red'></colgroup><td style='padding:0;border:25px solid green'></td></table>\
             <br id='clear' style='clear:both'>\
             <table id='e' style='border-collapse:collapse;float:left;border:25px solid red'><td style='padding:0;border:25px solid green'></td></table>\
             </body></html>",
        );
        let (a, a_size) = geometry(&document, "a");
        let (b, b_size) = geometry(&document, "b");
        let (c, c_size) = geometry(&document, "c");
        let (d, d_size) = geometry(&document, "d");
        let (e, _) = geometry(&document, "e");

        assert_eq!([a_size.width, b_size.width, c_size.width, d_size.width], [50.0; 4]);
        assert_eq!([a.x, b.x, c.x, d.x], [0.0, 50.0, 100.0, 150.0]);
        assert_eq!(e, Point::new(0.0, a.y + 50.0));
    }
}
