#[cfg(test)]
mod tests {
    use super::columns::{TableCellPlacement, TableGrid, build_table_grid};
    use crate::parser::DocumentFactory;
    use crate::test_support::TestGlyphShaper;
    use crate::{LaidOutDocument, LayoutConstraints};

    fn layout_html(html: &str, width: f64) -> LaidOutDocument {
        layout_html_css(html, None, width)
    }

    fn layout_html_css(html: &str, css: Option<&str>, width: f64) -> LaidOutDocument {
        let mut factory = DocumentFactory::new();
        let mut glyphs = TestGlyphShaper::new();
        factory.parse_with_new_pipeline(html, css).shape(&mut glyphs).expect("test glyphs shape").layout(LayoutConstraints::new(width, 16.0).unwrap())
    }

    fn layout_html_with_viewport_height(html: &str, width: f64, height: f64) -> LaidOutDocument {
        let mut factory = DocumentFactory::new();
        let mut glyphs = TestGlyphShaper::new();
        let constraints = LayoutConstraints::new(width, 16.0).unwrap().with_viewport_height(Some(height)).unwrap();
        factory.parse_with_new_pipeline(html, None).shape(&mut glyphs).expect("test glyphs shape").layout(constraints)
    }

    fn layout_html_with_x_height(html: &str, width: f64, x_height_ratio: f32) -> LaidOutDocument {
        let mut factory = DocumentFactory::new();
        let mut glyphs = TestGlyphShaper::with_x_height_ratio(x_height_ratio);
        factory.parse_with_new_pipeline(html, None).shape(&mut glyphs).expect("test glyphs shape").layout(LayoutConstraints::new(width, 16.0).unwrap())
    }

    fn box_size_by_id(document: &LaidOutDocument, id: &str) -> kurbo::Size {
        let view = document.render_view();
        let index = (0..view.boxes().len()).find(|&index| view.boxes().id(index).map(|value| view.string(value)) == Some(id)).unwrap_or_else(|| panic!("layout box #{id} should exist"));
        view.boxes().size(index).expect("box index in range")
    }

    fn box_point_by_id(document: &LaidOutDocument, id: &str) -> kurbo::Point {
        let view = document.render_view();
        let index = (0..view.boxes().len()).find(|&index| view.boxes().id(index).map(|value| view.string(value)) == Some(id)).unwrap_or_else(|| panic!("layout box #{id} should exist"));
        view.boxes().point(index).expect("box index in range")
    }

    fn decoration_rects_by_color(document: &LaidOutDocument, color: u32) -> Vec<kurbo::Rect> {
        document.render_view().fragments().decorations().iter().filter(|fragment| fragment.color() == color).map(|fragment| fragment.rect()).collect()
    }

    fn decoration_indices_by_color(document: &LaidOutDocument, color: u32) -> Vec<usize> {
        document.render_view().fragments().decorations().iter().enumerate().filter(|(_, fragment)| fragment.color() == color).map(|(index, _)| index).collect()
    }

    #[test]
    fn parallel_cell_measurement_matches_sequential_layout() {
        let html = "<html><body style='margin:0'><table style='width:320px;border-collapse:collapse'><tr><td id='a' style='padding:3px;border:1px solid'><div style='height:50%;overflow:hidden'>alpha alpha alpha alpha</div></td><td id='b' style='padding:4px;border:1px solid'><div style='height:50%;overflow:hidden'>beta beta beta beta</div></td></tr><tr><td id='c' style='padding:2px;border:1px solid'><div style='height:50%;overflow:hidden'>gamma gamma gamma</div></td><td id='d' style='padding:5px;border:1px solid'><div style='height:50%;overflow:hidden'>delta delta delta</div></td></tr><tr><td id='e'><div style='height:50%;overflow:hidden'>epsilon epsilon</div></td><td id='f'><div style='height:50%;overflow:hidden'>zeta zeta</div></td></tr><tr><td id='g'><div style='height:50%;overflow:hidden'>eta eta</div></td><td id='h'><div style='height:50%;overflow:hidden'>theta theta</div></td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let mut glyphs = TestGlyphShaper::new();
        let shaped = factory
            .parse_with_new_pipeline(html, None)
            .shape(&mut glyphs)
            .expect("test glyphs shape");
        let sequential = shaped
            .clone()
            .layout(LayoutConstraints::new(360.0, 16.0).unwrap());
        let parallel = shaped.layout(
            LayoutConstraints::new(360.0, 16.0)
                .unwrap()
                .with_parallel_workers(4),
        );

        let sequential_view = sequential.render_view();
        let parallel_view = parallel.render_view();
        assert_eq!(sequential_view.boxes().len(), parallel_view.boxes().len());
        for box_idx in 0..sequential_view.boxes().len() {
            assert_eq!(sequential_view.boxes().point(box_idx), parallel_view.boxes().point(box_idx));
            assert_eq!(sequential_view.boxes().size(box_idx), parallel_view.boxes().size(box_idx));
        }
        let sequential_lines = sequential_view.text().lines().iter().map(|line| (line.owner_box_idx(), line.glyphs(), line.point(), line.height(), line.baseline())).collect::<Vec<_>>();
        let parallel_lines = parallel_view.text().lines().iter().map(|line| (line.owner_box_idx(), line.glyphs(), line.point(), line.height(), line.baseline())).collect::<Vec<_>>();
        assert_eq!(sequential_lines, parallel_lines);
        let sequential_decorations = sequential_view.fragments().decorations().iter().map(|fragment| (fragment.rect(), fragment.color())).collect::<Vec<_>>();
        let parallel_decorations = parallel_view.fragments().decorations().iter().map(|fragment| (fragment.rect(), fragment.color())).collect::<Vec<_>>();
        assert_eq!(sequential_decorations, parallel_decorations);
    }

    #[test]
    fn direct_cells_before_a_row_group_form_the_same_rows_as_native_table_markup() {
        let document = layout_html(
            "<html><body style='margin:0;font:32px/normal monospace'><div id='candidate' style='display:table;border-spacing:0'><span id='candidate-1' style='display:table-cell'>Row 1, Col 1</span><span style='display:table-cell'>Row 1, Col 2</span><span style='display:table-cell'>Row 1, Col 3</span><span style='display:table-row-group'><span style='display:table-row'><span id='candidate-2' style='display:table-cell'>Row 22, Col 1</span><span style='display:table-cell'>Row 22, Col 2</span><span style='display:table-cell'>Row 22, Col 3</span></span><span style='display:table-row'><span id='candidate-3' style='display:table-cell'>Row 333, Col 1</span><span style='display:table-cell'>Row 333, Col 2</span><span style='display:table-cell'>Row 333, Col 3</span></span></span></div><table id='native' style='border-spacing:0'><tr><td id='native-1' style='padding:0'>Row 1, Col 1</td><td style='padding:0'>Row 1, Col 2</td><td style='padding:0'>Row 1, Col 3</td></tr><tr><td id='native-2' style='padding:0'>Row 22, Col 1</td><td style='padding:0'>Row 22, Col 2</td><td style='padding:0'>Row 22, Col 3</td></tr><tr><td id='native-3' style='padding:0'>Row 333, Col 1</td><td style='padding:0'>Row 333, Col 2</td><td style='padding:0'>Row 333, Col 3</td></tr></table></body></html>",
            800.0,
        );
        let candidate = box_point_by_id(&document, "candidate");
        let native = box_point_by_id(&document, "native");
        for row in 1..=3 {
            let candidate_cell = box_point_by_id(&document, &format!("candidate-{row}"));
            let native_cell = box_point_by_id(&document, &format!("native-{row}"));
            let candidate_start = candidate_cell.y - candidate.y;
            let native_start = native_cell.y - native.y;
            assert!((candidate_start - native_start).abs() < 0.01, "row {row} start: candidate={candidate_start}, native={native_start}");
            let candidate_size = box_size_by_id(&document, &format!("candidate-{row}"));
            let native_size = box_size_by_id(&document, &format!("native-{row}"));
            let candidate_height = candidate_size.height;
            let native_height = native_size.height;
            assert!((candidate_height - native_height).abs() < 0.01, "row {row} height: candidate={candidate_height}, native={native_height}");
            let candidate_x = candidate_cell.x - candidate.x;
            let native_x = native_cell.x - native.x;
            assert!((candidate_x - native_x).abs() < 0.01, "row {row} x: candidate={candidate_x}, native={native_x}");
            assert!((candidate_size.width - native_size.width).abs() < 0.01, "row {row} width: candidate={candidate_size:?}, native={native_size:?}");
        }
        assert!((box_size_by_id(&document, "candidate").width - box_size_by_id(&document, "native").width).abs() < 0.01);
    }

    #[test]
    fn column_group_background_covers_the_entire_assigned_grid() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='table' style='border-spacing:0;width:100px'><colgroup style='background:#0000ff'><col style='width:50px'><col style='width:50px'></colgroup><tr id='row' style='height:20px;padding:0'><td id='c1' style='height:20px;width:50px;padding:0'><div style='height:20px'></div></td><td id='c2' style='height:20px;width:50px;padding:0'><div style='height:20px'></div></td></tr></table></body></html>",
            300.0,
        );

        let table_point = box_point_by_id(&document, "table");
        let table_size = box_size_by_id(&document, "table");
        let group_rects = decoration_rects_by_color(&document, 0x0000ffff);
        assert_eq!(group_rects.len(), 1, "the group should emit one background fragment");
        let group = group_rects[0];
        assert!((group.x0 - table_point.x).abs() < 0.01, "group x should align to table");
        assert!((group.y0 - table_point.y).abs() < 0.01, "group y should align to table");
        assert!((group.width() - table_size.width).abs() < 0.01, "group width should cover the full grid width");
        assert!((group.height() - table_size.height).abs() < 0.01, "group height should cover the full grid height");
    }

    #[test]
    fn column_group_max_height_does_not_clip_background_coverage() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='table' style='border-spacing:0;width:100px'><colgroup style='background:#ff0000;max-height:0in'><col style='width:50px'><col style='width:50px'></colgroup><tr style='height:20px'><td style='height:20px;width:50px'><div style='height:20px'></div></td><td style='height:20px;width:50px'><div style='height:20px'></div></td></tr></table></body></html>",
            300.0,
        );

        let table_size = box_size_by_id(&document, "table");
        let red = decoration_rects_by_color(&document, 0xff0000ff);
        assert_eq!(red.len(), 1, "group background should remain visible");
        assert!((red[0].height() - table_size.height).abs() < 0.01);
    }

    #[test]
    fn table_background_layers_are_ordered_from_group_to_cell() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='table' style='border-collapse:collapse;width:100px;background:#ff00ff'><colgroup style='background:#0000aa'><col style='background:#00ff00;width:50px'><col style='width:50px'></colgroup><tr id='row' style='height:20px;background:#aa0000'><td id='cell' style='background:#0000ff;height:20px;width:50px'>a</td><td style='height:20px;width:50px'>b</td></tr></table></body></html>",
            300.0,
        );

        let table_indices = decoration_indices_by_color(&document, 0xff00ffff);
        let colgroup_indices = decoration_indices_by_color(&document, 0x0000aaff);
        let column_indices = decoration_indices_by_color(&document, 0x00ff00ff);
        let row_indices = decoration_indices_by_color(&document, 0xaa0000ff);
        let cell_indices = decoration_indices_by_color(&document, 0x0000ffff);
        assert!(!table_indices.is_empty(), "table background should emit");
        assert!(!colgroup_indices.is_empty(), "group background should emit");
        assert!(!column_indices.is_empty(), "column background should emit");
        assert!(!row_indices.is_empty(), "row background should emit");
        assert!(!cell_indices.is_empty(), "cell background should emit");

        let min_row = *row_indices.iter().min().expect("row has at least one fragment");
        let min_column = *column_indices.iter().min().expect("column has at least one fragment");
        let min_cell = *cell_indices.iter().min().expect("cell has at least one fragment");
        let min_group = *colgroup_indices.iter().min().expect("group has at least one fragment");
        let min_table = *table_indices.iter().min().expect("table has at least one fragment");
        assert!(min_table < min_group, "table backgrounds should paint below column-group backgrounds");
        assert!(min_group < min_column, "group backgrounds should paint below column backgrounds");
        assert!(min_column < min_row, "column backgrounds should paint below row backgrounds");
        assert!(min_row < min_cell, "row backgrounds should paint below cell backgrounds");
    }

    #[test]
    fn separate_column_groups_emit_discrete_spans() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='table' style='border-spacing:0;width:100px'><colgroup style='background:#0000aa'><col style='width:40px'></colgroup><colgroup style='background:#00aa00'><col style='width:60px'></colgroup><tr id='row' style='height:20px;padding:0'><td id='c1' style='height:20px;width:40px;padding:0'><div style='height:20px'></div></td><td id='c2' style='height:20px;width:60px;padding:0'><div style='height:20px'></div></td></tr></table></body></html>",
            300.0,
        );

        let table_point = box_point_by_id(&document, "table");
        let table_size = box_size_by_id(&document, "table");
        let blue = decoration_rects_by_color(&document, 0x0000aaff);
        let green = decoration_rects_by_color(&document, 0x00aa00ff);
        assert_eq!(blue.len(), 1, "first group emits one span");
        assert_eq!(green.len(), 1, "second group emits one span");
        assert!((blue[0].x0 - table_point.x).abs() < 0.01);
        assert!((blue[0].width() - 40.0).abs() < 0.01);
        assert!((green[0].x0 - (table_point.x + 40.0)).abs() < 0.01);
        assert!((green[0].width() - 60.0).abs() < 0.01);
        assert!((blue[0].width() + green[0].width() - table_size.width).abs() < 0.01);
    }

    #[test]
    fn standalone_column_span_paints_multiple_column_tracks() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='table' style='border-spacing:0;width:120px'><col style='background:#0000aa;width:60px' span='2'><tr id='row' style='height:20px;padding:0'><td id='c1' style='height:20px;width:60px;padding:0'><div style='height:20px'></div></td><td id='c2' style='height:20px;width:60px;padding:0'><div style='height:20px'></div></td></tr></table></body></html>",
            300.0,
        );

        let table_point = box_point_by_id(&document, "table");
        let table_size = box_size_by_id(&document, "table");
        let mut blue = decoration_rects_by_color(&document, 0x0000aaff);
        blue.sort_by(|left, right| left.x0.total_cmp(&right.x0));
        assert_eq!(blue.len(), 2, "standalone span should create one paint fragment per expanded track");
        assert!((blue[0].x0 - table_point.x).abs() < 0.01);
        assert!((blue[0].width() - 60.0).abs() < 0.01);
        assert!((blue[1].x0 - (table_point.x + 60.0)).abs() < 0.01);
        assert!((blue[1].width() - 60.0).abs() < 0.01);
        let total = blue.iter().map(|rect| rect.width()).sum::<f64>();
        assert!((total - table_size.width).abs() < 0.01);
    }

    #[test]
    fn placements_assign_columns_in_row_order() {
        let rows = vec![vec![(10usize, 2usize, 1usize), (11usize, 1usize, 2usize)], vec![(20usize, 1usize, 1usize), (21usize, 1usize, 1usize)]];
        let TableGrid { placements, column_count: cols } = build_table_grid(rows);
        assert_eq!(cols, 3);
        assert_eq!(placements.len(), 4);
        let first: TableCellPlacement = placements[0];
        let second: TableCellPlacement = placements[1];
        let third: TableCellPlacement = placements[2];
        let fourth: TableCellPlacement = placements[3];
        assert_eq!((first.row, first.col, first.colspan, first.cell_idx), (0, 0, 2, 10));
        assert_eq!((second.row, second.col, second.rowspan, second.cell_idx), (0, 2, 2, 11));
        assert_eq!((third.row, third.col, third.cell_idx), (1, 0, 20));
        assert_eq!((fourth.row, fourth.col, fourth.cell_idx), (1, 1, 21));
    }

    #[test]
    fn placements_respect_rowspan_reserved_slots() {
        let rows = vec![vec![(10usize, 1usize, 2usize), (11usize, 1usize, 1usize)], vec![(20usize, 1usize, 1usize)]];
        let TableGrid { placements, column_count: cols } = build_table_grid(rows);
        assert_eq!(cols, 2);
        assert_eq!(placements.len(), 3);
        assert_eq!((placements[0].row, placements[0].col), (0, 0));
        assert_eq!((placements[1].row, placements[1].col), (0, 1));
        assert_eq!((placements[2].row, placements[2].col), (1, 1));
    }

    #[test]
    fn auto_table_shrink_wraps_its_column_grid() {
        let document = layout_html(
            "<html><body><table id='table' style='border-spacing:0'><tr><td id='a' style='padding:0'><div style='width:20px;height:10px'></div></td><td id='b' style='padding:0'><div style='width:30px;height:10px'></div></td></tr></table></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "table").width - 50.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "a").width - 20.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "b").width - 30.0).abs() < 0.01);
    }

    #[test]
    fn auto_colspan_distributes_percentage_and_minimum_across_empty_columns() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='table' style='border-spacing:8px'><tr><td id='a' style='padding:0'></td><td style='padding:0'></td><td style='padding:0'>x</td></tr><tr><td colspan='2' style='width:20%;padding:0'><div style='width:100px'></div></td></tr></table></body></html>",
            800.0,
        );

        assert!((box_size_by_id(&document, "table").width - 492.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "a").width - 46.0).abs() < 0.01);
    }

    #[test]
    fn auto_colspan_minimum_uses_percentage_column_weights() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='table' style='border-spacing:8px'><tr><td id='a' style='width:25%;padding:0'><div style='width:50px'></div></td><td id='b' style='width:25%;padding:0'><div style='width:30px'></div></td><td style='padding:0'>x</td></tr><tr><td colspan='2' style='padding:0'><div style='width:300px'></div></td></tr></table></body></html>",
            800.0,
        );

        assert!((box_size_by_id(&document, "table").width - 616.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "a").width - 146.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "b").width - 146.0).abs() < 0.01);
    }

    #[test]
    fn full_percentage_auto_columns_make_the_table_use_available_width() {
        let document = layout_html(
            "<html><body style='margin:0'><div style='width:500px'><table id='table' style='border-spacing:8px'><tr><td id='a' style='width:50%;padding:0'><div style='width:100px'></div></td><td id='b' style='width:50%;padding:0'><div style='width:100px'></div></td><td style='width:100px;padding:0'><div style='width:100px'></div></td></tr></table></div></body></html>",
            800.0,
        );

        assert!((box_size_by_id(&document, "table").width - 500.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "a").width - 184.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "b").width - 184.0).abs() < 0.01);
    }

    #[test]
    fn rows_without_columns_fill_the_inner_table_width_and_share_its_height() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <table style='box-sizing:border-box;width:60px;height:60px;border-spacing:10px'><tr id='one'></tr></table>
                <table style='box-sizing:border-box;width:60px;height:60px;border:5px solid;border-spacing:10px'><tr id='first'></tr><tr id='second'></tr></table>
            </body></html>",
            800.0,
        );

        assert_eq!(box_size_by_id(&document, "one"), kurbo::Size::new(60.0, 40.0));
        assert_eq!(box_size_by_id(&document, "first"), kurbo::Size::new(50.0, 10.0));
        assert_eq!(box_size_by_id(&document, "second"), kurbo::Size::new(50.0, 10.0));
    }

    #[test]
    fn empty_row_group_keeps_its_authored_height() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='table' style='border-collapse:collapse'><tbody id='group' style='height:75px'></tbody></table></body></html>",
            800.0,
        );

        assert_eq!(box_size_by_id(&document, "group").height, 75.0);
        assert_eq!(box_size_by_id(&document, "table").height, 75.0);
    }

    #[test]
    fn auto_layout_merges_empty_tracks_but_fixed_layout_retains_authored_columns() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <table id='auto' style='border:10px solid;border-spacing:20px'><tr><td id='span' colspan='10' style='width:50px;height:10px;padding:0'></td><td style='width:50px;padding:0'></td></tr></table>
                <table id='fixed' style='box-sizing:border-box;table-layout:fixed;width:130px;border:10px solid;border-spacing:20px'><col span='10'><tr><td style='width:50px;padding:0'></td><td style='width:50px;padding:0'></td></tr></table>
            </body></html>",
            800.0,
        );

        assert_eq!(box_size_by_id(&document, "auto").width, 180.0);
        assert_eq!(box_size_by_id(&document, "span").width, 50.0);
        assert_eq!(box_size_by_id(&document, "fixed").width, 340.0);
    }

    #[test]
    fn table_intrinsic_width_trims_collapsible_inline_edge_whitespace() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <table id='compact' style='border-spacing:0'><tr><td style='padding:0'><span>word</span></td></tr></table>
                <table id='indented' style='border-spacing:0'><tr><td style='padding:0'>
                    <span style='padding-top:10em'><span>word</span></span>
                </td></tr></table>
            </body></html>",
            300.0,
        );

        assert_eq!(box_size_by_id(&document, "indented").width, box_size_by_id(&document, "compact").width);
    }

    #[test]
    fn non_box_table_column_ex_width_uses_shaped_x_height() {
        let document =
            layout_html_with_x_height("<html><body style='margin:0'><table id='table' style='border-spacing:0'><col style='font-size:20px;width:2ex'><tr><td id='cell' style='padding:0'></td></tr></table></body></html>", 300.0, 0.8);

        assert!((box_size_by_id(&document, "table").width - 32.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "cell").width - 32.0).abs() < 0.01);
    }

    #[test]
    fn definite_column_width_survives_cell_max_content_measurement() {
        let document = layout_html_css(
            "<html><body style='margin:0'><table id='table'><col id='middle'><tr><td id='cell' style='padding:0'>Filler Text Filler Text Filler Text</td></tr></table></body></html>",
            Some("table { border-spacing:0 } col#middle { width:80px }"),
            500.0,
        );

        assert!((box_size_by_id(&document, "table").width - 80.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "cell").width - 80.0).abs() < 0.01);
    }

    #[test]
    fn table_cell_and_inline_block_use_the_same_text_line_height() {
        let inline_block = layout_html(
            "<html><body><p>Test passes if the last line is aligned.</p><div><span style='font-size:16px'>--&gt; <span id='content' style='display:inline-block;width:80px'>Filler Text Filler Text Filler Text Filler Text Filler Text Filler Text</span> &lt;--</span></div></body></html>",
            500.0,
        );
        let table_cell = layout_html(
            "<html><body><p>Test passes if the last line is aligned.</p><table style='border-spacing:0'><col><col style='width:80px'><col><tr><td style='padding:0;vertical-align:bottom'>--&gt;&nbsp;</td><td id='content' style='padding:0'>Filler Text Filler Text Filler Text Filler Text Filler Text Filler Text</td><td style='padding:0;vertical-align:bottom'>&nbsp;&lt;--</td></tr></table></body></html>",
            500.0,
        );

        let inline_height = box_size_by_id(&inline_block, "content").height;
        let cell_height = box_size_by_id(&table_cell, "content").height;
        assert!((inline_height - cell_height).abs() < 0.01, "inline block={inline_height}, table cell={cell_height}");
        let inline_y = box_point_by_id(&inline_block, "content").y;
        let cell_y = box_point_by_id(&table_cell, "content").y;
        assert!((inline_y - cell_y).abs() < 0.01, "inline block y={inline_y}, table cell y={cell_y}");
    }

    #[test]
    fn box_dimensions_and_border_ex_units_use_shaped_x_height() {
        let document = layout_html_with_x_height("<html><body style='margin:0'><div id='box' style='box-sizing:border-box;font-size:20px;width:4ex;height:3ex;border:1ex solid'></div></body></html>", 300.0, 0.8);

        let size = box_size_by_id(&document, "box");
        assert!((size.width - 64.0).abs() < 0.01);
        assert!((size.height - 48.0).abs() < 0.01);
    }

    #[test]
    fn separated_border_spacing_contributes_to_intrinsic_table_width() {
        let document = layout_html(
            "<html><body><table id='table' style='border-spacing:2px'><tr><td style='padding:0'><div style='width:10px;height:10px'></div></td><td style='padding:0'><div style='width:10px;height:10px'></div></td></tr></table></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "table").width - 26.0).abs() < 0.01, "two columns have three separated-border gaps");
    }

    #[test]
    fn table_cell_margins_do_not_contribute_to_the_column_grid() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='table' style='display:table;border-spacing:0'><div style='display:table-row'><div id='cell' style='display:table-cell;margin:40px 50px 60px 70px;border-right:10px solid blue;height:20px'></div></div></div></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "table").width - 10.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "cell").width - 10.0).abs() < 0.01);
        assert!((box_point_by_id(&document, "cell").x - box_point_by_id(&document, "table").x).abs() < 0.01);
    }

    #[test]
    fn collapsed_single_cell_grid_matches_zero_spacing_separate_geometry() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='collapsed'><table id='collapsed-table' style='border-collapse:collapse'><tr><td style='box-sizing:content-box;width:100px;height:30px;border:4px solid orange'>cell</td></tr></table></div><div id='separate'><table id='separate-table' style='border-spacing:0'><tr><td style='box-sizing:content-box;width:100px;height:30px;border:4px solid orange'>cell</td></tr></table></div></body></html>",
            300.0,
        );
        assert_eq!(box_size_by_id(&document, "collapsed-table"), box_size_by_id(&document, "separate-table"));
        assert_eq!(box_size_by_id(&document, "collapsed"), box_size_by_id(&document, "separate"));
    }

    #[test]
    fn shrink_to_fit_wrapper_contains_complete_collapsed_outer_border() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='wrapper' style='border-right:10px solid orange;float:left'><div id='table' style='border-collapse:collapse;display:table'><div style='display:table-row'><div id='cell' style='border-right:10px solid blue;display:table-cell;height:20px'></div></div></div></div></body></html>",
            300.0,
        );

        assert_eq!(box_size_by_id(&document, "table").width, 10.0);
        assert_eq!(box_size_by_id(&document, "wrapper").width, 20.0);
        let blue = decoration_rects_by_color(&document, 0x0000ffff);
        let orange = decoration_rects_by_color(&document, 0xffa500ff);
        assert_eq!(blue.len(), 1);
        assert_eq!(orange.len(), 1);
        assert_eq!(blue[0].x1, orange[0].x0, "the collapsed table edge must end where the wrapper border begins");
    }

    #[test]
    fn authored_collapsed_table_width_contains_its_outer_borders() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='table' style='border-collapse:collapse;width:100px'><tr><td style='padding:0;border-left:10px solid blue;border-right:10px solid blue;height:20px'></td></tr></table></body></html>",
            300.0,
        );

        assert_eq!(box_size_by_id(&document, "table").width, 100.0);
        let blue = decoration_rects_by_color(&document, 0x0000ffff);
        assert_eq!(blue.iter().map(|rect| rect.x0).fold(f64::INFINITY, f64::min), box_point_by_id(&document, "table").x);
        assert_eq!(blue.iter().map(|rect| rect.x1).fold(f64::NEG_INFINITY, f64::max), box_point_by_id(&document, "table").x + 100.0);
    }

    #[test]
    fn collapsed_content_box_table_adds_its_outer_border_halves() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='block' style='box-sizing:content-box;border-collapse:collapse;width:100px;height:100px;border-style:solid;border-width:20px 40px 40px 20px'><tr><td></td></tr></table><span id='inline' style='display:inline-table;box-sizing:content-box;border-collapse:collapse;width:100px;height:100px;border-style:solid;border-width:20px 40px 40px 20px'><span style='display:table-cell'></span></span></body></html>",
            500.0,
        );

        assert_eq!(box_size_by_id(&document, "block"), kurbo::Size::new(130.0, 130.0));
        assert_eq!(box_size_by_id(&document, "inline"), kurbo::Size::new(130.0, 130.0));
    }

    #[test]
    fn collapsed_table_padding_has_zero_used_value_for_sizing() {
        for box_sizing in ["content-box", "border-box"] {
            let document = layout_html(
                &format!(
                    "<html><body style='margin:0'><div id='wrapper' style='float:left;background:green'><table id='table' style='border-collapse:collapse;box-sizing:{box_sizing};width:100px;height:100px;padding:100px'></table></div></body></html>"
                ),
                400.0,
            );

            assert_eq!(box_size_by_id(&document, "table"), kurbo::Size::new(100.0, 100.0), "{box_sizing}");
            assert_eq!(box_size_by_id(&document, "wrapper"), kurbo::Size::new(100.0, 100.0), "{box_sizing}");
        }
    }

    #[test]
    fn collapsed_border_conflicts_use_width_then_table_source_priority() {
        let equal_width =
            layout_html("<html><body style='margin:0'><table style='border-collapse:collapse;border:4px solid blue'><tr><td style='width:20px;height:10px;padding:0;border:4px solid red'></td></tr></table></body></html>", 200.0);
        assert!(decoration_rects_by_color(&equal_width, 0xff0000ff).len() >= 4, "the cell wins an equal-width conflict");
        assert!(decoration_rects_by_color(&equal_width, 0x0000ffff).is_empty(), "the losing table border is not painted separately");

        let wider_table =
            layout_html("<html><body style='margin:0'><table style='border-collapse:collapse;border:8px solid blue'><tr><td style='width:20px;height:10px;padding:0;border:4px solid red'></td></tr></table></body></html>", 200.0);
        assert!(decoration_rects_by_color(&wider_table, 0x0000ffff).len() >= 4, "border width wins before source priority");
        assert!(decoration_rects_by_color(&wider_table, 0xff0000ff).is_empty(), "the narrower cell border loses the outer conflict");
    }

    #[test]
    fn hidden_collapsed_borders_retain_table_geometry_without_painting() {
        let source = |visibility| format!("<html><body style='margin:0'><table id='table' style='visibility:{visibility};border-collapse:collapse;border:8px solid blue'><tr><td style='width:20px;height:10px;padding:0;border:4px solid red'>A</td></tr></table></body></html>");
        let visible = layout_html(&source("visible"), 200.0);
        let hidden = layout_html(&source("hidden"), 200.0);

        assert_eq!(box_size_by_id(&visible, "table"), box_size_by_id(&hidden, "table"));
        assert!(decoration_rects_by_color(&hidden, 0x0000ffff).is_empty());
        assert!(decoration_rects_by_color(&hidden, 0xff0000ff).is_empty());
    }

    #[test]
    fn anonymous_collapsed_table_emits_cell_border_grid() {
        let document = layout_html(
            "<html><body style='margin:0'><div style='border-collapse:collapse'><div style='display:table-row-group'><div style='display:table-row'><div style='display:table-cell;width:16px;height:16px;padding:0;border:8px solid green'>A</div></div></div></div></body></html>",
            200.0,
        );
        assert!(decoration_rects_by_color(&document, 0x008000ff).len() >= 4, "the anonymous collapsed table must paint the cell's resolved border edges");
    }

    #[test]
    fn inferred_first_row_uses_the_same_column_grid_as_explicit_rows() {
        let cells = |prefix: &str, row: usize| (0..3).map(|column| format!("<span id='{prefix}-{row}-{column}' style='display:table-cell'>Row {row}, Col {column}</span>")).collect::<String>();
        let inferred = format!(
            "<div id='inferred' style='display:table;border-spacing:0'>{}<span style='display:table-row-group'><span style='display:table-row'>{}</span><span style='display:table-row'>{}</span></span></div>",
            cells("i", 1),
            cells("i", 22),
            cells("i", 333)
        );
        let explicit = format!(
            "<div id='explicit' style='display:table;border-spacing:0'><span style='display:table-row'>{}</span><span style='display:table-row'>{}</span><span style='display:table-row'>{}</span></div>",
            cells("e", 1),
            cells("e", 22),
            cells("e", 333)
        );
        let document = layout_html(&format!("<html><body style='margin:0;font:32px monospace'>{inferred}{explicit}</body></html>"), 800.0);

        assert_eq!(box_size_by_id(&document, "inferred").width, box_size_by_id(&document, "explicit").width);
        for row in [1, 22, 333] {
            for column in 0..3 {
                assert_eq!(box_size_by_id(&document, &format!("i-{row}-{column}")).width, box_size_by_id(&document, &format!("e-{row}-{column}")).width, "row {row}, column {column}");
            }
        }
    }

    #[test]
    fn inferred_normal_flow_table_matches_positioned_explicit_table_width() {
        let cells = |prefix: &str, row: usize| (0..3).map(|column| format!("<span id='{prefix}-{row}-{column}' style='display:table-cell'>Row {row}, Col {column}</span>")).collect::<String>();
        let inferred = format!(
            "<div style='position:relative;padding:1px'><div id='inferred-positioned-pair' style='display:table;border-spacing:0'>{}<span style='display:table-row-group'><span style='display:table-row'>{}</span><span style='display:table-row'>{}</span></span></div></div>",
            cells("ip", 1),
            cells("ip", 22),
            cells("ip", 333)
        );
        let explicit = format!(
            "<div style='position:absolute;top:0;padding:1px'><div id='explicit-positioned-pair' style='display:table;border-spacing:0'><span style='display:table-row'>{}</span><span style='display:table-row'>{}</span><span style='display:table-row'>{}</span></div></div>",
            cells("ep", 1),
            cells("ep", 22),
            cells("ep", 333)
        );
        let document = layout_html(&format!("<html><body style='margin:0;font:32px monospace'><div style='position:relative'>{inferred}{explicit}</div></body></html>"), 800.0);

        assert_eq!(box_size_by_id(&document, "inferred-positioned-pair").width, box_size_by_id(&document, "explicit-positioned-pair").width);
        for row in [1, 22, 333] {
            for column in 0..3 {
                assert_eq!(box_size_by_id(&document, &format!("ip-{row}-{column}")).width, box_size_by_id(&document, &format!("ep-{row}-{column}")).width, "row {row}, column {column}");
            }
        }
    }

    #[test]
    fn css_inferred_table_matches_native_table_in_anonymous_objects_059() {
        let document = layout_html(include_str!("../../../../testdata/wpt/css/CSS2/tables/table-anonymous-objects-059.xht"), 800.0);
        let tables = (0..document.box_count()).filter(|&index| matches!(document.box_layout_mode(index), Some(crate::layout_model::LayoutMode::Table(_)))).collect::<Vec<_>>();
        assert_eq!(tables.len(), 2, "the fixture has one CSS table and one native table");
        let boxes = document.render_view().boxes();
        assert_eq!(boxes.size(tables[0]), boxes.size(tables[1]), "the inferred CSS table and equivalent native table must resolve the same used size");
        let rows = tables
            .iter()
            .map(|&table| match document.box_layout_mode(table) {
                Some(crate::layout_model::LayoutMode::Table(table)) => table.rows.clone(),
                _ => unreachable!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(rows[0].len(), rows[1].len());
        for row in 0..rows[0].len() {
            let cells = [rows[0][row], rows[1][row]].map(|row| match document.box_layout_mode(row as usize) {
                Some(crate::layout_model::LayoutMode::TableRow(row)) => row.cells.clone(),
                _ => panic!("table row should retain its cells"),
            });
            assert_eq!(cells[0].len(), cells[1].len());
            for column in 0..cells[0].len() {
                assert_eq!(boxes.size(cells[0][column] as usize), boxes.size(cells[1][column] as usize), "row {row}, column {column}");
                let first_offset = boxes.point(cells[0][column] as usize).unwrap().y - boxes.point(tables[0]).unwrap().y;
                let second_offset = boxes.point(cells[1][column] as usize).unwrap().y - boxes.point(tables[1]).unwrap().y;
                assert!((first_offset - second_offset).abs() < 0.01, "row {row}, column {column} vertical offset: {first_offset} vs {second_offset}");
            }
        }
    }

    #[test]
    fn spanning_cell_constrains_the_shared_column_grid() {
        let document = layout_html(
            "<html><body><table id='table' style='border-spacing:0'><tr><td style='padding:0'><div style='width:20px;height:10px'></div></td><td style='padding:0'><div style='width:20px;height:10px'></div></td></tr><tr><td id='span' colspan='2' style='padding:0'><div style='width:100px;height:10px'></div></td></tr></table></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "table").width - 100.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "span").width - 100.0).abs() < 0.01);
    }

    #[test]
    fn explicit_table_width_is_distributed_across_auto_columns() {
        let document = layout_html(
            "<html><body><table id='table' style='width:100px;border-spacing:0'><tr><td id='a' style='padding:0'><div style='width:10px;height:10px'></div></td><td id='b' style='padding:0'><div style='width:10px;height:10px'></div></td></tr></table></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "table").width - 100.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "a").width - 50.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "b").width - 50.0).abs() < 0.01);
    }

    #[test]
    fn fixed_column_widths_leave_table_surplus_for_auto_columns() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='table' style='display:table;table-layout:fixed;width:192px;border-spacing:0'><div style='display:table-column;width:48px'></div><div style='display:table-column;width:48px'></div><div style='display:table-row'><div id='a' style='display:table-cell'></div><div id='b' style='display:table-cell'></div><div id='auto' style='display:table-cell'></div></div></div></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "a").width - 48.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "b").width - 48.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "auto").width - 96.0).abs() < 0.01);
    }

    #[test]
    fn fixed_layout_ignores_later_rows_while_auto_layout_measures_them() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <table style='table-layout:fixed;width:100px;border-spacing:0'><tr><td id='fixed' style='padding:0'></td><td style='padding:0'></td></tr><tr><td style='padding:0'><div style='width:200px'></div></td><td></td></tr></table>
                <table style='table-layout:auto;width:100px;border-spacing:0'><tr><td id='auto-mode' style='padding:0'></td><td style='padding:0'></td></tr><tr><td style='padding:0'><div style='width:200px'></div></td><td></td></tr></table>
                <table style='table-layout:fixed;border-spacing:0'><tr><td id='fixed-auto-width' style='padding:0'></td><td style='padding:0'></td></tr><tr><td style='padding:0'><div style='width:200px'></div></td><td></td></tr></table>
            </body></html>",
            500.0,
        );

        assert!((box_size_by_id(&document, "fixed").width - 50.0).abs() < 0.01);
        assert!(box_size_by_id(&document, "auto-mode").width >= 200.0);
        assert!(box_size_by_id(&document, "fixed-auto-width").width >= 200.0, "fixed layout with auto table width falls back to auto layout");
    }

    #[test]
    fn auto_width_redistribution_uses_atomic_minimums_and_percentage_guesses() {
        let document = layout_html_css(
            "<html><body style='margin:0'>
                <table id='tight' style='width:50px'><tr>
                    <td id='tight-a' style='width:100px'><div style='width:50px'></div><div style='width:50px'></div></td>
                    <td id='tight-b' style='width:100px'><div style='width:50px'></div><div style='width:25px'></div></td>
                </tr></table>
                <table id='maximum' style='width:max-content'><tr>
                    <td id='maximum-auto'><div style='width:50px'></div><div style='width:50px'></div></td>
                    <td style='width:100px'><div style='width:50px'></div><div style='width:25px'></div></td>
                    <td id='maximum-percent' style='width:20%'><div style='width:50px'></div><div style='width:25px'></div></td>
                </tr></table>
            </body></html>",
            Some("table{border-spacing:8px}td{padding:0}td>div{display:inline-block}"),
            800.0,
        );

        assert_eq!(box_size_by_id(&document, "tight").width, 124.0);
        assert_eq!(box_size_by_id(&document, "tight-a").width, 50.0);
        assert_eq!(box_size_by_id(&document, "tight-b").width, 50.0);
        assert_eq!(box_size_by_id(&document, "maximum").width, 307.0);
        assert!((box_size_by_id(&document, "maximum-auto").width - 120.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "maximum-percent").width - 55.0).abs() < 0.01);
    }

    #[test]
    fn fixed_width_redistribution_uses_fixed_percentage_auto_priority() {
        let document = layout_html_css(
            "<html><body style='margin:0'>
                <table style='width:500px'><tr>
                    <td id='auto-1'><div style='width:10px'></div></td><td id='auto-2'><div style='width:20px'></div></td>
                    <td id='auto-3'><div style='width:30px'></div></td><td id='auto-4'><div style='width:40px'></div></td>
                    <td id='auto-5'><div style='width:120px'></div></td>
                </tr></table>
                <table style='width:100px'><tr>
                    <td id='percent-2' style='width:200%'></td><td id='percent-3' style='width:300%'></td><td id='percent-5' style='width:500%'></td>
                </tr></table>
                <table style='width:100px'><tr><td id='zero' style='width:0'></td><td id='ordinary-auto'></td></tr></table>
            </body></html>",
            Some("table{table-layout:fixed;border-spacing:0}td{padding:0}td>div{display:inline-block}"),
            800.0,
        );

        for id in ["auto-1", "auto-2", "auto-3", "auto-4", "auto-5"] {
            assert!((box_size_by_id(&document, id).width - 100.0).abs() < 0.01, "{id}");
        }
        assert!((box_size_by_id(&document, "percent-2").width - 20.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "percent-3").width - 30.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "percent-5").width - 50.0).abs() < 0.01);
        assert_eq!(box_size_by_id(&document, "zero").width, 0.0);
        assert_eq!(box_size_by_id(&document, "ordinary-auto").width, 100.0);
    }

    #[test]
    fn fixed_percentage_colspan_does_not_duplicate_its_padding_into_tracks() {
        let document = layout_html_css(
            "<html><body style='margin:0'><table style='width:448px'><tr>
                <td id='span-40' colspan='2' style='width:40%'></td><td id='span-20' colspan='2' style='width:20%'></td><td id='single-40' style='width:40%;box-sizing:border-box'></td>
                </tr><tr><td id='track-1'></td><td id='track-2'></td><td id='track-3'></td><td id='track-4'></td><td id='track-5'></td></tr></table></body></html>",
            Some("table{table-layout:fixed;border-spacing:8px}td{padding:6px;box-sizing:content-box}"),
            800.0,
        );

        assert_eq!(box_size_by_id(&document, "span-40").width, 168.0);
        assert_eq!(box_size_by_id(&document, "span-20").width, 88.0);
        assert_eq!(box_size_by_id(&document, "single-40").width, 160.0);
        for (id, expected) in [("track-1", 80.0), ("track-2", 80.0), ("track-3", 40.0), ("track-4", 40.0), ("track-5", 160.0)] {
            assert!((box_size_by_id(&document, id).width - expected).abs() < 0.01, "{id}");
        }
    }

    #[test]
    fn row_group_height_grows_auto_rows_in_proportion_to_natural_height() {
        let document = layout_html(
            "<html><body style='margin:0'><table style='border-spacing:0'><tbody id='group' style='height:100px'><tr id='short'><td style='padding:0'><div style='height:10px'></div></td></tr><tr id='tall'><td style='padding:0'><div style='height:30px'></div></td></tr></tbody></table></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "group").height - 100.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "short").height - 25.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "tall").height - 75.0).abs() < 0.01);
    }

    #[test]
    fn percentage_rows_resolve_against_their_definite_row_group_height() {
        let document = layout_html(
            "<html><body style='margin:0'><table style='border-spacing:0'><tbody style='height:100px'><tr id='quarter' style='height:25%'><td style='padding:0'></td></tr><tr id='half' style='height:50%'><td style='padding:0'></td></tr><tr id='remainder'><td style='padding:0'></td></tr></tbody></table></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "quarter").height - 25.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "half").height - 50.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "remainder").height - 25.0).abs() < 0.01);
    }

    #[test]
    fn mixed_length_percentage_calc_is_not_a_table_cell_width_hint() {
        let document = layout_html(
            "<html><body style='margin:0'>
                <table style='table-layout:fixed;width:500px;border-spacing:0'><tr><td id='calc' style='padding:0;width:calc(50% + 1px)'>x</td><td style='padding:0;width:100px'>y</td></tr></table>
                <table style='table-layout:fixed;width:500px;border-spacing:0'><tr><td id='auto' style='padding:0'>x</td><td style='padding:0;width:100px'>y</td></tr></table>
            </body></html>",
            800.0,
        );

        assert!((box_size_by_id(&document, "calc").width - box_size_by_id(&document, "auto").width).abs() < 0.01);
    }

    #[test]
    fn table_column_min_width_constrains_its_track() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='table' style='display:table;border-spacing:0'><div style='display:table-column;min-width:96px'></div><div style='display:table-row'><div id='cell' style='display:table-cell;height:96px;background:black'></div></div></div></body></html>",
            300.0,
        );

        let table_width = box_size_by_id(&document, "table").width;
        let cell_width = box_size_by_id(&document, "cell").width;
        assert!((table_width - 96.0).abs() < 0.01, "table width was {table_width}");
        assert!((cell_width - 96.0).abs() < 0.01, "cell width was {cell_width}");
    }

    #[test]
    fn table_column_group_min_width_constrains_its_track() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='table' style='display:table;border-spacing:0'><div style='display:table-column-group;min-width:96px'></div><div style='display:table-row'><div id='cell' style='display:table-cell;height:96px;background:black'></div></div></div></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "table").width - 96.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "cell").width - 96.0).abs() < 0.01);
    }

    #[test]
    fn table_column_max_width_constrains_its_authored_width() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='table' style='display:table;table-layout:fixed;border-spacing:0'><div style='display:table-column;width:288px;max-width:96px'></div><div style='display:table-row'><div id='cell' style='display:table-cell;height:96px;background:black'></div></div></div></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "table").width - 96.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "cell").width - 96.0).abs() < 0.01);
    }

    #[test]
    fn table_cell_max_width_constrains_its_authored_width() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='table' style='display:table;table-layout:fixed;border-spacing:0'><div style='display:table-row'><div id='cell' style='display:table-cell;width:288px;max-width:96px;height:96px;background:black'></div></div></div></body></html>",
            300.0,
        );

        let table_width = box_size_by_id(&document, "table").width;
        let cell_width = box_size_by_id(&document, "cell").width;
        assert!((table_width - 96.0).abs() < 0.01, "table width was {table_width}");
        assert!((cell_width - 96.0).abs() < 0.01, "cell width was {cell_width}");
    }

    #[test]
    fn percentage_sized_table_uses_its_flexed_assigned_width() {
        let document = layout_html(
            "<html><body><div style='display:flex;width:200px'><div id='left' style='width:200px'></div><table id='table' style='width:100%;border-spacing:0'><tr><td id='a' style='padding:0'><div style='width:10px;height:10px'></div><div style='width:10px;height:10px'></div></td><td id='b' style='padding:0'><div style='width:10px;height:10px'></div><div style='width:10px;height:10px'></div></td><td style='padding:0'><div style='width:10px;height:10px'></div><div style='width:10px;height:10px'></div></td><td style='padding:0'><div style='width:10px;height:10px'></div><div style='width:10px;height:10px'></div></td><td style='padding:0'><div style='width:10px;height:10px'></div><div style='width:10px;height:10px'></div></td></tr></table></div></body></html>",
            300.0,
        );

        let table_width = box_size_by_id(&document, "table").width;
        let left_width = box_size_by_id(&document, "left").width;
        assert!((table_width - 100.0).abs() < 0.01, "percentage table should receive half of the flex row, got table={table_width}, left={left_width}");
        assert!((box_size_by_id(&document, "a").width - 20.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "b").width - 20.0).abs() < 0.01);
    }

    #[test]
    fn nested_full_width_table_keeps_auto_outer_table_from_collapsing() {
        let document = layout_html(
            "<html><body><div style='width:100px'>
                <div style='display:table'>
                    <div style='display:table-cell'>
                        <div id='flex' style='display:flex;height:100px'>
                            <table style='width:100%'></table>
                        </div>
                    </div>
                </div>
            </div></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "flex").width - 100.0).abs() < 0.01);
    }

    #[test]
    fn improper_table_children_are_wrapped_and_contribute_intrinsic_width() {
        let document = layout_html(
            "<html><body><div style='width:100px'><div id='table' style='display:table;max-width:50%;border-spacing:0'><div style='float:left;width:20px;height:10px'></div><div style='float:left;width:20px;height:10px'></div><div style='float:left;width:20px;height:10px'></div></div></div></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "table").width - 50.0).abs() < 0.01);
    }

    #[test]
    fn row_group_has_geometry_covering_its_rows() {
        let document = layout_html(
            "<html><body><table id='table' style='border-spacing:0'><tbody id='body'><tr id='first'><td style='padding:0'><div style='width:20px;height:10px'></div></td></tr><tr id='second'><td style='padding:0'><div style='width:20px;height:15px'></div></td></tr></tbody></table></body></html>",
            300.0,
        );

        let group = box_size_by_id(&document, "body");
        assert!((group.width - 20.0).abs() < 0.01);
        assert!((group.height - 25.0).abs() < 0.01);
        assert_eq!(box_size_by_id(&document, "first").width, group.width);
        assert_eq!(box_size_by_id(&document, "second").width, group.width);
    }

    #[test]
    fn table_row_geometry_is_stable_across_viewport_heights() {
        let html = "<html><body style='margin:0'><div style='height:350px'></div><table id='table' style='border-spacing:0;width:500px'><tr id='first'><td style='padding:0'><div style='height:190px;break-inside:avoid'></div></td><td style='padding:0'>Independent cell</td></tr><tr id='second'><td id='span' rowspan='2' style='height:180px;padding:0'>rowspan</td><td style='height:90px;padding:0'>upper</td></tr><tr id='third'><td style='height:90px;padding:0'>lower</td></tr></table></body></html>";
        let baseline = layout_html_with_viewport_height(html, 500.0, 900.0);
        let expected = ["table", "first", "second", "third", "span"].map(|id| box_size_by_id(&baseline, id));

        for viewport_height in [420.0, 480.0, 520.0, 600.0, 720.0] {
            let document = layout_html_with_viewport_height(html, 500.0, viewport_height);
            let actual = ["table", "first", "second", "third", "span"].map(|id| box_size_by_id(&document, id));
            for ((id, actual), expected) in ["table", "first", "second", "third", "span"].into_iter().zip(actual).zip(expected) {
                assert!((actual.height - expected.height).abs() < 0.01, "{id} height changed at viewport height {viewport_height}: expected {expected:?}, got {actual:?}");
            }
        }
    }

    #[test]
    fn rowspan_cell_covers_the_rows_and_reserved_column() {
        let document = layout_html(
            "<html><body><table style='border-spacing:0'><tr><td id='span' rowspan='2' style='padding:0'><div style='width:20px;height:30px'></div></td><td id='top' style='padding:0'><div style='width:10px;height:10px'></div></td></tr><tr><td id='bottom' style='padding:0'><div style='width:10px;height:20px'></div></td></tr></table></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "span").height - 30.0).abs() < 0.01);
        assert!((box_point_by_id(&document, "bottom").y - box_point_by_id(&document, "top").y - 10.0).abs() < 0.01);
        assert!((box_point_by_id(&document, "bottom").x - box_point_by_id(&document, "top").x).abs() < 0.01);
    }

    #[test]
    fn rowspan_is_clamped_at_its_row_group_boundary() {
        let document = layout_html(
            "<html><body style='margin:0'><table style='border-spacing:0'><tbody id='first-group'><tr><td id='first' style='padding:0'><div style='height:10px'></div></td><td id='span' rowspan='5' style='padding:0'><div style='height:100px'></div></td></tr><tr><td id='second' style='padding:0'><div style='height:10px'></div></td></tr><tr id='empty'></tr></tbody><tbody><tr id='next-group'><td style='padding:0'><div style='height:20px'></div></td></tr></tbody></table></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "first-group").height - 100.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "first").height - 50.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "second").height - 50.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "empty").height - 0.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "span").height - 100.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "next-group").height - 20.0).abs() < 0.01);
    }

    #[test]
    fn vertical_align_bottom_offsets_short_cell_content() {
        let document = layout_html(
            "<html><body><table style='border-spacing:0'><tr><td style='padding:0'><div style='width:10px;height:20px'></div></td><td id='cell' style='padding:0;vertical-align:bottom'><div id='content' style='width:10px;height:5px'></div></td></tr></table></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "cell").height - 20.0).abs() < 0.01);
        assert!((box_point_by_id(&document, "content").y - box_point_by_id(&document, "cell").y - 15.0).abs() < 0.01);
    }

    #[test]
    fn top_caption_precedes_the_grid() {
        let document = layout_html(
            "<html><body><table id='table' style='border-spacing:0'><caption id='top' style='height:5px'></caption><tr><td id='cell' style='padding:0'><div style='width:20px;height:10px'></div></td></tr></table></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "table").height - 15.0).abs() < 0.01);
        assert!((box_point_by_id(&document, "cell").y - box_point_by_id(&document, "top").y - 5.0).abs() < 0.01);
    }

    #[test]
    fn caption_minimum_width_expands_the_table_grid() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='table' style='border-spacing:0;background:#800080'><caption style='width:190px;height:30px'></caption><tr><td style='padding:0'><div style='width:100px;height:30px'></div></td></tr></table></body></html>",
            300.0,
        );

        assert_eq!(box_size_by_id(&document, "table"), kurbo::Size::new(190.0, 60.0));
        assert_eq!(decoration_rects_by_color(&document, 0x800080ff), vec![kurbo::Rect::new(0.0, 30.0, 190.0, 60.0)]);
    }

    #[test]
    fn table_borders_satisfy_a_narrower_caption_minimum() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='table' style='border:50px solid;border-spacing:10px'><caption><span style='display:inline-block;width:50px'></span><span style='display:inline-block;width:50px'></span></caption></table></body></html>",
            300.0,
        );

        assert_eq!(box_size_by_id(&document, "table").width, 100.0);
    }

    #[test]
    fn percentage_cell_padding_does_not_create_a_cyclic_track_minimum() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='table' style='border-spacing:0'><caption><div style='width:300px'></div></caption><tr><td id='first' style='box-sizing:content-box;width:100px;border:10px solid;padding:30%'><div style='width:50px'></div></td><td style='box-sizing:content-box;width:100px;border:10px solid;padding:30%'><div style='width:50px'></div></td></tr></table></body></html>",
            800.0,
        );

        assert_eq!(box_size_by_id(&document, "table").width, 300.0);
        assert_eq!(box_size_by_id(&document, "first").width, 150.0);
    }

    #[test]
    fn percentage_height_child_resolves_after_cell_intrinsic_measurement() {
        let document = layout_html(
            "<html><body style='margin:0'><table style='border-spacing:0'><tr><td id='cell' style='padding:0;height:100px;font:50px/1 monospace'>y<span id='child' style='display:inline-block;height:100%;width:50px'></span></td></tr></table></body></html>",
            300.0,
        );

        assert_eq!(box_size_by_id(&document, "cell").height, 100.0);
        assert_eq!(box_size_by_id(&document, "child").height, 100.0);
    }

    #[test]
    fn zero_percent_columns_do_not_enlarge_an_auto_table() {
        let document = layout_html_css(
            "<html><body style='margin:0'><table id='table'><col style='width:0%'><col style='width:0%'><tr><td id='first' style='padding:0'><span id='outer' style='display:inline-block'><span id='inner' style='display:inline-block;width:100px'></span></span></td><td id='second' style='padding:0'><span style='display:inline-block'></span></td></tr></table></body></html>",
            Some("table { border-spacing:2px }"),
            400.0,
        );

        assert_eq!(box_size_by_id(&document, "inner").width, 100.0);
        assert_eq!(box_size_by_id(&document, "outer").width, 100.0);
        assert_eq!(
            (
                box_size_by_id(&document, "first").width,
                box_size_by_id(&document, "second").width,
                box_size_by_id(&document, "table").width,
            ),
            (100.0, 0.0, 106.0),
        );
    }

    #[test]
    fn float_with_full_percentage_column_uses_its_available_width() {
        let document = layout_html(
            "<html><body style='margin:0'><div style='box-sizing:border-box;width:600px;border:5px solid'><table id='table' style='float:left;border-spacing:10px'><tr><td style='width:100%;padding:0'><div style='width:30px'></div></td><td style='padding:0'><div style='width:100px'></div></td></tr></table></div></body></html>",
            800.0,
        );

        assert_eq!(box_size_by_id(&document, "table").width, 590.0);
    }

    #[test]
    fn nested_table_ignores_cell_percentage_for_outer_intrinsic_ratio_but_fills_its_cell() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='outer' style='width:300px;border-spacing:0'><tr><td id='outer-first' style='padding:0'><table id='inner' style='border-spacing:0'><tr><td id='inner-cell' style='padding:0;width:1%'><div style='width:20px;height:10px'></div></td></tr></table></td><td id='outer-second' style='padding:0'><div style='width:40px;height:10px'></div></td></tr></table></body></html>",
            500.0,
        );

        assert_eq!(box_size_by_id(&document, "outer-first").width, 100.0);
        assert_eq!(box_size_by_id(&document, "outer-second").width, 200.0);
        assert_eq!(box_size_by_id(&document, "inner").width, 100.0);
        assert_eq!(box_size_by_id(&document, "inner-cell").width, 100.0);
    }

    #[test]
    fn restricted_percentage_height_cell_descendant_uses_definite_table_height() {
        let document = layout_html_css(
            "<html><body style='margin:0'><div id='table' class='table'><div id='scroller' class='cell'><div style='width:100px;height:500px'></div></div></div></body></html>",
            Some(".table{display:table;height:100px}.cell{overflow:auto;width:100px;height:100%}"),
            300.0,
        );

        assert_eq!(box_size_by_id(&document, "table").height, 100.0);
        assert_eq!(box_size_by_id(&document, "scroller").height, 100.0);
    }

    #[test]
    fn caption_only_table_uses_the_caption_as_its_content() {
        let document = layout_html("<html><body style='margin:0'><table id='table' style='border-spacing:0'><caption id='caption' style='height:12px'>Caption</caption></table><div id='after' style='height:1px'></div></body></html>", 300.0);

        assert!((box_size_by_id(&document, "table").height - 12.0).abs() < 0.01);
        assert!((box_point_by_id(&document, "after").y - box_point_by_id(&document, "table").y - 12.0).abs() < 0.01);
    }

    #[test]
    fn table_caption_margins_position_the_caption_and_contribute_to_table_height() {
        let document = layout_html(
            "<html><body style='margin:0'><table id='table' style='border-spacing:0'><caption id='top' style='height:5px;margin:7px 0 11px 50px'></caption><tr><td id='cell' style='padding:0'><div style='width:20px;height:10px'></div></td></tr></table></body></html>",
            300.0,
        );

        let table = box_point_by_id(&document, "table");
        let top = box_point_by_id(&document, "top");
        let cell = box_point_by_id(&document, "cell");
        assert!((top.x - table.x - 50.0).abs() < 0.01);
        assert!((top.y - table.y - 7.0).abs() < 0.01);
        assert!((cell.y - table.y - 23.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "table").height - 33.0).abs() < 0.01);
    }

    #[test]
    fn definite_row_height_is_a_minimum() {
        let document = layout_html(
            "<html><body><table style='border-spacing:0'><tr id='first' style='height:30px'><td style='padding:0'><div style='width:10px;height:10px'></div></td></tr><tr id='second'><td style='padding:0'><div style='width:10px;height:10px'></div></td></tr></table></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "first").height - 30.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "second").height - 10.0).abs() < 0.01);
    }

    #[test]
    fn definite_table_height_is_distributed_to_rows() {
        let document = layout_html(
            "<html><body><table id='table' style='height:60px;border-spacing:0'><tr id='first'><td style='padding:0'><div style='width:10px;height:10px'></div></td></tr><tr id='second'><td style='padding:0'><div style='width:10px;height:10px'></div></td></tr></table></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "table").height - 60.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "first").height - 30.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "second").height - 30.0).abs() < 0.01);
    }

    #[test]
    fn table_min_height_is_distributed_to_an_empty_row_and_cell() {
        let document = layout_html(
            "<html><body style='margin:0'><div id='table' style='display:table;min-height:96px;border-spacing:0'><div id='row' style='display:table-row;height:inherit'><div id='cell' style='display:table-cell;height:inherit;width:96px;background:black'></div></div></div></body></html>",
            300.0,
        );

        assert!((box_size_by_id(&document, "table").height - 96.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "row").height - 96.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "cell").height - 96.0).abs() < 0.01);
        let black = decoration_rects_by_color(&document, 0x000000ff);
        assert_eq!(black.len(), 1);
        assert!((black[0].width() - 96.0).abs() < 0.01);
        assert!((black[0].height() - 96.0).abs() < 0.01);
    }

    #[test]
    fn max_height_limits_table_row_distribution() {
        let document =
            layout_html("<html><body><table id='table' style='height:288px;max-height:96px;border-spacing:0'><tr id='first'><td style='padding:0'>a</td></tr><tr id='second'><td style='padding:0'>b</td></tr></table></body></html>", 300.0);

        assert!((box_size_by_id(&document, "table").height - 96.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "first").height - 48.0).abs() < 0.01);
        assert!((box_size_by_id(&document, "second").height - 48.0).abs() < 0.01);
        let table_bottom = box_point_by_id(&document, "table").y + box_size_by_id(&document, "table").height;
        let second_bottom = box_point_by_id(&document, "second").y + box_size_by_id(&document, "second").height;
        assert!(second_bottom <= table_bottom + 0.01);
    }

    #[test]
    fn stretch_height_fills_a_definite_table_containing_block() {
        let document =
            layout_html("<html><body><div style='height:30px'><table id='table' style='height:stretch;border-spacing:0'><tr><td style='padding:0'><div style='width:20px;height:10px'></div></td></tr></table></div></body></html>", 300.0);

        assert!((box_size_by_id(&document, "table").height - 30.0).abs() < 0.01);
    }

    #[test]
    fn table_intrinsic_block_size_drives_its_flex_wrapper_without_extra_spacing() {
        let document = layout_html_css(
            "<html><body><div class='container'>
                <div class='first'>
                </div>
                <div id='wrapper' class='test'>
                    <table id='table'>
                        <tr><td id='cell'></td></tr>
                    </table>
                </div>
            </div></body></html>",
            Some(".container { display:flex; flex-direction:column; height:100px; width:50px } .first { flex:1 1 auto } .test { flex:0 0 auto; display:flex } td { padding:23px }"),
            300.0,
        );
        let wrapper = box_size_by_id(&document, "wrapper");
        let table = box_size_by_id(&document, "table");
        let cell = box_size_by_id(&document, "cell");

        assert!((table.height - 50.0).abs() < 0.01, "table={table:?}, cell={cell:?}");
        assert!((wrapper.height - 50.0).abs() < 0.01, "wrapper={wrapper:?}, table={table:?}, cell={cell:?}");
    }

    /// Manual release-mode benchmark for table layout architecture changes.
    ///
    /// Run with:
    /// `cargo test --release -p html-layout benchmark_table_relayout -- --ignored --exact --nocapture`
    #[test]
    #[ignore = "manual performance benchmark"]
    fn benchmark_table_relayout() {
        use std::time::Instant;

        let iterations = std::env::var("HTML_LAYOUT_BENCH_ITERATIONS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1_000);
        let warmup = std::env::var("HTML_LAYOUT_BENCH_WARMUP")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(50);
        let case = std::env::var("HTML_LAYOUT_BENCH_CASE")
            .unwrap_or_else(|_| "ordinary".to_owned());
        let with_captions = case == "captioned";
        let dependent_cells = case == "dependent-cells";
        let parallel_cells = case == "parallel-cells";
        let workers = std::env::var("HTML_LAYOUT_BENCH_WORKERS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1)
            .max(1);

        let mut html = String::from("<html><body style='margin:0;font:16px monospace'>");
        for table in 0..16 {
            html.push_str("<table style='width:760px;border-collapse:collapse'>");
            if with_captions {
                html.push_str("<caption style='margin:3px 0'>Measured caption text</caption>");
            }
            for row in 0..8 {
                html.push_str("<tr>");
                for column in 0..6 {
                    if dependent_cells || parallel_cells {
                        html.push_str(&format!("<td style='padding:2px;border:1px solid'><div style='height:50%;overflow:hidden'>T{table} R{row} C{column}"));
                        if parallel_cells {
                            for word in 0..48 {
                                html.push_str(&format!(" measurement{word}"));
                            }
                        }
                        html.push_str("</div></td>");
                    } else {
                        html.push_str(&format!("<td style='padding:2px;border:1px solid'>T{table} R{row} C{column}</td>"));
                    }
                }
                html.push_str("</tr>");
            }
            html.push_str("</table>");
        }
        html.push_str("</body></html>");

        let mut factory = DocumentFactory::new();
        let mut glyphs = TestGlyphShaper::new();
        let shaped = factory.parse_with_new_pipeline(&html, None).shape(&mut glyphs).expect("benchmark glyphs shape");
        let constraints = |width| LayoutConstraints::new(width, 16.0).unwrap().with_parallel_workers(workers);
        let mut document = shaped.layout(constraints(800.0));
        for iteration in 0..warmup {
            let width = if iteration % 2 == 0 { 800.0 } else { 801.0 };
            document.relayout(constraints(width));
        }

        let started = Instant::now();
        for iteration in 0..iterations {
            let width = if iteration % 2 == 0 { 800.0 } else { 801.0 };
            document.relayout(constraints(width));
        }
        let elapsed = started.elapsed();
        let view = document.render_view();
        let checksum = view.boxes().len()
            + view.text().lines().len()
            + view.fragments().decorations().len()
            + view.fragments().images().len();
        std::hint::black_box(checksum);
        println!(
            "TABLE_BENCH case={} workers={} iterations={} total_ns={} ns_per_relayout={} checksum={checksum}",
            case,
            workers,
            iterations,
            elapsed.as_nanos(),
            elapsed.as_nanos() / iterations.max(1) as u128,
        );
    }
}
