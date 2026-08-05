#[cfg(test)]
mod tests {
    use crate::layout_model::{BlockBox, Children, GlyphId, InlineItemKind, LayoutMode};
    use crate::parser::DocumentFactory;

    #[test]
    fn list_style_resolves_and_inherits_by_nesting() {
        use html_style_model::{ListStyleType, PreferredSize};
        let html = "<html><body><ul><li>a<ul><li>b<ul><li>c</li></ul></li></ul></li></ul><ol><li>d</li></ol></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let li_type = |n: usize| -> ListStyleType {
            let mut seen = 0;
            for i in 0..document.box_count() {
                if document.get_tag(i).eq_ignore_ascii_case("li") {
                    if seen == n {
                        let si = document.box_style_indices(i).expect("li style");
                        return document.style_view(si).list_style_type();
                    }
                    seen += 1;
                }
            }
            panic!("li #{n} not found");
        };

        assert_eq!(li_type(0), ListStyleType::Disc);
        assert_eq!(li_type(1), ListStyleType::Circle);
        assert_eq!(li_type(2), ListStyleType::Square);
        assert_eq!(li_type(3), ListStyleType::Decimal);

        let ul_idx = (0..document.box_count()).find(|&i| document.get_tag(i).eq_ignore_ascii_case("ul")).expect("ul box");
        let si = document.box_style_indices(ul_idx).expect("ul style");
        let pad_left = document.style_view(si).padding_left();
        assert!(matches!(pad_left, html_style_model::LengthPct::Px(px) if (px - 40.0).abs() < 0.01), "ul padding-left should be 40px, got {pad_left:?}");
        let _ = PreferredSize::Auto;
    }

    #[test]
    fn br_generates_inline_break_run() {
        let html = "<html><body><p>a<br>b</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        assert!(document.inline_content().inline_items().iter().any(|run| matches!(run.kind, InlineItemKind::Break { .. })));
    }

    #[test]
    fn floated_inline_span_becomes_float_anchor_run() {
        let html = "<html><body><p><span style=\"float:left\">T</span>ext</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        assert!(document.inline_content().inline_items().iter().any(|run| matches!(run.kind, InlineItemKind::FloatAnchor { .. })));

        let span_idx = (0..document.box_count()).find(|&i| document.get_tag(i).eq_ignore_ascii_case("span")).expect("float span box should exist");
        let box_mode = document.box_at(span_idx).map(|box_| box_.layout_mode()).expect("float span box should exist");
        assert!(matches!(box_mode, LayoutMode::Block(_)), "floated inline span should stay a block box");
        let style = document.box_at(span_idx).and_then(|box_| box_.style()).expect("float span should have style");
        assert!(matches!(document.styles().box_model_style(style).expect("validated style handle").float, html_style_model::Float::Left));
    }

    #[test]
    fn floated_first_letter_becomes_a_source_preserving_float_box() {
        let html = "<html><body><p>\u{201c}Text continues across several lines.</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("p::first-letter { float:left; font-size:48px; margin-right:4px }"));

        let float_box = document
            .inline_content()
            .inline_items()
            .iter()
            .find_map(|run| match run.kind {
                InlineItemKind::FloatAnchor { box_idx } => Some(box_idx as usize),
                _ => None,
            })
            .expect("a floated first letter must produce an inline float anchor");
        let box_ = document.box_at(float_box).expect("synthetic first-letter box");
        assert!(matches!(box_.layout_mode(), LayoutMode::Block(_)));
        assert!(box_.dom_element().is_none(), "the pseudo box must not forge a DOM element");
        let style = box_.style().expect("first-letter style");
        assert_eq!(document.styles().box_model_style(style).expect("box style").float, html_style_model::Float::Left);

        let LayoutMode::Block(block) = box_.layout_mode() else { panic!("first-letter float must be blockified") };
        let Children::InlineItems(runs) = &block.children else { panic!("first-letter float needs inline source children") };
        let selected = document.inline_content().inline_items()[runs.start as usize..runs.end as usize]
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(glyphs.clone()),
                _ => None,
            })
            .flatten()
            .map(|glyph| char::from_u32(document.inline_content().glyph_at(glyph as usize).expect("source glyph")).expect("Unicode scalar"))
            .collect::<String>();
        assert_eq!(selected, "\u{201c}T", "leading quote and first typographic letter belong to the float");
    }

    #[test]
    fn absolutely_positioned_inline_span_is_blockified_and_anchored_out_of_inline_flow() {
        let html = "<html><body><div><p><img/><span style='position:absolute;top:0;left:0;width:20px;height:10px'></span></p></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let span_idx = (0..document.box_count())
            .find(|&idx| document.get_tag(idx).eq_ignore_ascii_case("span"))
            .unwrap_or_else(|| panic!("absolute span box; built tags={:?}", (0..document.box_count()).map(|idx| document.get_tag(idx)).collect::<Vec<_>>()));
        assert!(matches!(document.box_at(span_idx).map(|box_| box_.layout_mode()), Some(LayoutMode::Block(_))), "absolute inline boxes must be blockified");
        assert!(
            document.inline_content().inline_items().iter().any(|run| matches!(run.kind, InlineItemKind::AbsoluteAnchor { box_idx } if box_idx as usize == span_idx)),
            "the absolute span needs a zero-width static-position anchor in the surrounding line"
        );
        assert!(!document.inline_content().inline_items().iter().any(|run| run.box_idx as usize == span_idx), "the absolute span must not be flattened into the surrounding line");
    }

    #[test]
    fn blockified_inline_svg_is_one_replaced_image_without_svg_child_boxes() {
        let html = r#"<html><body><div><svg xmlns="http://www.w3.org/2000/svg" style="position:absolute;width:200px;height:96px"><rect width="200" height="100" fill="blue"></rect></svg></div></body></html>"#;
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let svg_idx = (0..document.box_count()).find(|&idx| document.get_tag(idx).eq_ignore_ascii_case("svg")).expect("SVG replaced box");
        assert!(matches!(document.box_at(svg_idx).map(|box_| box_.layout_mode()), Some(LayoutMode::Block(block)) if matches!(block.children, Children::InlineItems(_))));
        assert!(document.inline_content().inline_items().iter().any(|run| run.box_idx as usize == svg_idx && matches!(run.kind, InlineItemKind::Image { .. })));
        assert!(!(0..document.box_count()).any(|idx| document.get_tag(idx).eq_ignore_ascii_case("rect")), "SVG descendants must not enter the CSS box tree");
    }

    #[test]
    fn empty_floated_inline_after_block_content_is_retained_as_a_blockified_float() {
        let html = "<html><body><div></div><span style=\"float:left;width:20px;height:10px\"></span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let span_idx = (0..document.box_count()).find(|&idx| document.get_tag(idx).eq_ignore_ascii_case("span")).expect("float span box should exist");
        assert!(matches!(document.box_at(span_idx).map(|box_| box_.layout_mode()), Some(LayoutMode::Block(_))), "a floated inline computes to a block principal box");
        let style = document.box_at(span_idx).and_then(|box_| box_.style()).expect("float span style");
        assert!(matches!(document.styles().box_model_style(style).expect("validated style handle").float, html_style_model::Float::Left));
    }

    #[test]
    fn block_descendant_splits_inline_and_preserves_its_empty_end_edge() {
        let html = "<html><body><div><span style='border-right:20px solid;margin-right:-20px'><div></div></span></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        assert!(document.inline_content().inline_items().iter().any(|run| { matches!(run.kind, InlineItemKind::InlineBoundary { inline_start: false, inline_end: true }) }));
        let outer = (0..document.box_count()).find(|&idx| document.get_tag(idx).eq_ignore_ascii_case("div") && document.box_at(idx).and_then(|box_| box_.parent()).is_some()).expect("outer div box");
        let Children::Blocks(children) = (match document.box_at(outer).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Block(block)) => &block.children,
            _ => panic!("outer div must establish block flow for its nested block descendant"),
        }) else {
            panic!("split inline must become block-flow siblings")
        };
        assert_eq!(children.len(), 2, "the nested block and trailing anonymous inline continuation must both remain in flow");
    }

    #[test]
    fn nested_inline_ancestors_are_fragmented_around_a_block_descendant() {
        let html = "<html><body><div><span><span>before<p>block</p>after</span></span></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let host = (0..document.box_count()).find(|&idx| document.get_tag(idx).eq_ignore_ascii_case("div")).expect("host div box");
        let Children::Blocks(children) = (match document.box_at(host).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Block(block)) => &block.children,
            _ => panic!("host must establish block flow around the nested paragraph"),
        }) else {
            panic!("nested block-in-inline fixup must produce block-flow siblings")
        };
        assert_eq!(children.len(), 3, "expected leading inline fragment, block, and trailing inline fragment");
        assert!(matches!(document.box_at(children[0] as usize).map(|box_| box_.layout_mode()), Some(LayoutMode::Anonymous(_))));
        assert!(document.get_tag(children[1] as usize).eq_ignore_ascii_case("p"));
        assert!(matches!(document.box_at(children[2] as usize).map(|box_| box_.layout_mode()), Some(LayoutMode::Anonymous(_))));

        let span_fragments = (0..document.box_count()).filter(|&idx| document.get_tag(idx).eq_ignore_ascii_case("span")).count();
        assert_eq!(span_fragments, 4, "both nested inline ancestors must continue after the block");
    }

    #[test]
    fn ordinary_empty_inline_with_horizontal_edges_is_preserved() {
        let html = "<html><body><span style='padding-right:10px;border-top:5px solid blue;border-bottom:5px solid blue'></span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let runs = document.inline_content().inline_items();
        let boundaries = runs
            .iter()
            .filter_map(|run| match run.kind {
                InlineItemKind::InlineBoundary { inline_start, inline_end } => Some((inline_start, inline_end)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(boundaries.contains(&(false, true)), "expected the empty inline's right edge boundary, got {boundaries:?} among {} runs", runs.len());
    }

    #[test]
    fn table_builds_table_row_and_cell_boxes() {
        let html = "<html><body><table><tr><td>A</td><td>B</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let table_idx = (0..document.box_count()).find(|&i| document.get_tag(i).eq_ignore_ascii_case("table")).expect("table box should exist");

        let rows = match document.box_at(table_idx) {
            Some(box_) => match box_.layout_mode() {
                LayoutMode::Table(table) => table.rows.clone(),
                _ => panic!("table should use LayoutMode::Table"),
            },
            None => panic!("table box should exist"),
        };
        assert_eq!(rows.len(), 1);

        let cells = match document.box_at(rows[0] as usize) {
            Some(box_) => match box_.layout_mode() {
                LayoutMode::TableRow(row) => row.cells.clone(),
                _ => panic!("row should use LayoutMode::TableRow"),
            },
            None => panic!("row box should exist"),
        };
        assert_eq!(cells.len(), 2);
        assert!(matches!(document.box_at(cells[0] as usize).map(|b| b.layout_mode()), Some(LayoutMode::TableCell(_))));
        assert!(matches!(document.box_at(cells[1] as usize).map(|b| b.layout_mode()), Some(LayoutMode::TableCell(_))));
    }

    #[test]
    fn table_cell_text_creates_run_children() {
        let html = "<html><body><table><tr><td>Hello</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let cell_idx = (0..document.box_count()).find(|&i| document.get_tag(i).eq_ignore_ascii_case("td")).expect("cell box should exist");

        let children = match document.box_at(cell_idx) {
            Some(box_) => match box_.layout_mode() {
                LayoutMode::TableCell(cell) => &cell.children,
                _ => panic!("cell should use LayoutMode::TableCell"),
            },
            None => panic!("cell box should exist"),
        };
        assert!(matches!(children, Children::InlineItems(_)));
    }

    #[test]
    fn table_source_indentation_does_not_create_anonymous_rows() {
        let html = "<html><body><table>\n  <tr><td>A</td></tr>\n</table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let table_idx = (0..document.box_count()).find(|&i| document.get_tag(i).eq_ignore_ascii_case("table")).expect("table box should exist");
        let rows = match document.box_at(table_idx).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Table(table)) => &table.rows,
            _ => panic!("table should use table layout"),
        };

        assert_eq!(rows.len(), 1, "collapsible whitespace between table rows is not a row");
    }

    #[test]
    fn direct_table_cells_receive_an_anonymous_row_without_an_extra_cell() {
        let html = "<html><body><div id='table' style='display:table;white-space:pre'>\n<div id='cell' style='display:table-cell'>A</div>\n</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let table_idx = (0..document.box_count()).find(|&i| document.get_id(i) == Some("table")).expect("table box should exist");
        let cell_idx = (0..document.box_count()).find(|&i| document.get_id(i) == Some("cell")).expect("cell box should exist");
        let rows = match document.box_at(table_idx).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Table(table)) => &table.rows,
            _ => panic!("table should use table layout"),
        };

        assert_eq!(rows.len(), 1);
        let row_idx = rows[0] as usize;
        let cells = match document.box_at(row_idx).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::TableRow(row)) => &row.cells,
            _ => panic!("anonymous wrapper should be a table row"),
        };
        assert_eq!(cells.as_slice(), &[cell_idx as u32]);
        assert_eq!(document.box_at(cell_idx).and_then(|box_| box_.parent()), Some(row_idx as u32));
    }

    #[test]
    fn direct_row_group_cells_receive_one_anonymous_row() {
        let html = "<html><body><div id='table' style='display:table'><div id='group' style='display:table-row-group'><span id='a' style='display:table-cell'>A</span><span id='b' style='display:table-cell'>B</span><span id='c' style='display:table-cell'>C</span></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let table_idx = (0..document.box_count()).find(|&i| document.get_id(i) == Some("table")).expect("table box");
        let table = match document.box_at(table_idx).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Table(table)) => table,
            _ => panic!("table layout mode"),
        };
        assert_eq!(table.rows.len(), 1);
        let row = match document.box_at(table.rows[0] as usize).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::TableRow(row)) => row,
            _ => panic!("anonymous row"),
        };
        assert_eq!(row.cells.iter().map(|&cell| document.get_id(cell as usize)).collect::<Vec<_>>(), vec![Some("a"), Some("b"), Some("c")]);
    }

    #[test]
    fn non_cell_between_row_group_cells_receives_only_its_own_anonymous_cell() {
        let html = "<html><body><div id='table' style='display:table'><div style='display:table-row-group'><span id='a' style='display:table-cell'>A</span><div id='middle'>B</div><span id='c' style='display:table-cell'>C</span></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let table_idx = (0..document.box_count()).find(|&i| document.get_id(i) == Some("table")).expect("table box");
        let table = match document.box_at(table_idx).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Table(table)) => table,
            _ => panic!("table layout mode"),
        };
        let row = match document.box_at(table.rows[0] as usize).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::TableRow(row)) => row,
            _ => panic!("anonymous row"),
        };
        assert_eq!(row.cells.len(), 3);
        assert_eq!(document.get_id(row.cells[0] as usize), Some("a"));
        assert_eq!(document.get_id(row.cells[2] as usize), Some("c"));
        let middle_idx = (0..document.box_count()).find(|&i| document.get_id(i) == Some("middle")).expect("middle block");
        assert_eq!(document.box_at(middle_idx).and_then(|box_| box_.parent()), Some(row.cells[1]));
    }

    #[test]
    fn ignores_whitespace_only_inline_content_between_block_children() {
        let html = "<html><body><section>\n  <p>One</p>\n  <p>Two</p>\n</section></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let section_idx = (0..document.box_count()).find(|&i| document.get_tag(i).eq_ignore_ascii_case("section")).expect("section box should exist");

        let children = match document.box_at(section_idx) {
            Some(box_) => match box_.layout_mode() {
                LayoutMode::Block(block) => &block.children,
                _ => panic!("section should use LayoutMode::Block"),
            },
            None => panic!("section box should exist"),
        };

        let Children::Blocks(children) = children else {
            panic!("section should contain block children");
        };

        assert_eq!(children.len(), 2, "whitespace-only anonymous boxes should be ignored");
    }

    #[test]
    fn preserves_separator_whitespace_inside_inline_segment() {
        let html = "<html><body><section><span>One</span> <span>Two</span><p>Block</p></section></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let section_idx = (0..document.box_count()).find(|&i| document.get_tag(i).eq_ignore_ascii_case("section")).expect("section box should exist");

        let children = match document.box_at(section_idx) {
            Some(box_) => match box_.layout_mode() {
                LayoutMode::Block(block) => &block.children,
                _ => panic!("section should use LayoutMode::Block"),
            },
            None => panic!("section box should exist"),
        };

        let Children::Blocks(children) = children else {
            panic!("section should contain block children");
        };

        assert_eq!(children.len(), 2, "inline content should become one anonymous block plus the paragraph");

        let anon_idx = children[0] as usize;
        let runs = match document.box_at(anon_idx) {
            Some(box_) => match box_.layout_mode() {
                LayoutMode::Anonymous(runs) => runs.clone(),
                _ => panic!("first child should be an anonymous inline wrapper"),
            },
            None => panic!("anonymous box should exist"),
        };

        let mut saw_space = false;
        for run in &document.inline_content().inline_items()[runs.start as usize..runs.end as usize] {
            if let InlineItemKind::Text { glyphs } = &run.kind
                && document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].contains(&(' ' as GlyphId))
            {
                saw_space = true;
                break;
            }
        }

        assert!(saw_space, "space between inline siblings should be preserved");
    }

    #[test]
    fn display_none_inline_descendants_do_not_generate_runs() {
        let html = "<html><body><p>Hello<span style=\"display:none\">hidden</span>World</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let rendered_text: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().map(|&glyph| char::from_u32(glyph).expect("glyph should be valid char")).collect::<String>()),
                _ => None,
            })
            .collect();

        assert_eq!(rendered_text, "HelloWorld");
    }

    #[test]
    fn inline_table_uses_table_layout_mode() {
        let html = "<html><body><div id=\"table\" style=\"display:inline-table\"><div style=\"display:table-row\"><div style=\"display:table-cell\">A</div></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let table_idx = (0..document.box_count()).find(|&i| document.get_id(i) == Some("table")).expect("inline-table box should exist");

        let display = document.box_at(table_idx).and_then(|b| b.style()).map(|style| document.styles().box_model_style(style).expect("validated style handle").display);
        assert_eq!(display, Some(html_style_model::Display::InlineTable));

        let rows = match document.box_at(table_idx) {
            Some(box_) => match box_.layout_mode() {
                LayoutMode::Table(table) => table.rows.clone(),
                _ => panic!("inline-table should use LayoutMode::Table"),
            },
            None => panic!("table box should exist"),
        };
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn anonymous_row_and_cell_inherit_text_style_from_table() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline("<html><body><div id='table' style='display:table;color:white'>text</div></body></html>", None);
        let table_idx = (0..document.box_count()).find(|&idx| document.get_id(idx) == Some("table")).expect("table box");
        let row_idx = match document.box_at(table_idx).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Table(table)) => table.rows[0] as usize,
            _ => panic!("display:table box must use table layout"),
        };
        let cell_idx = match document.box_at(row_idx).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::TableRow(row)) => row.cells[0] as usize,
            _ => panic!("anonymous row must use table-row layout"),
        };
        let row_style = document.style_view(document.box_style_indices(row_idx).expect("anonymous row style"));
        let cell_style = document.style_view(document.box_style_indices(cell_idx).expect("anonymous cell style"));

        assert_eq!(row_style.color(), 0xffffffff);
        assert_eq!(cell_style.color(), 0xffffffff);
    }

    #[test]
    fn empty_inline_table_after_block_content_is_not_pruned() {
        let html = "<html><body><div></div><table style='display:inline-table'><tr><td></td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        assert!(document.inline_content().inline_items().iter().any(|run| matches!(run.kind, InlineItemKind::AtomicBox { .. })));
        assert!((0..document.box_count()).any(|idx| document.get_tag(idx).eq_ignore_ascii_case("table")), "inline table box should exist");
    }

    #[test]
    fn table_row_child_gets_an_anonymous_table_in_mixed_container() {
        let html = "<html><body><div><span>Lead</span><div style=\"display:table-row\"><div style=\"display:table-cell\">Cell</div></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let div_idx = (0..document.box_count()).find(|&i| document.get_tag(i).eq_ignore_ascii_case("div")).expect("div box should exist");

        let children = match document.box_at(div_idx) {
            Some(box_) => match box_.layout_mode() {
                LayoutMode::Block(block) => &block.children,
                _ => panic!("div should use LayoutMode::Block"),
            },
            None => panic!("div box should exist"),
        };

        let Children::Blocks(children) = children else {
            panic!("div should contain block children");
        };

        assert_eq!(children.len(), 2, "inline lead-in should flush before the table row child");
        assert!(matches!(document.box_at(children[0] as usize).map(|b| b.layout_mode()), Some(LayoutMode::Anonymous(_))));
        let row = match document.box_at(children[1] as usize).map(|b| b.layout_mode()) {
            Some(LayoutMode::Table(table)) => *table.rows.first().expect("anonymous table should retain the row"),
            _ => panic!("improper row should be wrapped in an anonymous table"),
        };
        assert!(matches!(document.box_at(row as usize).map(|b| b.layout_mode()), Some(LayoutMode::TableRow(_))));
    }

    #[test]
    fn table_cell_child_gets_anonymous_table_and_row_boxes() {
        let html = "<html><body><div><span>Lead</span><div style=\"display:table-cell\">Cell</div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let div_idx = (0..document.box_count()).find(|&i| document.get_tag(i).eq_ignore_ascii_case("div")).expect("div box should exist");

        let children = match document.box_at(div_idx) {
            Some(box_) => match box_.layout_mode() {
                LayoutMode::Block(block) => &block.children,
                _ => panic!("div should use LayoutMode::Block"),
            },
            None => panic!("div box should exist"),
        };

        let Children::Blocks(children) = children else {
            panic!("div should contain block children");
        };

        assert_eq!(children.len(), 2);
        let row = match document.box_at(children[1] as usize).map(|b| b.layout_mode()) {
            Some(LayoutMode::Table(table)) => *table.rows.first().expect("anonymous table should contain a row"),
            _ => panic!("improper cell should be wrapped in an anonymous table"),
        };
        let cell = match document.box_at(row as usize).map(|b| b.layout_mode()) {
            Some(LayoutMode::TableRow(row)) => *row.cells.first().expect("anonymous row should retain the cell"),
            _ => panic!("anonymous table should contain a table row"),
        };
        assert!(matches!(document.box_at(cell as usize).map(|b| b.layout_mode()), Some(LayoutMode::TableCell(_))));
    }

    #[test]
    fn table_row_nested_in_a_row_gets_an_anonymous_cell_and_table() {
        let html = "<html><body><div id='outer' style='display:table-row'><span id='nested' style='display:table-row'><span style='display:table-cell'>nested</span></span></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let outer = (0..document.box_count()).find(|&idx| document.get_id(idx) == Some("outer")).expect("outer row box");
        let nested = (0..document.box_count()).find(|&idx| document.get_id(idx) == Some("nested")).expect("nested row box");
        let anonymous_cell = match document.box_at(outer).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::TableRow(row)) => *row.cells.first().expect("improper nested row should receive an anonymous cell"),
            _ => panic!("outer box should remain a table row"),
        };
        let anonymous_table = match document.box_at(anonymous_cell as usize).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::TableCell(cell)) => match &cell.children {
                Children::Blocks(children) => *children.first().expect("anonymous cell should contain a corrected table root"),
                _ => panic!("anonymous cell should establish block flow around the nested table"),
            },
            _ => panic!("nested row should be wrapped in an anonymous cell"),
        };
        match document.box_at(anonymous_table as usize).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Table(table)) => assert_eq!(table.rows, vec![nested as u32]),
            _ => panic!("nested row must be wrapped in an anonymous table"),
        }
    }

    #[test]
    fn consecutive_row_groups_get_one_anonymous_table_in_a_block() {
        let html = "<html><body><div id='host'><div id='head' style='display:table-header-group'><div id='head-row' style='display:table-row'><div style='display:table-cell'>H</div></div></div><div id='foot' style='display:table-footer-group'><div id='foot-row' style='display:table-row'><div style='display:table-cell'>F</div></div></div><div id='body' style='display:table-row-group'><div id='body-row' style='display:table-row'><div style='display:table-cell'>B</div></div></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let host = (0..document.box_count()).find(|&i| document.get_id(i) == Some("host")).expect("host box should exist");
        let children = match document.box_at(host).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Block(block)) => match &block.children {
                Children::Blocks(children) => children,
                _ => panic!("host should use block children"),
            },
            _ => panic!("host should be a block"),
        };

        assert_eq!(children.len(), 1, "one consecutive improper table-internal run needs one anonymous table");
        let table = match document.box_at(children[0] as usize).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Table(table)) => table,
            _ => panic!("row groups should be wrapped by an anonymous table"),
        };
        assert_eq!(table.row_groups.len(), 3);
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.rows.iter().map(|&row| document.get_id(row as usize)).collect::<Vec<_>>(), vec![Some("head-row"), Some("body-row"), Some("foot-row")]);
    }

    #[test]
    fn anonymous_table_inherits_collapsing_border_properties() {
        let html =
            "<html><body><div id='host' style='border-collapse:collapse;border-spacing:50px 25px'><div style='display:table-row-group'><div style='display:table-row'><div style='display:table-cell'>A</div></div></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let host = (0..document.box_count()).find(|&i| document.get_id(i) == Some("host")).expect("host box should exist");
        let table = match document.box_at(host).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Block(block)) => match &block.children {
                Children::Blocks(children) => children[0] as usize,
                _ => panic!("host should use block children"),
            },
            _ => panic!("host should be a block"),
        };
        let style = document.box_at(table).and_then(|box_| box_.style()).and_then(|style| document.styles().view(style)).expect("anonymous table should have an inherited style");
        assert_eq!(style.border_collapse(), html_style_model::BorderCollapseMode::Collapse);
        assert_eq!(style.border_spacing_horizontal(), 50.0);
        assert_eq!(style.border_spacing_vertical(), 25.0);
    }

    #[test]
    fn table_cell_inside_inline_content_gets_an_atomic_anonymous_table() {
        let html = "<html><body><span id='outer'>left <span id='cell' style='display:table-cell'>right</span></span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let outer = (0..document.box_count()).find(|&i| document.get_id(i) == Some("outer")).expect("outer inline box should exist");
        let range = match document.box_at(outer).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Inline(range)) => range.clone(),
            _ => panic!("the inline ancestor must not be split by table fixup"),
        };
        let table = document.inline_content().inline_items()[range.start as usize..range.end as usize]
            .iter()
            .find_map(|run| match run.kind {
                InlineItemKind::AtomicBox { box_idx } => Some(box_idx),
                _ => None,
            })
            .expect("improper table cell should be wrapped by an atomic anonymous table");
        let row = match document.box_at(table as usize).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Table(table)) => table.rows[0],
            _ => panic!("atomic wrapper should use table layout"),
        };
        let cells = match document.box_at(row as usize).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::TableRow(row)) => &row.cells,
            _ => panic!("anonymous table should contain an anonymous row"),
        };
        assert_eq!(cells.len(), 1);
        assert_eq!(document.get_id(cells[0] as usize), Some("cell"));
    }

    #[test]
    fn collapsible_space_after_an_inline_anonymous_table_stays_in_the_inline_item_stream() {
        let html = "<html><body><span id='outer'>a<span style='display:table-cell'>b</span><span style='display:table-cell'>c</span> <span>d</span></span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let outer = (0..document.box_count()).find(|&i| document.get_id(i) == Some("outer")).expect("outer inline box should exist");
        let range = match document.box_at(outer).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::Inline(range)) => range.clone(),
            _ => panic!("outer should remain an inline box"),
        };
        let runs = &document.inline_content().inline_items()[range.start as usize..range.end as usize];
        let atomic = runs.iter().position(|run| matches!(run.kind, InlineItemKind::AtomicBox { .. })).expect("table cells should form one atomic anonymous table");
        let following_text = runs[atomic + 1..]
            .iter()
            .find_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .expect("text should follow the atomic table");
        assert!(following_text.starts_with(' '), "the collapsible boundary space must not be swallowed by table fixup: {following_text:?}");
    }

    #[test]
    fn replaced_table_cell_and_preserved_row_text_get_an_anonymous_cell() {
        let html = "<html><body><div id='table' style='display:table;white-space:pre'><div id='row' style='display:table-row'> <img class='cell' src='missing.png' style='display:table-cell'/>\t<div id='proper' style='display:table-cell'>B</div></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let row = (0..document.box_count()).find(|&i| document.get_id(i) == Some("row")).expect("row box should exist");
        let cells = match document.box_at(row).map(|box_| box_.layout_mode()) {
            Some(LayoutMode::TableRow(row)) => &row.cells,
            _ => panic!("row should use table-row layout"),
        };
        assert_eq!(cells.len(), 2, "the improper inline run and proper cell should form two cells");
        assert!(document.box_at(cells[0] as usize).is_some_and(|box_| box_.dom_element().is_none()));
        assert_eq!(document.get_id(cells[1] as usize), Some("proper"));
        assert!(document.inline_content().inline_items().iter().any(|run| matches!(run.kind, InlineItemKind::Image { .. })), "the replaced element remains inline content inside the anonymous cell");
    }

    #[test]
    fn generated_before_and_after_strings_enter_inline_flow_in_tree_order() {
        let html = "<html><body><div data-x='B'>C</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("div:before{content:'A' attr(data-x)}div:after{content:'Z'}"));

        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();

        assert!(rendered.contains("ABCZ"), "generated content should surround the element's DOM text, got {rendered:?}");
        assert!((0..document.box_count()).any(|index| document.box_at(index).is_some_and(|box_| box_.dom_element().is_none() && box_.style().is_some())), "generated content should own a synthetic styled box");
    }

    #[test]
    fn generated_counters_increment_across_inline_siblings_and_nest_by_scope() {
        let html = "<html><body><div id='test'><span></span><span></span><span></span></div></body></html>";
        let css = "#test{counter-reset:c}#test span{counter-increment:c}#test span:before{content:counter(c)}";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));

        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();

        assert_eq!(rendered, "123");
    }

    #[test]
    fn generated_counter_pseudo_inherits_the_originating_font() {
        let html = "<html><body><span></span></body></html>";
        let css = "span:before{content:counter(c, square)}";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));

        let span_box = (0..document.box_count()).find(|&index| document.get_tag(index).eq_ignore_ascii_case("span")).expect("span box");
        let pseudo_box = document
            .inline_content()
            .inline_items()
            .iter()
            .find_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } if document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().any(|glyph| char::from_u32(*glyph) == Some('\u{25aa}')) => Some(run.box_idx as usize),
                _ => None,
            })
            .expect("generated square counter run");
        let span_style = document.box_style_indices(span_box).expect("span style");
        let pseudo_style = document.box_style_indices(pseudo_box).expect("pseudo style");

        assert_eq!(document.styles().font_style(pseudo_style), document.styles().font_style(span_style));
    }

    #[test]
    fn generated_inline_edges_surround_block_children_in_anonymous_flow_boxes() {
        let html = "<html><body><div id='outer'><div>X</div></div></body></html>";
        let css = "#outer:before{content:'A'}#outer:after{content:'Z'}";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));

        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();

        assert_eq!(rendered, "AXZ");
    }

    #[test]
    fn generated_nested_counters_survive_block_child_construction() {
        let html = "<html><body><div id='outer'><div></div></div></body></html>";
        let css = "div:before{content:counters(test,'.');counter-reset:test}";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));

        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();

        assert_eq!(rendered, "00.0");
    }

    #[test]
    fn unmatched_generated_close_quote_emits_nothing() {
        let html = "<html><body><q cite='PASS PASS'></q></body></html>";
        let css = "q:before{content:attr(cite)}";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));

        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();

        assert_eq!(rendered, "PASS PASS");
    }

    #[test]
    fn generated_quotes_use_authored_pairs_and_repeat_the_last_nested_pair() {
        let html = "<html><body><q><q><q></q></q></q></body></html>";
        let css = "body{quotes:'[' ']' '<' '>'}";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));

        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();

        assert_eq!(rendered, "[<<>>]");
    }

    #[test]
    fn authored_ascii_quote_pairs_emit_ascii_quote_codepoints() {
        let html = "<html><body><div lang='en'></div></body></html>";
        let css = "div:lang(en){quotes:'\"' '\"' \"'\" \"'\"}div:before{content:open-quote}div:after{content:close-quote}";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));

        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();
        assert_eq!(rendered, "\"\"");
    }

    #[test]
    fn empty_inline_after_a_block_is_retained_when_its_pseudo_generates_content() {
        let html = "<html><body><p>Lead</p><div></div></body></html>";
        let css = "div{display:inline}div:before{content:'PASS'}";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));

        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();

        assert_eq!(rendered, "LeadPASS");
    }

    #[test]
    fn authored_br_generated_content_replaces_the_native_break() {
        let html = "<html><body>A<br/>B</body></html>";
        let css = "br:before{content:'X'}br:after{content:'Y'}";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));

        assert!(!document.inline_content().inline_items().iter().any(|run| matches!(run.kind, InlineItemKind::Break { .. })));
        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();
        assert_eq!(rendered, "AXYB");
    }

    #[test]
    fn block_and_list_item_pseudos_create_block_flow_principal_boxes() {
        let html = "<html><body><div id='target'>B</div></body></html>";
        let css = "#target:before{content:'A';display:block}#target:after{content:'C';display:list-item;margin-left:1em}";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));

        let target = (0..document.box_count()).find(|&index| document.get_tag(index).eq_ignore_ascii_case("div")).expect("target div");
        let Children::Blocks(children) = &document
            .box_at(target)
            .and_then(|box_| match box_.layout_mode() {
                LayoutMode::Block(block) => Some(&block.children),
                _ => None,
            })
            .expect("target should establish block flow")
        else {
            panic!("generated block pseudos should split the target into block children");
        };

        assert_eq!(children.len(), 3, "before, anonymous inline content, and after should be separate block children");
        assert!(matches!(document.box_at(children[0] as usize).map(|box_| box_.layout_mode()), Some(LayoutMode::Block(_))));
        assert!(matches!(document.box_at(children[1] as usize).map(|box_| box_.layout_mode()), Some(LayoutMode::Anonymous(_))));
        assert!(matches!(document.box_at(children[2] as usize).map(|box_| box_.layout_mode()), Some(LayoutMode::Block(_))));

        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();
        assert_eq!(rendered, "ABC");
        assert!(document.inline_content().inline_items().iter().any(|run| matches!(run.kind, InlineItemKind::Marker { .. })), "display:list-item pseudo should own a list marker");
    }

    #[test]
    fn generated_table_principals_use_shared_table_fixup_topology() {
        use html_style_model::Display;

        for display in ["table", "table-row-group", "table-row", "table-cell", "table-caption"] {
            let html = "<html><body><div id='target'></div></body></html>";
            let css = format!("#target:before{{content:'X';display:{display}}}");
            let mut factory = DocumentFactory::new();
            let document = factory.parse_with_new_pipeline(html, Some(&css));
            let target = (0..document.box_count()).find(|&index| document.get_tag(index).eq_ignore_ascii_case("div")).expect("target div");
            let Children::Blocks(children) = &document
                .box_at(target)
                .and_then(|box_| match box_.layout_mode() {
                    LayoutMode::Block(block) => Some(&block.children),
                    _ => None,
                })
                .expect("generated table principal should enter block flow")
            else {
                panic!("generated table principal should be a block child");
            };
            assert_eq!(children.len(), 1, "{display} should produce one corrected table root");
            let table_idx = children[0] as usize;
            let LayoutMode::Table(table) = document.box_at(table_idx).expect("corrected table root").layout_mode() else {
                panic!("{display} should be wrapped by or itself establish a table");
            };
            let box_display = |index: usize| document.box_at(index).and_then(|box_| box_.style()).map(|style| document.styles().box_model_style(style).expect("validated style handle").display);

            match display {
                "table" => {
                    assert_eq!(box_display(table_idx), Some(Display::Table));
                    assert_eq!(table.rows.len(), 1);
                    let LayoutMode::TableRow(row) = document.box_at(table.rows[0] as usize).expect("anonymous row").layout_mode() else { panic!("table content should be wrapped in a row") };
                    assert_eq!(row.cells.len(), 1);
                }
                "table-row-group" => {
                    assert_eq!(table.row_groups.len(), 1);
                    assert_eq!(box_display(table.row_groups[0] as usize), Some(Display::TableRowGroup));
                    assert_eq!(table.rows.len(), 1);
                }
                "table-row" => {
                    assert_eq!(table.rows.len(), 1);
                    assert_eq!(box_display(table.rows[0] as usize), Some(Display::TableRow));
                }
                "table-cell" => {
                    assert_eq!(table.rows.len(), 1);
                    let LayoutMode::TableRow(row) = document.box_at(table.rows[0] as usize).expect("anonymous row").layout_mode() else { panic!("cell should receive an anonymous row") };
                    assert_eq!(row.cells.len(), 1);
                    assert_eq!(box_display(row.cells[0] as usize), Some(Display::TableCell));
                }
                "table-caption" => {
                    assert_eq!(table.captions_top.len(), 1);
                    assert_eq!(box_display(table.captions_top[0] as usize), Some(Display::TableCaption));
                }
                _ => unreachable!(),
            }

            let rendered: String = document
                .inline_content()
                .inline_items()
                .iter()
                .filter_map(|run| match &run.kind {
                    InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                    _ => None,
                })
                .collect();
            assert_eq!(rendered, "X", "{display} should retain generated content");
        }
    }

    #[test]
    fn display_contents_suppresses_only_the_principal_box() {
        let html = "<html><body><div><span id='transparent'><b>kept</b></span></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#transparent { display: contents }"));

        assert!(!(0..document.box_count()).any(|index| document.get_tag(index).eq_ignore_ascii_case("span")), "the contents element must not allocate a layout box");
        assert!((0..document.box_count()).any(|index| document.get_tag(index).eq_ignore_ascii_case("b")), "its child must still generate a box");
        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();
        assert!(rendered.contains("kept"));
    }

    #[test]
    fn display_contents_splices_block_descendants_into_block_flow() {
        let html = "<html><body><main><span id='transparent'>before<section>block</section>after</span></main></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#transparent { display: contents }"));

        assert!(!(0..document.box_count()).any(|index| document.get_tag(index).eq_ignore_ascii_case("span")));
        let main = (0..document.box_count()).find(|&index| document.get_tag(index).eq_ignore_ascii_case("main")).expect("main box");
        let section = (0..document.box_count()).find(|&index| document.get_tag(index).eq_ignore_ascii_case("section")).expect("section box");
        assert_eq!(document.box_at(section).and_then(|box_| box_.parent()), Some(main as u32), "the block descendant becomes a direct child of the contents element's parent box");
    }

    #[test]
    fn display_contents_children_become_flex_items() {
        let html = "<html><body><div id='flex'><span id='transparent'><i>A</i><b>B</b></span></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#flex { display:flex } #transparent { display:contents }"));

        let flex = (0..document.box_count()).find(|&index| document.get_tag(index).eq_ignore_ascii_case("div")).expect("flex box");
        let LayoutMode::Flex(layout) = document.box_at(flex).expect("flex box").layout_mode() else { panic!("expected flex layout") };
        assert_eq!(layout.children.len(), 2);
        assert!(layout.children.iter().all(|&child| document.box_at(child as usize).is_some_and(|box_| box_.parent() == Some(flex as u32))));
        assert!(!(0..document.box_count()).any(|index| document.get_tag(index).eq_ignore_ascii_case("span")));
    }

    #[test]
    fn display_contents_preserves_generated_counter_scope_and_tree_order() {
        let html = "<html><body><div><span class='reset'></span><span id='transparent'><span class='inc'></span></span><span class='result'></span></div></body></html>";
        let css = ".reset { counter-reset:x 6 } #transparent { display:contents } .inc { counter-increment:x } .result::before { content:counter(x) }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();
        assert_eq!(rendered, "7");
    }

    #[test]
    fn display_contents_exposes_table_internals_to_parent_fixup() {
        let html = "<html><body><main><div id='transparent'><div style='display:table-row'><div style='display:table-cell'>cell</div></div></div></main></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#transparent { display:contents }"));
        let main = (0..document.box_count()).find(|&index| document.get_tag(index).eq_ignore_ascii_case("main")).expect("main box");
        let LayoutMode::Block(block) = document.box_at(main).expect("main box").layout_mode() else { panic!("main should be a block") };
        let Children::Blocks(children) = &block.children else { panic!("table internals should make block children") };
        assert!(children.iter().any(|&child| matches!(document.box_at(child as usize).map(|box_| box_.layout_mode()), Some(LayoutMode::Table(_)))));
        assert!(!(0..document.box_count()).any(|index| document.get_tag(index).eq_ignore_ascii_case("div") && document.box_at(index).is_some_and(|box_| {
            box_.dom_element().and_then(|raw| document.document().node_id_from_raw(raw)).is_some_and(|node| document.document().get_dom_id(node) == Some("transparent"))
        })));
    }

    #[test]
    fn display_contents_generated_edges_render_without_an_origin_box() {
        let html = "<html><body><div><span id='transparent'>B</span></div></body></html>";
        let css = "#transparent { display:contents } #transparent::before { content:'A' } #transparent::after { content:'C' }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();
        assert_eq!(rendered, "ABC");
        assert!(!(0..document.box_count()).any(|index| document.get_tag(index).eq_ignore_ascii_case("span")));
    }

    #[test]
    fn display_contents_root_keeps_the_initial_containing_block() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline("<html style='display:contents'><body>root</body></html>", None);
        assert!((0..document.box_count()).any(|index| document.get_tag(index).eq_ignore_ascii_case("html")), "the document root receives a block-level used value");
    }

    #[test]
    fn display_contents_on_unboxable_html_elements_suppresses_their_subtree() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline("<html><body>A<object style='display:contents'>FAIL</object>B</body></html>", None);
        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();
        assert_eq!(rendered, "AB");
    }

    #[test]
    fn suppressed_children_and_collapsible_whitespace_do_not_create_a_line_box() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(
            "<html><body><div>\n<object style='display:contents'>FAIL</object>\n<img style='display:contents'>\n</div><p>PASS</p></body></html>",
            None,
        );
        let div = (0..document.box_count()).find(|&index| document.get_tag(index).eq_ignore_ascii_case("div")).expect("div box");
        assert!(matches!(document.box_at(div).map(|box_| box_.layout_mode()), Some(LayoutMode::Block(BlockBox { children: Children::Empty }))));
    }

    #[test]
    fn display_contents_pseudos_share_one_anonymous_flex_item() {
        let html = "<html><body><div id='flex'></div></body></html>";
        let css = "#flex { display:flex; flex-direction:column } #flex::before { display:contents; content:'A' } #flex::after { display:contents; content:'S' }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let flex = (0..document.box_count()).find(|&index| document.get_tag(index).eq_ignore_ascii_case("div")).expect("flex box");
        let LayoutMode::Flex(layout) = document.box_at(flex).expect("flex box").layout_mode() else { panic!("expected flex layout") };
        assert_eq!(layout.children.len(), 1, "contiguous boxless generated text belongs to one anonymous flex item");
        let rendered: String = document
            .inline_content()
            .inline_items()
            .iter()
            .filter_map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } => Some(document.inline_content().glyphs()[glyphs.start as usize..glyphs.end as usize].iter().filter_map(|glyph| char::from_u32(*glyph)).collect::<String>()),
                _ => None,
            })
            .collect();
        assert_eq!(rendered, "AS");
    }

    #[test]
    fn display_contents_bare_text_uses_the_contents_inherited_style() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline("<html><body><div><span style='display:contents;font-size:40px;white-space:pre'>Two\nlines</span></div></body></html>", None);
        let text_run = document
            .inline_content()
            .inline_items()
            .iter()
            .find(|run| matches!(run.kind, InlineItemKind::Text { .. }) && run.dom_text_node.is_some())
            .expect("contents text run");
        let style = document.box_at(text_run.box_idx as usize).and_then(|box_| box_.style()).expect("anonymous inherited style carrier");
        let view = document.styles().view(style).expect("validated style");
        assert!((view.font_size() - 40.0).abs() < 0.01);
        assert!(view.white_space().preserves_newlines());
        assert!(document.box_at(text_run.box_idx as usize).is_some_and(|box_| box_.dom_element().is_none()), "the carrier is anonymous, not the suppressed principal box");
    }

    #[test]
    fn display_contents_bare_text_keeps_its_style_inside_a_flex_item() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline("<html><body><div style='display:flex'><span style='display:contents;color:green'>S</span></div></body></html>", None);
        let text_run = document.inline_content().inline_items().iter().find(|run| matches!(run.kind, InlineItemKind::Text { .. }) && run.dom_text_node.is_some()).expect("styled flex text");
        let style = document.box_at(text_run.box_idx as usize).and_then(|box_| box_.style()).expect("style carrier");
        assert_eq!(document.styles().view(style).expect("validated style").color(), 0x008000FF);
    }

    #[test]
    fn display_contents_bare_text_keeps_its_style_through_table_fixup() {
        let html = "<html><body><div style='display:table;color:red'><span style='display:contents;color:green'>X<div style='display:table-cell'>X</div></span></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let source_runs: Vec<_> = document.inline_content().inline_items().iter().filter(|run| matches!(run.kind, InlineItemKind::Text { .. }) && run.dom_text_node.is_some()).collect();
        assert_eq!(source_runs.len(), 2);
        for run in source_runs {
            let style = document.box_at(run.box_idx as usize).and_then(|box_| box_.style()).expect("text style");
            assert_eq!(document.styles().view(style).expect("validated style").color(), 0x008000FF);
        }
    }

    #[test]
    fn display_contents_mixed_table_fixup_keeps_an_acyclic_box_tree() {
        let html = "<html><body><div style='display:table;color:red'><div style='display:contents;color:green'>X<div style='display:table-cell'>X</div>X<div style='display:table-row'>X</div>X</div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        for start in 0..document.box_count() {
            let mut current = Some(start);
            for _ in 0..=document.box_count() {
                let Some(index) = current else { break };
                current = document.box_at(index).and_then(|box_| box_.parent()).map(|parent| parent as usize);
            }
            assert!(current.is_none(), "box {start} participates in a parent cycle");
        }
    }
}
