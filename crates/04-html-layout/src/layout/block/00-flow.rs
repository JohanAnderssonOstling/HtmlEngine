//! Process each child through preparation, layout, and finishing in source order.
use super::finish_child::{finish_child, finish_flow};
use super::layout_child::layout_child;
use super::margins::parent_collapse_context;
use super::prepare_child::prepare_child;
use super::{BlockFlowInput, BlockFlowState};
use crate::layout::{ContentLayoutResult, LayoutEngine};
use kurbo::Vec2;

/// Child percentages use `available_width`; the parent's padding uses `parent_cb_width`.
pub fn layout_block_children(
    engine: &mut LayoutEngine<'_, '_>,
    parent_idx: usize,
    children: &[u32],
    available_width: f64,
    parent_cb_width: f64,
    parent_content_height: Option<f64>,
    first_line_indent: f64,
) -> ContentLayoutResult {
    let started = engine.start_timing();
    let style = engine.reader.style(parent_idx);
    let padding = style.get_top_left_padding(parent_cb_width)
        + Vec2::new(
            style.border_left_width() as f64,
            style.border_top_width() as f64,
        );
    let input = BlockFlowInput {
        parent_idx,
        available_width,
        parent_cb_width,
        parent_content_height,
        fragment_height: engine.fragment_height(),
        position: engine.geometry.point(parent_idx) + padding,
        parent_ctx: parent_collapse_context(&engine.reader, parent_idx),
    };
    let mut flow = BlockFlowState {
        indent_remaining: first_line_indent,
        ..BlockFlowState::default()
    };
    for &child in children {
        let Some(prepared) = prepare_child(engine, &input, &mut flow, child as usize) else {
            continue;
        };
        let Some(laid_out) = layout_child(engine, &input, &mut flow, prepared) else {
            continue;
        };
        finish_child(engine, &input, &mut flow, laid_out);
    }
    let size = finish_flow(engine, &input, &mut flow);
    engine.record_timing(|t| t.layout_block_children += started.elapsed());
    size
}

#[cfg(test)]
mod tests {
    use crate::GlyphResourceStore;

    use crate::LaidOutDocument;
    use crate::parser::DocumentFactory;
    use crate::test_support::TestGlyphShaper;
    use kurbo::{Point, Size};

    fn layout_html(body: &str, resources: &mut GlyphResourceStore) -> LaidOutDocument<'static> {
        let html = format!("<html><body style='margin:0'>{body}</body></html>");
        layout_document(&html, resources)
    }

    fn layout_document(html: &str, resources: &mut GlyphResourceStore) -> LaidOutDocument<'static> {
        layout_document_with_css(html, None, resources)
    }

    fn layout_document_with_css(
        html: &str,
        css: Option<&str>,
        resources: &mut GlyphResourceStore,
    ) -> LaidOutDocument<'static> {
        let mut factory = DocumentFactory::new();
        let mut glyphs = TestGlyphShaper::new();
        factory
            .parse_with_new_pipeline(html, css)
            .shape(&mut glyphs, resources)
            .expect("test glyphs shape")
            .layout(
                crate::LayoutConstraints::new(300.0, 16.0).unwrap(),
                resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles")
    }

    fn geometry(
        document: &LaidOutDocument,
        id: &str,
        resources: &GlyphResourceStore,
    ) -> (Point, Size) {
        let view = document.render_view(resources);
        let box_idx = (0..view.boxes().len())
            .find(|idx| view.boxes().id(*idx).map(|value| view.string(value)) == Some(id))
            .unwrap_or_else(|| panic!("missing box #{id}"));
        (
            view.boxes().point(box_idx).expect("box index in range"),
            view.boxes().size(box_idx).expect("box index in range"),
        )
    }

    #[test]
    fn fragmentation_moves_kept_subtrees_and_respects_book_composition() {
        use crate::{LayoutConstraints, TextCompositionPolicy};

        let html = "<html><body style='margin:0'>
            <div style='height:80px'></div>
            <div id='keep' style='height:15px;font:10px/10px serif;break-after:avoid;background:#f00'><span>X</span></div>
            <div id='following' style='height:20px'></div>
            <div id='forced' style='height:10px;break-before:page;break-after:page'></div>
            <div id='after' style='height:5px'></div>
            <div style='height:80px'></div>
            <div id='avoid' style='height:20px;break-inside:avoid'></div>
        </body></html>";
        for (policy, expected) in [
            (
                TextCompositionPolicy::WebCompatible,
                [100.0, 115.0, 200.0, 300.0, 400.0],
            ),
            (
                TextCompositionPolicy::BookOptimized,
                [80.0, 95.0, 115.0, 125.0, 210.0],
            ),
            (
                TextCompositionPolicy::BookOptimizedUnrestrictedHyphenation,
                [80.0, 95.0, 115.0, 125.0, 210.0],
            ),
        ] {
            let mut resources = GlyphResourceStore::default();
            let mut factory = DocumentFactory::new();
            let document = factory
                .parse_with_new_pipeline(html, None)
                .shape(&mut TestGlyphShaper::new(), &mut resources)
                .unwrap()
                .layout(
                    LayoutConstraints::new(300.0, 16.0)
                        .unwrap()
                        .with_viewport_height(Some(100.0))
                        .unwrap()
                        .with_text_composition_policy(policy),
                    &resources,
                )
                .unwrap();
            for (id, y) in ["keep", "following", "forced", "after", "avoid"]
                .into_iter()
                .zip(expected)
            {
                assert_eq!(
                    geometry(&document, id, &resources).0.y,
                    y,
                    "{policy:?}: {id}"
                );
            }
            let view = document.render_view(&resources);
            assert_eq!(view.text().line(0).unwrap().point().y, expected[0]);
            let background = view
                .fragments()
                .decorations()
                .iter()
                .find(|decoration| decoration.color() == 0xff0000ff)
                .expect("kept background");
            assert_eq!(background.rect().y0, expected[0]);
        }
    }

    #[test]
    fn adjoining_sibling_margins_use_the_largest_positive_value() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='first' style='height:10px;margin-bottom:30px'></div><div id='second' style='height:10px;margin-top:20px'></div>",
            &mut document_resources,
        );
        let (first, first_size) = geometry(&document, "first", &document_resources);
        let (second, _) = geometry(&document, "second", &document_resources);

        assert_eq!(second.y - (first.y + first_size.height), 30.0);
    }

    #[test]
    fn adjoining_positive_and_negative_margins_sum_their_extremes() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='first' style='height:10px;margin-bottom:30px'></div><div id='second' style='height:10px;margin-top:-10px'></div>",
            &mut document_resources,
        );
        let (first, first_size) = geometry(&document, "first", &document_resources);
        let (second, _) = geometry(&document, "second", &document_resources);

        assert_eq!(second.y - (first.y + first_size.height), 20.0);
    }

    #[test]
    fn first_child_margin_collapses_with_parent_even_when_child_has_a_border() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='anchor' style='height:10px'></div><div id='parent' style='margin-top:30px'><div id='child' style='height:10px;margin-top:50px;border-top:1px solid'></div></div>",
            &mut document_resources,
        );
        let (anchor, anchor_size) = geometry(&document, "anchor", &document_resources);
        let (parent, _) = geometry(&document, "parent", &document_resources);
        let (child, _) = geometry(&document, "child", &document_resources);

        assert_eq!(parent.y - (anchor.y + anchor_size.height), 50.0);
        assert_eq!(child.y, parent.y);
    }

    #[test]
    fn nested_empty_blocks_collapse_through_one_adjoining_set() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='first' style='height:10px;margin-bottom:10px'></div><div id='empty' style='height:0;margin-top:30px;margin-bottom:40px'></div><div id='last' style='height:10px;margin-top:20px'></div>",
            &mut document_resources,
        );
        let (first, first_size) = geometry(&document, "first", &document_resources);
        let (last, _) = geometry(&document, "last", &document_resources);

        assert_eq!(last.y - (first.y + first_size.height), 40.0);
    }

    #[test]
    fn parent_border_prevents_child_margin_from_escaping() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='parent' style='border-top:5px solid'><div id='child' style='height:10px;margin-top:30px'></div></div>",
            &mut document_resources,
        );
        let (parent, _) = geometry(&document, "parent", &document_resources);
        let (child, _) = geometry(&document, "child", &document_resources);

        assert_eq!(child.y - parent.y, 35.0);
    }

    #[test]
    fn overflow_formatting_context_contains_child_margin() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='parent' style='overflow:hidden'><div id='child' style='height:10px;margin-top:30px'></div></div>",
            &mut document_resources,
        );
        let (parent, _) = geometry(&document, "parent", &document_resources);
        let (child, _) = geometry(&document, "child", &document_resources);

        assert_eq!(child.y - parent.y, 30.0);
    }

    #[test]
    fn float_margins_do_not_collapse_with_the_preceding_block_margin() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='first' style='height:10px;margin-bottom:20px'></div><div id='float' style='float:left;width:10px;height:10px;margin-top:15px'></div>",
            &mut document_resources,
        );
        let (first, first_size) = geometry(&document, "first", &document_resources);
        let (float, _) = geometry(&document, "float", &document_resources);

        assert_eq!(float.y - (first.y + first_size.height), 35.0);
    }

    #[test]
    fn nested_float_keeps_the_preceding_adjoining_margin_position() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='first' style='height:10px;margin-bottom:20px'></div><div><div id='float' style='float:left;width:10px;height:10px;margin-top:15px'></div></div>",
            &mut document_resources,
        );
        let (first, first_size) = geometry(&document, "first", &document_resources);
        let (float, _) = geometry(&document, "float", &document_resources);

        assert_eq!(float.y - (first.y + first_size.height), 35.0);
    }

    #[test]
    fn inline_absolute_anchor_shares_a_nested_floats_collapsed_source_position() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='first' style='height:10px;margin-bottom:20px'></div>\
             <div id='wrapper' style='position:relative'>\
                 <div id='absolute' style='position:absolute;width:10px;height:10px'></div>\
                 <div id='float' style='float:left;width:10px;height:10px'></div>\
             </div>",
            &mut document_resources,
        );
        let (wrapper, _) = geometry(&document, "wrapper", &document_resources);
        let (absolute, _) = geometry(&document, "absolute", &document_resources);
        let (float, _) = geometry(&document, "float", &document_resources);

        assert_eq!(wrapper.y, float.y);
        assert_eq!(absolute.y, float.y);
    }

    #[test]
    fn inline_image_prevents_its_wrapper_from_collapsing_through() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='first' style='height:10px;margin-bottom:20px'></div>\
             <div id='wrapper'><svg id='image' width='96' height='96'></svg></div>",
            &mut document_resources,
        );
        let (first, first_size) = geometry(&document, "first", &document_resources);
        let (wrapper, _) = geometry(&document, "wrapper", &document_resources);
        let (image, _) = geometry(&document, "image", &document_resources);

        assert_eq!(wrapper.y, first.y + first_size.height + 20.0);
        assert!(
            (image.y - wrapper.y).abs() < 1.0,
            "wrapper={wrapper:?} image={image:?}"
        );
    }

    #[test]
    fn floated_box_contents_do_not_move_the_outer_float_source_position() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='first' style='height:10px;margin-bottom:20px'></div><div><span id='float' style='display:inline-block;float:right;width:20px;height:20px'><span style='display:block'>a</span><span style='display:block'>b</span></span></div>",
            &mut document_resources,
        );
        let (first, first_size) = geometry(&document, "first", &document_resources);
        let (float, _) = geometry(&document, "float", &document_resources);

        assert_eq!(float.y - (first.y + first_size.height), 20.0);
    }

    #[test]
    fn nonzero_min_height_prevents_empty_block_collapse_through() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='first' style='height:10px'></div><div id='empty' style='min-height:1px;margin-top:20px;margin-bottom:30px'></div><div id='last' style='height:10px'></div>",
            &mut document_resources,
        );
        let (first, first_size) = geometry(&document, "first", &document_resources);
        let (last, _) = geometry(&document, "last", &document_resources);

        assert_eq!(last.y - (first.y + first_size.height), 51.0);
    }

    #[test]
    fn nonzero_min_height_does_not_close_the_parent_last_child_edge() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='parent' style='min-height:10px;margin-bottom:20px'><div id='child' style='height:10px;margin-bottom:30px'></div></div><div id='next' style='height:10px'></div>",
            &mut document_resources,
        );
        let (parent, parent_size) = geometry(&document, "parent", &document_resources);
        let (next, _) = geometry(&document, "next", &document_resources);

        assert_eq!(parent_size.height, 10.0);
        assert_eq!(next.y - (parent.y + parent_size.height), 30.0);
    }

    #[test]
    fn binding_min_height_closes_the_parent_last_child_edge() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='parent' style='min-height:50px;margin-bottom:20px'><div id='child' style='margin-bottom:30px'></div></div><div id='next' style='height:10px'></div>",
            &mut document_resources,
        );
        let (parent, parent_size) = geometry(&document, "parent", &document_resources);
        let (next, _) = geometry(&document, "next", &document_resources);

        assert_eq!(parent_size.height, 50.0);
        assert_eq!(next.y - (parent.y + parent_size.height), 20.0);
    }

    #[test]
    fn nonzero_min_height_prevents_a_descendant_margin_from_escaping_both_edges() {
        let mut document_resources = GlyphResourceStore::default();
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
            &mut document_resources,
        );
        let (parent, parent_size) = geometry(&document, "parent", &document_resources);
        let (float, _) = geometry(&document, "float", &document_resources);
        let (next, _) = geometry(&document, "next", &document_resources);

        assert_eq!(parent.y, 80.0);
        assert_eq!(parent_size.height, 32.0);
        assert_eq!(float.y, parent.y);
        assert_eq!(next.y, parent.y + parent_size.height + 16.0);
    }

    #[test]
    fn binding_max_height_closes_the_parent_last_child_edge() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='parent' style='max-height:50px'><div id='child' style='height:51px;margin-bottom:10px'></div></div><div id='next' style='height:50px'></div>",
            &mut document_resources,
        );
        let (parent, parent_size) = geometry(&document, "parent", &document_resources);
        let (next, _) = geometry(&document, "next", &document_resources);

        assert_eq!(parent_size.height, 50.0);
        assert_eq!(next.y, parent.y + parent_size.height);
    }

    #[test]
    fn root_margin_does_not_collapse_with_the_first_descendant_margin() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_document(
            "<html style='margin-top:20px'><body style='margin:0'><div id='child' style='height:10px;margin-top:20px'></div></body></html>",
            &mut document_resources,
        );
        let (child, _) = geometry(&document, "child", &document_resources);

        assert_eq!(child.y, 40.0);
    }

    #[test]
    fn body_and_first_child_top_margins_collapse_normally() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_document(
            "<html style='margin:0'><body style='margin-top:40px'><div id='child' style='height:10px;margin-top:40px'></div></body></html>",
            &mut document_resources,
        );
        let (child, _) = geometry(&document, "child", &document_resources);

        assert_eq!(child.y, 40.0);
    }

    #[test]
    fn collapsed_child_margin_is_considered_before_clearance() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='float' style='float:left;width:10px;height:50px'></div><div id='cleared' style='clear:left'><div id='child' style='height:50px;margin-top:50px'></div></div>",
            &mut document_resources,
        );
        let (float, _) = geometry(&document, "float", &document_resources);
        let (child, _) = geometry(&document, "child", &document_resources);

        assert_eq!(
            child.y, float.y,
            "the already-sufficient collapsed margin must not receive extra clearance"
        );
    }

    #[test]
    fn actual_clearance_places_the_border_edge_at_the_float_bottom_once() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='float' style='float:left;width:10px;height:50px'></div><div id='cleared' style='clear:left;height:10px;margin-top:10px'></div>",
            &mut document_resources,
        );
        let (float, float_size) = geometry(&document, "float", &document_resources);
        let (cleared, _) = geometry(&document, "cleared", &document_resources);

        assert_eq!(cleared.y, float.y + float_size.height);
    }

    #[test]
    fn clear_without_a_matching_float_still_collapses_through() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='parent' style='height:100px'></div>\
             <div id='empty' style='clear:both;margin-top:100px'></div>",
            &mut document_resources,
        );
        let (parent, parent_size) = geometry(&document, "parent", &document_resources);
        let (empty, empty_size) = geometry(&document, "empty", &document_resources);

        assert_eq!(empty.y, parent.y + parent_size.height);
        assert_eq!(empty_size.height, 0.0);
    }

    #[test]
    fn following_margin_remains_after_clearance_on_a_collapsed_through_box() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='parent'>\
                 <div style='float:left;width:0;height:1px'></div>\
                 <div style='clear:left'></div>\
                 <div style='margin-top:99px'></div>\
             </div>",
            &mut document_resources,
        );
        let (_, parent_size) = geometry(&document, "parent", &document_resources);

        assert_eq!(parent_size.height, 100.0);
    }

    #[test]
    fn collapsed_margin_after_actual_clearance_cannot_escape_parent_bottom() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='parent' style='border-top:1px solid black'>\
                 <div style='float:left;width:10px;height:100px'></div>\
                 <div id='cleared-chain' style='clear:left;margin-top:40px;margin-bottom:140px'></div>\
             </div>\
             <div id='next' style='height:1px'></div>",
            &mut document_resources,
        );
        let (parent, parent_size) = geometry(&document, "parent", &document_resources);
        let (cleared, cleared_size) = geometry(&document, "cleared-chain", &document_resources);
        let (next, _) = geometry(&document, "next", &document_resources);

        assert_eq!(
            parent_size.height, 201.0,
            "parent={parent:?} cleared={cleared:?}/{cleared_size:?} next={next:?}"
        );
        assert_eq!(next.y, parent.y + parent_size.height);
    }

    #[test]
    fn clearance_stops_a_later_margin_from_escaping_past_a_nested_inline_float_anchor() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div id='parent' style='width:100px'>\
                 <div><div id='float' style='float:left;width:100px;height:50px'></div></div>\
                 <div id='cleared' style='clear:left;height:50px;margin-top:400px'></div>\
             </div>",
            &mut document_resources,
        );
        let (parent, parent_size) = geometry(&document, "parent", &document_resources);
        let (float, float_size) = geometry(&document, "float", &document_resources);
        let (cleared, cleared_size) = geometry(&document, "cleared", &document_resources);

        assert_eq!(float.y, parent.y);
        assert_eq!(cleared.y, float.y + float_size.height);
        assert_eq!(parent_size.height, float_size.height + cleared_size.height);
    }

    #[test]
    fn negative_clearance_after_an_empty_bottom_margin_contributes_parent_height() {
        let mut document_resources = GlyphResourceStore::default();
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
            &mut document_resources,
        );
        let (green, green_size) = geometry(&document, "green", &document_resources);
        let (lower, _) = geometry(&document, "lower", &document_resources);
        let (zero, zero_size) = geometry(&document, "zero", &document_resources);
        let (cleared, cleared_size) = geometry(&document, "cleared", &document_resources);

        assert_eq!(
            green_size.height, 50.0,
            "green={green:?} zero={zero:?}/{zero_size:?} cleared={cleared:?}/{cleared_size:?} lower={lower:?}"
        );
        assert_eq!(lower.y, green.y + green_size.height);
    }

    #[test]
    fn cleared_descendant_margin_does_not_shift_its_relative_wrapper() {
        let mut document_resources = GlyphResourceStore::default();
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
            &mut document_resources,
        );
        let (outer, _) = geometry(&document, "outer", &document_resources);
        let (float, float_size) = geometry(&document, "float", &document_resources);
        let (green, green_size) = geometry(&document, "green", &document_resources);
        let (lower, _) = geometry(&document, "lower", &document_resources);

        assert_eq!(float.y, outer.y);
        assert_eq!(
            green.y,
            float.y + float_size.height - 50.0,
            "outer={outer:?} float={float:?}/{float_size:?} green={green:?}/{green_size:?} lower={lower:?}"
        );
        assert_eq!(lower.y, green.y + green_size.height);
    }

    #[test]
    fn clearance_is_inserted_above_a_floats_negative_top_margin() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div style='height:192px;width:192px'>\
             <div id='first' style='float:right;width:96px;height:96px'></div>\
             <div id='second' style='float:right;clear:right;width:96px;height:96px;margin-top:-96px'></div>\
             </div>",
            &mut document_resources,
        );
        let (first, _) = geometry(&document, "first", &document_resources);
        let (second, _) = geometry(&document, "second", &document_resources);

        assert_eq!(
            second.y, first.y,
            "clearance belongs above the top margin, so the negative margin pulls the border box back over the first float"
        );
    }

    #[test]
    fn inline_float_starts_at_its_paragraphs_first_line() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<p id='paragraph'><span id='float' style='float:left'>PA</span>SS</p>",
            &mut document_resources,
        );
        let (paragraph, _) = geometry(&document, "paragraph", &document_resources);
        let (float, _) = geometry(&document, "float", &document_resources);

        assert_eq!(
            float.y, paragraph.y,
            "an inline float at the start of a paragraph must use the first line's source position"
        );
    }

    #[test]
    fn full_width_inline_float_after_text_starts_below_that_line() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<p id='paragraph' style='width:100px;font-size:5px'>H<span id='float' style='float:left;width:100px;height:20px'></span></p>",
            &mut document_resources,
        );
        let (paragraph, _) = geometry(&document, "paragraph", &document_resources);
        let (float, _) = geometry(&document, "float", &document_resources);

        assert!(
            float.y > paragraph.y,
            "paragraph={paragraph:?} float={float:?}"
        );
    }

    #[test]
    fn inline_float_after_text_shares_the_line_when_both_fit() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<p id='paragraph' style='width:200px;font-size:5px'>H<span id='float' style='float:right;width:100px;height:20px'></span></p>",
            &mut document_resources,
        );
        let (paragraph, _) = geometry(&document, "paragraph", &document_resources);
        let (float, _) = geometry(&document, "float", &document_resources);

        assert_eq!(
            float.y, paragraph.y,
            "paragraph={paragraph:?} float={float:?}"
        );
    }

    #[test]
    fn float_after_emergency_wrapping_uses_the_line_selected_by_composition() {
        let mut resources = GlyphResourceStore::default();
        let document = layout_html(
            "<p id='paragraph' style='margin:0;width:24px;font-size:10px;line-height:10px;word-break:break-all'>ABCDEFGHIJ<span id='float' style='float:right;width:8px;height:10px'></span></p>",
            &mut resources,
        );
        let (paragraph, _) = geometry(&document, "paragraph", &resources);
        let (float, _) = geometry(&document, "float", &resources);
        assert!(
            float.y >= paragraph.y + 20.0,
            "the float follows wrapped source text: paragraph={paragraph:?}, float={float:?}"
        );
    }

    #[test]
    fn multiple_inline_floats_keep_source_order_across_line_retries() {
        for width in [24, 32, 48, 80] {
            let mut resources = GlyphResourceStore::default();
            let html = format!(
                "<p style='margin:0;width:{width}px;font-size:10px;line-height:10px'>aa aa<span id='first' style='float:left;width:12px;height:20px;background:red'></span>bb bb<span id='second' style='float:right;width:12px;height:20px;background:blue'></span>cc cc</p>"
            );
            let document = layout_html(&html, &mut resources);
            let (first, _) = geometry(&document, "first", &resources);
            let (second, _) = geometry(&document, "second", &resources);
            assert!(
                second.y >= first.y,
                "width={width}: first={first:?}, second={second:?}"
            );
            let view = document.render_view(&resources);
            let colors = view
                .fragments()
                .decorations()
                .iter()
                .map(|decoration| decoration.color())
                .collect::<Vec<_>>();
            assert_eq!(
                colors.iter().filter(|&&color| color == 0xff0000ff).count(),
                1
            );
            assert_eq!(
                colors.iter().filter(|&&color| color == 0x0000ffff).count(),
                1
            );
        }
    }

    #[test]
    fn clearing_empty_wrappers_advance_between_their_floated_children() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_html(
            "<div style='clear:both'><p id='one' style='float:left;margin:0'>one</p></div>\
             <div style='clear:both;width:300px'><p id='two' style='float:left;margin:0'>two</p></div>\
             <div style='clear:both;height:0'><p id='three' style='float:left;margin:0'>three</p></div>",
            &mut document_resources,
        );
        let (one, one_size) = geometry(&document, "one", &document_resources);
        let (two, two_size) = geometry(&document, "two", &document_resources);
        let (three, _) = geometry(&document, "three", &document_resources);

        assert_eq!(two.y, one.y + one_size.height);
        assert_eq!(three.y, two.y + two_size.height);
        let text = document.render_view(&document_resources).text();
        let painted_lines = text
            .lines()
            .iter()
            .filter(|line| !line.glyphs().is_empty())
            .collect::<Vec<_>>();
        assert_eq!(
            painted_lines.len(),
            3,
            "each floated paragraph must retain its painted line"
        );
        let line_bottoms = text
            .lines()
            .iter()
            .map(|line| line.point().y + line.height())
            .collect::<Vec<_>>();
        assert!(
            line_bottoms.windows(2).all(|pair| pair[0] <= pair[1]),
            "spatial traversal must remain searchable: {line_bottoms:?}"
        );
        let paint_order = text.paint_order_indices();
        let mut covered = paint_order.iter().map(|id| id.index()).collect::<Vec<_>>();
        covered.sort_unstable();
        assert_eq!(
            covered,
            (0..text.line_count()).collect::<Vec<_>>(),
            "paint traversal must cover every spatial line exactly once"
        );
        // These floats are both sourced and positioned top-to-bottom, so the
        // valid paint traversal may equal spatial order. Non-spatial stacking
        // is covered by the positioned z-index and overlapping-inline tests.
    }

    #[test]
    fn clearing_break_starts_the_next_float_row_below_collapsed_tables() {
        let mut document_resources = GlyphResourceStore::default();
        let document = layout_document(
            "<html><body style='margin:0'>\
             <table id='a' style='border-collapse:collapse;float:left'><tr style='border:25px solid red'><td style='padding:0;border:25px solid green'></td></tr></table>\
             <table id='b' style='border-collapse:collapse;float:left'><tbody style='border:25px solid red'><td style='padding:0;border:25px solid green'></td></tbody></table>\
             <table id='c' style='border-collapse:collapse;float:left'><col style='border:25px solid red'><td style='padding:0;border:25px solid green'></td></table>\
             <table id='d' style='border-collapse:collapse;float:left'><colgroup style='border:25px solid red'></colgroup><td style='padding:0;border:25px solid green'></td></table>\
             <br id='clear' style='clear:both'>\
             <table id='e' style='border-collapse:collapse;float:left;border:25px solid red'><td style='padding:0;border:25px solid green'></td></table>\
             </body></html>",
            &mut document_resources,
        );
        let (a, a_size) = geometry(&document, "a", &document_resources);
        let (b, b_size) = geometry(&document, "b", &document_resources);
        let (c, c_size) = geometry(&document, "c", &document_resources);
        let (d, d_size) = geometry(&document, "d", &document_resources);
        let (e, _) = geometry(&document, "e", &document_resources);

        assert_eq!(
            [a_size.width, b_size.width, c_size.width, d_size.width],
            [50.0; 4]
        );
        assert_eq!([a.x, b.x, c.x, d.x], [0.0, 50.0, 100.0, 150.0]);
        assert_eq!(e, Point::new(0.0, a.y + 50.0));
    }
}
