use html_pipeline::engine::source_queries;
use html_pipeline::engine::reading_queries;
use html_pipeline::engine::semantic_queries;
use html_pipeline::engine::fragment_paint;
use html_engine_model::layout_state;
use html_pipeline::engine::box_queries;
use html_pipeline::engine::export_queries;
use html_pipeline::engine::geometry_queries;
use html_pipeline::engine::search_queries;
use html_pipeline::engine::text_paint;
use html_pipeline::MarkupSyntax;
use crate::export::TableExportFormat;
use html_pipeline::engine::TableExportRequest;
use html_pipeline::engine::TextExportFormat;
use crate::layout::BoxId;

use html_pipeline::parse_document;
use html_pipeline::DocumentTextIndex;
use html_pipeline::engine::{LaidOutDocument, LayoutConstraints};
use crate::layout::{CaretAffinity, ContentRole, LineId, SourceLocation};
use crate::targets::ElementTarget;
use html_pipeline::DocumentFactory;
use crate::text::{GlyphResourceGeneration, GlyphShaper, ShapeError, TextSearchOptions};
use html_pipeline::engine::{GlyphResourceStore, find_text_matches, text_matches};
use crate::text::{GlyphRequest, FontMetrics, GlyphHandle, ShapedGlyph};
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
    fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
        Ok(ShapedGlyph::new(
            GlyphHandle(request.character as u32),
            request.style.font_size / 2.0,
            FontMetrics {
                ascent: request.style.font_size,
                descent: 0.0,
            },
        ))
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
fn box_for(doc: &LaidOutDocument, id: &str, resources: &GlyphResourceStore) -> BoxId {
    let boxes = doc.render_view(resources).boxes();
    (0..box_queries::len(boxes))
        .map(layout_state::box_id_new)
        .find(|&index| box_queries::attribute(boxes, index, "id") == Some(id))
        .unwrap()
}

#[test]
fn logical_characters_retain_transforms_bidi_controls_and_discretionary_hyphens() {
    let mut resources = GlyphResourceStore::default();
    let doc = layout(
        "<p dir='rtl' style='text-transform:uppercase'>a(אב)\u{200f}&shy;🙂</p>",
        &mut resources,
    );
    let view = doc.render_view(&resources);
    let text = view.text();
    let expected = "A(אב)\u{200f}\u{00ad}🙂";
    let characters = (0..text.glyph_count())
        .map(|index| text_paint::render_text_view_character_at(text, index).unwrap())
        .collect::<String>();
    assert_eq!(characters, expected);
    assert_eq!(
        text_paint::render_text_view_character_at(text, text.glyph_count()),
        None
    );
    assert_eq!(
        text_paint::render_text_view_character_at(text, usize::MAX),
        None
    );
    for (index, character) in expected.chars().enumerate() {
        if matches!(character, '\u{200f}' | '\u{00ad}') {
            assert!(text_paint::paint_glyph_at(text, index as u32).is_none());
        }
    }
    assert_eq!(
        reading_queries::render_addressing_view_reading_fragments(view.addressing(), 0)[0].text,
        expected
    );
    assert_eq!(
        reading_queries::reading_page(view.addressing(), 0..u32::MAX, 3).next,
        8
    );
}
fn range(doc: &LaidOutDocument, query: &str, resources: &GlyphResourceStore) -> Range<u32> {
    search_queries::render_text_view_find_matches(doc.render_view(resources)
        .text(), query, TextSearchOptions::default())[0]
        .clone()
}

#[test]
fn parse_and_render_source_addresses_agree_across_text_chunks_and_astral_characters() {
    let body = "<p id='p'>😀<!-- split -->nee<span id='inline'>d</span>le</p>";
    let source = format!("<!doctype html><html><body>{body}</body></html>");
    let parsed = parse_document(&source, MarkupSyntax::Html).unwrap();
    let index = DocumentTextIndex::from_parsed(&parsed).unwrap();
    let mut resources = GlyphResourceStore::default();
    let document = layout(body, &mut resources);
    let addressing = document.render_view(&resources).addressing();
    for query in ["😀", "nee", "d", "le"] {
        let byte = index.text().find(query).unwrap();
        let character = index.text()[..byte].chars().count();
        let indexed = index.source_position_in(character..character + 1).unwrap();
        let glyph = range(&document, query, &resources).start;
        let rendered = addressing.source_position_for_glyph(glyph, false).unwrap();
        assert_eq!(
            rendered
                .elements()
                .map(|step| step.step())
                .collect::<Vec<_>>(),
            indexed.element_steps()
        );
        assert_eq!(rendered.text_step(), indexed.text_step());
        assert_eq!(rendered.utf16_offset(), indexed.utf16_offset());
        assert_eq!(
            addressing.resolve_source_position(
                indexed.element_steps(),
                Some(indexed.text_step()),
                Some(indexed.utf16_offset()),
                false
            ),
            Some(glyph)
        );
        let after = addressing.source_position_for_glyph(glyph, true).unwrap();
        assert_eq!(
            after.utf16_offset(),
            indexed.utf16_offset() + query.chars().next().unwrap().len_utf16()
        );
        assert!(rendered.elements().any(|step| step.id() == Some("p")));
    }
    let dom = parsed.into_dom();
    let emoji = dom
        .node_ids()
        .find(|&node| dom.text_ref(node).is_some_and(|text| text.text() == "😀"))
        .unwrap();
    assert!(
        dom.source_position_for_text_node(emoji, 1, true).is_none(),
        "a surrogate's second code unit is not a character boundary"
    );
}

#[test]
fn element_and_text_chunk_addresses_share_utf16_offset_resolution() {
    let mut resources = GlyphResourceStore::default();
    let document = layout(
        "<p>before</p><p style='text-transform:uppercase'>a   😀<!-- split -->ßZ</p>",
        &mut resources,
    );
    let addressing = document.render_view(&resources).addressing();
    let start = range(&document, "A 😀SSZ", &resources).start;
    let position = addressing.source_position_for_glyph(start, false).unwrap();
    let elements = position
        .elements()
        .map(|element| element.step())
        .collect::<Vec<_>>();
    for offset in [0, 1, 2, 3, 4, 5, 6, 7, 8, usize::MAX] {
        for allow_end in [false, true] {
            assert_eq!(
                addressing.resolve_source_position(&elements, None, Some(offset), allow_end),
                addressing.resolve_source_position(
                    &elements,
                    Some(position.text_step()),
                    Some(offset),
                    allow_end,
                ),
                "offset {offset}, allow_end {allow_end}",
            );
        }
    }
}

#[test]
fn element_addresses_choose_the_first_source_descendant_in_dom_order() {
    let mut resources = GlyphResourceStore::default();
    let document = layout(
        "<style>p::before { content:'generated'; }</style><p><span style='display:none'>hidden</span><span>first😀</span>later</p>",
        &mut resources,
    );
    let addressing = document.render_view(&resources).addressing();
    let first = range(&document, "first", &resources).start;
    let position = addressing.source_position_for_glyph(first, false).unwrap();
    let mut elements = position
        .elements()
        .map(|element| element.step())
        .collect::<Vec<_>>();
    elements.pop();
    assert_eq!(
        addressing.resolve_source_position(&elements, None, None, false),
        Some(first)
    );
}

#[test]
fn parse_only_excerpts_preserve_unicode_and_collapsed_context() {
    let index = crate::search::SearchIndex::from_source(
        "<p>😀  Ca<em>fé</em></p><p>e\u{301}\u{a0}終</p>",
        MarkupSyntax::Html,
    )
    .unwrap();
    let characters: Vec<_> = index.text().chars().collect();
    for start in 0..characters.len() {
        for end in start + 1..=characters.len() {
            for (before, after) in [(0, 0), (1, 2), (usize::MAX, usize::MAX)] {
                let left = start.saturating_sub(before);
                let right = end.saturating_add(after).min(characters.len());
                let expected: String = characters[left..right].iter().collect();
                let expected = expected.split_whitespace().collect::<Vec<_>>().join(" ");
                assert_eq!(index.excerpt(start..end, before, after), Some(expected));
            }
        }
    }
    assert_eq!(index.excerpt(0..0, 1, 1), None);
    assert_eq!(index.excerpt(1..0, 1, 1), None);
    assert_eq!(index.excerpt(0..usize::MAX, 0, 0), None);
}

#[test]
fn semantic_roles_cover_structures_and_ancestry_without_markup_queries() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout(
        "<h2 id='heading'>Title</h2><ol id='list'><li id='item'><em id='inline'>Item</em></li></ol><pre id='code'>x</pre><figure id='figure'><figcaption id='caption'>Caption</figcaption></figure><table id='table'><tr><th id='cell'>Header</th></tr></table>",
        &mut doc_resources,
    );
    let semantics = semantic_queries::render_view_semantics(doc.render_view(&doc_resources));
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
            semantic_queries::render_semantics_view_has_role(
                semantics,
                box_for(&doc, id, &doc_resources),
                role
            ),
            "{id}: {role:?}"
        );
    }
    assert_eq!(
        semantic_queries::content_semantics_heading_level(semantics.content(box_for(
            &doc,
            "heading",
            &doc_resources
        ))),
        Some(2)
    );
    assert_eq!(
        semantic_queries::render_semantics_view_nearest(
            semantics,
            box_for(&doc, "inline", &doc_resources),
            ContentRole::ListItem
        ),
        Some(box_for(&doc, "item", &doc_resources))
    );
    assert!(!semantic_queries::render_semantics_view_contains(
        semantics,
        layout_state::box_id_new(u32::MAX as usize),
        box_for(&doc, "inline", &doc_resources)
    ));
    assert_eq!(
        semantic_queries::render_semantics_view_block_container(
            semantics,
            layout_state::box_id_new(u32::MAX as usize)
        ),
        None
    );
}

#[test]
fn epub_and_aria_roles_preserve_exact_and_vocabulary_token_rules() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout(
        "<section id='chapter' epub:type='chapter'>Chapter</section><section id='prefixed' epub:type='book:chapter'>Other</section><aside id='note' role='doc-footnote'>Note</aside><div id='sidebar' role='complementary'>Sidebar</div><div id='statement' epub:type='book:theorem'><p id='label' epub:type='book:label'>Theorem</p>Proof</div>",
        &mut doc_resources,
    );
    let roles = semantic_queries::render_view_semantics(doc.render_view(&doc_resources));
    assert!(semantic_queries::render_semantics_view_has_role(
        roles,
        box_for(&doc, "chapter", &doc_resources),
        ContentRole::MajorSection
    ));
    assert!(!semantic_queries::render_semantics_view_has_role(
        roles,
        box_for(&doc, "prefixed", &doc_resources),
        ContentRole::MajorSection
    ));
    assert!(semantic_queries::render_semantics_view_has_role(
        roles,
        box_for(&doc, "note", &doc_resources),
        ContentRole::Note
    ));
    assert!(!semantic_queries::render_semantics_view_has_role(
        roles,
        box_for(&doc, "note", &doc_resources),
        ContentRole::Sidebar
    ));
    assert!(semantic_queries::render_semantics_view_has_role(
        roles,
        box_for(&doc, "sidebar", &doc_resources),
        ContentRole::Sidebar
    ));
    assert!(semantic_queries::render_semantics_view_has_role(
        roles,
        box_for(&doc, "statement", &doc_resources),
        ContentRole::CompactStructure
    ));
    assert!(semantic_queries::render_semantics_view_has_role(
        roles,
        box_for(&doc, "statement", &doc_resources),
        ContentRole::Statement
    ));
    assert!(semantic_queries::render_semantics_view_has_role(
        roles,
        box_for(&doc, "label", &doc_resources),
        ContentRole::Label
    ));
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
    assert!(semantic_queries::render_semantics_view_has_role(
        semantic_queries::render_view_semantics(doc.render_view(&doc_resources)),
        box_for(&doc, "chapter", &doc_resources),
        ContentRole::MajorSection
    ));
}

#[test]
fn xml_epub_verse_preserves_authored_spacing_with_reader_controls() {
    use crate::style::ReaderStyleOverrides;

    let parsed = parse_document(
        "<html xmlns='http://www.w3.org/1999/xhtml' xmlns:book='http://www.idpf.org/2007/ops'><body style='margin:0;line-height:1'><div book:type='z3998:verse' style='text-indent:12px'>First.<br/><br/>Second.</div></body></html>",
        MarkupSyntax::Xml,
    ).unwrap();
    let mut reference = None;
    for controls in [false, true] {
        let mut factory = DocumentFactory::new();
        if controls {
            factory.set_reader_overrides(ReaderStyleOverrides {
                paragraph_spacing_em: Some(1.0),
                first_line_indent_em: Some(2.0),
                ..Default::default()
            });
        }
        let mut resources = GlyphResourceStore::default();
        let document = factory
            .build_pipeline_from_parsed(&parsed, &[])
            .shape(&mut TestShaper::default(), &mut resources)
            .unwrap()
            .layout(LayoutConstraints::new(600.0, 16.0).unwrap(), &resources)
            .unwrap();
        let geometry = document
            .render_view(&resources)
            .text()
            .lines()
            .iter()
            .map(|line| (line.point(), line.height()))
            .collect::<Vec<_>>();
        if let Some(reference) = &reference {
            assert_eq!(&geometry, reference);
        } else {
            reference = Some(geometry);
        }
    }
}

#[test]
fn anchors_keep_empty_inline_fallbacks_inside_their_block() {
    let mut resources = GlyphResourceStore::default();
    let mut doc = layout(
        "<p>outside</p><p id='paragraph'><a id='before'></a><span>first</span><a id='middle'></a><span>last</span><a id='after'></a></p><p id='empty'></p><img id='image' src='cover.png' style='display:block;width:20px;height:30px'><p><span id='repeat'>one</span><span id='repeat'>second</span></p>",
        &mut resources,
    );
    let first = range(&doc, "first", &resources);
    let last = range(&doc, "last", &resources);
    let second = range(&doc, "second", &resources);
    for width in [600.0, 45.0] {
        doc.relayout(LayoutConstraints::new(width, 16.0).unwrap(), &resources)
            .unwrap();
        let view = doc.render_view(&resources);
        let addressing = view.addressing();
        for (id, glyph) in [
            ("paragraph", first.start),
            ("before", first.start),
            ("middle", last.start),
            ("after", last.end - 1),
            ("repeat", second.start),
        ] {
            let y = match addressing.element(id).unwrap() {
                ElementTarget::Text { y, .. } => y,
                ElementTarget::Position { y, .. } => Some(y),
                ElementTarget::Note => None,
            };
            let line = text_paint::render_text_view_line_for_glyph(view.text(), glyph).unwrap();
            assert_eq!(y, Some(view.text().line(line).unwrap().point().y), "{id}");
        }
        for id in ["empty", "image"] {
            assert!(
                matches!(addressing.element(id), Some(ElementTarget::Position { .. })),
                "{id}"
            );
        }
    }
    let doc = layout(
        "<style>body{margin:0}</style><div style='height:40px'><div id='zero' style='height:0'></div></div>",
        &mut resources,
    );
    let view = doc.render_view(&resources);
    assert_eq!(
        view.addressing().element("zero"),
        Some(ElementTarget::Position {
            y: 0.0,
            is_note: false
        }),
    );
}

#[test]
fn source_addresses_resolve_images_and_text_across_reflow() {
    let mut doc_resources = GlyphResourceStore::default();
    let mut doc = layout(
        "<p><img src='cover.png' width='10' height='10'>Before 😀 after</p>",
        &mut doc_resources,
    );
    let selected = range(&doc, "after", &doc_resources);
    let line = text_paint::render_text_view_line_for_glyph(
        doc.render_view(&doc_resources).text(),
        selected.start,
    )
    .unwrap();
    let location = source_queries::source_location_on_line(
        doc.render_view(&doc_resources).addressing(),
        line,
        Some(selected.start),
    )
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
    let line = text_paint::render_text_view_line_for_glyph(
        doc.render_view(&doc_resources).text(),
        selected.start,
    )
    .unwrap();
    let location = source_queries::source_location_on_line(
        doc.render_view(&doc_resources).addressing(),
        line,
        Some(selected.start),
    )
    .unwrap();
    assert_eq!(location, expected);
    let mut element = location.clone();
    element.path.pop();
    element.utf16_offset = None;
    assert!(
        source_queries::render_addressing_view_source_element_y(
            doc.render_view(&doc_resources).addressing(),
            &element
        )
        .is_some()
    );
    assert_eq!(
        source_queries::render_addressing_view_source_element_y(
            doc.render_view(&doc_resources).addressing(),
            &location
        ),
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
                    search_queries::render_text_view_word_range_at(doc.render_view(&doc_resources).text(), glyph),
                    Some(selected.clone()),
                    "{word}"
                );
            }
        }
    }
}

#[test]
fn indexed_words_exclude_hidden_and_generated_source_even_inside_a_word() {
    let mut resources = GlyphResourceStore::default();
    let doc = layout(
        "<style>p::before{content:'Generated'}</style><p>hel<span style='visibility:hidden'>Secret</span><em>lo</em> cafe\u{301} 👩‍👧 🇸🇪 can't foo_bar 中文!</p><p style='visibility:hidden'>Ghost<span style='visibility:visible'>alive</span></p>",
        &mut resources,
    );
    let view = doc.render_view(&resources);
    let visible: std::collections::HashSet<_> =
        text_paint::render_text_view_visible_source_runs(view.text())
            .flat_map(|run| run.glyphs())
            .collect();
    for word in [
        "hello",
        "cafe\u{301}",
        "👩‍👧",
        "🇸🇪",
        "can't",
        "foo_bar",
        "中",
        "文",
        "!",
        "alive",
    ] {
        let selected = if word == "hello" {
            range(&doc, "hel", &resources).start..range(&doc, "lo", &resources).end
        } else {
            range(&doc, word, &resources)
        };
        for glyph in selected.clone().filter(|glyph| visible.contains(glyph)) {
            assert_eq!(
                search_queries::render_text_view_word_range_at(view.text(), glyph),
                Some(selected.clone()),
                "{word}"
            );
        }
    }
    for glyph in 0..view.text().glyph_count() as u32 {
        if !visible.contains(&glyph) {
            assert_eq!(
                search_queries::render_text_view_word_range_at(view.text(), glyph),
                None,
                "non-source or hidden glyph {glyph}"
            );
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
        search_queries::render_text_view_find_matches(text, "target", TextSearchOptions::default())
            .len(),
        2
    );
    for omitted in ["Generated", "Secret"] {
        assert!(
            search_queries::render_text_view_find_matches(text, omitted, TextSearchOptions::default())
                .is_empty()
        );
        assert_eq!(search_queries::render_text_view_locate_quote(text, omitted, None, None), None);
    }
    let selected = search_queries::render_text_view_locate_quote(text, "target", Some("Second "), Some(" two."))
        .unwrap();
    assert_eq!(
        export_queries::render_export_view_text(doc.render_view(&doc_resources)
            .export(), selected.clone(), TextExportFormat::PlainAndMarkdown)
            .plain
            .as_deref(),
        Some("target")
    );
    assert_eq!(
        selected,
        search_queries::render_text_view_find_matches(text, "target", TextSearchOptions::default())[1]
    );
    assert_eq!(search_queries::render_text_view_locate_quote(text, "target", Some("missing"), None), None);
}

#[test]
fn search_iterators_own_inputs_and_preserve_overlaps_and_original_graphemes() {
    let mut matches = {
        let text = "İİİ 😀 Cafe\u{301}".to_owned();
        let query = "ii".to_owned();
        text_matches(&text, &query, TextSearchOptions::default())
    };
    assert_eq!(matches.next(), Some((0, 2)));
    assert_eq!(matches.next(), Some((1, 3)));
    assert_eq!(matches.next(), None);
    assert_eq!(matches.next(), None);
    assert_eq!(
        text_matches("İİİ 😀 Cafe\u{301}", "cafe", TextSearchOptions::default())
            .collect::<Vec<_>>(),
        vec![(6, 11)]
    );
    for (text, query) in [
        ("", "a"),
        ("a", "longer"),
        ("abc", ""),
        ("abc", "\u{301}"),
        ("abc", "a\0"),
    ] {
        assert_eq!(
            text_matches(text, query, TextSearchOptions::default()).next(),
            None
        );
    }
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
        let line = view.text().line(LineId::new(0)).unwrap();
        let geometry = geometry_queries::render_view_geometry(view);
        for glyph in text_paint::render_line_glyphs(&line).chain(std::iter::once(line.end())) {
            let x = geometry
                .caret_x(LineId::new(0), glyph, CaretAffinity::Leading)
                .unwrap();
            assert_eq!(
                geometry_queries::render_geometry_view_hit_test_line(geometry, LineId::new(0), x),
                Some(glyph)
            );
        }
        let (x0, x1) = geometry
            .caret_span(LineId::new(0), text_paint::render_line_glyphs(&line))
            .unwrap();
        let rects: Vec<_> = geometry_queries::render_geometry_view_line_selection_rects(
            geometry,
            LineId::new(0),
            text_paint::render_line_glyphs(&line),
        )
        .collect();
        let bounds = rects
            .iter()
            .copied()
            .reduce(|left, right| left.union(right))
            .unwrap();
        assert_eq!(bounds.x0, line.point().x + x0);
        assert_eq!(bounds.x1, line.point().x + x1);
        assert_eq!(bounds.y0, line.point().y);
        assert_eq!(
            geometry_queries::render_geometry_view_hit_test_line(
                geometry,
                LineId::new(0),
                f64::NAN
            ),
            None
        );
        assert_eq!(
            geometry.caret_x(
                LineId::new(u32::MAX as usize - 1),
                0,
                CaretAffinity::Leading
            ),
            None
        );
    }
}

#[test]
fn unstyled_table_export_preserves_spans_and_escapes_text() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout(
        "<table id='table' style='color:red'><tr><th colspan='2'>A &amp; B</th></tr><tr><td rowspan='2'>&lt;tag&gt;</td><td>x<br>y<span> z&#x2003; w </span><span></span>end<table><tr><td>nested</td></tr></table></td></tr><tr><td>y</td></tr></table>",
        &mut doc_resources,
    );
    let table = box_queries::render_box_view_table(doc
        .render_view(&doc_resources)
        .boxes(), box_for(&doc, "table", &doc_resources))
        .unwrap();
    assert_eq!(
        table.export(TableExportRequest::Whole(TableExportFormat::Html)).unwrap(),
        "<table><tr><th colspan=\"2\">A &amp; B</th></tr><tr><td rowspan=\"2\">&lt;tag&gt;</td><td>x y z w end</td></tr><tr><td>y</td></tr></table>"
    );
}

#[test]
fn book_roles_keep_epub_and_aria_vocabulary_boundaries() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout(
        "<section id='title' epub:type='titlepage'>Title</section><section id='aria-title' role='doc-titlepage'>Other</section><div id='example' role='book:DOC-EXAMPLE'>Example</div><div id='proof' epub:type='book:PROOF'>Proof</div><div id='aria-proof' role='book:doc-proof'>Ordinary</div><section id='chapter' role='doc-chapter'>Chapter</section><section id='prefixed' role='book:doc-chapter'>Ordinary</section>",
        &mut doc_resources,
    );
    let roles = semantic_queries::render_view_semantics(doc.render_view(&doc_resources));
    assert!(semantic_queries::render_semantics_view_has_role(
        roles,
        box_for(&doc, "title", &doc_resources),
        ContentRole::MajorSection
    ));
    assert!(!semantic_queries::render_semantics_view_has_role(
        roles,
        box_for(&doc, "aria-title", &doc_resources),
        ContentRole::MajorSection
    ));
    for id in ["example", "proof"] {
        assert!(semantic_queries::render_semantics_view_has_role(
            roles,
            box_for(&doc, id, &doc_resources),
            ContentRole::Statement
        ));
        assert!(semantic_queries::render_semantics_view_has_role(
            roles,
            box_for(&doc, id, &doc_resources),
            ContentRole::CompactStructure
        ));
    }
    assert!(!semantic_queries::render_semantics_view_has_role(
        roles,
        box_for(&doc, "aria-proof", &doc_resources),
        ContentRole::Statement
    ));
    assert!(semantic_queries::render_semantics_view_has_role(
        roles,
        box_for(&doc, "chapter", &doc_resources),
        ContentRole::MajorSection
    ));
    assert!(!semantic_queries::render_semantics_view_has_role(
        roles,
        box_for(&doc, "prefixed", &doc_resources),
        ContentRole::MajorSection
    ));
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
            text_paint::glyph_at(view.text(), glyph)
                .and_then(|id| text_paint::glyph_metric(view.text(), id))
                .is_some_and(|metric| metric.ch() == 'G')
        })
        .unwrap() as u32;
    for glyph in [generated, view.text().glyph_count() as u32, u32::MAX] {
        for after in [false, true] {
            assert!(
                view.addressing()
                    .source_position_for_glyph(glyph, after)
                    .is_none(),
                "glyph {glyph}, after={after}"
            );
        }
    }
    let line = text_paint::render_text_view_line_for_glyph(view.text(), generated).unwrap();
    let location =
        source_queries::source_location_on_line(view.addressing(), line, Some(generated)).unwrap();
    assert_eq!(location.utf16_offset, None);
    assert!(
        source_queries::render_addressing_view_source_element_y(view.addressing(), &location)
            .is_some()
    );
    for path in [vec![0], vec![2, 0], vec![1, 2], vec![usize::MAX - 1]] {
        for allow_end in [false, true] {
            assert_eq!(
                view.addressing()
                    .resolve_source_position(&path, Some(1), Some(0), allow_end),
                None
            );
        }
        assert_eq!(
            source_queries::render_addressing_view_source_element_y(
                view.addressing(),
                &SourceLocation {
                    path,
                    utf16_offset: None
                }
            ),
            None
        );
    }
    assert_eq!(
        source_queries::source_location_on_line(
            view.addressing(),
            LineId::new(u32::MAX as usize - 1),
            Some(generated)
        ),
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
            search_queries::render_text_view_find_matches(view.text(), partial, options).is_empty(),
            "{partial}"
        );
    }
    for query in ["foo_bar", "can't", "cafe", "中", "文"] {
        let found = search_queries::render_text_view_find_matches(view.text(), query, options);
        assert_eq!(found.len(), 1, "{query}");
        for glyph in found[0].clone() {
            assert_eq!(
                search_queries::render_text_view_word_range_at(view.text(), glyph),
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
        let image = fragment_paint::render_fragment_view_images(view.fragments())
            .iter()
            .next()
            .unwrap();
        let line = view
            .text()
            .line(fragment_paint::render_image_fragment_line(&image))
            .unwrap();
        let bounds = geometry_queries::render_geometry_view_line_vertical_bounds(
            geometry_queries::render_view_geometry(view),
            line.id(),
        )
        .unwrap();
        assert!(bounds.start <= line.point().y);
        assert!(bounds.end >= line.point().y + line.height());
        let top = fragment_paint::render_image_fragment_offset(&image)
            .y
            .max(fragment_paint::render_image_fragment_clip(&image).y0);
        let bottom = (fragment_paint::render_image_fragment_offset(&image).y
            + fragment_paint::render_image_fragment_size(&image).height)
            .min(fragment_paint::render_image_fragment_clip(&image).y1);
        assert!(bounds.start <= line.point().y + top);
        assert!(bounds.end >= line.point().y + bottom);
    }
}

#[test]
fn vertical_queries_agree_with_intrinsic_bounds_for_overlapping_layout() {
    let mut doc_resources = GlyphResourceStore::default();
    let doc = layout(
        "<table><caption style='caption-side:bottom'>Caption</caption><tr><td style='line-height:80px'>Tall</td><td>Small<br>Next</td></tr></table><p style='float:left'>Float</p><p>Flow</p><p style='position:relative;top:-30px'>Positioned</p><p>Images<img src='low' width='12' height='8' style='position:relative;top:140px'><img src='high' width='12' height='8' style='position:relative;top:70px'><img src='empty' width='0' height='20' style='position:relative;top:300px'></p><p>Last</p>",
        &mut doc_resources,
    );
    let view = doc.render_view(&doc_resources);
    let geometry = geometry_queries::render_view_geometry(view);
    let bounds = text_paint::render_text_view_spatial_order(view.text())
        .iter()
        .map(|&id| {
            geometry_queries::render_geometry_view_line_vertical_bounds(geometry, id).unwrap()
        })
        .collect::<Vec<_>>();
    for y in (0..500).map(f64::from) {
        assert_eq!(
            geometry_queries::render_geometry_view_start_spatial_rank_at(geometry, y, 0.1),
            bounds
                .iter()
                .position(|bounds| bounds.end > y + 0.1)
                .unwrap_or(bounds.len())
        );
        assert_eq!(
            geometry_queries::render_geometry_view_end_spatial_rank_at(geometry, y, 0.1),
            bounds.iter().rposition(|bounds| bounds.end <= y + 0.1)
        );
        assert_eq!(
            geometry_queries::render_geometry_view_line_intersecting_vertical_range(
                geometry,
                y,
                y + 2.0
            ),
            bounds
                .iter()
                .position(|bounds| bounds.end >= y && bounds.start <= y + 2.0)
                .map(|rank| text_paint::render_text_view_spatial_order(view.text())[rank])
        );
    }
}

#[test]
fn table_queries_keep_reserved_columns_when_spanning_cells_overlap() {
    for (rowspan, colspan) in [("3", "2"), (" 3 ", " 2 ")] {
        let mut resources = GlyphResourceStore::default();
        let document = layout(
            &format!(
                "<table id='grid'><tr><td>A</td><td rowspan='{rowspan}'>B</td></tr><tr><td colspan='{colspan}'>C</td></tr><tr><td>D</td><td>E</td></tr></table>",
            ),
            &mut resources,
        );
        let table = box_queries::render_box_view_table(document
            .render_view(&resources)
            .boxes(), box_for(&document, "grid", &resources))
            .unwrap();
        assert_eq!(table.column_count(), 3);
        assert_eq!(table.rows()[0].cells()[1].rowspan(), 3);
        assert_eq!(table.rows()[1].cells()[0].colspan(), 2);
        assert_eq!(table.rows()[2].cells()[1].column(), 2);
    }
}

#[test]
fn table_copy_classifies_spans_with_the_same_rules_as_layout() {
    for (span, complex) in [(" 2 ", true), ("0", false), ("-1", false), (" 1 ", false)] {
        let mut resources = GlyphResourceStore::default();
        let document = layout(
            &format!(
                "<table><tr><td colspan='{span}'>Merged</td></tr><tr><td>Left</td><td>Right</td></tr></table>",
            ),
            &mut resources,
        );
        let view = document.render_view(&resources);
        let markdown = export_queries::render_export_view_text(view
            .export(), 0..view.text().glyph_count() as u32, TextExportFormat::Markdown).markdown
            .unwrap();
        assert_eq!(markdown.contains("<table>"), complex, "colspan={span}");
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
        let outer = box_queries::render_box_view_table(view
            .boxes(), box_for(&doc, "outer", &doc_resources))
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
        let inner = box_queries::render_box_view_table(view
            .boxes(), box_for(&doc, "inner", &doc_resources))
            .unwrap();
        assert_eq!(
            inner.rows()[0].cells()[0].glyphs(),
            Some(range(&doc, "Inner", &doc_resources))
        );
    }
}
