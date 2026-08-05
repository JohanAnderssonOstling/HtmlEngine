use crate::LaidOutDocument;
use crate::parser::DocumentFactory;
use crate::test_support::TestGlyphShaper as GlyphCache;
use kurbo::{Size, Vec2};

fn layout_html(html: &str, width: f64) -> LaidOutDocument {
    let mut factory = DocumentFactory::new();
    let mut glyph_cache = GlyphCache::new();
    factory.parse_with_new_pipeline(html, None).shape(&mut glyph_cache).expect("test shaper must register every glyph").layout(crate::LayoutConstraints::new(width, 16.0).unwrap())
}

fn layout_html_with_font_relative_ratios(html: &str, width: f64, x_height_ratio: f32, ch_advance_ratio: f32) -> LaidOutDocument {
    let mut factory = DocumentFactory::new();
    let mut glyph_cache = GlyphCache::with_font_relative_ratios(x_height_ratio, ch_advance_ratio);
    factory.parse_with_new_pipeline(html, None).shape(&mut glyph_cache).expect("test shaper must register every glyph").layout(crate::LayoutConstraints::new(width, 16.0).unwrap())
}

fn div_size(document: &LaidOutDocument) -> Size {
    let boxes = document.render_view().boxes();
    let div_idx = (0..boxes.len()).find(|&idx| boxes.tag(idx).is_some_and(|tag| tag.eq_ignore_ascii_case("div"))).expect("div box should exist");
    boxes.size(div_idx).expect("box index in range")
}

fn tag_size(document: &LaidOutDocument, tag: &str) -> Size {
    let boxes = document.render_view().boxes();
    let box_idx = (0..boxes.len()).find(|&idx| boxes.tag(idx).is_some_and(|candidate| candidate.eq_ignore_ascii_case(tag))).unwrap_or_else(|| panic!("{tag} box should exist"));
    boxes.size(box_idx).expect("box index in range")
}

fn id_geometry(document: &LaidOutDocument, id: &str) -> (kurbo::Point, Size) {
    let view = document.render_view();
    let box_idx = (0..view.boxes().len()).find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some(id)).unwrap_or_else(|| panic!("#{id} box should exist"));
    (view.boxes().point(box_idx).expect("box index in range"), view.boxes().size(box_idx).expect("box index in range"))
}

#[test]
fn shrink_to_fit_intrinsic_widths_include_text_indent_on_the_applicable_line() {
    let html = "<!doctype html><html><body style='margin:0'>
        <div id='single' style='float:left;clear:left;text-indent:30px'><span style='display:inline-block;width:10px'></span></div>
        <div id='soft' style='float:left;clear:left;font-size:2px;text-indent:30px'><span style='display:inline-block;width:10px'></span> <span style='display:inline-block;width:20px'></span></div>
        <div id='forced' style='float:left;clear:left;text-indent:30px'><span style='display:inline-block;width:10px'></span><br><span style='display:inline-block;width:20px'></span></div>
        <div id='negative' style='float:left;clear:left;text-indent:-30px'><span style='display:inline-block;width:50px'></span></div>
        <div id='negative-soft' style='float:left;clear:left;text-indent:-30px'><span style='display:inline-block;width:10px'></span>&#x200B;<span style='display:inline-block;width:30px'></span></div>
    </body></html>";
    let document = layout_html(html, 600.0);

    assert_eq!(id_geometry(&document, "single").1.width, 40.0);
    assert_eq!(id_geometry(&document, "soft").1.width, 61.0);
    assert_eq!(id_geometry(&document, "forced").1.width, 40.0);
    assert_eq!(id_geometry(&document, "negative").1.width, 20.0);
    assert_eq!(id_geometry(&document, "negative-soft").1.width, 30.0);
}

#[test]
fn definite_float_height_transfers_percentage_replaced_height_to_intrinsic_width() {
    let html = "<!doctype html><html><body style='margin:0'><div id='float' style='float:left;height:100px'><canvas width='10' height='10' style='height:100%'></canvas></div></body></html>";
    let document = layout_html(html, 600.0);

    assert_eq!(id_geometry(&document, "float").1, Size::new(100.0, 100.0));
}

#[test]
fn content_box_width_adds_padding_and_border() {
    let html = "<html><body><div style=\"width:100px; height:50px; padding:10px; border:5px solid black;\">x</div></body></html>";
    let size = div_size(&layout_html(html, 600.0));

    assert_eq!(size.width, 130.0, "content-box: border box = width + padding + border");
    assert_eq!(size.height, 80.0);
}

#[test]
fn ch_box_lengths_use_the_selected_fonts_zero_advance() {
    let html = "<html><body><div style='font-size:80px;width:5ch;height:2ch'></div></body></html>";
    let size = div_size(&layout_html_with_font_relative_ratios(html, 600.0, 0.4, 0.625));

    assert_eq!(size.width, 250.0);
    assert_eq!(size.height, 100.0);
}

#[test]
fn compound_font_relative_calc_uses_selected_font_metrics() {
    let html = "<html><body><div style='font-size:80px;width:calc(1ex + 1ch + 1em);height:calc(1ex + 1ch + 1em)'></div></body></html>";
    let size = div_size(&layout_html_with_font_relative_ratios(html, 600.0, 0.4, 0.625));

    assert_eq!(size, Size::new(162.0, 162.0));
}

#[test]
fn mixed_calc_width_height_and_padding_resolve_against_their_bases() {
    let html = "<!doctype html><html><body style='margin:0'>
        <div style='width:200px;height:200px'>
          <div id='target' style='box-sizing:content-box;width:calc(50% - 10px);height:calc(50% - 20px);padding-left:calc(10% + 5px)'></div>
        </div>
    </body></html>";
    let document = layout_html(html, 600.0);
    let (_, size) = id_geometry(&document, "target");

    assert!((size.width - 115.0).abs() < 0.001, "90px content width plus 25px mixed-calc padding, got {}", size.width);
    assert!((size.height - 80.0).abs() < 0.001);
}

#[test]
fn mixed_calc_absolute_insets_resolve_against_containing_block_axes() {
    let html = "<!doctype html><html><body style='margin:0'>
        <div style='position:relative;width:200px;height:100px'>
          <div id='target' style='position:absolute;width:20px;height:10px;right:calc(25% - 5px);top:calc(50% - 10px)'></div>
        </div>
    </body></html>";
    let document = layout_html(html, 600.0);
    let (point, _) = id_geometry(&document, "target");

    assert_eq!(point.x, 135.0);
    assert_eq!(point.y, 40.0);
}

#[test]
fn absolute_replaced_child_does_not_contribute_to_auto_containing_block_width() {
    let html = "<html xmlns='http://www.w3.org/1999/xhtml' xmlns:svg='http://www.w3.org/2000/svg'><body style='margin:0'>
      <div id='parent' style='position:absolute;height:288px'>
        <svg:svg id='image' height='50' style='position:absolute;left:44px;right:44px;margin-left:auto;margin-right:auto'></svg:svg>
        <div style='width:200px;height:50px;margin-left:44px;margin-right:44px'></div>
      </div>
    </body></html>";
    let parsed = html_parse::parse_xml_document(html).expect("valid XHTML test document");
    let styled = html_style::style_document(parsed.build_dom(), &[]);
    let (dom, styles) = styled.into_parts();
    let prepared = crate::PreparedDocument::try_new(dom, styles).expect("style resolver must produce complete styles");
    assert_eq!(prepared.document().images().len(), 1, "inline SVG must register one image resource");
    let mut glyph_cache = GlyphCache::new();
    let shaped = prepared.shape(&mut glyph_cache).expect("test shaper must register every glyph");
    let mut image_metrics = crate::ImageMetrics::default();
    image_metrics.set(0, 300, 50);
    let document = shaped.layout_with_metrics(crate::LayoutConstraints::new(800.0, 16.0).unwrap(), &image_metrics);
    let (parent_point, parent_size) = id_geometry(&document, "parent");
    let (image_point, image_size) = id_geometry(&document, "image");

    assert_eq!(parent_size, Size::new(288.0, 288.0));
    assert_eq!(image_size.width, 300.0);
    assert_eq!(image_point.x - parent_point.x, 44.0);
}

#[test]
fn mixed_calc_min_height_uses_the_definite_parent_content_height() {
    let html = "<!doctype html><html><body style='margin:0'>
        <div style='height:400px;width:100px;border-bottom:100px solid transparent;border-right:100px solid transparent'>
          <div id='target' style='box-sizing:border-box;height:50px;min-height:calc(50% - 100px)'></div>
        </div>
    </body></html>";
    let document = layout_html(html, 600.0);
    let (_, size) = id_geometry(&document, "target");

    assert_eq!(size.height, 100.0);
}

#[test]
fn min_height_constrained_explicit_height_is_the_descendant_percentage_basis() {
    let html = "<!doctype html><html><body style='margin:0'>
        <div style='height:400px'>
          <div id='parent' style='box-sizing:border-box;height:50px;min-height:calc(50% - 100px)'>
            <div id='child' style='box-sizing:border-box;height:17px;min-height:100%;border-top:12px solid;border-bottom:34px solid'></div>
          </div>
        </div>
    </body></html>";
    let document = layout_html(html, 600.0);

    assert_eq!(id_geometry(&document, "parent").1.height, 100.0);
    assert_eq!(id_geometry(&document, "child").1.height, 100.0);
}

#[test]
fn border_box_width_includes_padding_and_border() {
    let html = "<html><body><div style=\"width:100px; height:50px; padding:10px; border:5px solid black; box-sizing:border-box;\">x</div></body></html>";
    let size = div_size(&layout_html(html, 600.0));

    assert_eq!(size.width, 100.0, "border-box: specified width is the border-box width");
    assert_eq!(size.height, 50.0);
}

#[test]
fn border_box_width_never_goes_negative() {
    // Padding+border exceed the specified width; content clamps to 0 and the
    // border box becomes just padding + border.
    let html = "<html><body><div style=\"width:10px; padding:10px; border:5px solid black; box-sizing:border-box;\"></div></body></html>";
    let size = div_size(&layout_html(html, 600.0));

    assert_eq!(size.width, 30.0, "content width clamps at 0; padding and border remain");
}

#[test]
fn minimum_size_wins_over_smaller_maximum_size() {
    let html = "<html><body><div style=\"width:50px; height:50px; min-width:70px; max-width:60px; min-height:70px; max-height:60px;\"></div></body></html>";
    let size = div_size(&layout_html(html, 600.0));

    assert_eq!(size, Size::new(70.0, 70.0));
}

#[test]
fn resolved_logical_box_properties_reach_layout_as_physical_geometry() {
    let html = "<html><body><div dir='rtl' style='inline-size:100px;block-size:50px;padding-inline:10px 20px;padding-block:4px 6px;border-inline-width:3px 7px;border-block-width:2px 8px;border-style:solid'>x</div></body></html>";
    let size = div_size(&layout_html(html, 600.0));

    assert_eq!(size.width, 140.0, "inline size plus resolved left/right padding and borders");
    assert_eq!(size.height, 70.0, "block size plus resolved top/bottom padding and borders");
}

#[test]
fn replaced_stretch_fills_its_margin_box_and_preserves_ratio() {
    let html = "<html><body><div style='width:200px;height:100px'><canvas width='100' height='100' style='width:stretch;margin:5px;padding:2px;border:3px solid'></canvas></div></body></html>";
    let size = tag_size(&layout_html(html, 600.0), "canvas");

    assert_eq!(size, Size::new(190.0, 190.0));
}

#[test]
fn replaced_intrinsic_keyword_transfers_a_definite_opposite_size() {
    let html = "<html><body><canvas width='100' height='100' style='width:min-content;height:50px;padding:2px;border:3px solid'></canvas></body></html>";
    let size = tag_size(&layout_html(html, 600.0), "canvas");

    assert_eq!(size, Size::new(60.0, 60.0));
}

#[test]
fn flex_replaced_border_box_ratio_uses_the_content_box() {
    let html = "<html><body><div style='display:flex;width:40px;height:40px;align-items:flex-start'><canvas width='16' height='16' style='box-sizing:border-box;width:30px;min-width:0;min-height:0;padding:1px 2px 3px 4px'></canvas></div></body></html>";
    let size = tag_size(&layout_html(html, 600.0), "canvas");

    assert_eq!(size, Size::new(30.0, 28.0));
}

#[test]
fn rtl_fixed_width_block_solves_the_inline_end_margin() {
    let document = layout_html("<html><body><div id='parent' style='direction:rtl;width:100px'><div id='child' style='width:20px;margin-right:10px'></div></div></body></html>", 300.0);
    let (parent_point, _) = id_geometry(&document, "parent");
    let (child_point, child_size) = id_geometry(&document, "child");

    assert_eq!(child_size.width, 20.0);
    assert_eq!(child_point.x - parent_point.x, 70.0);
}

#[test]
fn block_overconstraint_uses_the_containing_blocks_direction() {
    let document = layout_html("<html><body><div id='parent' style='direction:ltr;width:100px'><div id='child' style='direction:rtl;width:20px;margin-right:10px'></div></div></body></html>", 300.0);
    let (parent_point, _) = id_geometry(&document, "parent");
    let (child_point, _) = id_geometry(&document, "child");

    assert_eq!(child_point.x - parent_point.x, 0.0);
}

#[test]
fn inline_table_bottom_padding_contributes_once_to_parent_line_height() {
    let document = layout_html(
        "<html><body><div id='wrapper' style='width:200px'><div id='table' style='display:inline-table;padding-bottom:50px;vertical-align:top'><div style='display:table-row'><div style='display:table-cell'><div style='border-bottom:10px solid;width:200px'></div></div></div></div></div></body></html>",
        800.0,
    );
    let (wrapper_point, wrapper_size) = id_geometry(&document, "wrapper");
    let (table_point, table_size) = id_geometry(&document, "table");

    assert_eq!(table_size.height, 60.0, "10px cell content plus 50px table padding");
    assert_eq!(wrapper_size.height, 60.0, "the atomic inline table must contribute its border-box height exactly once");
    assert_eq!(table_point.y, wrapper_point.y, "vertical-align: top aligns the table to the line top");
}

#[test]
fn empty_inline_table_rows_do_not_add_a_text_strut_below_the_table_margin_box() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='wrapper'><table id='table' style='display:inline-table;border:3px solid;border-spacing:0;margin:16px 0 0 16px'>\
         <tr><td style='height:16px;width:128px;padding:0'></td></tr>\
         <tr><td style='height:16px;width:128px;padding:0'></td></tr>\
         <tr><td style='height:16px;width:128px;padding:0'></td></tr>\
         </table></div></body></html>",
        800.0,
    );
    let (_, wrapper_size) = id_geometry(&document, "wrapper");
    let (_, table_size) = id_geometry(&document, "table");

    assert_eq!(table_size.height, 54.0);
    assert_eq!(wrapper_size.height, 70.0, "the 54px border box plus its 16px top margin is the complete inline-table line contribution");
}

#[test]
fn inline_block_padding_separates_nested_and_outer_borders() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='outer' style='display:inline-block;height:200px;padding-right:50px;border-right:10px solid orange'><div id='inner' style='display:inline-block;height:200px;border-right:10px solid blue'></div></div></body></html>",
        800.0,
    );
    let (outer_point, outer_size) = id_geometry(&document, "outer");
    let (inner_point, inner_size) = id_geometry(&document, "inner");

    assert_eq!(outer_size.width, 70.0);
    assert_eq!(inner_size.width, 10.0);
    assert_eq!(inner_point.x, outer_point.x);
    assert_eq!(outer_size.width - inner_size.width, 60.0, "50px padding and the outer 10px border must follow the inner border");
    let decorations = document.render_view().fragments().decorations();
    let blues = decorations.iter().filter(|fragment| fragment.color() == 0x0000FFFF).map(|fragment| fragment.rect()).collect::<Vec<_>>();
    let oranges = decorations.iter().filter(|fragment| fragment.color() == 0xFFA500FF).map(|fragment| fragment.rect()).collect::<Vec<_>>();
    assert_eq!(blues.len(), 1, "isolated intrinsic measurement must not leak a duplicate border fragment: {blues:?}; outer={outer_point:?}, inner={inner_point:?}");
    assert_eq!(oranges.len(), 1);
    let blue = blues[0];
    let orange = oranges[0];
    assert_eq!(blue.x0, outer_point.x);
    assert_eq!(blue.width(), 10.0);
    assert_eq!(orange.x0 - blue.x1, 50.0);
    assert_eq!(orange.width(), 10.0);
}

#[test]
fn isolated_measurement_does_not_leak_deferred_absolute_boxes() {
    let document = layout_html(
        "<html><body style='margin:0'><div style='display:inline-block'><div style='width:20px;height:20px'></div><div id='absolute' style='position:absolute;left:0;top:0;width:10px;height:10px;background:blue'></div></div></body></html>",
        800.0,
    );
    let decorations = document.render_view().fragments().decorations();
    let blue = decorations.iter().filter(|fragment| fragment.color() == 0x0000FFFF).collect::<Vec<_>>();

    assert_eq!(blue.len(), 1, "the disposable measurement pass must not enqueue a second positioned descendant: {blue:?}");
}

#[test]
fn inline_block_shrink_to_fit_honors_a_descendants_max_width() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='outer' style='display:inline-block'><span id='inner' style='display:inline-block;max-width:100px;white-space:nowrap'>xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx</span></div></body></html>",
        800.0,
    );
    let (_, outer_size) = id_geometry(&document, "outer");
    let (_, inner_size) = id_geometry(&document, "inner");

    assert_eq!(inner_size.width, 100.0);
    assert_eq!(outer_size.width, 100.0, "the constrained child contribution must determine the ancestor's shrink-to-fit width");
    assert!((outer_size.height - inner_size.height).abs() < 0.001, "propagating the inner inline-block baseline must not add a synthetic descender to the outer box: outer={outer_size:?}, inner={inner_size:?}");
}

#[test]
fn inline_block_uses_an_empty_terminal_block_childs_bottom_baseline_before_own_padding() {
    let document =
        layout_html("<html><body style='margin:0'><span id='outer' style='display:inline-block;padding-bottom:20px'><span id='inner' style='display:block;width:30px;height:30px;overflow:hidden'></span></span>XX</body></html>", 800.0);
    let (outer_point, _) = id_geometry(&document, "outer");
    let line = document.render_view().text().line(0).expect("outer atomic box and following text share a line");
    let baseline_in_outer = line.point().y + line.baseline() - outer_point.y;

    assert!((baseline_in_outer - 30.0).abs() < 0.001, "the child bottom supplies the baseline before the outer padding; got {baseline_in_outer}");
}

#[test]
fn absolute_shrink_to_fit_honors_a_floated_descendants_em_max_width() {
    let mut factory = DocumentFactory::new();
    let mut glyph_cache = GlyphCache::new();
    let document = factory
        .parse_with_new_pipeline(
            "<html><body><p>preceding normal-flow content</p><div id='outer'><div id='inner'>1234567812345678</div></div></body></html>",
            Some("body { margin: 8px } div#outer { position:absolute;left:auto;right:auto;width:auto;font:30px/4 Ahem;background:red } div#inner { float:left;max-width:4em;background:green }"),
        )
        .shape(&mut glyph_cache)
        .expect("test shaper must register every glyph")
        .layout(crate::LayoutConstraints::new(800.0, 16.0).unwrap());
    let (_, outer_size) = id_geometry(&document, "outer");
    let (_, inner_size) = id_geometry(&document, "inner");

    assert_eq!(inner_size.width, 120.0, "the float's max-width must constrain its final border box");
    assert_eq!(outer_size.width, 120.0, "the constrained float must determine its absolute ancestor's shrink-to-fit width");
}

#[test]
fn outside_list_marker_hangs_before_border_and_padding() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='item' style='display:list-item;width:200px;height:200px;border-left:10px solid blue;padding-left:50px'><div id='child' style='border-left:10px solid orange;height:200px'></div></div></body></html>",
        800.0,
    );
    let view = document.render_view();
    let boxes = view.boxes();
    let item_idx = (0..boxes.len()).find(|&idx| boxes.id(idx).map(|value| view.string(value)) == Some("item")).expect("list item box");
    let marker_idx = boxes.list_marker(item_idx).expect("outside marker").marker_box();
    let item_point = boxes.point(item_idx).expect("list item geometry");
    let marker_point = boxes.point(marker_idx).expect("marker geometry");
    let marker_size = boxes.size(marker_idx).expect("marker size");
    let (child_point, _) = id_geometry(&document, "child");

    assert!(marker_point.x + marker_size.width < item_point.x, "outside marker must remain left of the list item's border edge: marker={marker_point:?} size={marker_size:?}, item={item_point:?}");
    assert_eq!(child_point.x - (item_point.x + 10.0), 50.0, "the child border must follow the full left padding");
}

#[test]
fn paired_auto_margins_center_a_fixed_width_block() {
    let document = layout_html("<html><body><div id='parent' style='width:100px'><div id='child' style='width:20px;margin-left:auto;margin-right:auto'></div></div></body></html>", 300.0);
    let (parent_point, _) = id_geometry(&document, "parent");
    let (child_point, _) = id_geometry(&document, "child");

    assert_eq!(child_point.x - parent_point.x, 40.0);
}

#[test]
fn relative_position_offsets_paint_without_consuming_flow_space() {
    let document = layout_html(
        "<html><body><div id='first' style='width:5px;height:5px'></div><div id='second' style='position:relative;left:7px;top:11px;width:5px;height:5px;background:orange'></div><div id='third' style='width:5px;height:5px'></div></body></html>",
        300.0,
    );
    let (first, _) = id_geometry(&document, "first");
    let (second, _) = id_geometry(&document, "second");
    let (third, _) = id_geometry(&document, "third");
    let orange = document.render_view().fragments().decorations().iter().find(|fragment| fragment.color() == 0xFFA500FF).expect("relative box background");

    assert_eq!(second.x - first.x, 7.0);
    assert_eq!(second.y - first.y, 16.0);
    assert_eq!(third.y - first.y, 10.0, "relative movement must not change following flow positions");
    assert_eq!(orange.rect().origin(), second);
}

#[test]
fn relative_block_after_inline_replaced_content_keeps_ancestor_origin() {
    let html = "<html xmlns='http://www.w3.org/1999/xhtml'><body><p>Test passes if there is a filled green square and no red.</p><div id='parent'>\n<svg:svg xmlns:svg='http://www.w3.org/2000/svg' id='replaced' width='150'><svg:rect width='600' height='300' fill='red'/></svg:svg>\n<div id='overlay'></div></div></body></html>";
    let css = "div { height:300px; width:300px } svg#replaced { display:inline; vertical-align:top } div#overlay { background-color:green; bottom:150px; height:150px; position:relative; width:150px }";
    let parsed = html_parse::parse_xml_document(html).expect("valid XHTML test document");
    let styled = html_style::style_document(parsed.build_dom(), &[css]);
    let (dom, styles) = styled.into_parts();
    let prepared = crate::PreparedDocument::try_new(dom, styles).expect("style resolver must produce complete styles");
    assert_eq!(prepared.document().images().len(), 1, "inline SVG must register one image resource");
    assert!(prepared.inline_content().inline_items().iter().any(|run| matches!(run.kind, crate::layout_model::InlineItemKind::Image { .. })), "inline SVG must remain an image run before a block sibling");
    let mut glyph_cache = GlyphCache::new();
    let shaped = prepared.shape(&mut glyph_cache).expect("test shaper must register every glyph");
    let mut image_metrics = crate::ImageMetrics::default();
    image_metrics.set(0, 150, 150);
    let document = shaped.layout_with_metrics(crate::LayoutConstraints::new(800.0, 16.0).unwrap(), &image_metrics);
    let (parent, _) = id_geometry(&document, "parent");
    let (replaced, replaced_size) = id_geometry(&document, "replaced");
    let (overlay, _) = id_geometry(&document, "overlay");
    let green = document.render_view().fragments().decorations().iter().find(|fragment| fragment.color() == 0x008000FF).expect("relative overlay background");

    assert!(parent.y > 0.0, "preceding paragraph must move the containing block below the document origin");
    assert!((replaced.y - parent.y).abs() < 0.001, "replaced content and its containing block must share an origin: replaced={replaced:?}, parent={parent:?}");
    assert_eq!(replaced_size, Size::new(150.0, 150.0));
    assert!((overlay.y - replaced.y).abs() < 0.001, "bottom positioning must remain in the ancestor's document-space coordinate system: overlay={overlay:?}, replaced={replaced:?}");
    assert_eq!(green.rect().origin(), overlay);
}

#[test]
fn block_after_baseline_inline_replaced_content_starts_after_the_line_descender() {
    let document = layout_html("<html><body style='margin:0'><div id='parent'>\n<img id='image' src='pixel.png' style='width:200px;height:50px'>\n<div id='after' style='width:200px;height:50px'></div></div></body></html>", 800.0);
    let (parent, _) = id_geometry(&document, "parent");
    let (image, image_size) = id_geometry(&document, "image");
    let (after, _) = id_geometry(&document, "after");
    let fragment = document.render_view().fragments().images().iter().next().expect("inline image fragment");
    let line = document.render_view().text().line(fragment.line_idx()).expect("image line");

    assert_eq!(image_size.height, 50.0);
    assert_eq!(image.y, parent.y, "a baseline-aligned replaced box begins at the line top");
    assert!(line.height() > image_size.height, "the line must retain the parent font strut's descender: line={:?}, image={image_size:?}", line.height());
    assert!((after.y - parent.y - line.height()).abs() < 0.001, "the following block must begin after the complete anonymous line box: parent={parent:?}, image={image:?}, after={after:?}, line_height={:?}", line.height());
}

#[test]
fn block_after_baseline_inline_svg_starts_after_the_line_descender() {
    let html = "<html xmlns='http://www.w3.org/1999/xhtml' xmlns:svg='http://www.w3.org/2000/svg'><body style='margin:0'><div id='parent'>\n<svg:svg id='image' version='1.1' height='50'><svg:rect width='200' height='100' fill='blue'/></svg:svg>\n<div id='after'></div></div></body></html>";
    let css = "#parent { width:288px; height:288px } svg { margin-left:auto; margin-right:auto } #after { width:200px; height:50px }";
    let parsed = html_parse::parse_xml_document(html).expect("valid XHTML test document");
    let styled = html_style::style_document(parsed.build_dom(), &[css]);
    let (dom, styles) = styled.into_parts();
    let prepared = crate::PreparedDocument::try_new(dom, styles).expect("style resolver must produce complete styles");
    let image_box = (0..prepared.box_count()).find(|&idx| prepared.get_id(idx) == Some("image")).expect("SVG layout box");
    let image_style = prepared.style_view(prepared.box_style_indices(image_box).expect("SVG style"));
    assert_eq!(image_style.display(), html_style_model::Display::Inline, "the UA stylesheet must keep embedded SVG in inline flow by default");
    assert!(matches!(image_style.vertical_align(), html_style_model::VerticalAlignValue::Baseline), "inline SVG should have the initial baseline alignment, got {:?}", image_style.vertical_align());
    let mut glyph_cache = GlyphCache::new();
    let shaped = prepared.shape(&mut glyph_cache).expect("test shaper must register every glyph");
    let mut image_metrics = crate::ImageMetrics::default();
    image_metrics.set(0, 300, 50);
    let document = shaped.layout_with_metrics(crate::LayoutConstraints::new(800.0, 16.0).unwrap(), &image_metrics);
    let (parent, _) = id_geometry(&document, "parent");
    let (image, image_size) = id_geometry(&document, "image");
    let (after, _) = id_geometry(&document, "after");
    let fragment = document.render_view().fragments().images().iter().next().expect("inline SVG fragment");
    let line = document.render_view().text().line(fragment.line_idx()).expect("SVG line");

    assert_eq!(image_size.height, 50.0);
    assert_eq!(image.y, parent.y);
    assert!(line.height() > image_size.height, "the line must retain the parent font strut's descender: line={:?}, image={image_size:?}", line.height());
    assert!((after.y - parent.y - line.height()).abs() < 0.001, "the following block must begin after the complete anonymous line box: parent={parent:?}, image={image:?}, after={after:?}, line_height={:?}", line.height());
}

#[test]
fn block_replaced_percentage_width_with_auto_margins_is_centered() {
    let document = layout_html("<html><body style='margin:0'><div id='parent' style='width:200px'><img id='image' src='pixel.png' style='display:block;width:50%;height:10px;margin-left:auto;margin-right:auto'></div></body></html>", 800.0);
    let (parent, _) = id_geometry(&document, "parent");
    let (image, image_size) = id_geometry(&document, "image");

    assert_eq!(image_size.width, 100.0);
    assert_eq!(image.x - parent.x, 50.0, "paired auto margins split the space remaining after the replaced element's used width");
}

#[test]
fn auto_height_block_formatting_context_contains_nested_float_margin_box() {
    let document = layout_html("<html><body style='margin:0'><div id='context' style='overflow:hidden;width:96px'><div><div id='float' style='float:left;width:96px;height:48px;margin-bottom:48px'></div></div></div></body></html>", 800.0);
    let (_, context_size) = id_geometry(&document, "context");
    let (_, float_size) = id_geometry(&document, "float");

    assert_eq!(float_size.height, 48.0);
    assert_eq!(context_size.height, 96.0, "the BFC's auto height must reach the bottom margin edge of its nested float");
}

#[test]
fn flow_root_span_contains_its_child_float() {
    let document =
        layout_html("<html><body style='margin:0'><div id='outer' style='border:1px solid'><span id='context' style='display:flow-root'><div id='float' style='float:left;width:20px;height:40px'></div></span></div></body></html>", 800.0);
    let (_, outer_size) = id_geometry(&document, "outer");
    let (_, context_size) = id_geometry(&document, "context");
    let (_, float_size) = id_geometry(&document, "float");

    assert_eq!(float_size.height, 40.0);
    assert_eq!(context_size.height, 40.0);
    assert_eq!(outer_size.height, 42.0);
}

#[test]
fn flow_root_list_item_keeps_child_margin_inside_after_a_float() {
    let document = layout_html(
        "<html><body style='margin:0;padding-left:100px'><div id='prior' style='border:1px solid;margin-bottom:20px'><div id='float' style='float:left;width:20px;height:40px'></div><span style='display:flow-root;border:1px solid'>x</span></div><span><span id='context' style='display:flow-root list-item;background:gray'><div id='child' style='margin:20px'>x</div></span></span></body></html>",
        800.0,
    );
    let (prior, prior_size) = id_geometry(&document, "prior");
    let (context, context_size) = id_geometry(&document, "context");
    let (child, _) = id_geometry(&document, "child");

    assert_eq!(context.y, prior.y + prior_size.height + 20.0, "the child's contained margin must not move the flow-root list-item itself");
    assert_eq!(child.y - context.y, 20.0);
    let (_, child_size) = id_geometry(&document, "child");
    assert_eq!(context_size.height, child_size.height + 40.0);
}

#[test]
fn flow_root_list_item_matches_equivalent_reference_geometry_after_float() {
    let actual = layout_html(
        "<html><body style='color:black;background:white;font:16px/1 monospace;padding:0 0 0 100px;margin:0'><div style='border:1px solid;margin-bottom:20px'><div style='float:left;width:20px;height:40px'></div><span style='display:flow-root list-item;border:1px solid'>x</span></div><span><span id='subject' style='display:flow-root list-item;background:gray'><div style='margin:20px'>x</div></span></span></body></html>",
        800.0,
    );
    let reference = layout_html(
        "<html><body style='color:black;background:white;font:16px/1 monospace;padding:0 0 0 100px;margin:0'><div id='reference-prior' style='border:1px solid'><div style='float:left;width:20px;height:40px'></div><div style='display:list-item;border:1px solid;margin-left:20px'>x</div></div><span><div id='wrapper' style='display:flow-root;margin-top:20px'><span id='subject' style='display:list-item;background:gray'><div style='padding:20px'>x</div></span></div></span></body></html>",
        800.0,
    );

    let (reference_prior, reference_prior_size) = id_geometry(&reference, "reference-prior");
    let (wrapper, _) = id_geometry(&reference, "wrapper");
    assert_eq!(wrapper.y, reference_prior.y + reference_prior_size.height + 20.0);
    assert_eq!(id_geometry(&actual, "subject"), id_geometry(&reference, "subject"));
    let marker_point = |document: &LaidOutDocument| {
        let view = document.render_view();
        let item = (0..view.boxes().len()).find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some("subject")).expect("list item");
        let marker = view.boxes().list_marker(item).expect("outside marker").marker_box();
        view.boxes().point(marker).expect("marker geometry")
    };
    assert_eq!(marker_point(&actual), marker_point(&reference));
}

#[test]
fn block_formatting_context_avoids_an_adjacent_float_margin_box() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='parent' style='width:200px'><div id='float' style='float:left;width:50px;height:50px'></div><div id='context' style='overflow:hidden;width:50px;height:50px'><div style='width:96px;height:96px'></div></div></div></body></html>",
        800.0,
    );
    let (float, float_size) = id_geometry(&document, "float");
    let (context, context_size) = id_geometry(&document, "context");

    assert_eq!(context_size, Size::new(50.0, 50.0));
    assert_eq!(context.x, float.x + float_size.width, "the BFC border box must start after the float's margin edge");
    assert_eq!(context.y, float.y, "a BFC that fits beside the float stays on the same row");
}

#[test]
fn zero_width_flow_root_descends_below_crossed_float_edges() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='outer' style='width:400px'><div style='float:right;width:250px;height:100px'></div><div style='float:left;width:250px;height:100px'></div><div id='context' style='display:flow-root;width:0;height:200px'></div></div></body></html>",
        800.0,
    );
    let (context, context_size) = id_geometry(&document, "context");

    assert_eq!(context, kurbo::Point::new(250.0, 100.0));
    assert_eq!(context_size, Size::new(0.0, 200.0));
}

#[test]
fn auto_width_block_formatting_context_fills_the_band_beside_a_float() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='parent' style='width:100px'><div id='float' style='float:left;width:50px;height:100px'></div><div id='context' style='overflow:hidden;height:100px;min-width:25px'></div></div></body></html>",
        800.0,
    );
    let (float, float_size) = id_geometry(&document, "float");
    let (context, context_size) = id_geometry(&document, "context");

    assert_eq!(context, float + Vec2::new(float_size.width, 0.0));
    assert_eq!(context_size, Size::new(50.0, 100.0));
}

#[test]
fn overflowing_content_does_not_push_an_auto_width_bfc_below_a_float() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='parent' style='width:300px'><div id='float' style='float:left;width:100px;height:100px'></div><div id='context' style='overflow:hidden'><span id='content' style='display:inline-block;width:250px;height:50px'></span></div></div></body></html>",
        800.0,
    );
    let (float, float_size) = id_geometry(&document, "float");
    let (context, context_size) = id_geometry(&document, "context");
    let (content, content_size) = id_geometry(&document, "content");

    assert_eq!(context, float + Vec2::new(float_size.width, 0.0));
    assert_eq!(context_size.width, 200.0);
    assert!(context_size.height >= content_size.height, "the line box must contain the overflowing atomic inline");
    assert_eq!(content, context);
    assert_eq!(content_size, Size::new(250.0, 50.0));
}

#[test]
fn table_bfc_uses_the_first_full_height_float_band_that_fits() {
    let mut floats = String::new();
    for width in (5..=150).rev().step_by(5) {
        floats.push_str(&format!("<div style='float:left;clear:left;width:{width}px;height:1px'></div>"));
    }
    let html = format!(
        "<html><body style='margin:0'><div id='parent' style='width:300px'>{floats}<table id='context' style='border-spacing:0'><caption style='height:30px;width:100px'>Caption</caption><tbody><tr><td style='padding:0'><div style='width:230px;height:30px'></div></td></tr></tbody></table></div></body></html>"
    );
    let document = layout_html(&html, 800.0);
    let (context, context_size) = id_geometry(&document, "context");

    assert_eq!((context, context_size), (kurbo::Point::new(70.0, 16.0), Size::new(230.0, 60.0)));
}

#[test]
fn trailing_margin_at_the_container_edge_does_not_push_a_fitting_bfc_below_a_float() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='parent' style='width:100px'><div id='float' style='float:left;width:50px;height:100px'></div><div id='context' style='overflow:hidden;width:50px;height:100px;margin-right:1px'></div></div></body></html>",
        800.0,
    );
    let (float, float_size) = id_geometry(&document, "float");
    let (context, context_size) = id_geometry(&document, "context");

    assert_eq!(context, float + Vec2::new(float_size.width, 0.0));
    assert_eq!(context_size, Size::new(50.0, 100.0));
}

#[test]
fn bfc_that_cannot_fit_beside_an_adjoining_float_separates_its_top_margin() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='outer' style='display:flow-root;width:100px;position:relative'><div id='wrapper'><div><div id='float' style='float:right;width:50px;height:30px'></div></div><div><div id='context' style='overflow:hidden;width:100px;height:70px;margin-top:100px'></div></div></div></div></body></html>",
        800.0,
    );
    let (outer, _) = id_geometry(&document, "outer");
    let (wrapper, _) = id_geometry(&document, "wrapper");
    let (float, float_size) = id_geometry(&document, "float");
    let (context, context_size) = id_geometry(&document, "context");

    assert_eq!(wrapper, outer);
    assert_eq!(float, outer + Vec2::new(50.0, 0.0));
    assert_eq!(float_size, Size::new(50.0, 30.0));
    assert_eq!(context, outer + Vec2::new(0.0, 30.0));
    assert_eq!(context_size, Size::new(100.0, 70.0));
}

#[test]
fn absolutely_positioned_root_uses_the_initial_containing_block() {
    let document = layout_html("<html id='root' style='position:absolute;left:50px;top:40px;width:50%;border:10px solid'><body style='margin:0'></body></html>", 400.0);
    let (root, root_size) = id_geometry(&document, "root");

    assert_eq!(root, kurbo::Point::new(50.0, 40.0));
    assert_eq!(root_size.width, 220.0);
}

#[test]
fn negative_margin_expands_an_auto_width_bfc_border_box_without_expanding_its_float_band() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='parent' style='width:100px;direction:rtl'><div id='float' style='float:left;width:50px;height:100px'></div><div id='context' style='overflow:hidden;height:100px;margin-left:-20px'></div></div></body></html>",
        800.0,
    );
    let (float, _) = id_geometry(&document, "float");
    let (context, context_size) = id_geometry(&document, "context");

    assert_eq!(context, float + Vec2::new(30.0, 0.0));
    assert_eq!(context_size, Size::new(70.0, 100.0));
}

#[test]
fn absolute_child_uses_positioned_ancestor_padding_box_and_leaves_flow() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='parent' style='position:relative;width:200px;padding:10px;border:5px solid;height:auto'><div id='absolute' style='position:absolute;left:30px;top:40px;width:20px;height:50px;background:green'></div><div id='flow' style='width:10px;height:12px'></div></div></body></html>",
        400.0,
    );
    let (parent_point, parent_size) = id_geometry(&document, "parent");
    let (absolute_point, absolute_size) = id_geometry(&document, "absolute");
    let (flow_point, _) = id_geometry(&document, "flow");

    assert_eq!(absolute_point, parent_point + Vec2::new(5.0 + 30.0, 5.0 + 40.0));
    assert_eq!(absolute_size, Size::new(20.0, 50.0));
    assert_eq!(flow_point, parent_point + Vec2::new(15.0, 15.0));
    assert_eq!(parent_size.height, 42.0, "the 50px absolute child must not increase normal-flow height");
}

#[test]
fn blockified_absolute_inline_uses_authored_geometry_and_positioned_paint_layer() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='parent' style='position:relative;width:300px'><p style='width:200%'><img style='width:50%;height:100px'/><span id='absolute' style='position:absolute;top:0;left:0;background:green;width:300px;height:100px'></span></p></div></body></html>",
        800.0,
    );
    let (parent, _) = id_geometry(&document, "parent");
    let (absolute, size) = id_geometry(&document, "absolute");
    let decorations = document.render_view().fragments().decorations().iter().collect::<Vec<_>>();
    let green = decorations.iter().find(|fragment| fragment.color() == 0x008000FF).unwrap_or_else(|| panic!("absolute background; colors={:?}", decorations.iter().map(|fragment| fragment.color()).collect::<Vec<_>>()));

    assert_eq!(absolute, parent);
    assert_eq!(size, Size::new(300.0, 100.0));
    assert_eq!(green.rect(), kurbo::Rect::from_origin_size(parent, size));
    assert!(green.is_in_positioned_layer());
    assert!(!green.is_in_negative_positioned_layer());
    assert!(green.line_idx().is_none(), "blockified absolute paint must be replayed by the global positioned pass");
}

#[test]
fn blockified_absolute_inline_uses_its_later_positioned_sibling_as_containing_block() {
    let document = layout_html(
        "<html><body style='margin:0'><div style='width:300px;height:100px'></div><div id='parent' style='position:relative;width:300px'><p style='margin:0;width:200%'><img style='width:50%;height:100px'/><span id='absolute' style='position:absolute;top:0;left:0;background:green;width:300px;height:100px'></span></p></div></body></html>",
        800.0,
    );
    let (parent, _) = id_geometry(&document, "parent");
    let (absolute, size) = id_geometry(&document, "absolute");

    assert_eq!(parent, kurbo::Point::new(0.0, 100.0));
    assert_eq!(absolute, parent, "the absolute child must use the translated padding edge of its own positioned ancestor");
    assert_eq!(size, Size::new(300.0, 100.0));
}

#[test]
fn absolute_sibling_with_top_zero_overlays_the_containing_block_start() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='parent' style='position:relative;font-size:32px'><div id='flow' style='position:relative;padding:1px'><span style='display:table-cell'>a b</span><span style='display:table-cell'>c d</span></div><div id='absolute' style='position:absolute;top:0;padding:1px'>a bc d</div></div></body></html>",
        800.0,
    );
    let (parent, _) = id_geometry(&document, "parent");
    let (flow, _) = id_geometry(&document, "flow");
    let (absolute, _) = id_geometry(&document, "absolute");

    assert_eq!(flow, parent);
    assert_eq!(absolute, parent, "top:0 must use the positioned ancestor's padding edge, not the later static position");
    let line_points = document.render_view().text().lines().iter().map(|line| line.point()).collect::<Vec<_>>();
    let min_y = line_points.iter().map(|point| point.y).fold(f64::INFINITY, f64::min);
    let max_y = line_points.iter().map(|point| point.y).fold(f64::NEG_INFINITY, f64::max);
    assert!((max_y - min_y).abs() < 0.01, "the absolute text output must translate with its box: {line_points:?}");
}

#[test]
fn absolute_nested_table_text_translates_with_top_zero_container() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='parent' style='position:relative;font-size:32px'><div style='position:relative'>same text</div><div id='absolute' style='position:absolute;top:0'><table id='table' style='border-spacing:0'><tr><td style='padding:0'>same text</td></tr></table></div></div></body></html>",
        800.0,
    );
    let (parent, _) = id_geometry(&document, "parent");
    let (absolute, _) = id_geometry(&document, "absolute");
    let (table, _) = id_geometry(&document, "table");
    assert_eq!(absolute, parent);
    assert_eq!(table, parent);

    let line_points = document.render_view().text().lines().iter().map(|line| line.point()).collect::<Vec<_>>();
    let min_y = line_points.iter().map(|point| point.y).fold(f64::INFINITY, f64::min);
    let max_y = line_points.iter().map(|point| point.y).fold(f64::NEG_INFINITY, f64::max);
    assert!((max_y - min_y).abs() < 0.01, "nested table lines must receive the absolute subtree translation: {line_points:?}");
}

#[test]
fn z_index_orders_positioned_text_and_backgrounds_independently_of_dom_order() {
    let document = layout_html(
        "<html><body style='margin:0'><div style='position:relative'><div style='position:absolute;z-index:2;top:0;left:0;width:10px;height:10px;background:red'>H</div><div style='position:absolute;z-index:1;top:0;left:0;width:10px;height:10px;background:green'>L</div></div></body></html>",
        100.0,
    );
    let text = document.render_view().text();
    let lines = text.lines().iter().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "positioned fixture lines: {lines:?}");
    let painted_starts = text.paint_order_indices().iter().map(|&index| lines[index as usize].start()).collect::<Vec<_>>();
    assert!(painted_starts[0] > painted_starts[1], "the later low-z line must paint before the earlier high-z line");

    let colors = document.render_view().fragments().decorations().iter().map(|fragment| fragment.color()).filter(|color| matches!(color, 0x008000FF | 0xFF0000FF)).collect::<Vec<_>>();
    assert_eq!(colors, vec![0x008000FF, 0xFF0000FF], "positioned decoration traversal: {colors:?}");
}

#[test]
fn later_relative_border_paints_above_earlier_absolute_background_at_auto_z() {
    let document = layout_html(
        "<html><body style='margin:0'><div style='position:relative'><div style='position:absolute;inset:0;width:20px;height:20px;background:red'></div><div style='position:relative;width:0;height:0;border:10px solid blue'></div></div></body></html>",
        100.0,
    );
    let colors = document.render_view().fragments().decorations().iter().map(|fragment| fragment.color()).filter(|color| matches!(color, 0x0000ffff | 0xff0000ff)).collect::<Vec<_>>();
    assert_eq!(colors.last(), Some(&0x0000ffff), "the later relative border must be the final overlapping positioned decoration: {colors:?}");
}

#[test]
fn inline_block_background_paints_above_a_later_overlapping_block_background() {
    let document = layout_html(
        "<html><body style='margin:0'><div style='width:20px;height:10px'><span style='display:inline-block;width:20px;height:10px;background:green'></span></div><div style='width:20px;height:10px;margin-top:-10px;background:red'></div></body></html>",
        100.0,
    );
    let colors = document.render_view().fragments().decorations().iter().map(|fragment| fragment.color()).filter(|color| matches!(color, 0x008000FF | 0xFF0000FF)).collect::<Vec<_>>();

    assert_eq!(colors, vec![0xFF0000FF, 0x008000FF], "an atomic inline paints in the inline layer, after ordinary in-flow block backgrounds");
}

#[test]
fn descendant_background_stays_in_its_inline_block_atomic_paint_group() {
    let document = layout_html(
        "<html><body style='margin:0'><div style='width:20px;height:10px'><span style='display:inline-block;width:20px;height:10px'><span style='display:block;width:20px;height:10px;background:green'></span></span></div><div style='width:20px;height:10px;margin-top:-10px;background:red'></div></body></html>",
        100.0,
    );
    let colors = document.render_view().fragments().decorations().iter().map(|fragment| fragment.color()).filter(|color| matches!(color, 0x008000FF | 0xFF0000FF)).collect::<Vec<_>>();

    assert_eq!(colors, vec![0xFF0000FF, 0x008000FF], "descendant backgrounds must not escape the atomic inline paint group");
}

#[test]
fn later_inline_text_paints_after_earlier_inline_block_text() {
    let document = layout_html("<html><body style='margin:0'><div style='height:10px'><span style='display:inline-block'>R</span></div><div style='height:10px;margin-top:-10px'><span>G</span></div></body></html>", 100.0);
    let text = document.render_view().text();
    let glyph_for = |target| (0..text.glyph_count() as u32).find(|index| text.glyph_at(*index as usize).and_then(|glyph| text.glyph_metric(glyph)).is_some_and(|metric| metric.ch() == target)).expect("fixture glyph must exist");
    let earlier_atomic_line = text.line_index_for_glyph(glyph_for('R')).expect("inline-block text must own a line");
    let later_inline_line = text.line_index_for_glyph(glyph_for('G')).expect("later inline text must own a line");

    let paint_order = text.paint_order_indices();
    let earlier_position = paint_order.iter().position(|&line| line as usize == earlier_atomic_line).expect("inline-block line must occur in paint traversal");
    let later_position = paint_order.iter().position(|&line| line as usize == later_inline_line).expect("later inline line must occur in paint traversal");
    assert!(earlier_position < later_position, "later inline content must replay last");
}

#[test]
fn negative_z_child_of_auto_positioned_parent_paints_behind_parent_background() {
    let document = layout_html(
        "<html><body style='margin:0'><div style='position:absolute;z-index:auto;width:20px;height:20px;background:green'><div style='position:absolute;z-index:-1;width:20px;height:20px;background:red'></div></div></body></html>",
        100.0,
    );
    let colors = document.render_view().fragments().decorations().iter().map(|fragment| fragment.color()).filter(|color| matches!(color, 0x008000FF | 0xFF0000FF)).collect::<Vec<_>>();
    assert_eq!(colors, vec![0xFF0000FF, 0x008000FF]);
}

#[test]
fn negative_absolute_multiline_text_is_marked_for_the_negative_paint_pass() {
    let document = layout_html(
        "<html><body style='margin:0'><div style='position:relative'><div style='height:60px;background:lime;white-space:pre'>\n\nG</div><div style='position:absolute;z-index:-1;inset:0'>R<br>R<br>R</div></div></body></html>",
        200.0,
    );
    let lines = document.render_view().text().lines().iter().collect::<Vec<_>>();
    let negative = lines.iter().filter(|line| line.is_in_negative_positioned_layer()).count();

    assert_eq!(negative, 3, "all absolute descendant lines must stay in the negative stacking layer: {lines:?}");
}

#[test]
fn floated_pre_block_border_contains_two_exact_line_heights() {
    let document = layout_html("<html><body style='margin:0'><div id='target' style='float:left;border:medium solid blue;font-size:20px;line-height:20px;white-space:pre'>x\nx</div></body></html>", 200.0);
    let (_, size) = id_geometry(&document, "target");

    assert_eq!(size.height, 46.0, "two 20px lines plus two 3px medium borders");
}

#[test]
fn float_backgrounds_paint_after_ordinary_in_flow_block_backgrounds() {
    let document = layout_html("<html><body style='margin:0'><div><div style='float:left;width:20px;height:20px;background:green'></div><div style='width:20px;height:20px;background:red'></div></div></body></html>", 100.0);
    let colors = document.render_view().fragments().decorations().iter().map(|fragment| fragment.color()).filter(|color| matches!(color, 0x008000FF | 0xFF0000FF)).collect::<Vec<_>>();
    assert_eq!(colors, vec![0xFF0000FF, 0x008000FF]);
}

#[test]
fn opposing_horizontal_insets_stretch_absolute_auto_width() {
    let document = layout_html("<html><body style='margin:0'><div id='parent' style='position:relative;width:200px;height:100px'><div id='child' style='position:absolute;left:20px;right:30px;height:10px'></div></div></body></html>", 400.0);
    let (parent_point, _) = id_geometry(&document, "parent");
    let (child_point, child_size) = id_geometry(&document, "child");

    assert_eq!(child_point.x, parent_point.x + 20.0);
    assert_eq!(child_size.width, 150.0);
}

#[test]
fn opposing_vertical_insets_stretch_absolute_auto_height() {
    let document = layout_html("<html><body style='margin:0'><div id='parent' style='position:relative;width:100px;height:200px'><div id='child' style='position:absolute;top:20px;bottom:30px;width:10px'></div></div></body></html>", 400.0);
    let (parent_point, _) = id_geometry(&document, "parent");
    let (child_point, child_size) = id_geometry(&document, "child");

    assert_eq!(child_point.y, parent_point.y + 20.0);
    assert_eq!(child_size.height, 150.0);
}

#[test]
fn blockified_absolute_row_group_keeps_its_anonymous_table_descendants() {
    let document = layout_html(
        "<html><body style='margin:0'><div style='display:table'><div id='group' style='display:table-row-group;position:absolute;left:0'><div style='display:table-row'><div id='cell' style='display:table-cell;width:20px;height:20px;background:green'></div></div></div></div></body></html>",
        200.0,
    );
    let (group_point, group_size) = id_geometry(&document, "group");
    let (cell_point, cell_size) = id_geometry(&document, "cell");

    assert_eq!(group_point.x, 0.0);
    assert_eq!(group_size, Size::new(20.0, 20.0));
    assert_eq!(cell_point, group_point);
    assert_eq!(cell_size, Size::new(20.0, 20.0));
}

#[test]
fn blockified_absolute_table_cell_is_retained_as_row_out_of_flow_content() {
    let document = layout_html(
        "<html><body style='margin:0'><div style='display:table'><div style='display:table-row'><div id='cell' style='display:table-cell;position:absolute;left:0;width:20px;height:20px;background:green'></div></div></div></body></html>",
        200.0,
    );
    let (cell_point, cell_size) = id_geometry(&document, "cell");

    assert_eq!(cell_point.x, 0.0);
    assert_eq!(cell_size, Size::new(20.0, 20.0));
    assert!(document.render_view().fragments().decorations().iter().any(|decoration| decoration.color() == 0x008000ff && decoration.rect().origin() == cell_point));
}

#[test]
fn rtl_absolute_static_position_keeps_its_line_inside_the_shrink_to_fit_box() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='parent' style='position:relative;direction:rtl;width:200px;height:200px'><div id='child' style='position:absolute;font:100px/1 serif;background:red;color:blue'>X</div></div></body></html>",
        400.0,
    );
    let (parent_point, _) = id_geometry(&document, "parent");
    let (child_point, child_size) = id_geometry(&document, "child");
    let lines = document.render_view().text().lines().iter().collect::<Vec<_>>();
    let line = lines.iter().find(|line| line.height() >= 99.0).expect("absolute child line");

    assert_eq!(child_point.x + child_size.width, parent_point.x + 200.0);
    assert!(line.is_in_positioned_layer(), "line layers: {:?}", lines.iter().map(|line| (line.point(), line.height(), line.is_in_positioned_layer())).collect::<Vec<_>>());
    assert!(line.point().x >= child_point.x && line.point().x < child_point.x + child_size.width, "line must start inside the absolute border box: line={:?}, box={child_point:?} {child_size:?}", line.point());
}

#[test]
fn inline_block_percentage_child_uses_max_height_constrained_basis() {
    let document = layout_html(
        "<html><body style='margin:0'><div id='parent' style='display:inline-block;width:100px;height:200px;max-height:100px;background:red'><span id='child' style='display:inline-block;width:100px;height:100%;background:green'></span></div></body></html>",
        300.0,
    );
    let (parent_point, parent_size) = id_geometry(&document, "parent");
    let (child_point, child_size) = id_geometry(&document, "child");
    let decorations = document.render_view().fragments().decorations().iter().map(|fragment| (fragment.color(), fragment.rect())).collect::<Vec<_>>();

    assert_eq!(parent_size, Size::new(100.0, 100.0));
    assert_eq!(child_size, Size::new(100.0, 100.0));
    assert_eq!(child_point, parent_point);
    assert!(decorations.iter().any(|(color, rect)| *color == 0x008000ff && *rect == kurbo::Rect::from_origin_size(child_point, child_size)), "green child background must cover the constrained parent: {decorations:?}");
}

#[test]
fn block_percentage_child_uses_inline_block_content_height() {
    let mut factory = DocumentFactory::new();
    let mut glyph_cache = GlyphCache::new();
    let document = factory
        .parse_with_new_pipeline(
            "<html><body><p>Test passes if there is a filled green square and no red.</p><div id='parent'><div id='child'></div></div></body></html>",
            Some("#parent{display:inline-block;width:100px;height:100px;background:red} div div{width:100%;height:100%;background:green}"),
        )
        .shape(&mut glyph_cache)
        .expect("test shaper must register every glyph")
        .layout(crate::LayoutConstraints::new(300.0, 16.0).unwrap());
    let (parent_point, parent_size) = id_geometry(&document, "parent");
    let (child_point, child_size) = id_geometry(&document, "child");

    assert_eq!(parent_size, Size::new(100.0, 100.0));
    assert_eq!(child_point, parent_point);
    assert_eq!(child_size, parent_size);
    let decorations = document.render_view().fragments().decorations();
    let parent_rect = decorations.iter().find(|decoration| decoration.color() == 0xff0000ff).expect("red parent background").rect();
    let child_rect = decorations.iter().find(|decoration| decoration.color() == 0x008000ff).expect("green child background").rect();
    assert_eq!(child_rect, parent_rect);
    let view = document.render_view();
    let line_idx = (0..view.text().lines().len()).find(|&line_idx| view.fragments().decorations_for_line(line_idx).iter().any(|decoration| decoration.color() == 0xff0000ff)).expect("inline-block line");
    let line_colors = document.render_view().fragments().decorations_for_line(line_idx).iter().map(|decoration| decoration.color()).collect::<Vec<_>>();
    let parent_background = line_colors.iter().position(|color| *color == 0xff0000ff).expect("red parent background");
    let child_background = line_colors.iter().position(|color| *color == 0x008000ff).expect("green child background");
    assert!(parent_background < child_background, "inline-block parent background must paint below its block child: {line_colors:?}");
}
