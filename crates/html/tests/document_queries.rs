use html::document::{MarkupSyntax, parse_document};
use html::layout::{
    CaretAffinity, ContentRole, LaidOutDocument, LayoutConstraints, PositionedShapedLine,
    SourceLocation,
};
use html::pipeline::DocumentFactory;
use html::text::{
    FontSlant, GlyphId, GlyphMetric, GlyphResourceGeneration, GlyphResourceStore, GlyphShaper,
    ShapeError, ShapedLine, TextSearchOptions, find_text_matches,
};
use std::ops::Range;

#[derive(Default)]
struct TestShaper(GlyphResourceGeneration);
impl GlyphShaper for TestShaper {
    fn reset(&mut self) {
        self.0 = GlyphResourceGeneration::default();
    }
    fn resource_generation(&self) -> GlyphResourceGeneration {
        self.0
    }
    fn shape_glyph(
        &mut self,
        ch: char,
        size: f32,
        _: u16,
        _: FontSlant,
        _: u32,
        _: Option<&str>,
        resources: &mut GlyphResourceStore,
    ) -> Result<GlyphId, ShapeError> {
        resources.register(GlyphMetric::try_new(ch, size / 2.0, size, 0.0, 0.0).unwrap())
    }
}

fn layout(body: &str, resources: &mut GlyphResourceStore) -> LaidOutDocument<'static> {
    DocumentFactory::new()
        .parse_with_new_pipeline(
            &format!("<!doctype html><html><body>{body}</body></html>"),
            None,
        )
        .shape(&mut TestShaper::default(), resources)
        .unwrap()
        .layout(LayoutConstraints::new(600.0, 16.0).unwrap(), resources)
        .expect("layout requires the shaper-aware API for first-line styles")
}
fn box_for(doc: &LaidOutDocument, id: &str, resources: &GlyphResourceStore) -> usize {
    let boxes = doc.render_view(resources).boxes();
    (0..boxes.len())
        .find(|&index| boxes.attribute(index, "id") == Some(id))
        .unwrap()
}
fn range(doc: &LaidOutDocument, query: &str, resources: &GlyphResourceStore) -> Range<u32> {
    doc.render_view(resources)
        .text()
        .find_matches(query, TextSearchOptions::default())[0]
        .clone()
}

#[test]
fn semantic_roles_cover_structures_and_ancestry_without_markup_queries() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout(
        "<h2 id='heading'>Title</h2><ol id='list'><li id='item'><em id='inline'>Item</em></li></ol><pre id='code'>x</pre><figure id='figure'><figcaption id='caption'>Caption</figcaption></figure><table id='table'><tr><th id='cell'>Header</th></tr></table>",
        &mut doc_resources,
    );
    let semantics = doc.render_view(&doc_resources).semantics();
    for (id, role) in [
        ("heading", ContentRole::Heading),
        ("list", ContentRole::List),
        ("item", ContentRole::ListItem),
        ("code", ContentRole::CodeBlock),
        ("figure", ContentRole::Figure),
        ("caption", ContentRole::Caption),
        ("table", ContentRole::Table),
        ("cell", ContentRole::TableHeaderCell),
        ("cell", ContentRole::TableCell),
    ] {
        assert!(
            semantics.has_role(box_for(&doc, id, &doc_resources), role),
            "{id}: {role:?}"
        );
    }
    assert_eq!(
        semantics
            .content(box_for(&doc, "heading", &doc_resources))
            .heading_level(),
        Some(2)
    );
    assert_eq!(
        semantics.nearest(
            box_for(&doc, "inline", &doc_resources),
            ContentRole::ListItem
        ),
        Some(box_for(&doc, "item", &doc_resources))
    );
    assert!(!semantics.contains(usize::MAX, box_for(&doc, "inline", &doc_resources)));
    assert_eq!(semantics.block_container(usize::MAX), None);
}

#[test]
fn epub_and_aria_roles_preserve_exact_and_vocabulary_token_rules() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout(
        "<section id='chapter' epub:type='chapter'>Chapter</section><section id='prefixed' epub:type='book:chapter'>Other</section><aside id='note' role='doc-footnote'>Note</aside><div id='sidebar' role='complementary'>Sidebar</div><div id='statement' epub:type='book:theorem'><p id='label' epub:type='book:label'>Theorem</p>Proof</div>",
        &mut doc_resources,
    );
    let roles = doc.render_view(&doc_resources).semantics();
    assert!(roles.has_role(
        box_for(&doc, "chapter", &doc_resources),
        ContentRole::MajorSection
    ));
    assert!(!roles.has_role(
        box_for(&doc, "prefixed", &doc_resources),
        ContentRole::MajorSection
    ));
    assert!(roles.has_role(box_for(&doc, "note", &doc_resources), ContentRole::Note));
    assert!(!roles.has_role(box_for(&doc, "note", &doc_resources), ContentRole::Sidebar));
    assert!(roles.has_role(
        box_for(&doc, "sidebar", &doc_resources),
        ContentRole::Sidebar
    ));
    assert!(roles.has_role(
        box_for(&doc, "statement", &doc_resources),
        ContentRole::CompactStructure
    ));
    assert!(roles.has_role(
        box_for(&doc, "statement", &doc_resources),
        ContentRole::Statement
    ));
    assert!(roles.has_role(box_for(&doc, "label", &doc_resources), ContentRole::Label));
}

#[test]
fn xml_epub_semantics_use_the_namespace_uri() {
    let parsed = parse_document("<html xmlns='http://www.w3.org/1999/xhtml' xmlns:book='http://www.idpf.org/2007/ops'><body><section id='chapter' book:type='chapter'>Chapter</section></body></html>", MarkupSyntax::Xml).unwrap();
    let mut doc_resources = GlyphResourceStore::default();
    let doc = DocumentFactory::new()
        .build_pipeline_from_parsed(&parsed, &[])
        .shape(&mut TestShaper::default(), &mut doc_resources)
        .unwrap()
        .layout(LayoutConstraints::new(600.0, 16.0).unwrap(), &doc_resources)
        .expect("layout requires the shaper-aware API for first-line styles");
    assert!(doc.render_view(&doc_resources).semantics().has_role(
        box_for(&doc, "chapter", &doc_resources),
        ContentRole::MajorSection
    ));
}

#[test]
fn source_addresses_resolve_images_and_text_across_reflow() {
    let mut doc_resources = GlyphResourceStore::default();
    let mut doc = layout(
        "<p><img src='cover.png' width='10' height='10'>Before 😀 after</p>",
        &mut doc_resources,
    );
    let selected = range(&doc, "after", &doc_resources);
    let line = doc
        .render_view(&doc_resources)
        .text()
        .line_index_for_glyph(selected.start)
        .unwrap();
    let location = doc
        .render_view(&doc_resources)
        .addressing()
        .source_location_on_line(line, Some(selected.start))
        .unwrap();
    assert_eq!(
        location,
        SourceLocation {
            path: vec![4, 2, 2],
            utf16_offset: None
        },
        "the image precedes text on the same line"
    );
    let original = doc
        .render_view(&doc_resources)
        .addressing()
        .source_position_for_glyph(selected.start, false)
        .unwrap();
    let mut expected = original
        .elements()
        .map(|step| step.step())
        .collect::<Vec<_>>();
    expected.push(original.text_step());
    let expected = SourceLocation {
        path: expected,
        utf16_offset: Some(original.utf16_offset()),
    };
    doc.relayout(LayoutConstraints::new(50.0, 16.0).unwrap(), &doc_resources)
        .expect("layout requires the shaper-aware API for first-line styles");
    let line = doc
        .render_view(&doc_resources)
        .text()
        .line_index_for_glyph(selected.start)
        .unwrap();
    let location = doc
        .render_view(&doc_resources)
        .addressing()
        .source_location_on_line(line, Some(selected.start))
        .unwrap();
    assert_eq!(location, expected);
    let mut element = location.clone();
    element.path.pop();
    element.utf16_offset = None;
    assert!(
        doc.render_view(&doc_resources)
            .addressing()
            .source_element_y(&element)
            .is_some()
    );
    assert_eq!(
        doc.render_view(&doc_resources)
            .addressing()
            .source_element_y(&location),
        None
    );
}

#[test]
fn word_selection_crosses_inline_styles_and_reflow_and_keeps_graphemes() {
    let mut doc_resources = GlyphResourceStore::default();
    let mut doc = layout(
        "<p>hel<em>lo</em> cafe\u{301} 😀 end</p><p>other</p>",
        &mut doc_resources,
    );
    for width in [600.0, 35.0] {
        doc.relayout(LayoutConstraints::new(width, 16.0).unwrap(), &doc_resources)
            .expect("layout requires the shaper-aware API for first-line styles");
        for word in ["hello", "cafe\u{301}", "😀"] {
            let selected = range(&doc, word, &doc_resources);
            for glyph in selected.clone() {
                assert_eq!(
                    doc.render_view(&doc_resources).text().word_range_at(glyph),
                    Some(selected.clone()),
                    "{word}"
                );
            }
        }
    }
}

#[test]
fn search_and_quote_queries_ignore_hidden_and_generated_content() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout(
        "<style>p::before{content:'Generated'}</style><p>First target one. Second target two.<span style='visibility:hidden'>Secret</span></p>",
        &mut doc_resources,
    );
    let text = doc.render_view(&doc_resources).text();
    assert_eq!(
        text.find_matches("target", TextSearchOptions::default())
            .len(),
        2
    );
    for omitted in ["Generated", "Secret"] {
        assert!(
            text.find_matches(omitted, TextSearchOptions::default())
                .is_empty()
        );
        assert_eq!(text.locate_quote(omitted, None, None), None);
    }
    let selected = text
        .locate_quote("target", Some("Second "), Some(" two."))
        .unwrap();
    assert_eq!(
        doc.render_view(&doc_resources)
            .export()
            .selection_text(selected.clone())
            .0
            .as_deref(),
        Some("target")
    );
    assert_eq!(
        selected,
        text.find_matches("target", TextSearchOptions::default())[1]
    );
    assert_eq!(text.locate_quote("target", Some("missing"), None), None);
}

#[test]
fn normalized_search_returns_complete_original_graphemes_and_character_offsets() {
    let options = TextSearchOptions::default();
    assert_eq!(
        find_text_matches("😀 cafe\u{301} CAFÉ", "cafe", options),
        vec![(2, 7), (8, 12)]
    );
    assert_eq!(
        find_text_matches(
            "scatter cat",
            "cat",
            TextSearchOptions {
                whole_word: true,
                ..options
            }
        ),
        vec![(8, 11)]
    );
    assert!(
        find_text_matches(
            "CAFÉ",
            "café",
            TextSearchOptions {
                match_case: true,
                ..options
            }
        )
        .is_empty()
    );
    assert!(
        find_text_matches(
            "café",
            "cafe",
            TextSearchOptions {
                match_diacritics: true,
                ..options
            }
        )
        .is_empty()
    );
    assert!(find_text_matches("text", "", options).is_empty());
}

#[test]
fn fallback_geometry_tracks_bidi_and_document_rectangles() {
    for body in [
        "<p>abcd</p>",
        "<p dir='rtl'>אבגד</p>",
        "<p style='letter-spacing:2px'>abcd</p>",
    ] {
        let mut doc_resources = GlyphResourceStore::default();
        let doc = layout(body, &mut doc_resources);
        let view = doc.render_view(&doc_resources);
        let line = view.text().line(0).unwrap();
        let geometry = view.geometry();
        for glyph in line.glyphs().chain(std::iter::once(line.end())) {
            let x = geometry
                .caret_x(0, glyph, CaretAffinity::Leading, &[])
                .unwrap();
            assert_eq!(geometry.hit_test_line(0, x, &[]), Some(glyph));
        }
        let (x0, x1) = geometry.caret_span(0, line.glyphs(), &[]).unwrap();
        let rects: Vec<_> = geometry
            .line_selection_rects(0, line.glyphs(), &[])
            .collect();
        let bounds = rects
            .iter()
            .copied()
            .reduce(|left, right| left.union(right))
            .unwrap();
        assert_eq!(bounds.x0, line.point_x() + x0);
        assert_eq!(bounds.x1, line.point_x() + x1);
        assert_eq!(bounds.y0, line.point_y());
        assert_eq!(geometry.hit_test_line(0, f64::NAN, &[]), None);
        assert_eq!(
            geometry.caret_x(usize::MAX, 0, CaretAffinity::Leading, &[]),
            None
        );
    }
}

#[test]
fn native_geometry_uses_backend_carets_and_avoids_cluster_interiors() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout("<p>ffi</p>", &mut doc_resources);
    let line = doc.render_view(&doc_resources).text().line(0).unwrap();
    let shaped = [PositionedShapedLine {
        offset_x: 0.0,
        shaped: ShapedLine {
            line_index: 0,
            text_range: line.glyphs(),
            run: 0,
            ascent: 16.0,
            caret_stops: vec![0.0, 4.0, 8.0, 12.0].into(),
            cluster_boundaries: vec![true, false, false, true].into(),
        },
    }];
    let geometry = doc.render_view(&doc_resources).geometry();
    assert_eq!(
        geometry.caret_x(0, line.start() + 1, CaretAffinity::Leading, &shaped),
        Some(line.optical_offset_x() + 4.0)
    );
    assert_eq!(
        geometry.hit_test_line(0, line.optical_offset_x() + 5.0, &shaped),
        Some(line.start())
    );
    assert_eq!(
        geometry.hit_test_line(0, line.optical_offset_x() + 9.0, &shaped),
        Some(line.end())
    );
}

#[test]
fn unstyled_table_export_preserves_spans_and_escapes_text() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout(
        "<table id='table' style='color:red'><tr><th colspan='2'>A &amp; B</th></tr><tr><td rowspan='2'>&lt;tag&gt;</td><td>x</td></tr><tr><td>y</td></tr></table>",
        &mut doc_resources,
    );
    let table = doc
        .render_view(&doc_resources)
        .boxes()
        .table(box_for(&doc, "table", &doc_resources))
        .unwrap();
    assert_eq!(
        table.unstyled_html().unwrap(),
        "<table><tr><th colspan=\"2\">A &amp; B</th></tr><tr><td rowspan=\"2\">&lt;tag&gt;</td><td>x</td></tr><tr><td>y</td></tr></table>"
    );
}

#[cfg(feature = "math-export")]
#[test]
fn public_formula_export_prefers_authored_tex_and_ignores_normal_images() {
    assert_eq!(html::export::mathml_to_latex("<math><semantics><mi>x</mi><annotation encoding='application/x-tex'>x^2</annotation></semantics></math>").unwrap(), "x^2");
    assert!(html::export::mathml_to_latex("<math>").is_err());
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout("<p><img src='image.png'></p>", &mut doc_resources);
    assert!(!doc.render_view().export().is_equation(0));
    assert!(html::export::equation_latex(doc.render_view(), 0).is_none());
}

#[test]
fn book_roles_keep_epub_and_aria_vocabulary_boundaries() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout(
        "<section id='title' epub:type='titlepage'>Title</section><section id='aria-title' role='doc-titlepage'>Other</section><div id='example' role='book:DOC-EXAMPLE'>Example</div><div id='proof' epub:type='book:PROOF'>Proof</div><div id='aria-proof' role='book:doc-proof'>Ordinary</div><section id='chapter' role='doc-chapter'>Chapter</section><section id='prefixed' role='book:doc-chapter'>Ordinary</section>",
        &mut doc_resources,
    );
    let roles = doc.render_view(&doc_resources).semantics();
    assert!(roles.has_role(
        box_for(&doc, "title", &doc_resources),
        ContentRole::MajorSection
    ));
    assert!(!roles.has_role(
        box_for(&doc, "aria-title", &doc_resources),
        ContentRole::MajorSection
    ));
    for id in ["example", "proof"] {
        assert!(roles.has_role(box_for(&doc, id, &doc_resources), ContentRole::Statement));
        assert!(roles.has_role(
            box_for(&doc, id, &doc_resources),
            ContentRole::CompactStructure
        ));
    }
    assert!(!roles.has_role(
        box_for(&doc, "aria-proof", &doc_resources),
        ContentRole::Statement
    ));
    assert!(roles.has_role(
        box_for(&doc, "chapter", &doc_resources),
        ContentRole::MajorSection
    ));
    assert!(!roles.has_role(
        box_for(&doc, "prefixed", &doc_resources),
        ContentRole::MajorSection
    ));
}

#[test]
fn native_caret_affinity_selects_the_correct_fragment_at_a_shared_boundary() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout("<p>ab</p>", &mut doc_resources);
    let line = doc.render_view(&doc_resources).text().line(0).unwrap();
    let start = line.start();
    let fragment = |offset_x, range| PositionedShapedLine {
        offset_x,
        shaped: ShapedLine {
            line_index: 0,
            text_range: range,
            run: 0,
            ascent: 16.0,
            caret_stops: vec![0.0, 8.0].into(),
            cluster_boundaries: vec![true, true].into(),
        },
    };
    let shaped = [
        fragment(0.0, start..start + 1),
        fragment(40.0, start + 1..start + 2),
    ];
    let geometry = doc.render_view(&doc_resources).geometry();
    assert_eq!(
        geometry.caret_x(0, start + 1, CaretAffinity::Leading, &shaped),
        Some(line.optical_offset_x() + 40.0)
    );
    assert_eq!(
        geometry.caret_x(0, start + 1, CaretAffinity::Trailing, &shaped),
        Some(line.optical_offset_x() + 8.0)
    );
}

#[test]
fn generated_text_addresses_its_owner_and_invalid_element_steps_are_rejected() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout(
        "<style>p::before { content: 'G'; }</style><p>Body</p>",
        &mut doc_resources,
    );
    let view = doc.render_view(&doc_resources);
    let generated = (0..view.text().glyph_count())
        .find(|&glyph| {
            view.text()
                .glyph_at(glyph)
                .and_then(|id| view.text().glyph_metric(id))
                .is_some_and(|metric| metric.ch() == 'G')
        })
        .unwrap() as u32;
    let line = view.text().line_index_for_glyph(generated).unwrap();
    let location = view
        .addressing()
        .source_location_on_line(line, Some(generated))
        .unwrap();
    assert_eq!(location.utf16_offset, None);
    assert!(view.addressing().source_element_y(&location).is_some());
    for path in [vec![0], vec![2, 0], vec![1, 2], vec![usize::MAX - 1]] {
        for allow_end in [false, true] {
            assert_eq!(
                view.addressing()
                    .resolve_source_position(&path, Some(1), Some(0), allow_end),
                None
            );
        }
        assert_eq!(
            view.addressing().source_element_y(&SourceLocation {
                path,
                utf16_offset: None
            }),
            None
        );
    }
    assert_eq!(
        view.addressing()
            .source_location_on_line(usize::MAX, Some(generated)),
        None
    );
}

#[test]
fn whole_word_search_and_word_selection_share_unicode_boundaries() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout("<p>foo_bar can't cafe\u{301} 中文</p>", &mut doc_resources);
    let view = doc.render_view(&doc_resources);
    let options = TextSearchOptions {
        whole_word: true,
        ..TextSearchOptions::default()
    };
    for partial in ["foo", "bar", "can", "t", "caf"] {
        assert!(
            view.text().find_matches(partial, options).is_empty(),
            "{partial}"
        );
    }
    for query in ["foo_bar", "can't", "cafe", "中", "文"] {
        let found = view.text().find_matches(query, options);
        assert_eq!(found.len(), 1, "{query}");
        for glyph in found[0].clone() {
            assert_eq!(
                view.text().word_range_at(glyph),
                Some(found[0].clone()),
                "{query}"
            );
        }
    }
}

#[test]
fn line_bounds_preserve_text_height_and_include_offset_images() {
    for (size, offset) in [(1, 0), (40, 12), (8, -20)] {
        let mut doc_resources = GlyphResourceStore::default();
        let doc = layout(
            &format!(
                "<p style='line-height:30px'>Text<img src='icon' width='{size}' height='{size}' style='position:relative;top:{offset}px'></p>"
            ),
            &mut doc_resources,
        );
        let view = doc.render_view(&doc_resources);
        let image = view.fragments().images().iter().next().unwrap();
        let line = view.text().line(image.line_idx()).unwrap();
        let bounds = view.geometry().line_vertical_bounds(line.index()).unwrap();
        assert!(bounds.start <= line.point_y());
        assert!(bounds.end >= line.point_y() + line.height());
        let top = image.offset().y.max(image.clip().y0);
        let bottom = (image.offset().y + image.size().height).min(image.clip().y1);
        assert!(bounds.start <= line.point_y() + top);
        assert!(bounds.end >= line.point_y() + bottom);
    }
}

#[test]
fn vertical_queries_agree_with_intrinsic_bounds_for_overlapping_layout() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout(
        "<table><caption style='caption-side:bottom'>Caption</caption><tr><td style='line-height:80px'>Tall</td><td>Small<br>Next</td></tr></table><p style='float:left'>Float</p><p>Flow</p><p style='position:relative;top:-30px'>Positioned</p>",
        &mut doc_resources,
    );
    let view = doc.render_view(&doc_resources);
    let geometry = view.geometry();
    let bounds = (0..view.text().line_count())
        .map(|index| geometry.line_vertical_bounds(index).unwrap())
        .collect::<Vec<_>>();
    for y in (0..300).map(f64::from) {
        assert_eq!(
            geometry.start_line_at(y, 0.1),
            bounds
                .iter()
                .position(|bounds| bounds.end > y + 0.1)
                .unwrap_or(bounds.len())
        );
        assert_eq!(
            geometry.end_line_at(y, 0.1),
            bounds.iter().rposition(|bounds| bounds.end <= y + 0.1)
        );
        assert_eq!(
            geometry.line_intersecting_vertical_range(y, y + 2.0),
            bounds
                .iter()
                .position(|bounds| bounds.end >= y && bounds.start <= y + 2.0)
        );
    }
}

#[test]
fn table_cell_intervals_keep_ownership_without_nested_table_text() {
    let mut doc_resources = GlyphResourceStore::default();
    let mut doc = layout(
        "<table id='outer'><tr><td>Outer<table id='inner'><tr><td>Inner</td></tr></table></td><td style='display:block'><em>Other</em></td><td></td></tr></table>",
        &mut doc_resources,
    );
    for width in [600.0, 180.0] {
        doc.relayout(LayoutConstraints::new(width, 16.0).unwrap(), &doc_resources)
            .expect("layout requires the shaper-aware API for first-line styles");
        let view = doc.render_view(&doc_resources);
        let outer = view
            .boxes()
            .table(box_for(&doc, "outer", &doc_resources))
            .unwrap();
        let cells = outer.rows()[0].cells();
        assert_eq!(
            cells[0].glyphs(),
            Some(range(&doc, "Outer", &doc_resources))
        );
        assert_eq!(
            cells[1].glyphs(),
            Some(range(&doc, "Other", &doc_resources))
        );
        assert_eq!(cells[2].glyphs(), None);
        let inner = view
            .boxes()
            .table(box_for(&doc, "inner", &doc_resources))
            .unwrap();
        assert_eq!(
            inner.rows()[0].cells()[0].glyphs(),
            Some(range(&doc, "Inner", &doc_resources))
        );
    }
}
