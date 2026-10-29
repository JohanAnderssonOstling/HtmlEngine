use html::document::ImageSource;
use html::document::MarkupSyntax;
use html::engine::{DocumentOptions, Engine};
use html::geometry::LinePlacement;
use html::geometry::WordHitPolicy;
use html::pipeline::PipelineChanges;
use html::render::{DocumentPainter, FragmentRenderOptions, RecordingPainter, RenderScene};
use html::resources::{
    ImageLoadCompletion, ImageLoader, ImageService, InlineJobSpawner, ResourceMetadata,
    ResourceProvider,
};
use html::search::{SearchIndex, SearchResults};
use html::selection::{SelectionAnchor, SelectionText, TableCellRange};
use html::structure::{BoundaryIndex, HeaderPolicy, TableGroupKind, TableScratch};
use html::text::{
    GlyphMetric, GlyphRequest, GlyphResourceGeneration, GlyphShaper, ShapeError, ShapedGlyph,
    TextSearchOptions,
};
use kurbo::{Point, Rect, Size, Vec2};
use std::collections::{HashMap, HashSet};
use std::io;
use std::ops::Range;
use std::sync::Arc;

#[derive(Default)]
struct Shaper(GlyphResourceGeneration);

impl GlyphShaper for Shaper {
    // Handles are character codes, so this fixture has no native append state.
    fn begin_append_shaping(&mut self) -> Result<(), ShapeError> {
        Ok(())
    }

    fn reset(&mut self) {
        self.0 = GlyphResourceGeneration::default();
    }

    fn resource_generation(&self) -> GlyphResourceGeneration {
        self.0
    }

    fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
        Ok(ShapedGlyph::from_metric(
            request.character as u32,
            GlyphMetric::try_new(
                request.character,
                request.font_size / 2.0,
                request.font_size,
                0.0,
            )?,
        ))
    }
}

struct Provider {
    files: HashMap<String, Vec<u8>>,
    metadata: ResourceMetadata,
}

impl ResourceProvider for Provider {
    fn read_bytes(&self, uri: &str) -> io::Result<Vec<u8>> {
        self.files
            .get(uri)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, uri.to_owned()))
    }

    fn metadata(&self, uri: &str) -> io::Result<ResourceMetadata> {
        if uri.ends_with(".css") {
            Ok(ResourceMetadata {
                media_type: Some("text/css".into()),
                charset: None,
            })
        } else {
            Ok(self.metadata.clone())
        }
    }

    fn exists(&self, uri: &str) -> bool {
        self.files.contains_key(uri)
    }

    fn resolve(&self, base: &str, href: &str) -> String {
        base.rsplit_once('/')
            .map_or_else(|| href.to_owned(), |(dir, _)| format!("{dir}/{href}"))
    }

    fn list_html_candidates(&self, _root: &str) -> io::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

fn provider(source: &str) -> Arc<dyn ResourceProvider> {
    Arc::new(Provider {
        files: HashMap::from([("document.html".into(), source.as_bytes().to_vec())]),
        metadata: ResourceMetadata::default(),
    })
}

fn load(source: &str) -> Engine {
    Engine::load(
        provider(source),
        "document.html",
        DocumentOptions::new(600.0),
        &mut Shaper::default(),
    )
    .unwrap()
}

fn find(engine: &Engine, text: &str) -> Range<u32> {
    engine
        .render_view()
        .unwrap()
        .text()
        .find_matches(text, TextSearchOptions::default())[0]
        .clone()
}

#[test]
fn disjoint_line_export_deduplicates_and_preserves_source_structure() {
    let engine = load(
        "<p>one <b>two</b></p><p>skip</p><p>three</p><p style='visibility:hidden'>hidden</p><ol><li>item</li></ol>",
    );
    let content = engine.content().unwrap();
    let one = find(&engine, "one");
    let three = find(&engine, "three");
    let selected = content
        .text_for_ranges([three.clone(), one.clone(), one])
        .unwrap();
    assert!(selected.starts_with("one") && selected.ends_with("three"));
    assert!(!selected.contains("skip") && selected.contains('\n'));
    let view = engine.render_view().unwrap();
    let lines: Vec<_> = (0..view.text().line_count())
        .rev()
        .flat_map(|line| [line, line])
        .collect();
    let all = content.text_for_lines(lines).unwrap();
    assert_eq!(all.matches("one").count(), 1);
    assert!(!all.contains("hidden") && !all.contains("1."));
    assert_eq!(
        content.statistics().non_whitespace_characters,
        html::content::TextStatistics::from_text(&all).non_whitespace_characters
    );
}

#[test]
fn intrinsic_reading_regions_retain_cells_and_translate_without_columns() {
    let engine = load(
        "<table><tr><td>left</td><td>right</td></tr></table><p style='visibility:hidden'>hidden</p>",
    );
    let view = engine.render_view().unwrap();
    let line = view
        .text()
        .line_index_for_glyph(find(&engine, "left").start)
        .unwrap();
    let geometry = engine.geometry().unwrap();
    let natural = geometry.reading_line(line).unwrap();
    assert!(natural.cell.is_some());
    let original = view.text().line(line).unwrap().point();
    let delta = Vec2::new(250.0, 60.0);
    let translated = geometry
        .placed_reading_line(
            LinePlacement {
                line,
                origin: original + delta,
            },
            Rect::new(250.0, 60.0, 850.0, 200.0),
        )
        .unwrap();
    assert_eq!(translated.bounds, natural.bounds + delta);
    assert_eq!(translated.cell, natural.cell);
    assert_eq!(translated.glyphs, natural.glyphs);
    for reading in (0..view.text().line_count()).filter_map(|index| geometry.reading_line(index)) {
        assert!(
            !view
                .export()
                .plain_text(reading.glyphs)
                .unwrap_or_default()
                .contains("hidden")
        );
    }
}

#[test]
fn word_hits_respect_ink_and_caller_selected_lines() {
    let engine = load("<p>first word</p><p>next</p>");
    let geometry = engine.geometry().unwrap();
    let range = find(&engine, "word");
    let rect = geometry.selection_rects(range.clone())[0];
    assert_eq!(
        geometry.word_at(rect.center(), WordHitPolicy::InkOnly),
        Some(range.clone())
    );
    assert!(
        geometry
            .word_at(Point::new(-100.0, -100.0), WordHitPolicy::InkOnly)
            .is_none()
    );
    let text = engine.render_view().unwrap().text();
    let line = text.line_index_for_glyph(range.start).unwrap();
    let placements = [LinePlacement {
        line,
        origin: text.line(line).unwrap().point(),
    }];
    let nearest = geometry
        .word_at_lines(
            Point::new(rect.x1 + 100.0, rect.y1 + 100.0),
            placements,
            WordHitPolicy::Nearest,
        )
        .unwrap();
    assert!(nearest.end <= text.line(line).unwrap().end());
    let fixed = SelectionAnchor::new(range.clone());
    assert_eq!(fixed.extend_to(0..2), 0..range.end);
    let handle = SelectionAnchor::new(5..5);
    assert_eq!(handle.extend_to(1..3), 1..5);
    assert_eq!(handle.extend_to(8..10), 5..10);
}

#[test]
fn structural_selection_uses_the_same_cells_for_geometry_and_containment() {
    let engine = load(
        "<p>before</p><table><tr><td rowspan='2'>span</td><td>a</td></tr><tr><td>b</td></tr></table><p>after</p>",
    );
    let selected = find(&engine, "span");
    let plan = engine
        .selection(selected.clone())
        .unwrap()
        .with_semantics()
        .geometry_plan();
    assert!(!plan.cell_boxes.is_empty());
    assert!(
        !plan
            .text_ranges
            .iter()
            .any(|range| range.start < selected.end && range.end > selected.start)
    );
    let view = engine.render_view().unwrap();
    let cell_box = plan.cell_boxes[0];
    let rect = Rect::from_origin_size(
        view.boxes().point(cell_box).unwrap(),
        view.boxes().size(cell_box).unwrap(),
    );
    assert!(
        engine
            .geometry()
            .unwrap()
            .selection_contains(&plan, rect.center())
    );
    assert!(
        !engine
            .geometry()
            .unwrap()
            .selection_contains(&plan, Point::new(-1.0, -1.0))
    );
    assert!(
        engine
            .geometry()
            .unwrap()
            .selection_contains_lines(&plan, Point::ZERO, [], |index| index == cell_box)
    );
    let table = (0..view.boxes().len())
        .find_map(|index| view.boxes().table(index))
        .unwrap();
    let range = TableCellRange {
        table_box: table.box_idx(),
        row_start: 1,
        row_end: 1,
        column_start: 0,
        column_end: 0,
    };
    let cells: Vec<_> = range.cells(&table).collect();
    assert_eq!(cells.len(), 1);
    assert_eq!(cells[0].rowspan(), 2);
    assert_eq!(cells[0].text(), "span");
    assert_eq!(
        TableCellRange {
            table_box: usize::MAX,
            ..range
        }
        .cells(&table)
        .count(),
        0
    );
}

#[test]
fn parse_only_search_exposes_unicode_source_addresses_and_explicit_statistics() {
    let source = "<p>😀 <b>Café</b> tail</p><p style='display:none'>hidden</p>";
    let index = SearchIndex::from_source(source, MarkupSyntax::Html).unwrap();
    let hits: Vec<_> = index.find("cafe", TextSearchOptions::default()).collect();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].range.end - hits[0].range.start, 4);
    assert!(!hits[0].source.element_steps().is_empty());
    assert_eq!(hits[0].source.utf16_offset(), 0);
    assert_eq!(
        index.excerpt(hits[0].range.clone(), 2, 5).as_deref(),
        Some("😀 Café tail")
    );
    assert_eq!(
        index.find("hidden", TextSearchOptions::default()).count(),
        1,
        "parse-only search does not evaluate CSS"
    );
    assert!(
        load(source)
            .search()
            .unwrap()
            .find("hidden", TextSearchOptions::default())
            .ranges()
            .is_empty()
    );
    assert_eq!(
        index.statistics(),
        html::content::TextStatistics::from_text(index.text())
    );
}

#[test]
fn structural_boundaries_include_empty_elements_and_authored_constraints() {
    let engine = load(
        "<p>alpha</p><div id='empty' style='break-before:page'></div><p id='flow' style='orphans:3;widows:4;break-inside:avoid'>beta</p>",
    );
    let view = engine.render_view().unwrap();
    let structure = engine.structure().unwrap();
    let mut index = BoundaryIndex::default();
    structure.index_lines(0..view.text().line_count(), &mut index);
    let empty = (0..view.boxes().len())
        .find(|&box_idx| view.boxes().attribute(box_idx, "id") == Some("empty"))
        .unwrap();
    let flow = (0..view.boxes().len())
        .find(|&box_idx| view.boxes().attribute(box_idx, "id") == Some("flow"))
        .unwrap();
    let beta_line = view
        .text()
        .line_index_for_glyph(find(&engine, "beta").start)
        .unwrap();
    assert_eq!(index.before(empty), Some(beta_line));
    assert!(
        structure
            .authored_boundaries(&index)
            .any(|boundary| boundary.trigger_line == beta_line && boundary.forced.is_some())
    );
    let constraints = structure.constraints(flow, &index);
    assert!(constraints.avoid_inside && constraints.owns_line_flow);
    assert_eq!((constraints.orphans, constraints.widows), (3, 4));
    structure.index_lines(2..2, &mut index);
    assert_eq!(index.lines().count(), 0);
}

#[test]
fn table_dependencies_expose_span_groups_and_keep_header_inference_optional() {
    let engine = load(
        "<table><caption>caption</caption><tr><th>head</th><th>title</th></tr><tr><td rowspan='2'>span</td><td>one</td></tr><tr><td>two</td></tr></table>",
    );
    let view = engine.render_view().unwrap();
    let structure = engine.structure().unwrap();
    let mut index = BoundaryIndex::default();
    structure.index_lines(0..view.text().line_count(), &mut index);
    let mut scratch = TableScratch::default();
    let explicit = structure.table_dependencies(&index, HeaderPolicy::ExplicitOnly, &mut scratch);
    assert!(explicit.iter().all(|group| group.header.is_none()));
    let groups =
        structure.table_dependencies(&index, HeaderPolicy::InferLeadingHeaderCells, &mut scratch);
    let whole = groups
        .iter()
        .find(|group| group.kind == TableGroupKind::WholeTable)
        .unwrap();
    let captions = structure.table_captions(whole.table_box, &index);
    assert_eq!(captions.len(), 1);
    assert!(captions[0].first_line.is_some() && captions[0].last_line.is_some());
    assert!(captions[0].bounds.y0 >= whole.top && captions[0].bounds.y1 <= whole.bottom);

    let body = groups
        .iter()
        .find(|group| group.kind == TableGroupKind::RowGroup && group.header.is_some())
        .unwrap();
    assert!(body.bottom > body.top && body.bottom <= whole.bottom);
    let span_line = view
        .text()
        .line_index_for_glyph(find(&engine, "span").start)
        .unwrap();
    let two_line = view
        .text()
        .line_index_for_glyph(find(&engine, "two").start)
        .unwrap();
    assert!(body.top <= view.text().line(span_line).unwrap().point().y);
    assert!(
        body.bottom
            >= view.text().line(two_line).unwrap().point().y
                + view.text().line(two_line).unwrap().height()
    );
}

#[test]
fn semantic_relationships_expose_facts_without_keep_lengths() {
    let engine = load(
        "<figure><p>figure</p><figcaption id='caption'>caption</figcaption></figure><dl><dt>term</dt><dd>definition</dd></dl><h2>heading</h2><p>following</p>",
    );
    let view = engine.render_view().unwrap();
    let structure = engine.structure().unwrap();
    let mut index = BoundaryIndex::default();
    structure.index_lines(0..view.text().line_count(), &mut index);
    let pairs = structure.sibling_pairs(&index);
    assert!(pairs.iter().any(|pair| pair.definition_pair));
    assert!(pairs.iter().any(|pair| pair.caption_pair));
    assert!(pairs.iter().any(|pair| pair.heading_following));
    let caption = (0..view.boxes().len())
        .find(|&box_idx| view.boxes().attribute(box_idx, "id") == Some("caption"))
        .unwrap();
    let owner = structure.caption_owner(caption).unwrap();
    assert_eq!(view.boxes().tag(owner), Some("figure"));
}

#[test]
fn document_revisions_track_successful_changes_without_viewport_state() {
    let mut shaper = Shaper::default();
    let mut engine = Engine::load(
        provider("<p>text</p>"),
        "document.html",
        DocumentOptions::new(600.0),
        &mut shaper,
    )
    .unwrap();
    let before = engine.revisions();
    let update = engine
        .apply_changes(PipelineChanges::default(), &mut shaper)
        .unwrap();
    assert_eq!(update.stage, html::pipeline::EarliestStage::Paint);
    assert_eq!(engine.revisions().layout, before.layout);
    assert_eq!(engine.revisions().shape, before.shape);
    let paint = engine.inputs().unwrap().paint.next();
    engine
        .apply_changes(
            PipelineChanges {
                paint: Some(paint),
                ..Default::default()
            },
            &mut shaper,
        )
        .unwrap();
    assert_eq!(engine.revisions().layout, before.layout);
    assert!(engine.revisions().paint > before.paint);
    let painted = engine.revisions();
    engine
        .apply_changes(
            PipelineChanges {
                source: Some("<p>replacement</p>".into()),
                ..Default::default()
            },
            &mut shaper,
        )
        .unwrap();
    assert!(
        engine.revisions().document > painted.document
            && engine.revisions().layout > painted.layout
    );
    let revised = engine.revisions();
    let mut invalid = engine.inputs().unwrap().layout;
    invalid.viewport_width = f64::NAN;
    assert!(
        engine
            .apply_changes(
                PipelineChanges {
                    layout: Some(invalid),
                    ..Default::default()
                },
                &mut shaper
            )
            .is_err()
    );
    assert_eq!(engine.revisions(), revised);
    engine.release_resources();
    assert_eq!(engine.revisions().layout, revised.layout);
    assert!(engine.revisions().resources > revised.resources);
}

#[test]
fn loading_reports_only_measured_phases_and_preserves_resource_payloads() {
    let provider: Arc<dyn ResourceProvider> = Arc::new(Provider {
        files: HashMap::from([
            (
                "document.html".into(),
                b"<p>text</p><img src='pic.svg'>".to_vec(),
            ),
            ("pic.svg".into(), b"<svg width='20' height='10'/>".to_vec()),
        ]),
        metadata: ResourceMetadata::default(),
    });
    let (engine, timings) = Engine::load_timed(
        provider,
        "document.html",
        DocumentOptions::new(600.0),
        &mut Shaper::default(),
    )
    .unwrap();
    assert!(engine.document().is_some());
    assert_eq!(timings.total(), timings.resource + timings.pipeline);
    let source = engine.image_source_request(0).unwrap().read().unwrap();
    assert_eq!(source.uri, "pic.svg");
    assert_eq!(source.bytes, b"<svg width='20' height='10'/>");
    let view = engine.render_view().unwrap();
    let images = engine
        .content()
        .unwrap()
        .image_ids_for_lines(0..view.text().line_count());
    assert_eq!(images, [0].into_iter().collect::<HashSet<_>>());
    assert!(
        engine
            .content()
            .unwrap()
            .image_ids_in(Rect::new(-100.0, -100.0, -10.0, -10.0))
            .is_empty()
    );
}

#[test]
fn image_queries_painting_and_scene_extent_agree_on_object_fit_and_placement() {
    for (fit, fitted_size) in [
        ("contain", Size::new(100.0, 20.0)),
        ("cover", Size::new(500.0, 100.0)),
    ] {
        let mut engine = load(&format!(
            "<body style='margin:0'><img src='wide.png' style='width:100px;height:100px;object-fit:{fit};position:relative;top:80px;vertical-align:top'></body>",
        ));
        let inputs = engine.inputs().unwrap();
        let mut metrics = inputs.image_metrics.clone();
        metrics.set(0, 100, 20);
        let revision = inputs.image_metrics_revision.next();
        engine
            .apply_changes(
                PipelineChanges {
                    image_metrics: Some(metrics),
                    image_metrics_revision: Some(revision),
                    ..Default::default()
                },
                &mut Shaper::default(),
            )
            .unwrap();
        let view = engine.render_view().unwrap();
        let image = view.fragments().images().iter().next().unwrap();
        let line = view.text().line(image.line_idx()).unwrap();
        let rect = Rect::from_origin_size(line.point() + image.offset().to_vec2(), image.size());
        let clip = image.clip() + line.point().to_vec2();
        assert_eq!(rect.size(), fitted_size);
        let visible = rect.intersect(clip);
        let inside = visible.center();
        let padding = if fit == "contain" {
            Point::new(inside.x, rect.y1 + 10.0)
        } else {
            Point::new(rect.x0 + 10.0, inside.y)
        };
        assert_ne!(rect.contains(padding), clip.contains(padding));
        let geometry = engine.geometry().unwrap();
        assert_eq!(geometry.image_at(inside), Some(0));
        assert_eq!(geometry.image_at(padding), None);
        let region = Rect::from_origin_size(padding, Size::new(1.0, 1.0));
        assert!(engine.content().unwrap().image_ids_in(region).is_empty());
        assert_eq!(
            engine.content().unwrap().image_ids_in(rect),
            HashSet::from([0])
        );

        let delta = Vec2::new(17.25, -31.125);
        assert_eq!(
            geometry.image_at_lines(
                inside + delta,
                [LinePlacement {
                    line: line.index(),
                    origin: line.point() + delta
                }],
            ),
            Some(0)
        );
        let mut painter = RecordingPainter::default();
        engine.painter().unwrap().paint_region(
            Rect::new(0.0, 0.0, 600.0, 300.0),
            Point::ORIGIN + delta,
            &mut painter,
        );
        assert_eq!(painter.resource_images[0].2, rect + delta);
        assert!(painter.clips.contains(&(clip + delta)));

        let expected_height = view
            .text()
            .lines()
            .iter()
            .map(|line| line.point_y() + line.height())
            .fold(visible.y1, f64::max);
        let scene = RenderScene::for_view(view, &FragmentRenderOptions::new(600.0, None).unwrap());
        assert_eq!(scene.content_height(), expected_height);
    }
}

#[test]
fn hidden_source_does_not_promote_visible_content_during_semantic_selection() {
    let engine = load(
        "<style>li::before{content:'Generated'}</style><ol><li><span style='visibility:hidden'>secret</span>visible</li></ol><p style='visibility:hidden'>concealed <b style='visibility:visible'>Restored</b></p>",
    );
    let view = engine.render_view().unwrap();
    let text = view.text();
    let source_text = |range: Range<u32>| {
        range
            .map(|index| text.character_at(index as usize).unwrap())
            .collect::<String>()
    };
    let hidden = text
        .text_runs()
        .find(|run| source_text(run.glyphs()) == "secret")
        .unwrap()
        .glyphs();
    let selection = engine.selection(hidden).unwrap().with_semantics();
    assert!(selection.promoted_glyph_ranges().is_empty());
    assert_eq!(selection.text(), SelectionText::default());
    let visible: Vec<_> = text
        .visible_source_runs()
        .map(|run| source_text(run.glyphs()))
        .filter(|text| !text.is_empty())
        .collect();
    assert_eq!(visible, ["visible", "Restored"]);
    let reading: String = engine
        .reading()
        .unwrap()
        .fragments(0..text.glyph_count() as u32)
        .into_iter()
        .map(|fragment| fragment.text)
        .collect();
    assert_eq!(reading, "visibleRestored");
}

#[test]
fn fragments_tables_and_search_previews_share_unicode_whitespace_normalization() {
    use html::document::plain_text_from_fragment;

    let source = "<table><tr><td style='white-space:pre-wrap'>\t Café\u{a0} \n<span>漢\u{2003}字</span> \r\n🙂e\u{301}\u{202f}</td></tr></table>";
    let expected = "Café 漢 字 🙂e\u{301}";
    assert_eq!(plain_text_from_fragment(source), expected);
    let engine = load(source);
    let view = engine.render_view().unwrap();
    let boxes = view.boxes();
    let table = (0..boxes.len())
        .find_map(|index| boxes.table(index))
        .unwrap();
    assert_eq!(table.rows()[0].cells()[0].text(), expected);
    let search = engine.search().unwrap();
    assert_eq!(
        search
            .excerpt(find(&engine, "Café"), u32::MAX, u32::MAX)
            .as_deref(),
        Some(expected)
    );
}

#[test]
fn document_search_excerpts_use_visible_source_and_unicode_character_ranges() {
    let engine = load(
        "<p>Before <b>Café</b>   after</p><p style='display:none'>secret</p>\
         <p style='visibility:hidden'>concealed</p><ol><li>Cafe\u{301}</li></ol>",
    );
    let search = engine.search().unwrap();
    let mut results = search.find("cafe", TextSearchOptions::default());
    assert_eq!(results.ranges().len(), 2);
    let first = results.current().unwrap();
    assert_eq!(
        search.excerpt(first.clone(), 7, 8).as_deref(),
        Some("Before Café after")
    );
    assert_eq!(search.excerpt(first, 0, 0).as_deref(), Some("Café"));
    assert!(results.next_match());
    let second = results.current().unwrap();
    assert_eq!(
        second.end - second.start,
        5,
        "retain the combining mark's source position"
    );
    assert_eq!(search.excerpt(second, 0, 0).as_deref(), Some("Cafe\u{301}"));
    for query in ["secret", "concealed", "1"] {
        assert!(
            search
                .find(query, TextSearchOptions::default())
                .ranges()
                .is_empty()
        );
    }
    assert!(search.excerpt(10..2, 0, 0).is_none());
    assert!(search.excerpt(0..u32::MAX, 0, 0).is_none());
    let preview = search
        .excerpt(results.ranges()[0].clone(), u32::MAX, u32::MAX)
        .unwrap();
    assert!(
        !preview.contains("secret") && !preview.contains("concealed") && !preview.contains("1.")
    );
}

#[test]
fn document_search_cursor_wraps_and_survives_a_smaller_result_set() {
    let mut results = SearchResults::new(vec![1..3, 8..10, 20..22]);
    assert!(results.previous_match());
    assert_eq!(results.current(), Some(20..22));
    assert!(results.next_match());
    assert_eq!(results.current_index(), 0);
    assert!(results.select(1));
    assert!(!results.select(3));
    assert_eq!(results.current(), Some(8..10));
    results.focus(usize::MAX);
    assert_eq!(results.current_index(), 2);
    results.replace(vec![1..3]);
    assert_eq!(results.current(), Some(1..3));
    results.replace(Vec::new());
    assert_eq!(results.current(), None);
    assert_eq!(results.current_index(), 0);
    assert!(!results.next_match() && !results.previous_match() && !results.select(0));
}

#[test]
fn table_hit_testing_retains_spans_and_accepts_host_projection() {
    let engine = load(
        "<table><tr><td rowspan='2' colspan='2'>spanned</td><td>other</td></tr>\
         <tr><td>last</td></tr></table>",
    );
    let view = engine.render_view().unwrap();
    let boxes = view.boxes();
    let table = (0..boxes.len())
        .find_map(|index| boxes.table(index))
        .unwrap();
    let cell = &table.rows()[0].cells()[0];
    let rect = Rect::from_origin_size(
        boxes.point(cell.box_idx()).unwrap(),
        boxes.size(cell.box_idx()).unwrap(),
    );
    let geometry = engine.geometry().unwrap();
    let hit = geometry.table_at(rect.center()).unwrap();
    assert_eq!(
        (hit.table_box, hit.row, hit.column),
        (table.box_idx(), 0, 0)
    );
    assert_eq!((hit.row_end, hit.column_end), (1, 1));
    // A host can test page fragments or any other arrangement of cell boxes.
    let projected = geometry
        .table_at_where(|box_idx| box_idx == cell.box_idx())
        .unwrap();
    assert_eq!((projected.row_end, projected.column_end), (1, 1));
    assert_eq!(cell.row_range(), 0..2);
    assert_eq!(cell.column_range(), 0..2);
    let range = TableCellRange {
        table_box: projected.table_box,
        row_start: projected.row,
        row_end: projected.row_end,
        column_start: projected.column,
        column_end: projected.column_end,
    };
    assert_eq!(
        range
            .cells(&table)
            .map(|cell| cell.box_idx())
            .collect::<Vec<_>>(),
        vec![cell.box_idx()]
    );
    let selection = engine
        .selection(find(&engine, "spanned"))
        .unwrap()
        .with_semantics();
    assert_eq!(selection.table_cell_boxes(), vec![cell.box_idx()]);
    assert_eq!(
        selection.cell_glyph_ranges(),
        vec![find(&engine, "spanned")]
    );
    // A rectangle starting inside a merged cell still exports its text once.
    for (rows, columns) in [(0..2, 0..2), (1..2, 1..2)] {
        let markdown = table.markdown_in(rows, columns).unwrap();
        assert_eq!(markdown.matches("spanned").count(), 1, "{markdown}");
        assert!(!markdown.contains("other") && !markdown.contains("last"));
    }
    assert!(geometry.table_at_where(|_| false).is_none());
    assert!(geometry.table_at(Point::new(f64::NAN, 0.0)).is_none());
}

#[test]
fn preceding_anchors_support_host_filters_and_document_coordinates() {
    let engine = load("<p id='first'>alpha</p><p id='second'>beta</p>");
    let targets = engine.targets().unwrap();
    let beta = find(&engine, "beta").start;
    assert_eq!(targets.preceding_anchor(beta), Some("second"));
    assert_eq!(
        targets.preceding_anchor_where(beta, |id| id == "first"),
        Some("first")
    );
    assert_eq!(targets.preceding_anchor_where(beta, |_| false), None);
    let y = targets.element("second").unwrap().y.unwrap();
    assert_eq!(targets.anchor_at_or_before_y(y), Some("second"));
    assert_eq!(
        targets.anchor_at_or_before_y_where(y, |id| id == "first"),
        Some("first")
    );
    assert_eq!(targets.anchor_at_or_before_y(-1.0), None);
    assert_eq!(targets.anchor_at_or_before_y(f64::NAN), None);
}

#[test]
fn anchors_at_the_same_height_follow_source_order() {
    let engine = load("<p><span id='one'></span><span id='two'></span>text</p>");
    let targets = engine.targets().unwrap();
    let first_y = targets.element("one").unwrap().y.unwrap();
    let second_y = targets.element("two").unwrap().y.unwrap();
    assert_eq!(first_y, second_y);
    assert_eq!(targets.anchor_at_or_before_y(second_y), Some("two"));
}

#[test]
fn resource_loading_preserves_transport_metadata_css_and_worker_preparation() {
    let provider: Arc<dyn ResourceProvider> = Arc::new(Provider {
        files: HashMap::from([
            (
                "books/chapter.html".into(),
                b"<html xmlns='http://www.w3.org/1999/xhtml'><head><link rel='stylesheet' href='style.css'/></head><body><a id='page'/><p>Caf\xe9</p></body></html>".to_vec(),
            ),
            (
                "books/style.css".into(),
                b"@font-face { font-family: Custom; src: url(fonts/custom.ttf); } p { color: red; }".to_vec(),
            ),
        ]),
        metadata: ResourceMetadata {
            media_type: Some("application/xhtml+xml; charset=windows-1252".into()),
            charset: Some("windows-1252".into()),
        },
    });
    let mut prepared = Engine::prepare(
        provider.clone(),
        "books/chapter.html",
        DocumentOptions::new(320.0),
    )
    .unwrap();
    assert_eq!(
        SearchIndex::load(provider.as_ref(), "books/chapter.html")
            .unwrap()
            .text(),
        "Café"
    );
    assert!(prepared.document().is_none());
    assert!(prepared.selection(0..1).is_none());
    assert_eq!(prepared.inputs().unwrap().markup_syntax, MarkupSyntax::Xml);
    assert!(prepared.inputs().unwrap().source.contains("Café"));
    assert_eq!(prepared.font_faces()[0].family, "Custom");
    prepared.rehydrate_glyphs(&mut Shaper::default()).unwrap();

    let loaded = Engine::load(
        provider,
        "books/chapter.html",
        DocumentOptions::new(320.0),
        &mut Shaper::default(),
    )
    .unwrap();
    for engine in [&prepared, &loaded] {
        let range = find(engine, "Café");
        assert_eq!(
            engine
                .selection(range.clone())
                .unwrap()
                .text()
                .plain
                .as_deref(),
            Some("Café")
        );
        assert_eq!(
            engine
                .render_view()
                .unwrap()
                .text()
                .paint_color_at(range.start),
            Some(0xff0000ff)
        );
        assert_eq!(engine.inputs().unwrap().layout.viewport_height, None);
    }
}

#[test]
fn resource_loading_returns_errors_for_invalid_options_and_missing_sources() {
    for width in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            Engine::prepare(
                provider("<p>text</p>"),
                "document.html",
                DocumentOptions::new(width)
            )
            .is_err()
        );
    }
    for size in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let mut options = DocumentOptions::new(320.0);
        options.root_font_size = size;
        assert!(Engine::prepare(provider("<p>text</p>"), "document.html", options).is_err());
    }
    let error = Engine::load(
        provider("<p>text</p>"),
        "missing.html",
        DocumentOptions::new(320.0),
        &mut Shaper::default(),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("missing.html"));
}

#[test]
fn semantic_list_copy_and_highlighting_promote_the_same_complete_item() {
    let engine = load("<ul><li>First <em>chosen</em> tail</li><li>Other item</li></ul>");
    let range = find(&engine, "chosen");
    let plain = engine.selection(range.clone()).unwrap();
    assert_eq!(plain.text().plain.as_deref(), Some("chosen"));
    assert!(plain.promoted_glyph_ranges().is_empty());
    let semantic = plain.with_semantics();
    let text = semantic.text();
    assert_eq!(text.plain.as_deref(), Some("chosen"));
    let markdown = text.markdown.unwrap();
    assert!(markdown.contains("First *chosen* tail"), "{markdown}");
    assert!(!markdown.contains("Other item"));
    assert!(
        semantic
            .promoted_glyph_ranges()
            .iter()
            .any(|promoted| promoted.start <= range.start && promoted.end >= range.end)
    );
}

#[test]
fn code_is_character_selectable_until_the_selection_crosses_its_boundary() {
    let engine = load("<p>Before</p><pre><code>alpha beta</code></pre><p>After</p>");
    let inside = engine
        .selection(find(&engine, "beta"))
        .unwrap()
        .with_semantics();
    assert!(inside.promoted_glyph_ranges().is_empty());
    let across = engine
        .selection(find(&engine, "Before").start..find(&engine, "beta").end)
        .unwrap()
        .with_semantics();
    assert!(!across.promoted_glyph_ranges().is_empty());
    let markdown = across.text().markdown.unwrap();
    assert!(markdown.contains("alpha beta"), "{markdown}");
    assert!(!markdown.contains("After"));
}

#[test]
fn semantic_tables_include_spans_and_empty_cells_inside_the_selection_rectangle() {
    let engine = load(
        "<table><tr><td rowspan='2'>One</td><td>Two</td><td>Three</td></tr><tr><td></td><td>Six</td></tr></table>",
    );
    let selection = engine
        .selection(find(&engine, "One").start..find(&engine, "Two").end)
        .unwrap()
        .with_semantics();
    assert_eq!(selection.table_cell_boxes().len(), 3);
    let ranges = selection.cell_glyph_ranges();
    assert!(ranges.contains(&find(&engine, "One")));
    assert!(ranges.contains(&find(&engine, "Two")));
    assert!(!ranges.contains(&find(&engine, "Three")));
    let markdown = selection.text().markdown.unwrap();
    assert!(
        markdown.contains("One") && markdown.contains("Two"),
        "{markdown}"
    );
    assert!(!markdown.contains("Three") && !markdown.contains("Six"));
}

#[test]
fn semantic_geometry_partitions_promoted_text_and_multiple_tables() {
    let engine = load(
        "<p>before</p><ul><li>lead<table><tr><td rowspan='2'>One</td><td>Two</td></tr><tr><td></td></tr></table>tail</li></ul><p>between</p><table><tr><td>Three</td><td>Four</td></tr></table><p>after</p>",
    );
    let view = engine.render_view().unwrap();
    for range in [
        find(&engine, "lead").start + 1..find(&engine, "after").end - 1,
        find(&engine, "One").start..find(&engine, "Four").end,
        find(&engine, "between"),
    ] {
        let selection = engine.selection(range.clone()).unwrap().with_semantics();
        let promoted = selection.promoted_glyph_ranges();
        let cells = selection.cell_glyph_ranges();
        let plan = selection.geometry_plan();
        assert_eq!(plan.cell_boxes, selection.table_cell_boxes());
        assert!(
            plan.text_ranges
                .windows(2)
                .all(|pair| pair[0].end < pair[1].start)
        );
        for position in 0..view.text().glyph_count() as u32 {
            let selected =
                range.contains(&position) || promoted.iter().any(|range| range.contains(&position));
            let in_cell = cells.iter().any(|range| range.contains(&position));
            let copies = plan
                .text_ranges
                .iter()
                .filter(|range| range.contains(&position))
                .count();
            assert_eq!(
                copies,
                usize::from(selected && !in_cell),
                "position {position}"
            );
        }
    }
}

#[test]
fn invalid_selections_never_promote_or_export_content() {
    let engine = load("<ul><li>item</li></ul><table><tr><td>cell</td></tr></table>");
    let count = engine.render_view().unwrap().text().glyph_count() as u32;
    for range in [
        0..0,
        Range { start: 2, end: 1 },
        0..count + 1,
        count..u32::MAX,
    ] {
        let selection = engine.selection(range).unwrap().with_semantics();
        assert_eq!(selection.text(), SelectionText::default());
        assert!(selection.promoted_glyph_ranges().is_empty());
        assert!(selection.cell_glyph_ranges().is_empty());
        assert!(selection.table_cell_boxes().is_empty());
    }
}

#[test]
fn the_same_geometry_supports_document_and_caller_placed_lines() {
    let engine = load("<p>First line</p><p>Selected words</p>");
    let range = find(&engine, "Selected");
    let geometry = engine.geometry().unwrap();
    let source = geometry.selection_bounds(range.clone()).unwrap();
    let point = Point::new(source.x0 + 1.0, (source.y0 + source.y1) / 2.0);
    let hit = geometry.hit_test(point).unwrap();
    let delta = Vec2::new(310.5, -25.25);
    let view = engine.render_view().unwrap();
    let lines = view.text().lines();
    let placements = || {
        lines.iter().map(|line| LinePlacement {
            line: line.index(),
            origin: line.point() + delta,
        })
    };
    assert_eq!(
        geometry.hit_test_lines(point + delta, placements()),
        Some(hit)
    );
    assert_eq!(
        geometry.selection_bounds_lines(range.clone(), placements()),
        Some(source + delta)
    );
    let (start, end) = geometry.selection_endpoints(range.clone());
    let projected = geometry.selection_endpoints_lines(range, placements());
    assert_eq!(
        projected,
        (
            start.map(|point| point + delta),
            end.map(|point| point + delta)
        )
    );
}

#[test]
fn source_anchors_survive_reflow_and_recover_from_changed_text_offsets() {
    let mut engine = load("<p>prefix target suffix</p>");
    let original = find(&engine, "target");
    let quote = engine
        .targets()
        .unwrap()
        .quote(original.clone(), 8)
        .unwrap();
    assert_eq!(
        engine.targets().unwrap().resolve_range(Some(0..3), &quote),
        Some(original.clone())
    );
    let anchor = engine
        .targets()
        .unwrap()
        .capture(original.clone(), 3)
        .unwrap();
    assert_eq!(engine.targets().unwrap().resolve(&anchor), Some(original));
    let mut layout = engine.inputs().unwrap().layout;
    layout.viewport_width = 90.0;
    engine
        .apply_changes(
            PipelineChanges {
                layout: Some(layout),
                ..Default::default()
            },
            &mut Shaper::default(),
        )
        .unwrap();
    assert_eq!(
        engine.targets().unwrap().resolve(&anchor),
        Some(find(&engine, "target"))
    );
    let revision = engine.inputs().unwrap().source_revision.next();
    engine
        .apply_changes(
            PipelineChanges {
                source: Some("<p>Inserted before prefix target suffix</p>".into()),
                source_revision: Some(revision),
                ..Default::default()
            },
            &mut Shaper::default(),
        )
        .unwrap();
    assert_eq!(
        engine.targets().unwrap().resolve(&anchor),
        Some(find(&engine, "target"))
    );
}

#[test]
fn translated_regions_share_css_painting_and_preserve_fractional_positions() {
    let engine = load(
        "<p style='margin:0;background:#ff0000;border:1px solid blue'><span style='background:#00ff00'>content</span></p>",
    );
    let view = engine.render_view().unwrap();
    let source = Rect::new(0.0, 0.0, 600.0, 100.0);
    let delta = Vec2::new(17.25, 31.125);
    let mut first = RecordingPainter::default();
    let mut second = RecordingPainter::default();
    let painter = DocumentPainter::new(view);
    painter.paint_region(source, Point::ORIGIN, &mut first);
    painter.paint_region(source, Point::ORIGIN + delta, &mut second);
    assert!(!first.glyphs.is_empty());
    assert!(!first.fills.is_empty());
    assert_eq!(first.glyph_colors, second.glyph_colors);
    assert_eq!(
        first
            .glyphs
            .iter()
            .map(|(glyph, point)| (
                *glyph,
                Point::new(point.x + delta.x, (point.y + delta.y).round())
            ))
            .collect::<Vec<_>>(),
        second.glyphs
    );
    assert_eq!(
        first
            .fills
            .iter()
            .map(|(rect, color)| (*rect + delta, *color))
            .collect::<Vec<_>>(),
        second.fills
    );
}

#[test]
fn logical_selection_state_and_table_ranges_need_no_viewport() {
    use html::selection::{SelectionMode, SelectionState, TableHit, TableSelectionState};
    let mut selection = SelectionState::default();
    selection.begin_at_glyph(8, SelectionMode::Plain);
    selection.update_interaction(Some(3), SelectionMode::Semantic);
    selection.finish_interaction();
    assert_eq!(selection.interaction().anchor(), Some(8));
    assert_eq!(selection.interaction().active(), Some(3));
    assert!(selection.interaction().is_semantic());
    let mut table = TableSelectionState::default();
    let first = TableHit {
        table_box: 5,
        row: 1,
        row_end: 2,
        column: 0,
        column_end: 1,
    };
    table.begin(first);
    assert!(table.begin_drag(first));
    assert_eq!(
        table.update_drag(Some(TableHit {
            row: 3,
            row_end: 3,
            column: 4,
            column_end: 4,
            ..first
        })),
        Some(true)
    );
    let range = table.range().unwrap();
    assert_eq!(
        (
            range.row_start,
            range.row_end,
            range.column_start,
            range.column_end
        ),
        (1, 3, 0, 4)
    );
    assert_eq!(table.update_drag(None), Some(false));
    assert_eq!(
        table.update_drag(Some(TableHit {
            table_box: 6,
            ..first
        })),
        Some(false)
    );
    assert_eq!(table.range(), Some(range));
    let reverse = TableHit {
        row: 0,
        row_end: 0,
        column: 0,
        column_end: 3,
        ..first
    };
    assert_eq!(table.update_drag(Some(reverse)), Some(true));
    assert_eq!(
        table.range(),
        Some(TableCellRange {
            table_box: 5,
            row_start: 0,
            row_end: 2,
            column_start: 0,
            column_end: 3,
        })
    );
    assert_eq!(table.update_drag(Some(reverse)), Some(false));
    assert_eq!(table.update_drag(Some(first)), Some(true));
    assert_eq!(table.range().unwrap().row_start, first.row);
    assert_eq!(table.range().unwrap().column_end, first.column_end);
    assert!(table.finish_drag());
    assert_eq!(table.update_drag(Some(reverse)), None);
}

struct SvgLoader;
impl ImageLoader for SvgLoader {
    fn load(&self, source: ImageSource, complete: ImageLoadCompletion) {
        let size = match source {
            ImageSource::Uri(uri) if uri.contains("replacement") => 90,
            _ => 42,
        };
        complete(Ok(format!("<svg xmlns='http://www.w3.org/2000/svg' width='{size}' height='31'><rect width='{size}' height='31'/></svg>").into_bytes()));
    }
}

#[test]
fn document_resources_update_intrinsic_layout_and_drop_replaced_image_identity() {
    use std::collections::HashSet;
    let mut engine = load("<p><img src='picture.svg'/></p>");
    engine.set_image_service(ImageService::with_loader(
        Arc::new(SvgLoader),
        Arc::new(InlineJobSpawner),
        1024 * 1024,
    ));
    engine.request_images(&HashSet::from([0]));
    let update = engine.update_resources(&mut Shaper::default()).unwrap();
    assert!(update.decoded);
    assert!(
        matches!(engine.image_payload(0), Some(html::export::ImagePayload::Svg { bytes }) if bytes.starts_with(b"<svg"))
    );
    assert!(engine.image_payload(u32::MAX).is_none());
    assert_eq!(
        engine.images().get_decoded(0).unwrap().dimensions(),
        (42, 31)
    );
    let mut content = engine.painter().unwrap();
    let mut visible = RecordingPainter::default();
    content.paint_region(
        Rect::new(0.0, 0.0, 600.0, 600.0),
        Point::ORIGIN,
        &mut visible,
    );
    assert!(!visible.svgs.is_empty());
    content.image_policy = html::render::FragmentImagePolicy::Omit;
    let mut omitted = RecordingPainter::default();
    content.paint_region(
        Rect::new(0.0, 0.0, 600.0, 600.0),
        Point::ORIGIN,
        &mut omitted,
    );
    assert!(omitted.svgs.is_empty());
    assert_eq!(
        engine.inputs().unwrap().image_metrics.get(0),
        Some((42, 31))
    );
    let update = engine.update_resources(&mut Shaper::default()).unwrap();
    assert!(!update.decoded);
    assert!(update.pipeline.is_none());

    let revision = engine.inputs().unwrap().source_revision.next();
    engine
        .apply_changes(
            PipelineChanges {
                source: Some("<p><img src='replacement.svg'/></p>".into()),
                source_revision: Some(revision),
                ..Default::default()
            },
            &mut Shaper::default(),
        )
        .unwrap();
    assert!(engine.images().get_decoded(0).is_none());
    engine.request_images(&HashSet::from([0]));
    // A host may defer an update, or fail to apply it. The proposed intrinsic
    // sizes remain pending until the document accepts them.
    assert!(engine.poll_resources().changes.is_some());
    engine.release_resources();
    assert!(engine.resources_pending());
    engine.update_resources(&mut Shaper::default()).unwrap();
    assert!(!engine.resources_pending());
    assert_eq!(
        engine.inputs().unwrap().image_metrics.get(0),
        Some((90, 31))
    );
    engine.request_images(&HashSet::from([0]));
    engine.update_resources(&mut Shaper::default()).unwrap();
    assert_eq!(
        engine.images().get_decoded(0).unwrap().dimensions(),
        (90, 31)
    );
    engine.release_resources();
    assert!(engine.images().get_decoded(0).is_none());
    assert_eq!(
        engine.inputs().unwrap().image_metrics.get(0),
        Some((90, 31))
    );
}

#[test]
fn element_links_and_notes_resolve_without_navigation_policy() {
    let engine = load(
        "<p><a href='other.html#part'>linked</a></p><h2 id='part'>Heading</h2><aside id='note' role='doc-footnote'>Note</aside>",
    );
    let targets = engine.targets().unwrap();
    let link = targets.link(find(&engine, "linked").start).unwrap();
    assert_eq!(link.href, "other.html#part");
    let resolved = engine.resolve_link(link.href).unwrap();
    assert_eq!(resolved.resource, "other.html");
    assert_eq!(resolved.fragment.as_deref(), Some("part"));
    assert!(targets.has_element("part"));
    assert!(targets.element("part").unwrap().y.is_some());
    assert!(targets.element("note").unwrap().is_note);
    assert_eq!(
        engine.resolve_link("#part").unwrap().resource,
        "document.html"
    );
}

#[test]
fn sparse_paint_candidates_restore_css_order_and_include_inline_decorations() {
    use html::layout::RenderPaintItem;
    let engine = load(
        "<div style='position:relative;background:red'><p style='background:blue'>one <b style='background:green'>two</b></p><span style='position:absolute;z-index:-1;background:yellow'>behind</span></div>",
    );
    let view = engine.render_view().unwrap();
    let mut blocks = Vec::new();
    view.fragments()
        .block_decoration_indices_intersecting(-1e6, 1e6, &mut blocks);
    let mut lines = (0..view.text().line_count()).rev().collect::<Vec<_>>();
    lines.extend_from_within(..);
    lines.push(usize::MAX);
    let signature = |item| match item {
        RenderPaintItem::Line(line) => format!("line:{line}"),
        RenderPaintItem::Decoration(decoration) => format!("decoration:{decoration:?}"),
    };
    let full = view
        .fragments()
        .paint_order()
        .map(signature)
        .collect::<Vec<_>>();
    let selected = view
        .fragments()
        .paint_order_for(&lines, &blocks)
        .into_iter()
        .map(signature)
        .collect::<Vec<_>>();
    assert_eq!(selected, full);
}

#[test]
fn bounded_region_paint_omits_text_entirely_outside_the_source_clip() {
    let engine = load(
        "<body style='margin:0'><p style='margin:0;line-height:20px'>One</p><p style='margin:0;line-height:20px'>Excluded</p></body>",
    );
    let mut painter = RecordingPainter::default();
    engine.painter().unwrap().paint_region(
        Rect::new(0.0, 0.0, 600.0, 20.0),
        Point::ORIGIN,
        &mut painter,
    );
    assert_eq!(painter.glyphs.len(), 3);
}

#[test]
fn block_selection_and_clipped_hit_testing_use_visible_authored_lines() {
    let engine = load(
        "<html><body style='margin:0'><div style='height:20px;overflow:hidden'><p style='margin:0;line-height:20px'>Visible</p><p style='margin:0;line-height:20px'>Clipped</p></div><p style='margin:0;line-height:20px'>Last <em>paragraph</em></p></body></html>",
    );
    let view = engine.render_view().unwrap();
    let geometry = view.geometry();
    let hit = engine
        .geometry()
        .unwrap()
        .hit_test(Point::new(4.0, 30.0))
        .unwrap();
    let text = view.text();
    let clipped = text.find_matches("Clipped", Default::default())[0].clone();
    assert!(
        engine
            .geometry()
            .unwrap()
            .selection_rects(clipped.clone())
            .is_empty()
    );
    assert!(!clipped.contains(&hit.glyph));
    let last = text.find_matches("Last", Default::default())[0].clone();
    let bounds = engine.geometry().unwrap().selection_bounds(last).unwrap();
    let range = engine
        .geometry()
        .unwrap()
        .block_range_at(bounds.center())
        .unwrap();
    assert_eq!(
        view.export().plain_text(range).as_deref(),
        Some("Last paragraph")
    );
    let all = 0..text.glyph_count() as u32;
    assert!(
        geometry
            .line_selection_rects(hit.line, all, &[])
            .all(|rect| rect.height() > 0.0)
    );
}

#[test]
fn plain_text_block_separator_preserves_authored_breaks_and_unicode() {
    let engine = load("<p>First &amp; café<br>next line</p><p>Second paragraph</p>");
    let view = engine.render_view().unwrap();
    let all = 0..view.text().glyph_count() as u32;
    let expected = Some("First & café\nnext line\nSecond paragraph");
    assert_eq!(view.export().plain_text(all.clone()).as_deref(), expected);
    assert_eq!(
        view.export().selection_text(all.clone()).0.as_deref(),
        expected
    );
    assert_eq!(
        view.export().plain_text_ranges([all.clone()]).as_deref(),
        expected
    );
    assert_eq!(
        view.export()
            .plain_text_with_paragraph_breaks(all)
            .as_deref(),
        Some("First & café\nnext line\n\nSecond paragraph")
    );
}

mod document_operations {
    use super::*;
    use html::annotations::{AnnotationAnchor, AnnotationIndex, DocumentAnnotation};
    use html::export::{ExportFormat, ExportPayload, ExportTarget, ImagePayload};
    use html::layout::LayoutConstraints;
    use html::search::DocumentSearch;
    use html::selection::SelectionMode;

    #[test]
    fn scopes_share_owner_resources_and_invalidate_without_stale_selection() {
        let source = "<p>outside</p><section id='part'><p>Scoped <b>content</b> here.</p></section><aside id='note' role='doc-footnote'>Note content.</aside>";
        let mut shaper = Shaper::default();
        let mut engine = Engine::load(
            provider(source),
            "document.html",
            DocumentOptions::new(600.0),
            &mut shaper,
        )
        .unwrap();
        let constraints = LayoutConstraints::new(160.0, 16.0).unwrap();
        let mut scope = engine.scope("part", constraints, &mut shaper).unwrap();
        let view = scope.render_view(&engine).unwrap();
        let count = view.text().glyph_count() as u32;
        let exported = scope
            .export(&engine)
            .unwrap()
            .request(ExportTarget::Text(0..count), ExportFormat::PlainText)
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap();
        assert!(exported.contains("Scoped content"));
        assert!(!exported.contains("outside") && !exported.contains("Note"));
        assert!(scope.scene(&engine).unwrap().content_height() > 0.0);
        let first = scope.geometry(&engine).unwrap().selection_rects(0..1)[0].center();
        assert!(scope.begin_selection(&engine, first, SelectionMode::Plain));
        assert!(scope.select_range(&engine, 0..count, SelectionMode::Plain));
        assert!(scope.finish_selection(&engine));
        assert!(
            scope
                .selection_text(&engine)
                .unwrap()
                .plain
                .unwrap()
                .contains("Scoped content")
        );
        assert!(
            !scope
                .selection_geometry(&engine)
                .unwrap()
                .text_ranges
                .is_empty()
        );
        let second = engine.scope("note", constraints, &mut shaper).unwrap();
        assert!(second.render_view(&engine).is_some());
        assert!(
            scope.render_view(&engine).is_some(),
            "appending a second scope preserves the first"
        );
        assert!(engine.scope("missing", constraints, &mut shaper).is_none());
        assert!(
            scope.render_view(&load(source)).is_none(),
            "an unrelated owner cannot supply DOM or resources"
        );
        let revision = engine.inputs().unwrap().source_revision.next();
        engine
            .apply_changes(
                PipelineChanges {
                    source: Some("<p id='part'>replacement</p>".into()),
                    source_revision: Some(revision),
                    ..Default::default()
                },
                &mut shaper,
            )
            .unwrap();
        assert!(scope.render_view(&engine).is_none());
        assert!(scope.scene(&engine).is_none());
        assert!(scope.export(&engine).is_none());
        assert!(scope.selection_text(&engine).is_none());
        assert!(!scope.extend_selection(&engine, first, SelectionMode::Plain));
    }

    #[test]
    fn typed_hits_keep_content_identity_after_host_translation() {
        let engine = load(
            "<table><tr><td><a href='#destination'>linked</a></td></tr></table><p id='destination'>target</p><a href='#image'><img src='picture.svg' style='width:40px;height:30px'></a>",
        );
        let geometry = engine.geometry().unwrap();
        let range = find(&engine, "linked");
        let point = geometry.selection_rects(range.start..range.start + 1)[0].center();
        let hit = geometry.content_at(point).unwrap();
        assert_eq!(hit.link.unwrap().href, "#destination");
        assert_eq!(geometry.link_at(point).unwrap().href, "#destination");
        assert!(hit.table.is_some() && hit.text.is_some());
        let line = hit.text.unwrap().line;
        let origin = engine
            .render_view()
            .unwrap()
            .text()
            .line(line)
            .unwrap()
            .point();
        let delta = Vec2::new(220.0, 75.0);
        let projected = geometry
            .content_at_lines(
                point + delta,
                [LinePlacement {
                    line,
                    origin: origin + delta,
                }],
                |_| false,
            )
            .unwrap();
        assert_eq!(projected.link.unwrap().href, "#destination");
        assert_eq!(projected.text.unwrap().glyph, hit.text.unwrap().glyph);
        assert!(
            projected.table.is_none(),
            "cell projection is supplied by the host"
        );
        assert!(
            geometry.content_at(Point::new(4000.0, 4000.0)).is_none(),
            "nearest carets must not activate links"
        );
        assert!(geometry.content_at(Point::new(f64::NAN, point.y)).is_none());
        let view = engine.render_view().unwrap();
        let image_point = view
            .text()
            .lines()
            .iter()
            .find_map(|line| {
                view.fragments()
                    .images_for_line(line.index())
                    .iter()
                    .next()
                    .map(|image| (image.visible_rect() + line.point().to_vec2()).center())
            })
            .unwrap();
        let hit = geometry.content_at(image_point).unwrap();
        assert_eq!(hit.link.unwrap().href, "#image");
        assert_eq!(geometry.link_at(image_point).unwrap().href, "#image");
        let image = hit.image.unwrap();
        assert_eq!(image.image, 0);
        assert!(!image.is_equation);
    }

    #[test]
    fn typed_hits_do_not_activate_clipped_or_hidden_link_text() {
        let engine = load(
            "<body style='margin:0'><p style='width:32px;overflow:hidden;white-space:nowrap;margin:0'><a href='#x'>abcdefghijk</a></p><p style='visibility:hidden'><a href='#hidden'>invisible</a></p></body>",
        );
        let geometry = engine.geometry().unwrap();
        let range = find(&engine, "abcdefghijk");
        let inside = geometry.selection_rects(range.start..range.start + 1)[0].center();
        assert!(
            geometry
                .content_at(inside)
                .and_then(|hit| hit.link)
                .is_some()
        );
        assert!(
            geometry
                .content_at(Point::new(90.0, inside.y))
                .and_then(|hit| hit.link)
                .is_none()
        );
        assert!(
            geometry
                .content_at(Point::new(4.0, inside.y + 60.0))
                .and_then(|hit| hit.link)
                .is_none()
        );
    }

    #[test]
    fn typed_image_hits_apply_ancestor_clips_in_document_and_host_coordinates() {
        let engine = load(
            "<body style='margin:0'><div style='width:20px;overflow:hidden'><img src='picture.svg' style='width:80px;height:30px'></div></body>",
        );
        let view = engine.render_view().unwrap();
        let (line, bounds) = view
            .text()
            .lines()
            .iter()
            .find_map(|line| {
                view.fragments()
                    .images_for_line(line.index())
                    .iter()
                    .next()
                    .map(|image| (line.index(), image.visible_rect() + line.point().to_vec2()))
            })
            .unwrap();
        let inside = Point::new(bounds.x0 + 4.0, bounds.y0 + 4.0);
        let outside = Point::new(bounds.x0 + 60.0, inside.y);
        let geometry = engine.geometry().unwrap();
        assert!(geometry.content_at(inside).unwrap().image.is_some());
        assert!(
            geometry
                .content_at(outside)
                .and_then(|hit| hit.image)
                .is_none()
        );
        let delta = Vec2::new(170.0, 90.0);
        let placement = LinePlacement {
            line,
            origin: view.text().line(line).unwrap().point() + delta,
        };
        assert!(
            geometry
                .content_at_lines(inside + delta, [placement], |_| false)
                .unwrap()
                .image
                .is_some()
        );
        assert!(
            geometry
                .content_at_lines(outside + delta, [placement], |_| false)
                .and_then(|hit| hit.image)
                .is_none()
        );
    }

    #[test]
    fn annotation_index_recovers_anchors_and_preserves_host_precedence() {
        let mut engine = load("<p>prefix target suffix; other target later.</p>");
        let range = find(&engine, "target");
        let anchor = engine.targets().unwrap().capture(range.clone(), 8).unwrap();
        let quote = anchor.quote.clone();
        let mut index = AnnotationIndex::default();
        index.replace([
            DocumentAnnotation {
                id: "source".to_owned(),
                anchor: AnnotationAnchor::Text(anchor),
            },
            DocumentAnnotation {
                id: "quoted".to_owned(),
                anchor: AnnotationAnchor::Range {
                    preferred: Some(0..3),
                    quote,
                },
            },
        ]);
        index.resolve(engine.render_view().unwrap());
        assert_eq!(index.range(&"source".to_owned()), Some(range.clone()));
        let geometry = engine.geometry().unwrap();
        let point = geometry.selection_rects(range.start..range.start + 1)[0].center();
        assert_eq!(index.at_point(geometry, point).count(), 2);
        assert_eq!(
            index.at_point(geometry, Point::new(4000.0, 4000.0)).count(),
            0
        );
        assert_eq!(
            index
                .at(range.start)
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["source", "quoted"]
        );
        assert_eq!(index.overlapping(range.end..range.end + 1).count(), 0);
        assert_eq!(index.overlapping(range.start..range.start).count(), 0);
        let revision = engine.inputs().unwrap().source_revision.next();
        engine
            .apply_changes(
                PipelineChanges {
                    source: Some(
                        "<p>Inserted before prefix target suffix; other target later.</p>".into(),
                    ),
                    source_revision: Some(revision),
                    ..Default::default()
                },
                &mut Shaper::default(),
            )
            .unwrap();
        index.resolve(engine.render_view().unwrap());
        assert_eq!(
            index.range(&"source".to_owned()),
            Some(find(&engine, "target"))
        );
        assert_eq!(
            index.range(&"quoted".to_owned()),
            Some(find(&engine, "target"))
        );
        index.remove(&"quoted".to_owned());
        assert_eq!(index.resolved().len(), 1);
        index.replace([]);
        assert!(index.resolved().is_empty());
    }

    #[test]
    fn document_search_owns_query_options_and_refreshes_visibility_and_cursor() {
        let mut engine = load("<p>Café cafe cafeteria</p><p style='display:none'>cafe</p>");
        let mut search = DocumentSearch::default();
        let options = TextSearchOptions {
            whole_word: true,
            ..Default::default()
        };
        search.set_query(engine.render_view().unwrap(), "cafe".into(), options);
        assert_eq!(search.query(), "cafe");
        assert_eq!(search.options(), options);
        assert_eq!(search.results().ranges().len(), 2);
        assert!(search.results_mut().next_match());
        assert_eq!(search.results().current_index(), 1);
        let revision = engine.inputs().unwrap().source_revision.next();
        engine
            .apply_changes(
                PipelineChanges {
                    source: Some("<p>cafe</p>".into()),
                    source_revision: Some(revision),
                    ..Default::default()
                },
                &mut Shaper::default(),
            )
            .unwrap();
        search.refresh(engine.render_view().unwrap());
        assert_eq!(search.results().ranges().len(), 1);
        assert_eq!(search.results().current_index(), 0);
        search.set_query(
            engine.render_view().unwrap(),
            "CAFÉ".into(),
            TextSearchOptions {
                match_case: true,
                match_diacritics: true,
                ..options
            },
        );
        assert!(search.results().ranges().is_empty());
    }

    #[test]
    fn unified_export_returns_text_tables_cells_and_errors_without_clipboard_policy() {
        let engine = load(
            "<p>Some <b>bold</b> text.</p><table id='t'><tr><td>A</td><td>B</td></tr><tr><td>C</td><td>D</td></tr></table>",
        );
        let range = find(&engine, "Some bold text.");
        let export = engine.export().unwrap();
        let plain = export
            .request(ExportTarget::Text(range.clone()), ExportFormat::PlainText)
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap();
        let markdown = export
            .request(ExportTarget::Text(range), ExportFormat::Markdown)
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap();
        assert_eq!(plain, "Some bold text.");
        assert!(markdown.contains("**bold**"));
        let view = engine.render_view().unwrap();
        let table = (0..view.boxes().len())
            .find_map(|index| view.boxes().table(index))
            .unwrap();
        for format in [
            ExportFormat::Markdown,
            ExportFormat::Html,
            ExportFormat::AuthoredHtml,
        ] {
            let text = export
                .request(ExportTarget::Table(table.box_idx()), format)
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap();
            assert!(text.contains('A') && text.contains('D'));
        }
        let cells = TableCellRange {
            table_box: table.box_idx(),
            row_start: 1,
            row_end: 1,
            column_start: 0,
            column_end: 1,
        };
        let text = export
            .request(ExportTarget::TableCells(cells), ExportFormat::Markdown)
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap();
        assert!(text.contains('C') && text.contains('D') && !text.contains('A'));
        assert!(
            export
                .request(ExportTarget::Table(usize::MAX), ExportFormat::Html)
                .unwrap()
                .is_none()
        );
        assert!(
            export
                .request(ExportTarget::Text(0..1), ExportFormat::Image)
                .is_err()
        );
        assert!(
            export
                .request(
                    ExportTarget::Text(u32::MAX - 1..u32::MAX),
                    ExportFormat::PlainText
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unified_export_preserves_decoded_media_and_formula_payloads() {
        let mut engine =
            load("<p><img src='picture.svg'/></p><math><mi>x</mi><mo>+</mo><mn>1</mn></math>");
        assert!(
            engine
                .export()
                .unwrap()
                .request(ExportTarget::Image(0), ExportFormat::Image)
                .unwrap()
                .is_none()
        );
        engine.set_image_service(ImageService::with_loader(
            Arc::new(SvgLoader),
            Arc::new(InlineJobSpawner),
            1024 * 1024,
        ));
        engine.request_images(&HashSet::from([0]));
        engine.update_resources(&mut Shaper::default()).unwrap();
        assert!(
            matches!(engine.export().unwrap().request(ExportTarget::Image(0), ExportFormat::Image).unwrap(), Some(ExportPayload::Image(ImagePayload::Svg { bytes })) if bytes.starts_with(b"<svg"))
        );
        let view = engine.render_view().unwrap();
        let point = view
            .text()
            .lines()
            .iter()
            .find_map(|line| {
                view.fragments()
                    .images_for_line(line.index())
                    .iter()
                    .find(|image| image.image_idx() == 1)
                    .map(|image| (image.visible_rect() + line.point().to_vec2()).center())
            })
            .unwrap();
        assert!(
            engine
                .geometry()
                .unwrap()
                .content_at(point)
                .unwrap()
                .image
                .unwrap()
                .is_equation
        );
        #[cfg(not(feature = "math-export"))]
        assert!(
            engine
                .export()
                .unwrap()
                .request(ExportTarget::Equation(1), ExportFormat::Latex)
                .is_err()
        );
        #[cfg(feature = "math-export")]
        {
            let formula = engine
                .export()
                .unwrap()
                .request(ExportTarget::Equation(1), ExportFormat::Latex)
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap();
            assert!(formula.contains('x') && formula.contains('1'));
            assert!(
                engine
                    .export()
                    .unwrap()
                    .request(ExportTarget::Equation(0), ExportFormat::Latex)
                    .unwrap()
                    .is_none()
            );
        }
    }
}

mod document_refinement {
    use super::*;
    use html::configuration::DocumentUpdate;
    use html::geometry::RangeRect;
    use html::pipeline::{MediaEnvironment, MediaType};
    use html::reading::Reading;

    #[test]
    fn configuration_keeps_layout_and_media_in_sync_and_preserves_print() {
        let mut engine = load("<p>configuration</p>");
        let mut environment = engine.inputs().unwrap().style_environment;
        environment.media = MediaEnvironment::new(MediaType::Print, 600.0, None).unwrap();
        engine
            .apply_changes(
                PipelineChanges {
                    style_environment: Some(environment),
                    ..Default::default()
                },
                &mut Shaper::default(),
            )
            .unwrap();
        let changes = engine
            .configuration_changes(
                DocumentUpdate::default()
                    .width(320.0)
                    .height(Some(480.0))
                    .font_size(21.6),
            )
            .unwrap();
        // Building changes leaves the document intact until the host accepts them.
        assert_eq!(engine.inputs().unwrap().layout.viewport_width, 600.0);
        let layout = changes.layout.unwrap();
        let style = changes.style_environment.unwrap();
        assert_eq!(layout.viewport_width, style.media.viewport_width());
        assert_eq!(layout.viewport_height, style.media.viewport_height());
        assert_eq!(style.media.media_type(), MediaType::Print);
        assert_eq!(style.root_font_size, 22);
        assert_eq!(layout.line_height, 22.0);
        engine
            .apply_changes(changes, &mut Shaper::default())
            .unwrap();
        let keep = engine
            .configuration_changes(DocumentUpdate::default().width(400.0))
            .unwrap();
        assert_eq!(keep.layout.unwrap().viewport_height, Some(480.0));
        let clear = engine
            .configuration_changes(DocumentUpdate::default().height(None))
            .unwrap();
        assert_eq!(clear.layout.unwrap().viewport_height, None);
        assert_eq!(
            clear.style_environment.unwrap().media.viewport_height(),
            None
        );
        let zero = engine
            .configuration_changes(DocumentUpdate::default().height(Some(0.0)))
            .unwrap();
        assert_eq!(zero.layout.unwrap().viewport_height, Some(0.0));
        assert_eq!(
            zero.style_environment.unwrap().media.viewport_height(),
            None
        );
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                engine
                    .configuration_changes(DocumentUpdate::default().width(value))
                    .is_err()
            );
        }
        for value in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(
                engine
                    .configuration_changes(DocumentUpdate::default().height(Some(value)))
                    .is_err()
            );
        }
        for value in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(
                engine
                    .configuration_changes(DocumentUpdate::default().font_size(value))
                    .is_err()
            );
        }
    }

    #[test]
    fn batched_ranges_preserve_overlap_order_clipping_and_repeated_placements() {
        let engine = load(
            "<div style='width:60px;height:18px;overflow:hidden;white-space:nowrap'>first second third</div><p>last</p>",
        );
        let geometry = engine.geometry().unwrap();
        let text = engine.render_view().unwrap().text();
        let first = find(&engine, "first");
        let last = find(&engine, "last");
        // Unsorted, nested and duplicate ranges retain caller order.
        let ranges = vec![
            ("last", last.clone()),
            ("all", 0..u32::MAX),
            ("first", first.clone()),
            ("duplicate", first.clone()),
            ("empty", 2..2),
        ];
        let placements: Vec<_> = (0..text.line_count())
            .rev()
            .flat_map(|line| {
                let origin = text.line(line).unwrap().point();
                [
                    LinePlacement {
                        line,
                        origin: origin + Vec2::new(100.0, 200.0),
                    },
                    LinePlacement { line, origin },
                ]
            })
            .chain([LinePlacement {
                line: usize::MAX,
                origin: Point::ZERO,
            }])
            .collect();
        let mut expected = Vec::new();
        for &placement in &placements {
            for (id, range) in &ranges {
                expected.extend(
                    geometry
                        .placed_selection_rects(placement, range.clone())
                        .map(|bounds| RangeRect {
                            id: *id,
                            line: placement.line,
                            bounds,
                        }),
                );
            }
        }
        let actual = geometry.range_geometry_lines(ranges.clone(), placements);
        assert!(!actual.is_empty());
        assert_eq!(actual, expected);
        assert_eq!(
            geometry
                .range_geometry([("all", 0..u32::MAX)])
                .into_iter()
                .map(|rect| rect.bounds)
                .collect::<Vec<_>>(),
            geometry.selection_rects(0..u32::MAX)
        );
        let clipped = geometry.range_geometry([("first", first)]);
        assert!(
            clipped
                .iter()
                .all(|rect| rect.bounds.width() <= 60.0 && rect.bounds.height() <= 18.0)
        );
    }

    #[test]
    fn reading_cursor_seeks_and_exhausts_without_wrapping() {
        let engine = load(
            "<p>first <b>unit</b></p><p>second unit</p><table><tr><td>left</td><td>right</td></tr></table><p>third unit</p>",
        );
        let reading = Reading::new(engine.render_view().unwrap());
        let targets = reading.targets(0..u32::MAX);
        assert!(targets.len() >= 3);
        let mut cursor = reading.cursor(0);
        assert_eq!(cursor.current(), targets.first());
        assert!(!cursor.previous());
        for target in &targets[1..] {
            assert!(cursor.advance());
            assert_eq!(cursor.current(), Some(target));
        }
        assert!(!cursor.advance());
        assert!(!cursor.advance());
        assert_eq!(cursor.index(), targets.len());
        assert!(cursor.previous());
        assert_eq!(cursor.current(), targets.last());
        for glyph in [
            0,
            find(&engine, "second").start,
            find(&engine, "right").start,
            u32::MAX,
        ] {
            cursor.seek(glyph);
            assert_eq!(
                cursor.index(),
                targets
                    .iter()
                    .position(|target| target.glyph_ranges.iter().any(|range| range.end > glyph))
                    .unwrap_or(targets.len())
            );
        }
        let empty = load("<div></div>");
        let mut cursor = Reading::new(empty.render_view().unwrap()).cursor(0);
        assert!(cursor.current().is_none());
        assert!(!cursor.advance() && !cursor.previous());
    }

    #[test]
    fn resource_requests_follow_the_callers_lines_and_region_and_release_empty_sets() {
        let mut engine = load(
            "<p><img src='first.svg' width='42' height='31'></p><p>space</p><p><img src='second.svg' width='42' height='31'></p>",
        );
        engine.set_image_service(ImageService::with_loader(
            Arc::new(SvgLoader),
            Arc::new(InlineJobSpawner),
            1024 * 1024,
        ));
        let view = engine.render_view().unwrap();
        let fragments = view.fragments();
        let images: Vec<_> = fragments.images().iter().collect();
        let first = &images[0];
        let first_line = first.line_idx();
        let second = &images[1];
        let region = second.visible_rect()
            + view
                .text()
                .line(second.line_idx())
                .unwrap()
                .point()
                .to_vec2();
        engine.request_images_for_lines([first_line, first_line, usize::MAX]);
        engine.poll_resources();
        assert!(engine.images().get_decoded(0).is_some());
        assert!(engine.images().get_decoded(1).is_none());
        engine.request_images_in(region);
        engine.poll_resources();
        assert!(engine.images().get_decoded(0).is_none());
        assert!(engine.images().get_decoded(1).is_some());
        engine.request_images_for_lines([]);
        assert!(engine.images().get_decoded(1).is_none());
        assert!(!engine.resources_pending());
        engine.request_images_in(Rect::new(-200.0, -200.0, -100.0, -100.0));
        assert!(engine.images().get_decoded(0).is_none());
    }
}

#[cfg(feature = "font-data")]
mod font_preparation {
    use super::*;
    use html::fonts::{
        FontData, FontLoadFuture, FontSourceLoader, MAX_FONT_BYTES, decode, prepare_face,
        resolve_face,
    };
    use html::style::FontFaceSource;
    use std::future::Future;
    use std::sync::Mutex;
    use std::task::{Context, Poll, Wake, Waker};

    const WOFF2: &[u8] = include_bytes!("fixtures/fonts/reader-test.woff2");
    struct Loader(Mutex<Vec<String>>);
    impl FontSourceLoader for Loader {
        fn local_family(&self, name: &str) -> Option<String> {
            (name == "installed").then(|| name.into())
        }
        fn load<'a>(&'a self, url: &'a str) -> FontLoadFuture<'a> {
            self.0.lock().unwrap().push(url.into());
            Box::pin(async move {
                match url {
                    "broken" => Ok(b"bad font".to_vec()),
                    "valid" => Ok(WOFF2.to_vec()),
                    _ => Err("missing".into()),
                }
            })
        }
    }
    // Fixture I/O completes immediately; a pending future signals a broken fixture.
    fn ready<T>(future: impl Future<Output = T>) -> T {
        struct Noop;
        impl Wake for Noop {
            fn wake(self: Arc<Self>) {}
        }
        let waker = Waker::from(Arc::new(Noop));
        match std::pin::pin!(future)
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("fixture font I/O must complete immediately"),
        }
    }

    #[test]
    fn font_sources_fall_back_in_authored_order_including_native_rejection() {
        let engine = load(
            "<style>@font-face { font-family:Test; src:local(installed),url(unsupported) format(svg),url(missing),url(broken),url(valid#face) format(woff2),local(installed); }</style><p>font</p>",
        );
        let face = &engine.font_faces()[0];
        let loader = Loader(Mutex::new(Vec::new()));
        let mut prepared = 0;
        let result = ready(prepare_face(face, &loader, |data| {
            prepared += 1;
            match (prepared, data) {
                (1, FontData::Local(_)) => Err("native local rejection".into()),
                (2, FontData::Decoded(_)) => Err("native decoded rejection".into()),
                (3, FontData::Local(family)) => Ok(family),
                _ => panic!("unexpected source order"),
            }
        }))
        .unwrap();
        assert_eq!(result, "installed");
        assert_eq!(prepared, 3);
        assert_eq!(*loader.0.lock().unwrap(), ["missing", "broken", "valid"]);
        let mut face = face.clone();
        face.sources = vec![
            FontFaceSource::Url {
                url: "valid".into(),
                format: Some("woff2".into()),
                technologies: vec!["color-COLRv1".into()],
            },
            FontFaceSource::Local("installed".into()),
        ];
        assert!(matches!(
            ready(resolve_face(&face, &loader)).unwrap(),
            FontData::Local(_)
        ));
        assert_eq!(loader.0.lock().unwrap().len(), 3);
    }

    #[test]
    fn font_resolution_transfers_sfnt_buffers_without_copying() {
        struct OwnedLoader(Mutex<Option<Vec<u8>>>);
        impl FontSourceLoader for OwnedLoader {
            fn local_family(&self, _: &str) -> Option<String> {
                None
            }
            fn load<'a>(&'a self, _: &'a str) -> FontLoadFuture<'a> {
                Box::pin(async move {
                    self.0
                        .lock()
                        .unwrap()
                        .take()
                        .ok_or_else(|| "already loaded".into())
                })
            }
        }
        let sfnt = decode(WOFF2).unwrap();
        let pointer = sfnt.as_ptr();
        let capacity = sfnt.capacity();
        let loader = OwnedLoader(Mutex::new(Some(sfnt)));
        let engine = load("<style>@font-face { font-family:Test; src:url(sfnt); }</style>");
        let mut face = engine.font_faces()[0].clone();
        face.sources = vec![FontFaceSource::Url {
            url: "sfnt".into(),
            format: Some("OpEnTyPe".into()),
            technologies: vec![],
        }];
        let FontData::Decoded(bytes) = ready(resolve_face(&face, &loader)).unwrap() else {
            panic!("expected decoded SFNT data");
        };
        assert_eq!(bytes.as_ptr(), pointer);
        assert_eq!(bytes.capacity(), capacity);
    }

    #[test]
    fn font_decode_validates_woff2_sfnt_data_urls_and_size_limits() {
        let sfnt = decode(WOFF2).unwrap();
        assert_eq!(decode(&sfnt).unwrap(), sfnt);
        assert!(decode(b"invalid").is_err());
        assert!(decode(b"wOF2").is_err());
        let mut oversized_header = WOFF2.to_vec();
        oversized_header[16..20].copy_from_slice(&((MAX_FONT_BYTES + 1) as u32).to_be_bytes());
        assert!(
            decode(&oversized_header)
                .unwrap_err()
                .contains("size limit")
        );
        let engine = load("<style>@font-face { font-family:Test; src:url(broken); }</style>");
        let mut face = engine.font_faces()[0].clone();
        let percent_encoded: String = WOFF2.iter().map(|byte| format!("%{byte:02X}")).collect();
        face.sources = vec![FontFaceSource::Url {
            url: format!("data:font/woff2,{percent_encoded}"),
            format: None,
            technologies: vec![],
        }];
        let loader = Loader(Mutex::new(Vec::new()));
        assert!(
            matches!(ready(resolve_face(&face, &loader)).unwrap(), FontData::Decoded(bytes) if bytes == sfnt)
        );
        assert!(loader.0.lock().unwrap().is_empty());
        face.sources = vec![FontFaceSource::Url {
            url: "broken".into(),
            format: None,
            technologies: vec![],
        }];
        assert!(
            ready(resolve_face(&face, &loader))
                .err()
                .unwrap()
                .contains("broken")
        );
    }
}
