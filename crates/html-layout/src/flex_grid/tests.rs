#[cfg(test)]
mod tests {
    // Integration coverage for the shared flex/grid implementation.
    use crate::parser::DocumentFactory;
    use crate::test_support::TestGlyphShaper;
    use crate::{LaidOutDocument, LayoutConstraints};
    use kurbo::{Point, Size};
    use taffy::geometry::Size as TaffySize;
    use taffy::prelude::{AvailableSpace, Dimension, Style, TaffyTree};
    use taffy::style::{Display as TaffyDisplay, FlexDirection as TaffyFlexDirection};

    fn layout_html(html: &str, width: f64) -> LaidOutDocument {
        let mut factory = DocumentFactory::new();
        let mut shaper = TestGlyphShaper::new();
        factory.parse_with_new_pipeline(html, None).shape(&mut shaper).expect("test shaper registers every glyph").layout(LayoutConstraints::new(width, 16.0).unwrap())
    }

    fn layout_html_with_css(html: &str, css: &str, width: f64) -> LaidOutDocument {
        let mut factory = DocumentFactory::new();
        let mut shaper = TestGlyphShaper::new();
        factory.parse_with_new_pipeline(html, Some(css)).shape(&mut shaper).expect("test shaper registers every glyph").layout(LayoutConstraints::new(width, 16.0).unwrap())
    }

    fn box_index(document: &LaidOutDocument, id: &str) -> usize {
        let view = document.render_view();
        (0..view.boxes().len()).find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some(id)).unwrap_or_else(|| panic!("missing box #{id}"))
    }

    fn geometry(document: &LaidOutDocument, id: &str) -> (Point, Size) {
        let idx = box_index(document, id);
        let boxes = document.render_view().boxes();
        (boxes.point(idx).expect("box index in range"), boxes.size(idx).expect("box index in range"))
    }

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 0.05, "expected {expected}, got {actual}");
    }

    /// Runs Taffy's flex algorithm without any renderer-owned DOM, box tree,
    /// style conversion, or measurement code. Each leaf context contains its
    /// fixed `(min-content, max-content)` measurement.
    fn raw_taffy_min_content_width(items: &[(Style, (f32, f32))]) -> f32 {
        let mut taffy = TaffyTree::<(f32, f32)>::with_capacity(items.len() + 1);
        taffy.disable_rounding();
        let children = items.iter().map(|(style, intrinsic)| taffy.new_leaf_with_context(style.clone(), *intrinsic).expect("raw Taffy leaf")).collect::<Vec<_>>();
        let root_style = Style { display: TaffyDisplay::Flex, flex_direction: TaffyFlexDirection::Row, size: TaffySize { width: Dimension::auto(), height: Dimension::auto() }, ..Style::default() };
        let root = taffy.new_with_children(root_style, &children).expect("raw Taffy root");
        taffy
            .compute_layout_with_measure(root, TaffySize { width: AvailableSpace::MinContent, height: AvailableSpace::MaxContent }, |known, available, _, context, _| {
                let Some((min_content, max_content)) = context.copied() else { return TaffySize::ZERO };
                let width = known.width.unwrap_or(match available.width {
                    AvailableSpace::Definite(width) => width,
                    AvailableSpace::MinContent => min_content,
                    AvailableSpace::MaxContent => max_content,
                });
                TaffySize { width, height: known.height.unwrap_or(0.0) }
            })
            .expect("raw Taffy intrinsic layout");
        taffy.layout(root).expect("raw Taffy root layout").size.width
    }

    fn raw_taffy_flex_item(width: f32, min_width: Option<f32>, flex_basis: f32, flex_grow: f32, flex_shrink: f32) -> Style {
        Style {
            size: TaffySize { width: Dimension::length(width), height: Dimension::auto() },
            min_size: TaffySize { width: min_width.map(Dimension::length).unwrap_or_else(Dimension::auto), height: Dimension::auto() },
            flex_basis: Dimension::length(flex_basis),
            flex_grow,
            flex_shrink,
            ..Style::default()
        }
    }

    #[test]
    fn flex_row_uses_gap_and_explicit_item_sizes() {
        let document = layout_html("<html><body><div id='c' style='display:flex;width:300px'><div id='a' style='width:100px;height:20px'></div><div id='b' style='width:50px;height:20px'></div></div></body></html>", 500.0);
        let (container, _) = geometry(&document, "c");
        let (a, a_size) = geometry(&document, "a");
        let (b, b_size) = geometry(&document, "b");

        close(a.x, container.x);
        close(b.x, container.x + 100.0);
        close(a_size.width, 100.0);
        close(b_size.width, 50.0);
    }

    #[test]
    fn flex_gap_and_order_are_applied_without_reordering_dom_boxes() {
        let document = layout_html(
            "<html><body><div id='c' style='display:flex;width:300px;column-gap:10px'><div id='a' style='order:2;width:40px;height:10px'></div><div id='b' style='order:-1;width:50px;height:10px'></div></div></body></html>",
            500.0,
        );
        let (container, _) = geometry(&document, "c");
        let (a, _) = geometry(&document, "a");
        let (b, _) = geometry(&document, "b");

        close(b.x, container.x);
        close(a.x, container.x + 60.0);
        assert!(box_index(&document, "a") < box_index(&document, "b"), "layout order must not mutate DOM/box-tree order");
    }

    #[test]
    fn absolute_flex_children_do_not_consume_main_axis_space() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:flex;width:800px'>
                    <div style='position:absolute;width:510px'></div>
                    <div id='capped' style='width:800px;max-width:150px;overflow:auto'></div>
                    <div id='growing' style='flex:.8 0 0'></div>
                </div>
                <div style='display:flex;width:800px'>
                    <div style='position:absolute;width:150px;max-width:150px'></div>
                    <div id='alone' style='flex:.8 0 0'></div>
                </div>
            </body></html>",
            900.0,
        );

        close(geometry(&document, "capped").1.width, 150.0);
        close(geometry(&document, "growing").1.width, 520.0);
        close(geometry(&document, "alone").1.width, 640.0);
    }

    #[test]
    fn flex_grow_distributes_free_space() {
        let document = layout_html("<html><body><div id='c' style='display:flex;width:300px'><div id='a' style='flex:1 1 0;height:10px'></div><div id='b' style='flex:2 1 0;height:10px'></div></div></body></html>", 500.0);
        let (_, a) = geometry(&document, "a");
        let (_, b) = geometry(&document, "b");
        close(a.width, 100.0);
        close(b.width, 200.0);
    }

    #[test]
    fn fractional_flex_factors_leave_unclaimed_free_space() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:flex;width:100px'>
                    <div id='grow-half' style='flex-grow:.5'></div>
                </div>
                <div style='display:flex;width:100px'>
                    <div id='grow-a' style='flex-grow:.5'></div>
                    <div id='grow-b' style='flex-grow:.25'></div>
                </div>
                <div style='display:flex;width:100px'>
                    <div id='shrink-half' style='width:200px;flex-shrink:.5'></div>
                </div>
                <div style='display:flex;width:100px'>
                    <div id='shrink-a' style='width:200px;flex-shrink:.5'></div>
                    <div id='shrink-b' style='width:200px;flex-shrink:.25'></div>
                </div>
                <div style='display:flex;flex-direction:column;height:100px'>
                    <div id='column-shrink-a' style='height:200px;flex-shrink:.5'></div>
                    <div id='column-shrink-b' style='height:200px;flex-shrink:.25'></div>
                </div>
            </body></html>",
            300.0,
        );

        close(geometry(&document, "grow-half").1.width, 50.0);
        close(geometry(&document, "grow-a").1.width, 50.0);
        close(geometry(&document, "grow-b").1.width, 25.0);
        close(geometry(&document, "shrink-half").1.width, 150.0);
        close(geometry(&document, "shrink-a").1.width, 50.0);
        close(geometry(&document, "shrink-b").1.width, 125.0);
        close(geometry(&document, "column-shrink-a").1.height, 50.0);
        close(geometry(&document, "column-shrink-b").1.height, 125.0);
    }

    #[test]
    fn auto_column_uses_a_definite_pixel_flex_basis_for_its_compatible_main_size() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:flex;flex-direction:column'><div id='half' style='font-size:14px;line-height:1;min-height:0;flex:.5 1 0px'>text</div></div>
                <div style='display:flex;flex-direction:column'><div id='one' style='font-size:14px;line-height:1;min-height:0;flex:1 1 0px'>text</div></div>
            </body></html>",
            300.0,
        );

        close(geometry(&document, "half").1.height, 0.0);
        close(geometry(&document, "one").1.height, 0.0);
    }

    #[test]
    fn intrinsic_block_constraints_use_the_flex_containers_natural_height() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div id='minimum' style='display:flex;height:0;min-height:max-content;border:5px solid'>
                    <div style='height:25px;border:5px solid'></div>
                </div>
                <div id='maximum' style='display:flex;height:200px;max-height:min-content;border:5px solid'>
                    <div style='height:25px;border:5px solid'></div>
                </div>
                <div id='wrapped-column' style='display:flex;flex-flow:column wrap;height:0;min-height:max-content'>
                    <div style='height:25px'></div><div style='height:25px'></div>
                </div>
            </body></html>",
            300.0,
        );

        close(geometry(&document, "minimum").1.height, 45.0);
        close(geometry(&document, "maximum").1.height, 45.0);
        close(geometry(&document, "wrapped-column").1.height, 25.0);
    }

    #[test]
    fn infinite_flex_grow_dominates_finite_sibling_without_non_finite_layout_input() {
        let document = layout_html(
            "<html><body style='margin:0'><div style='display:flex;width:100px;height:100px'><div id='infinite' style='flex:calc(infinity) 0 0px;height:100px'></div><div id='finite' style='flex:1 0 0px;height:100px'></div></div></body></html>",
            100.0,
        );
        let (_, infinite) = geometry(&document, "infinite");
        let (_, finite) = geometry(&document, "finite");

        close(infinite.width, 100.0);
        close(finite.width, 0.0);
    }

    #[test]
    fn flex_automatic_minimum_is_zero_only_on_the_scrollable_axis() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:flex;align-items:flex-start;width:0'>
                    <div id='row-x' style='overflow-x:hidden;overflow-y:clip'><div style='width:100px;height:100px'></div></div>
                    <div id='row-y' style='overflow-x:clip;overflow-y:hidden'><div style='width:100px;height:100px'></div></div>
                </div>
                <div style='display:flex;align-items:flex-start;flex-direction:column;height:0'>
                    <div id='column-y' style='overflow-x:clip;overflow-y:hidden'><div style='width:100px;height:100px'></div></div>
                    <div id='column-x' style='overflow-x:hidden;overflow-y:clip'><div style='width:100px;height:100px'></div></div>
                </div>
            </body></html>",
            300.0,
        );

        assert_eq!(geometry(&document, "row-x").1, Size::new(0.0, 100.0));
        assert_eq!(geometry(&document, "row-y").1, Size::new(100.0, 100.0));
        assert_eq!(geometry(&document, "column-y").1, Size::new(100.0, 0.0));
        assert_eq!(geometry(&document, "column-x").1, Size::new(100.0, 100.0));
    }

    // Reduced from WPT css/css-flexbox/flex-aspect-ratio-img-row-013.html.
    // A stretched cross size is a transferred size suggestion for a replaced
    // flex item's automatic minimum in the main axis.
    #[test]
    fn replaced_row_flex_auto_minimum_uses_stretched_cross_size() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:flex;width:0;height:120px'>
                    <canvas id='margin' width='200' height='200' style='margin-bottom:20px'></canvas>
                </div>
                <div style='display:flex;width:0;height:120px'>
                    <canvas id='zero' width='200' height='200' style='margin-bottom:160px'></canvas>
                </div>
                <div style='display:flex;width:0;height:120px'>
                    <canvas id='border' width='200' height='200' style='border-right:10px solid black;margin-bottom:20px'></canvas>
                </div>
                <div style='display:flex;width:0;height:50px'>
                    <canvas id='minimum' width='200' height='200' style='min-height:100px'></canvas>
                </div>
            </body></html>",
            300.0,
        );

        assert_eq!(geometry(&document, "margin").1, Size::new(100.0, 100.0));
        assert_eq!(geometry(&document, "zero").1, Size::new(0.0, 0.0));
        assert_eq!(geometry(&document, "border").1, Size::new(110.0, 100.0));
        assert_eq!(geometry(&document, "minimum").1, Size::new(100.0, 100.0));
    }

    // Reduced from WPT css/css-flexbox/flex-aspect-ratio-img-column-011.html.
    // Definite min/max constraints in the cross axis must clamp the content
    // size suggestion after conversion through the intrinsic aspect ratio.
    #[test]
    fn replaced_flex_auto_minimum_converts_cross_axis_constraints() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:flex;width:10px'>
                    <canvas id='row-max' width='100' height='100' style='max-height:5px'></canvas>
                </div>
                <div style='display:flex;width:10px'>
                    <canvas id='row-height' width='10' height='10' style='height:100px'></canvas>
                </div>
                <div style='display:flex;flex-direction:column;height:10px'>
                    <canvas id='column-max' width='100' height='100' style='max-width:5px'></canvas>
                </div>
                <div style='display:flex;flex-direction:column;width:50px;height:10px'>
                    <canvas id='column-stretch' width='10' height='10'></canvas>
                </div>
                <div style='display:flex;flex-direction:column;height:10px'>
                    <canvas id='column-width' width='10' height='10' style='width:100px'></canvas>
                </div>
            </body></html>",
            300.0,
        );

        assert_eq!(geometry(&document, "row-max").1, Size::new(5.0, 5.0));
        close(geometry(&document, "row-height").1.width, 100.0);
        assert_eq!(geometry(&document, "column-max").1, Size::new(5.0, 5.0));
        close(geometry(&document, "column-stretch").1.height, 50.0);
        close(geometry(&document, "column-width").1.height, 100.0);
    }

    // Reduced from WPT image-as-flexitem-size-005/006. Cross-axis constraints
    // affect the hypothetical size, but do not become hard caps on a flexible
    // replaced item's subsequently grown main size.
    #[test]
    fn flexible_replaced_item_can_grow_past_cross_axis_constraints() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:flex;align-items:flex-start;width:40px;height:40px'>
                    <canvas id='row-max' width='16' height='16' style='flex:1;min-width:0;min-height:0;max-height:10px'></canvas>
                </div>
                <div style='display:flex;align-items:flex-start;width:40px;height:40px'>
                    <canvas id='row-min-max' width='16' height='16' style='flex:1;min-width:30px;min-height:0;max-height:10px'></canvas>
                </div>
                <div style='display:flex;align-items:flex-start;flex-direction:column;width:40px;height:40px'>
                    <canvas id='column-max' width='16' height='16' style='flex:1;min-width:0;min-height:0;max-width:10px'></canvas>
                </div>
                <div style='display:flex;align-items:flex-start;flex-direction:column;width:40px;height:40px'>
                    <canvas id='column-min-max' width='16' height='16' style='flex:1;min-width:0;min-height:30px;max-width:10px'></canvas>
                </div>
            </body></html>",
            300.0,
        );

        assert_eq!(geometry(&document, "row-max").1, Size::new(40.0, 10.0));
        assert_eq!(geometry(&document, "row-min-max").1, Size::new(40.0, 10.0));
        assert_eq!(geometry(&document, "column-max").1, Size::new(10.0, 40.0));
        assert_eq!(geometry(&document, "column-min-max").1, Size::new(10.0, 40.0));
    }

    #[test]
    fn intrinsic_block_constraints_feed_every_replaced_formatting_context() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div id='block-min' style='width:max-content;border:5px solid'><canvas width='50' height='50' style='display:block;width:max-content;height:0;min-height:max-content'></canvas></div>
                <div id='block-max' style='width:max-content;border:5px solid'><canvas width='50' height='50' style='display:block;width:max-content;height:100px;max-height:max-content'></canvas></div>
                <div id='row-min' style='display:flex;flex-direction:row;width:max-content;border:5px solid'><canvas width='50' height='50' style='display:block;width:max-content;height:0;min-height:max-content'></canvas></div>
                <div id='row-max' style='display:flex;flex-direction:row;width:max-content;border:5px solid'><canvas width='50' height='50' style='display:block;width:max-content;height:100px;max-height:max-content'></canvas></div>
                <div id='column-min' style='display:flex;flex-direction:column;width:max-content;border:5px solid'><canvas width='50' height='50' style='display:block;width:max-content;height:0;min-height:max-content'></canvas></div>
                <div id='column-max' style='display:flex;flex-direction:column;width:max-content;border:5px solid'><canvas width='50' height='50' style='display:block;width:max-content;height:100px;max-height:max-content'></canvas></div>
                <div id='grid-min' style='display:grid;width:max-content;border:5px solid'><canvas width='50' height='50' style='display:block;width:max-content;height:0;min-height:max-content'></canvas></div>
                <div id='grid-max' style='display:grid;width:max-content;border:5px solid'><canvas width='50' height='50' style='display:block;width:max-content;height:100px;max-height:max-content'></canvas></div>
            </body></html>",
            300.0,
        );

        for id in ["block-min", "block-max", "row-min", "row-max", "column-min", "column-max", "grid-min", "grid-max"] {
            assert_eq!(geometry(&document, id).1, Size::new(60.0, 60.0), "unexpected geometry for #{id}");
        }
    }

    // Reduced from WPT css/css-flexbox/percentage-heights-001.html. A flex
    // item's stretched cross size is definite, so its descendant percentage
    // height resolves even when the flex container itself has an auto height.
    #[test]
    fn stretched_flex_item_resolves_descendant_percentage_height() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div id='flex' style='display:flex;width:50px'>
                    <div id='item' style='min-width:0;min-height:0'>
                        <div id='percentage' style='height:50%;overflow:hidden'>
                            <div id='overflow' style='width:50px;height:50px'></div>
                        </div>
                    </div>
                </div>
            </body></html>",
            300.0,
        );

        close(geometry(&document, "flex").1.height, 50.0);
        close(geometry(&document, "item").1.height, 50.0);
        close(geometry(&document, "percentage").1.height, 25.0);
        close(geometry(&document, "overflow").1.height, 50.0);
    }

    // The same assigned cross size is not definite when stretch does not
    // apply. The percentage therefore computes as auto and uses its content.
    #[test]
    fn non_stretched_flex_item_keeps_descendant_percentage_height_indefinite() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div id='flex' style='display:flex;width:50px;height:50px'>
                    <div id='item' style='align-self:flex-start;min-width:0;min-height:0'>
                        <div id='percentage' style='height:50%;overflow:hidden'>
                            <div style='width:50px;height:50px'></div>
                        </div>
                    </div>
                </div>
            </body></html>",
            300.0,
        );

        close(geometry(&document, "item").1.height, 50.0);
        close(geometry(&document, "percentage").1.height, 50.0);
    }

    // Reduced from WPT css/css-flexbox/flex-minimum-size-002.html. An
    // indefinite percentage minimum must not become a negative/nonzero floor
    // while flexing, and an inflexible sibling retains its authored line box.
    #[test]
    fn column_flex_percentage_minimum_does_not_block_shrinking() {
        let html = "<html><body style='margin:0'>
                <div id='flex' class='flexbox column' style='max-height:0;overflow:hidden;line-height:13px'>
                    <div id='flexible' style='min-height:100%'>This is a flex item.</div>
                    <div id='inflexible' style='flex:none'>Inflexible</div>
                </div>
            </body></html>";
        let css = ".flexbox { display:-webkit-flex; display:flex; }
            .column { -webkit-flex-direction:column; flex-direction:column; }";
        let mut factory = DocumentFactory::new();
        let mut shaper = TestGlyphShaper::new();
        let document = factory.parse_with_new_pipeline(html, Some(css)).shape(&mut shaper).expect("test shaper registers every glyph").layout(LayoutConstraints::new(800.0, 16.0).unwrap());

        close(geometry(&document, "flex").1.height, 0.0);
        close(geometry(&document, "flexible").1.height, 0.0);
        close(geometry(&document, "inflexible").1.height, 13.0);
    }

    #[test]
    fn flex_container_honors_intrinsic_min_width_keywords() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div id='min' style='display:flex;flex-wrap:wrap;width:10px;min-width:min-content'>
                    <div style='width:20px;height:20px'></div><div style='width:20px;height:20px'></div>
                </div>
                <div id='max' style='display:flex;flex-wrap:wrap;width:10px;min-width:max-content'>
                    <div style='width:20px;height:20px'></div><div style='width:20px;height:20px'></div>
                </div>
                <div id='fit-wide' style='display:flex;flex-wrap:wrap;width:10px;min-width:fit-content'>
                    <div style='width:20px;height:20px'></div><div style='width:20px;height:20px'></div>
                </div>
                <div style='width:30px'>
                    <div id='fit-30' style='display:flex;flex-wrap:wrap;width:10px;min-width:fit-content'>
                        <div style='width:20px;height:20px'></div><div style='width:20px;height:20px'></div>
                    </div>
                </div>
                <div style='width:10px'>
                    <div id='fit-10' style='display:flex;flex-wrap:wrap;width:10px;min-width:fit-content'>
                        <div style='width:20px;height:20px'></div><div style='width:20px;height:20px'></div>
                    </div>
                </div>
            </body></html>",
            100.0,
        );

        close(geometry(&document, "min").1.width, 20.0);
        close(geometry(&document, "max").1.width, 40.0);
        close(geometry(&document, "fit-wide").1.width, 40.0);
        close(geometry(&document, "fit-30").1.width, 30.0);
        close(geometry(&document, "fit-10").1.width, 20.0);
    }

    #[test]
    fn row_flex_intrinsic_widths_include_only_gaps_on_the_same_line() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div id='single-min' style='display:flex;width:min-content;column-gap:10px'>
                    <div style='width:20px'></div><div style='width:20px'></div>
                </div>
                <div id='wrap-min' style='display:flex;flex-wrap:wrap;width:min-content;column-gap:10px'>
                    <div style='width:20px'></div><div style='width:20px'></div>
                </div>
                <div id='wrap-max' style='display:flex;flex-wrap:wrap;width:max-content;column-gap:10px'>
                    <div style='width:20px'></div><div style='width:20px'></div>
                </div>
            </body></html>",
            100.0,
        );

        close(geometry(&document, "single-min").1.width, 50.0);
        close(geometry(&document, "wrap-min").1.width, 20.0);
        close(geometry(&document, "wrap-max").1.width, 50.0);
    }

    #[test]
    fn percentage_dependent_flex_basis_uses_min_content_in_a_min_content_contribution() {
        let document = layout_html(
            "<html><body style='margin:0'><div style='display:grid;grid-template-columns:repeat(4,1fr);width:800px'>
                <div id='a' style='display:flex;width:100%'><div style='flex:1 0 calc(70% - 10.5px)'>Some News Headline</div><div style='width:50px'></div></div>
                <div id='b' style='display:flex;width:100%'><div style='flex:1 0 calc(70% - 10.5px)'>Some Other News Headline 2</div><div style='width:50px'></div></div>
                <div id='c' style='display:flex;width:100%'><div style='flex:1 0 calc(70% - 10.5px)'>Even another Headline 3</div><div style='width:50px'></div></div>
                <div id='d' style='display:flex;width:100%'><div style='flex:1 0 calc(70% - 10.5px)'>Peets Coffee announces plans to move Oakland</div><div style='width:50px'></div></div>
            </div></body></html>",
            900.0,
        );

        for id in ["a", "b", "c", "d"] {
            close(geometry(&document, id).1.width, 200.0);
        }
    }

    #[test]
    fn flex_intrinsic_widths_include_floated_descendants() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div id='min' style='display:flex;width:min-content'>
                    <div><span style='float:left;width:100px;height:25px'></span><span style='float:left;width:100px;height:25px'></span><span style='float:left;width:100px;height:25px'></span><span style='float:left;width:100px;height:25px'></span></div><div></div>
                </div>
                <div id='max' style='display:flex;flex-flow:column wrap;width:max-content;height:100px'>
                    <div style='width:100%;height:50px;min-height:0'><div style='float:left;width:50px'></div><div style='float:left;width:50px'></div></div>
                </div>
            </body></html>",
            500.0,
        );

        close(geometry(&document, "min").1.width, 100.0);
        close(geometry(&document, "max").1.width, 100.0);
    }

    // Reduced from WPT css/css-flexbox/intrinsic-size/row-005.html.
    // https://github.com/web-platform-tests/wpt/blob/7ea3d665ff9ae1df1f742433d51bd1e9c6499a3b/css/css-flexbox/intrinsic-size/row-005.html
    #[test]
    fn wpt_row_flex_min_content_uses_shrinking_item_contributions() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='width:0'>
                    <div id='flex' style='display:flex;float:left'>
                        <div style='flex:1 1 200px;width:50px;min-width:0'><div style='width:100px;height:10px'></div></div>
                        <div style='flex:1 1 400px;width:50px'><div style='width:100px;height:10px'></div></div>
                    </div>
                </div>
            </body></html>",
            800.0,
        );

        close(geometry(&document, "flex").1.width, 200.0);
    }

    // Reduced from the positive desired-flex-fraction case in the same WPT.
    #[test]
    fn wpt_row_flex_min_content_uses_growing_item_contributions() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='width:0'>
                    <div id='flex' style='display:flex;float:left'>
                        <div style='flex:1 0 50px;width:200px'><div style='width:100px;height:10px'></div></div>
                        <div style='flex:2 0 100px;width:200px'><div style='width:100px;height:10px'></div></div>
                    </div>
                </div>
            </body></html>",
            800.0,
        );

        close(geometry(&document, "flex").1.width, 600.0);
    }

    #[test]
    fn raw_taffy_min_content_handles_the_shrinking_wpt_case() {
        let width = raw_taffy_min_content_width(&[(raw_taffy_flex_item(50.0, Some(0.0), 200.0, 1.0, 1.0), (100.0, 100.0)), (raw_taffy_flex_item(50.0, None, 400.0, 1.0, 1.0), (100.0, 100.0))]);

        // CSS Flexbox intrinsic sizing requires 100px from each item (200px
        // total). Raw Taffy gets this right, so our production path's 100px
        // result comes from its style conversion or intrinsic measurements.
        close(width as f64, 200.0);
    }

    #[test]
    fn raw_taffy_min_content_omits_the_shared_growing_flex_fraction() {
        let width = raw_taffy_min_content_width(&[(raw_taffy_flex_item(200.0, None, 50.0, 1.0, 0.0), (200.0, 200.0)), (raw_taffy_flex_item(200.0, None, 100.0, 2.0, 0.0), (200.0, 200.0))]);

        // CSS chooses a shared 150px desired flex fraction and produces 600px;
        // raw Taffy returns only the two 200px item contributions.
        close(width as f64, 400.0);
    }

    #[test]
    fn anonymous_flex_and_grid_items_own_their_text_runs() {
        let document = layout_html(
            "<html><body style='margin:0;line-height:20px'>
                <div id='flex' style='display:flex'>Anonymous flex item.</div>
                <div id='grid' style='display:grid'>Anonymous grid item.</div>
            </body></html>",
            300.0,
        );

        close(geometry(&document, "flex").1.height, 20.0);
        close(geometry(&document, "grid").1.height, 20.0);
    }

    #[test]
    fn first_line_and_first_letter_apply_to_grid_items_but_not_grid_containers() {
        let html = "<html><body>
                <div style='display:grid'><div id='line'>First line.</div><div id='letter'>First letter.</div></div>
                <div id='container' style='display:grid'>Anonymous grid item.</div>
            </body></html>";
        let css = "body { margin: 0; line-height: 20px; }
            #line::first-line, #letter::first-letter { line-height: 100px; }
            #container::first-line { line-height: 200px; }";
        let mut factory = DocumentFactory::new();
        let mut shaper = TestGlyphShaper::new();
        let prepared = factory.parse_with_new_pipeline(html, Some(css));
        let line_node = prepared.document().node_ids().find(|&node| prepared.document().get_dom_id(node) == Some("line")).expect("line element");
        let letter_node = prepared.document().node_ids().find(|&node| prepared.document().get_dom_id(node) == Some("letter")).expect("letter element");
        let line_style = prepared.styles().first_line_style_for_node(line_node).expect("computed ::first-line style");
        let letter_style = prepared.styles().first_letter_style_for_node(letter_node).expect("computed ::first-letter style");
        close(prepared.style_view(prepared.styles().style_for_node(line_node).expect("line element style")).line_height() as f64, 20.0);
        close(prepared.style_view(line_style).line_height() as f64, 100.0);
        close(prepared.style_view(letter_style).line_height() as f64, 100.0);
        let document = prepared.shape(&mut shaper).expect("test shaper registers every glyph").layout(LayoutConstraints::new(300.0, 16.0).unwrap());

        close(geometry(&document, "line").1.height, 100.0);
        close(geometry(&document, "letter").1.height, 100.0);
        close(geometry(&document, "container").1.height, 20.0);
    }

    #[test]
    fn ancestor_first_letter_does_not_skip_a_grid_container_to_reach_a_later_sibling() {
        let html = "<html><body>
                <div class='outer'>
                    <div class='grid'><div>Grid item.</div></div>
                    <div id='sibling'>Outside grid.</div>
                </div>
            </body></html>";
        let css = "body { margin: 0; line-height: 20px; }
            .grid { display: grid; }
            .outer::first-letter { line-height: 200px; }
            .grid::first-letter { line-height: 100px; }";
        let mut factory = DocumentFactory::new();
        let mut shaper = TestGlyphShaper::new();
        let document = factory
            .parse_with_new_pipeline(html, Some(css))
            .shape(&mut shaper)
            .expect("test shaper registers every glyph")
            .layout(LayoutConstraints::new(300.0, 16.0).unwrap());

        close(geometry(&document, "sibling").1.height, 20.0);
    }

    #[test]
    fn flex_auto_margin_absorbs_free_space() {
        let document = layout_html("<html><body><div id='c' style='display:flex;width:300px'><div id='a' style='margin-left:auto;width:50px;height:10px'></div></div></body></html>", 500.0);
        let (container, _) = geometry(&document, "c");
        let (item, size) = geometry(&document, "a");
        close(item.x, container.x + 250.0);
        close(size.width, 50.0);
    }

    #[test]
    fn flex_baseline_alignment_uses_text_baselines_not_box_bottoms() {
        let document =
            layout_html("<html><body><div style='display:flex;align-items:baseline;width:200px'><div style='font-size:10px;line-height:10px'>A</div><div style='font-size:30px;line-height:30px'>B</div></div></body></html>", 500.0);
        let baseline_for = |wanted: char| {
            document
                .render_view()
                .text()
                .lines()
                .iter()
                .find(|line| document.render_view().text().glyph_slice(line.glyphs()).expect("line glyph range").iter().any(|&glyph| document.render_view().text().glyph_metric(glyph).expect("registered glyph").ch() == wanted))
                .map(|line| line.point().y + line.baseline())
                .unwrap_or_else(|| panic!("missing line for {wanted}"))
        };
        close(baseline_for('A'), baseline_for('B'));
    }

    #[test]
    fn cross_axis_auto_margin_suppresses_flex_baseline_alignment() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='c' style='display:flex;align-items:baseline;width:100px;height:100px'><div id='a' style='margin-top:auto;padding-top:10px;font-size:10px;line-height:10px'>A</div><div style='height:100px;font-size:20px;line-height:20px'>B</div></div></body></html>",
            200.0,
        );
        let (container, _) = geometry(&document, "c");
        let (auto_item, auto_item_size) = geometry(&document, "a");

        close(auto_item.y, container.y + 100.0 - auto_item_size.height);
    }

    #[test]
    fn wrapped_flex_aligns_baselines_per_line() {
        let document = layout_html(
            "<html><body><div style='display:flex;flex-wrap:wrap;align-items:baseline;width:100px;row-gap:5px'><div style='box-sizing:border-box;width:50px;font-size:10px;line-height:10px'>A</div><div style='box-sizing:border-box;width:50px;font-size:20px;line-height:20px'>B</div><div style='box-sizing:border-box;width:50px;font-size:12px;line-height:12px'>C</div><div style='box-sizing:border-box;width:50px;font-size:24px;line-height:24px'>D</div></div></body></html>",
            500.0,
        );
        let baseline_for = |wanted: char| {
            document
                .render_view()
                .text()
                .lines()
                .iter()
                .find(|line| document.render_view().text().glyph_slice(line.glyphs()).expect("line glyph range").iter().any(|&glyph| document.render_view().text().glyph_metric(glyph).expect("registered glyph").ch() == wanted))
                .map(|line| line.point().y + line.baseline())
                .unwrap_or_else(|| panic!("missing line for {wanted}"))
        };

        close(baseline_for('A'), baseline_for('B'));
        close(baseline_for('C'), baseline_for('D'));
        assert!(baseline_for('C') > baseline_for('A') + 5.0, "the second flex line must remain below the first");
    }

    #[test]
    fn grid_aligns_baselines_per_row() {
        let document = layout_html(
            "<html><body><div style='display:grid;grid-template-columns:50px 50px;align-items:baseline;row-gap:5px'><div style='font-size:10px;line-height:10px'>A</div><div style='font-size:20px;line-height:20px'>B</div><div style='font-size:12px;line-height:12px'>C</div><div style='font-size:24px;line-height:24px'>D</div></div></body></html>",
            500.0,
        );
        let baseline_for = |wanted: char| {
            document
                .render_view()
                .text()
                .lines()
                .iter()
                .find(|line| document.render_view().text().glyph_slice(line.glyphs()).expect("line glyph range").iter().any(|&glyph| document.render_view().text().glyph_metric(glyph).expect("registered glyph").ch() == wanted))
                .map(|line| line.point().y + line.baseline())
                .unwrap_or_else(|| panic!("missing line for {wanted}"))
        };

        close(baseline_for('A'), baseline_for('B'));
        close(baseline_for('C'), baseline_for('D'));
        assert!(baseline_for('C') > baseline_for('A') + 5.0, "the second grid row must remain below the first");
    }

    // Reduced from WPT css/css-grid/layout-algorithm/
    // grid-minimum-contribution-baseline-shim.html.
    #[test]
    fn generated_inline_block_pseudos_supply_grid_item_baselines() {
        let document = layout_html_with_css(
            "<html><body style='margin:0'>
                <div style='display:grid;position:relative;font-size:0;height:0;width:0;grid-template-columns:50px 50px;grid-template-rows:minmax(auto,0);align-items:baseline'>
                    <div id='first-item' class='item' style='padding-top:25px'></div>
                    <div id='second-item' class='item second' style='padding-bottom:25px'></div>
                </div>
            </body></html>",
            ".item::before { content:'';display:inline-block;width:25px;height:25px;vertical-align:bottom }
                .second::before { vertical-align:top }",
            200.0,
        );

        close(geometry(&document, "first-item").1.height, 50.0);
        close(geometry(&document, "second-item").1.height, 50.0);
        close(geometry(&document, "second-item").0.y - geometry(&document, "first-item").0.y, 50.0);
    }

    #[test]
    fn wrapping_flex_moves_items_to_the_next_line() {
        let document =
            layout_html("<html><body><div id='c' style='display:flex;flex-wrap:wrap;width:120px;row-gap:5px'><div id='a' style='width:70px;height:20px'></div><div id='b' style='width:70px;height:20px'></div></div></body></html>", 500.0);
        let (a, _) = geometry(&document, "a");
        let (b, _) = geometry(&document, "b");
        close(b.y, a.y + 25.0);
    }

    #[test]
    fn column_flex_honors_main_and_cross_axis_alignment() {
        let document = layout_html(
            "<html><body><div id='c' style='display:flex;flex-direction:column;justify-content:end;align-items:center;width:100px;height:100px'><div id='a' style='width:20px;height:20px'></div><div id='b' style='width:40px;height:30px'></div></div></body></html>",
            500.0,
        );
        let (container, _) = geometry(&document, "c");
        let (a, _) = geometry(&document, "a");
        let (b, _) = geometry(&document, "b");
        close(a.x, container.x + 40.0);
        close(b.x, container.x + 30.0);
        close(a.y, container.y + 50.0);
        close(b.y, container.y + 70.0);
    }

    #[test]
    fn taffy_and_renderer_agree_on_content_box_padding_and_border() {
        let document = layout_html(
            "<html><body><div id='c' style='display:flex;width:300px'><div id='a' style='box-sizing:content-box;width:100px;height:10px;padding:10px;border:5px solid'></div><div id='b' style='width:20px;height:10px'></div></div></body></html>",
            500.0,
        );
        let (a, a_size) = geometry(&document, "a");
        let (b, _) = geometry(&document, "b");
        close(a_size.width, 130.0);
        close(a_size.height, 40.0);
        close(b.x, a.x + 130.0);
    }

    #[test]
    fn grid_tracks_gap_and_explicit_placement_reach_geometry() {
        let document = layout_html(
            "<html><body><div id='c' style='display:grid;width:300px;grid-template-columns:100px 1fr;column-gap:10px'><div id='a' style='height:20px'></div><div id='b' style='grid-column:2;height:20px'></div></div></body></html>",
            500.0,
        );
        let (container, _) = geometry(&document, "c");
        let (a, a_size) = geometry(&document, "a");
        let (b, b_size) = geometry(&document, "b");
        close(a.x, container.x);
        close(a_size.width, 100.0);
        close(b.x, container.x + 110.0);
        close(b_size.width, 190.0);
    }

    #[test]
    fn legacy_grid_gap_shorthand_reaches_taffy_track_sizing() {
        let document = layout_html(
            "<html><body style='margin:0'><div style='display:grid;width:200px;height:100px;grid-gap:10px 20px;grid:50px 1fr / 100px 1fr'><div id='a'></div><div id='b'></div><div id='c'></div><div id='d'></div></div></body></html>",
            300.0,
        );

        close(geometry(&document, "a").1.width, 100.0);
        close(geometry(&document, "b").1.width, 80.0);
        close(geometry(&document, "c").1.height, 40.0);
        close(geometry(&document, "d").1.height, 40.0);
    }

    #[test]
    // Reduced from WPT css/css-grid/grid-model/grid-gutters-as-percentage-001.html.
    // Auto tracks with equal max-content contributions must stretch equally,
    // regardless of differences in their min-content contributions.
    fn percentage_gap_auto_tracks_stretch_equally() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:grid;width:400px;column-gap:10%;font-size:10px;line-height:10px'>
                    <div id='a' style='grid-column:1;grid-row:1'>XXX X XX X</div>
                    <div id='b' style='grid-column:2;grid-row:1'>XX XXX X X</div>
                    <div id='c' style='grid-column:1;grid-row:2'>X XX XXX X</div>
                    <div id='d' style='grid-column:2;grid-row:2'>XXXXX X XX</div>
                </div>
            </body></html>",
            800.0,
        );

        for id in ["a", "b", "c", "d"] {
            close(geometry(&document, id).1.width, 180.0);
        }
    }

    // Reduced from WPT css/css-grid/grid-items/grid-items-percentage-margins-001.html.
    // A definite track gives a percentage item a definite basis even when
    // the parent disables the item's default stretch alignment.
    #[test]
    fn percentage_grid_item_uses_fixed_track_under_start_alignment() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:grid;grid-template-columns:100px;width:500px;justify-items:start'>
                    <div id='item' style='width:100%;height:10px'></div>
                </div>
            </body></html>",
            800.0,
        );

        close(geometry(&document, "item").1.width, 100.0);
    }

    #[test]
    fn percentage_padding_uses_grid_area_width_without_shrinking_measured_content_twice() {
        let document = layout_html(
            "<html><body style='margin:0'><div style='display:grid;width:500px;grid-template-columns:minmax(auto,100px);justify-items:start;font-size:20px'><div id='item' style='padding-left:50%'>X</div><div id='fill' style='width:100%;height:10px'></div></div></body></html>",
            800.0,
        );

        close(geometry(&document, "item").1.width, 60.0);
        close(geometry(&document, "fill").1.width, 100.0);
    }

    #[test]
    fn grid_auto_repeat_intrinsic_width_preserves_definite_minimum() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='grid' style='display:grid;float:left;box-sizing:border-box;min-width:300px;border:10px solid;grid-template-columns:repeat(auto-fill,100px)'><div style='grid-column:-2'></div></div></body></html>",
            600.0,
        );

        close(geometry(&document, "grid").1.width, 320.0);
    }

    #[test]
    fn grid_auto_repeat_intrinsic_width_resolves_percentage_maximum() {
        let document = layout_html(
            "<html><body style='margin:0'><div style='width:600px'><div id='grid' style='display:grid;width:min-content;max-width:50%;grid-template-columns:repeat(auto-fill,100px)'><div style='grid-column:-2'></div></div></div></body></html>",
            700.0,
        );

        close(geometry(&document, "grid").1.width, 300.0);
    }

    #[test]
    fn grid_auto_repeat_intrinsic_width_uses_fixed_maximum() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='grid' style='display:grid;max-width:300px;grid-template-columns:repeat(auto-fill,100px)'><div style='grid-column:-2'></div></div></body></html>",
            800.0,
        );

        close(geometry(&document, "grid").1.width, 300.0);
    }

    #[test]
    fn intrinsic_border_box_auto_repeat_reserves_percentage_padding_from_maximum() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='wrapper' style='width:min-content'><div id='grid' style='display:grid;box-sizing:border-box;width:min-content;max-width:300px;padding:10%;grid-template-columns:repeat(auto-fill,100px)'><div style='grid-column:-2'></div></div></div></body></html>",
            800.0,
        );

        close(geometry(&document, "wrapper").1.width, 300.0);
        close(geometry(&document, "grid").1.width, 260.0);
    }

    // Reduced from WPT css/css-grid/layout-algorithm/
    // flex-sizing-rows-min-max-height-001.html.
    #[test]
    fn grid_flexible_rows_use_the_constrained_container_height() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div id='height-70' style='display:grid;width:50px;height:70px;grid-template-rows:minmax(10px,1fr) minmax(10px,4fr);row-gap:33px'>
                    <div id='h70-a'></div><div id='h70-b'></div>
                </div>
                <div id='max-70' style='display:grid;width:50px;max-height:70px;grid-template-rows:minmax(10px,1fr) minmax(10px,4fr);row-gap:33px'>
                    <div id='m70-a'></div><div id='m70-b'></div>
                </div>
                <div id='min-108' style='display:grid;width:50px;min-height:108px;max-height:60px;grid-template-rows:minmax(10px,1fr) minmax(10px,4fr);row-gap:33px'>
                    <div id='m108-a'></div><div id='m108-b'></div>
                </div>
                <div id='float-max-70' style='display:grid;float:left;margin:3px;max-height:70px;grid:minmax(10px,1fr) minmax(10px,4fr) / 50px;grid-row-gap:33px;border:5px dashed;padding:2px'>
                    <div id='fm70-a'></div><div id='fm70-b'></div>
                </div>
                <div id='float-min-108' style='display:grid;float:left;margin:3px;min-height:108px;max-height:60px;grid:minmax(10px,1fr) minmax(10px,4fr) / 50px;grid-row-gap:33px;border:5px dashed;padding:2px'>
                    <div id='fm108-a'></div><div id='fm108-b'></div>
                </div>
                <div id='float-wrapper' style='float:left'>
                    <div id='wrapped-grid' style='display:grid;margin:3px;min-height:max-content;grid:minmax(10px,1fr) minmax(10px,4fr) / 50px;grid-row-gap:33px;border:5px dashed;padding:2px'>
                        <div></div><div></div>
                    </div>
                </div>
            </body></html>",
            300.0,
        );

        for container in ["height-70", "max-70"] {
            close(geometry(&document, container).1.height, 70.0);
        }
        for item in ["h70-a", "m70-a"] {
            close(geometry(&document, item).1.height, 10.0);
        }
        for item in ["h70-b", "m70-b"] {
            close(geometry(&document, item).1.height, 27.0);
        }
        close(geometry(&document, "min-108").1.height, 108.0);
        close(geometry(&document, "m108-a").1.height, 15.0);
        close(geometry(&document, "m108-b").1.height, 60.0);
        close(geometry(&document, "float-max-70").1.height, 84.0);
        close(geometry(&document, "fm70-a").1.height, 10.0);
        close(geometry(&document, "fm70-b").1.height, 27.0);
        close(geometry(&document, "float-min-108").1.height, 122.0);
        close(geometry(&document, "fm108-a").1.height, 15.0);
        close(geometry(&document, "fm108-b").1.height, 60.0);
        close(geometry(&document, "wrapped-grid").1.width, 64.0);
    }

    // Reduced from WPT css/css-grid/grid-model/grid-min-max-height-001.html.
    #[test]
    fn wpt_grid_explicit_rows_contribute_before_max_height_clamps() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div id='grid' style='display:grid;max-height:100px;grid-template-columns:40px;grid-template-rows:150px 50px'></div>
            </body></html>",
            800.0,
        );

        close(geometry(&document, "grid").1.height, 100.0);
    }

    // Reduced from WPT css/css-grid/grid-definition/
    // flex-content-resolution-rows-002.html.
    #[test]
    fn percentage_height_grid_items_stretch_within_their_rows() {
        let document = layout_html(
            "<html><body style='margin:0'><div style='height:40px'><div style='display:grid;height:100%;grid-template-columns:50px;grid-template-rows:minmax(10px,max-content) minmax(10px,1fr)'><div id='first' style='height:100%;font:10px/1 serif'>XXXXX</div><div id='second' style='height:100%;grid-row:2'></div></div></div></body></html>",
            800.0,
        );

        close(geometry(&document, "first").1.height, 10.0);
        close(geometry(&document, "second").1.height, 30.0);
    }

    // Reduced from WPT css/css-grid/grid-definition/
    // grid-percentage-rows-indefinite-height-001.html.
    #[test]
    fn indefinite_grid_height_resolves_percentage_rows_against_intrinsic_height() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='grid' style='display:grid;grid-template-rows:60%;font:25px/1 serif'><div id='item'>X<br>X</div></div></body></html>",
            200.0,
        );

        close(geometry(&document, "grid").1.height, 50.0);
        close(geometry(&document, "item").1.height, 30.0);
    }

    // Reduced from WPT css/css-grid/grid-definition/
    // grid-percentage-rows-indefinite-height-002.html.
    #[test]
    fn indefinite_grid_height_reruns_mixed_percentage_row_sizing() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='grid' style='display:grid;border:5px solid;grid-template-rows:auto 60% auto;font:25px/1 serif'>
                <div id='first' style='grid-row:1;grid-column:1'></div>
                <div id='spanning' style='grid-row:1 / 4;grid-column:2'>X</div>
                <div id='last' style='grid-row:3;grid-column:3'></div>
            </div></body></html>",
            200.0,
        );

        close(geometry(&document, "grid").1.height, 35.0);
        close(geometry(&document, "first").1.height, 5.0);
        close(geometry(&document, "spanning").1.height, 25.0);
        close(geometry(&document, "last").1.height, 5.0);
    }

    // Reduced from WPT css/css-grid/grid-definition/grid-auto-repeat-min-size-004.html.
    #[test]
    fn wpt_floated_auto_repeat_grid_uses_resolved_minimum_height() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='height:1000px'>
                    <div id='grid' style='display:inline-grid;grid-template-rows:repeat(auto-fill,20%);grid-auto-columns:100px;min-height:50%;float:left'>
                        <div id='a'>Cell 1</div><div id='b'>Cell 2</div><div id='c'>Cell 3</div><div id='d'>Cell 4</div><div id='e'>Cell 5</div>
                    </div>
                </div>
            </body></html>",
            800.0,
        );

        close(geometry(&document, "grid").1.width, 100.0);
        close(geometry(&document, "grid").1.height, 500.0);
        for id in ["a", "b", "c", "d", "e"] {
            close(geometry(&document, id).1.width, 100.0);
            close(geometry(&document, id).1.height, 100.0);
        }
    }

    // Reduced from WPT css/css-contain/contain-inline-size-grid-auto-fit.html.
    // `contain:inline-size` is intentionally unsupported, but the otherwise
    // valid zero-minimum auto-fit grid must still remain bounded and render.
    #[test]
    fn zero_minimum_auto_fit_grid_does_not_overflow_repetition_count() {
        let document = layout_html(
            "<html><body style='margin:0'><div style='width:100px'><div id='grid' style='display:grid;height:100px;grid-template-columns:repeat(auto-fit,minmax(0,1fr))'><div id='item' style='background:green'></div></div></div></body></html>",
            800.0,
        );

        close(geometry(&document, "grid").1.width, 100.0);
        close(geometry(&document, "item").1.width, 100.0);
        close(geometry(&document, "item").1.height, 100.0);
    }

    // Reduced from WPT css/css-grid/grid-items/
    // grid-item-min-contribution-fit-content-001.html.
    #[test]
    fn wpt_grid_fit_content_item_uses_its_min_content_contribution() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div id='grid' style='display:inline-grid;grid-template-columns:minmax(auto,50px)'>
                    <div style='width:fit-content'><div style='width:100px;height:25px'></div></div>
                </div>
            </body></html>",
            800.0,
        );

        close(geometry(&document, "grid").1.width, 100.0);
    }

    #[test]
    fn grid_fixed_item_outer_minimum_floors_a_smaller_auto_track_maximum() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:grid;width:100px;grid-template-columns:minmax(auto,0px);grid-template-rows:10px 10px'>
                    <div id='sized' style='width:60px;margin-left:5px;margin-right:10px;padding-left:6px;padding-right:3px;border-left:2px solid;border-right:4px solid'></div>
                    <div id='stretched'></div>
                </div>
            </body></html>",
            800.0,
        );

        close(geometry(&document, "sized").1.width, 75.0);
        close(geometry(&document, "stretched").1.width, 90.0);
    }

    #[test]
    fn grid_auto_item_remains_capped_when_no_definite_minimum_exceeds_the_track() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div id='grid' style='display:inline-grid;grid-template-columns:minmax(auto,50px)'>
                    <div><div style='width:100px;height:10px'></div></div>
                </div>
            </body></html>",
            800.0,
        );

        close(geometry(&document, "grid").1.width, 50.0);
    }

    #[test]
    fn grid_definite_minimum_below_the_fixed_track_limit_does_not_suppress_track_growth() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:grid;width:50px;grid-template-columns:minmax(auto,20px) minmax(auto,20px);grid-template-rows:10px 10px'>
                    <div id='sized' style='grid-column:span 2;width:10px'></div>
                    <div id='stretched' style='grid-column:span 2'></div>
                </div>
            </body></html>",
            800.0,
        );

        close(geometry(&document, "sized").1.width, 10.0);
        close(geometry(&document, "stretched").1.width, 40.0);
    }

    #[test]
    fn grid_auto_item_outer_insets_floor_zero_auto_track_maxima() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:grid;width:50px;grid-template-columns:minmax(auto,0px) minmax(auto,0px)'>
                    <div id='margin-item' style='grid-column:span 2;margin:0 5px'></div>
                    <div id='margin-peer' style='grid-column:span 2'></div>
                </div>
                <div style='display:grid;width:50px;grid-template-columns:minmax(auto,0px) minmax(auto,0px)'>
                    <div id='padding-item' style='grid-column:span 2;padding:0 5px'></div>
                    <div id='padding-peer' style='grid-column:span 2'></div>
                </div>
                <div style='display:grid;width:50px;grid-template-columns:minmax(auto,0px) minmax(auto,0px)'>
                    <div id='border-item' style='grid-column:span 2;border-left:5px solid;border-right:5px solid'></div>
                    <div id='border-peer' style='grid-column:span 2'></div>
                </div>
            </body></html>",
            800.0,
        );

        close(geometry(&document, "margin-item").1.width, 0.0);
        close(geometry(&document, "padding-item").1.width, 10.0);
        close(geometry(&document, "border-item").1.width, 10.0);
        for id in ["margin-peer", "padding-peer", "border-peer"] {
            close(geometry(&document, id).1.width, 10.0);
        }
    }

    // Reduced from WPT css/css-grid/grid-lanes/track-sizing/auto-repeat/
    // column-auto-repeat-010.html.
    #[test]
    fn wpt_column_auto_repeat_uses_resolved_percentage_minimum_width() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='width:600px'>
                    <div id='grid' style='display:inline-grid;grid-template-columns:repeat(auto-fill,25%);min-width:50%;height:200px;float:left'>
                        <div id='a' style='width:100%'>Cell 1</div><div id='b' style='width:100%'>Cell 2</div><div id='c' style='width:100%'>Cell 3</div><div id='d' style='width:100%'>Cell 4</div><div id='e' style='width:100%'>Cell 5</div>
                    </div>
                </div>
            </body></html>",
            800.0,
        );

        close(geometry(&document, "grid").1.width, 300.0);
        for id in ["a", "b", "c", "d", "e"] {
            close(geometry(&document, id).1.width, 75.0);
        }
    }

    // Reduced from WPT css/css-grid/grid-lanes/track-sizing/auto-repeat/
    // row-auto-repeat-009.html.
    #[test]
    fn wpt_row_auto_repeat_percentage_items_fill_percentage_tracks() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='height:1000px'>
                    <div id='grid' style='display:inline-grid;grid-template-rows:repeat(auto-fill,20%);min-height:50%;width:100px;float:left'>
                        <div id='a' style='width:100px;height:100%'>Cell 1</div><div id='b' style='width:100px;height:100%'>Cell 2</div><div id='c' style='width:100px;height:100%'>Cell 3</div><div id='d' style='width:100px;height:100%'>Cell 4</div><div id='e' style='width:100px;height:100%'>Cell 5</div>
                    </div>
                </div>
            </body></html>",
            800.0,
        );

        close(geometry(&document, "grid").1.height, 500.0);
        for id in ["a", "b", "c", "d", "e"] {
            close(geometry(&document, id).1.height, 100.0);
        }
    }

    // Reduced from WPT css/css-flexbox/intrinsic-size/row-compat-001.html.
    // Each percentage-sized grid item must resolve against its 1fr grid area,
    // not against the full grid container.
    #[test]
    fn wpt_percentage_grid_items_use_their_fractional_track_width() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:grid;grid-template-columns:repeat(4,1fr);width:800px'>
                    <div id='a' style='display:flex;width:100%'></div>
                    <div id='b' style='display:flex;width:100%'></div>
                    <div id='c' style='display:flex;width:100%'></div>
                    <div id='d' style='display:flex;width:100%'></div>
                </div>
            </body></html>",
            800.0,
        );

        for id in ["a", "b", "c", "d"] {
            close(geometry(&document, id).1.width, 200.0);
        }
    }

    #[test]
    fn raw_taffy_percentage_grid_items_resolve_against_fractional_tracks() {
        let mut taffy = TaffyTree::<()>::new();
        taffy.disable_rounding();
        let children = (0..4).map(|_| taffy.new_leaf(Style { size: TaffySize { width: Dimension::percent(1.0), height: Dimension::auto() }, ..Style::default() }).expect("raw Taffy grid item")).collect::<Vec<_>>();
        let root = taffy
            .new_with_children(
                Style { display: TaffyDisplay::Grid, size: TaffySize { width: Dimension::length(800.0), height: Dimension::auto() }, grid_template_columns: vec![taffy::style_helpers::fr(1.0); 4], ..Style::default() },
                &children,
            )
            .expect("raw Taffy grid root");
        taffy.compute_layout(root, TaffySize { width: AvailableSpace::MaxContent, height: AvailableSpace::MaxContent }).expect("raw Taffy grid layout");

        for (column, child) in children.into_iter().enumerate() {
            let layout = taffy.layout(child).expect("raw Taffy child layout");
            close(layout.location.x as f64, column as f64 * 200.0);
            close(layout.size.width as f64, 200.0);
        }
    }

    #[test]
    fn grid_template_areas_and_repeat_place_named_items() {
        let document = layout_html(
            "<html><body><div id='c' style=\"display:grid;width:200px;grid-template-columns:repeat(2,1fr);grid-template-rows:repeat(2,30px);grid-template-areas:'head head' 'side main'\"><div id='main' style='grid-area:main'></div><div id='head' style='grid-area:head'></div><div id='side' style='grid-area:side'></div></div></body></html>",
            500.0,
        );
        let (container, _) = geometry(&document, "c");
        let (main, main_size) = geometry(&document, "main");
        let (head, head_size) = geometry(&document, "head");
        let (side, side_size) = geometry(&document, "side");
        close(head.x, container.x);
        close(head.y, container.y);
        close(head_size.width, 200.0);
        close(side.x, container.x);
        close(side.y, container.y + 30.0);
        close(side_size.width, 100.0);
        close(main.x, container.x + 100.0);
        close(main.y, container.y + 30.0);
        close(main_size.width, 100.0);
    }

    #[test]
    fn nested_grid_inside_flex_keeps_independent_stage_geometry() {
        let document = layout_html(
            "<html><body><div id='outer' style='display:flex;width:240px'><div id='grid' style='display:grid;flex:1;grid-template-columns:1fr 1fr'><div id='a' style='height:10px'></div><div id='b' style='height:10px'></div></div></div></body></html>",
            500.0,
        );
        let (grid, grid_size) = geometry(&document, "grid");
        let (a, a_size) = geometry(&document, "a");
        let (b, b_size) = geometry(&document, "b");
        close(grid_size.width, 240.0);
        close(a.x, grid.x);
        close(a_size.width, 120.0);
        close(b.x, grid.x + 120.0);
        close(b_size.width, 120.0);
    }

    #[test]
    fn inline_flex_is_one_atomic_item_in_the_surrounding_line() {
        let document = layout_html("<html><body><p>A<span id='c' style='display:inline-flex'><span style='width:20px;height:12px'></span><span style='width:30px;height:12px'></span></span>B</p></body></html>", 300.0);
        let (_, size) = geometry(&document, "c");
        close(size.width, 50.0);
        close(size.height, 12.0);
        assert_eq!(document.line_count(), 1, "atomic descendants must not leak their internal runs into the parent line");
    }

    #[test]
    fn column_inline_flex_intrinsic_width_is_its_widest_item() {
        let document = layout_html("<html><body><p><span id='c' style='display:inline-flex;flex-direction:column'><span style='width:20px;height:10px'></span><span style='width:30px;height:10px'></span></span></p></body></html>", 300.0);
        let (_, size) = geometry(&document, "c");

        close(size.width, 30.0);
    }

    #[test]
    fn wrapped_column_inline_flex_intrinsic_width_uses_its_definite_height() {
        let document = layout_html(
            "<html><body style='margin:0'><span id='c' style='display:inline-flex;flex-flow:column wrap;align-content:flex-start;height:35px'>
                <span style='flex:none;width:10px;height:20px'></span>
                <span style='flex:none;width:50px;height:10px'></span>
                <span style='flex:none;width:80px;height:10px'></span>
                <span style='flex:none;width:40px;height:20px'></span>
            </span></body></html>",
            300.0,
        );

        close(geometry(&document, "c").1.width, 130.0);
        close(geometry(&document, "c").1.height, 35.0);
    }

    #[test]
    fn column_wrap_intrinsic_sizing_transfers_an_auto_cross_size_through_ratio() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='c' style='display:flex;flex-flow:column wrap;width:max-content;height:100px'>
                <div id='ratio' style='width:10%;aspect-ratio:1/5;min-height:0;flex:0 0 auto'>
                    <span style='float:left;width:10px;height:200px'></span><span style='float:left;width:10px;height:200px'></span>
                </div>
                <div style='width:80px;flex:0 0 1px;min-height:0'></div>
            </div></body></html>",
            500.0,
        );

        close(geometry(&document, "c").1.width, 100.0);
        close(geometry(&document, "ratio").1.height, 50.0);
    }

    #[test]
    fn inline_grid_intrinsic_width_includes_explicit_tracks_and_gap() {
        let document = layout_html("<html><body><p><span id='c' style='display:inline-grid;grid-template-columns:100px 100px;column-gap:10px'><span></span><span></span></span></p></body></html>", 400.0);
        let (_, size) = geometry(&document, "c");

        close(size.width, 210.0);
    }

    #[test]
    fn empty_inline_grid_intrinsic_width_includes_explicit_tracks_and_gap() {
        let document = layout_html("<html><body><p><span id='c' style='display:inline-grid;grid-template-columns:100px 100px;column-gap:10px'></span></p></body></html>", 400.0);
        let (_, size) = geometry(&document, "c");

        close(size.width, 210.0);
    }

    #[test]
    fn size_contained_grid_uses_empty_intrinsic_size_but_lays_out_its_item() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='grid' style='display:grid;contain:size;width:min-content;height:min-content;grid:calc(100px + 50%) / calc(100px + 200%)'><div id='item'></div></div></body></html>",
            400.0,
        );

        assert_eq!(geometry(&document, "grid").1, Size::ZERO);
        assert_eq!(geometry(&document, "item").1, Size::new(100.0, 100.0));
    }

    #[test]
    fn definite_grid_axes_resolve_calc_track_percentages() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='grid' style='display:grid;contain:size;width:100px;height:100px;grid:calc(100px + 50%) / calc(100px + 200%)'><div id='item'></div></div></body></html>",
            400.0,
        );

        assert_eq!(geometry(&document, "grid").1, Size::new(100.0, 100.0));
        assert_eq!(geometry(&document, "item").1, Size::new(300.0, 150.0));
    }

    #[test]
    fn contained_zero_width_grid_floors_percentage_track_maximum_by_intrinsic_minimum() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='grid' style='display:grid;contain:size;width:min-content;grid-template-columns:minmax(min-content,200%)'><div id='item' style='width:300px;height:10px'></div></div></body></html>",
            400.0,
        );

        close(geometry(&document, "grid").1.width, 0.0);
        close(geometry(&document, "item").1.width, 300.0);
    }

    #[test]
    fn empty_inline_flex_keeps_zero_intrinsic_width() {
        let document = layout_html("<html><body style='margin:0'><p style='margin:0'><span id='c' style='display:inline-flex'></span></p></body></html>", 200.0);
        let (_, size) = geometry(&document, "c");

        close(size.width, 0.0);
    }

    #[test]
    fn top_level_inline_flex_after_block_content_is_not_pruned() {
        let document = layout_html("<html><body style='margin:0'><div></div><div id='c' style='display:inline-flex'><div style='width:20px;height:10px'></div></div></body></html>", 200.0);
        let (_, size) = geometry(&document, "c");

        close(size.width, 20.0);
        close(size.height, 10.0);
    }

    #[test]
    fn top_level_inline_grid_after_block_content_is_not_pruned() {
        let document = layout_html("<html><body style='margin:0'><div></div><div id='c' style='display:inline-grid;grid-template-columns:20px'><div style='height:10px'></div></div></body></html>", 200.0);
        let (_, size) = geometry(&document, "c");

        close(size.width, 20.0);
        close(size.height, 10.0);
    }

    #[test]
    fn inline_flex_percentage_spacing_uses_containing_block_width() {
        let document =
            layout_html("<html><body style='margin:0'><p style='margin:0;width:200px'><span id='c' style='display:inline-flex;margin-left:10%;padding-left:10%'><span style='width:20px;height:10px'></span></span></p></body></html>", 400.0);
        let (point, size) = geometry(&document, "c");

        close(point.x, 20.0);
        close(size.width, 40.0);
    }

    #[test]
    fn floated_flex_container_shrink_wraps_its_contents() {
        let document = layout_html("<html><body><div id='c' style='float:left;display:flex'><div style='width:20px;height:10px'></div><div style='width:30px;height:10px'></div></div></body></html>", 300.0);
        let (_, size) = geometry(&document, "c");

        close(size.width, 50.0);
    }

    #[test]
    fn floated_grid_container_shrink_wraps_its_tracks() {
        let document = layout_html("<html><body><div id='c' style='float:left;display:grid;grid-template-columns:20px 30px'><div style='height:10px'></div><div style='height:10px'></div></div></body></html>", 300.0);
        let (_, size) = geometry(&document, "c");

        close(size.width, 50.0);
    }

    #[test]
    fn nested_floated_grid_container_shrink_wraps_fixed_tracks() {
        let html = "<html><body style='margin:0'>
                <div class='container'>
                    <div id='grid' class='grid'>
                        <div></div><div></div><div></div><div></div>
                    </div>
                </div>
            </body></html>";
        let css = ".grid { display:grid; grid-template-columns:200px 200px; grid-template-rows:200px 200px; }
            .container { width:600px; height:600px; }
            #grid { float:left; }";
        let mut factory = DocumentFactory::new();
        let mut shaper = TestGlyphShaper::new();
        let document = factory
            .parse_with_new_pipeline(html, Some(css))
            .shape(&mut shaper)
            .expect("test shaper registers every glyph")
            .layout(LayoutConstraints::new(800.0, 16.0).unwrap());

        close(geometry(&document, "grid").1.width, 400.0);
    }

    #[test]
    fn floated_flex_grid_shrink_to_fit_honors_min_and_max_width() {
        let min_document = layout_html("<html><body style='margin:0'><div id='c' style='float:left;display:flex;min-width:100px'><div style='width:20px;height:10px'></div></div></body></html>", 300.0);
        let max_document = layout_html("<html><body style='margin:0'><div id='c' style='float:left;display:grid;max-width:10px;grid-template-columns:20px'><div style='height:10px'></div></div></body></html>", 300.0);

        close(geometry(&min_document, "c").1.width, 100.0);
        close(geometry(&max_document, "c").1.width, 10.0);
    }

    #[test]
    fn non_stretched_text_flex_item_uses_fit_content_width_and_contains_child_margins() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:flex;width:80px;height:80px;align-items:flex-start'>
                    <div id='item' style='min-width:0;min-height:0'>
                        <p style='font-size:20px;line-height:20px;margin:5px'>xx xxx</p>
                        <p style='font-size:20px;line-height:20px;margin:5px'>xx</p>
                    </div>
                </div>
            </body></html>",
            200.0,
        );
        let (_, item) = geometry(&document, "item");

        close(item.width, 70.0);
        close(item.height, 55.0);
    }

    #[test]
    fn column_flex_stretch_height_is_auto_under_an_indefinite_max_height() {
        let document = layout_html(
            "<html><body style='margin:0'><div style='display:inline-flex;flex-direction:column;width:100px;max-height:100px'>
                <div id='item' style='flex:none;height:stretch;margin:5px;padding:2px;border:3px solid;font:20px/1 serif'>X</div>
                <div style='flex:none;height:30px'></div>
                <div style='flex:none;height:30px'></div>
                <div style='flex:none;height:30px'></div>
            </div></body></html>",
            300.0,
        );

        assert_eq!(geometry(&document, "item").1.height, 30.0);
    }

    #[test]
    fn row_flex_stretch_constraints_use_the_max_constrained_line_size() {
        let document = layout_html(
            "<html><body style='margin:0'><div style='display:flex;width:100px;max-height:100px'>
                <div id='minimum' style='flex:none;height:0;min-height:stretch;margin:5px;padding:2px;border:3px solid'></div>
                <div id='maximum' style='flex:none;height:500px;max-height:stretch;margin:5px;padding:2px;border:3px solid'></div>
            </div></body></html>",
            300.0,
        );

        assert_eq!(geometry(&document, "minimum").1.height, 90.0);
        assert_eq!(geometry(&document, "maximum").1.height, 90.0);
    }

    #[test]
    fn grid_item_contains_child_margins() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <div style='display:grid;grid-template-columns:max-content;width:80px'>
                    <div id='item'>
                        <p style='font-size:20px;line-height:20px;margin:5px'>xx xxx</p>
                        <p style='font-size:20px;line-height:20px;margin:5px'>xx</p>
                    </div>
                </div>
            </body></html>",
            200.0,
        );
        let (_, item) = geometry(&document, "item");

        close(item.height, 55.0);
    }

    #[test]
    fn viewport_relayout_recomputes_flexible_item_allocations() {
        let mut document = layout_html("<html><body style='margin:0'><div id='c' style='display:flex'><div id='a' style='flex:1;height:10px'></div><div id='b' style='flex:1;height:10px'></div></div></body></html>", 300.0);
        let (_, wide_a) = geometry(&document, "a");
        document.relayout(LayoutConstraints::new(200.0, 16.0).unwrap());
        let (_, narrow_a) = geometry(&document, "a");
        close(wide_a.width, 150.0);
        close(narrow_a.width, 100.0);
    }

    #[test]
    fn normal_flow_does_not_enter_taffy_or_its_measurement_path() {
        let mut document = layout_html("<html><body><p>ordinary document text</p></body></html>", 300.0);
        let timings = document.relayout_with_timings(LayoutConstraints::new(240.0, 16.0).unwrap());

        assert!(timings.layout_flex_grid.is_zero());
        assert!(timings.measure_flex_grid_item.is_zero());
    }
}
