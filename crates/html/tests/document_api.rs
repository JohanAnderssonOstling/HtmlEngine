use html_resources::resources::images::{decoded_image_dimensions, decoded_images_get};
use html_pipeline::engine::pagination_queries;
use html_pipeline::engine::reading_queries;
use crate::pagination::PaginationTarget;
use html_pipeline::engine::box_queries;
use html_pipeline::engine::geometry_queries;
use html_pipeline::engine::search_queries;
use html_dom as dom_values;
use html_engine_model::layout_state;
use html_pipeline::engine::export_queries;
use html_pipeline::engine::fragment_paint;
use html_pipeline::engine::text_paint;
use html_render_core as render_core;
use crate::configuration::{DocumentUpdate, Update};

use html_pipeline::MarkupSyntax;
use crate::document::ImageId;
use html_dom::ImageSource;
use html_pipeline::engine::TableExportRequest;
use html_pipeline::engine::TextExport;
use html_pipeline::engine::TextExportFormat;
use crate::geometry::{SelectionHighlight, WordHit, WordHitPolicy};
use crate::layout::BoxId;

use crate::{DocumentOptions, Document};

use crate::render::LinePlacement;

use crate::layout::LineId;

use html_render_core::{RecordingPainter};
use crate::render::DocumentPainter;
use crate::render::RenderScene;
use html_render_core::SceneOptions;
use html_resources::ImageLoadCompletion;
use crate::resources::ImageLoader;
use crate::resources::ImageService;
use crate::resources::InlineJobSpawner;
use crate::resources::ResourceMetadata;
use crate::resources::ResourceProvider;
use crate::search::SearchIndex;
use crate::targets::ElementTarget;
use crate::selection::{Selection, SelectionAnchor, SelectionInput, TableSelectionState};
use crate::structure::{Structure, HeaderPolicy, TableGroupKind};
use crate::text::{
    GlyphRequest, GlyphResourceGeneration, GlyphShaper, ShapeError, FontMetrics, GlyphHandle,
    ShapedGlyph, TextSearchOptions,
};
use kurbo::{Point, Rect, Size, Vec2};
use std::collections::HashMap;
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

fn load(source: &str) -> Document {
    Document::load(
        provider(source),
        "document.html",
        DocumentOptions::new(600.0).unwrap(),
        &mut Shaper::default(),
    )
    .unwrap()
}

fn find(engine: &Document, text: &str) -> Range<u32> {
    search_queries::render_text_view_find_matches(engine
        .render_view()
        .unwrap()
        .text(), text, TextSearchOptions::default())[0]
        .clone()
}

#[test]
fn containing_element_selects_authored_blocks_with_inline_children() {
    for (markup, needle, expected) in [
        (
            "<p>Before</p><p>Alpha <strong>bold <em>word</em></strong> omega.</p><p>After</p>",
            "word",
            "Alpha bold word omega.",
        ),
        (
            "<h2>Chapter <span>title</span></h2><p>Body</p>",
            "title",
            "Chapter title",
        ),
        (
            "<ul><li>First item</li><li>Second <span>item</span> here</li><li>Last item</li></ul>",
            "here",
            "Second item here",
        ),
        (
            "<table><tr><td>Left cell</td><td>Right <b>cell</b> here</td></tr></table>",
            "here",
            "Right cell here",
        ),
        (
            "<div><p>First paragraph</p><p>Second <span>paragraph</span></p></div>",
            "Second",
            "Second paragraph",
        ),
    ] {
        let engine = load(markup);
        let view = engine.render_view().unwrap();
        let selection = Selection::containing_element(view, find(&engine, needle).start).unwrap();
        assert_eq!(selection.text().plain.unwrap().trim(), expected);
        assert!(selection.range().start < selection.range().end);
        assert!(Selection::containing_element(view, u32::MAX).is_none());
    }
}

#[test]
fn containing_element_stops_at_flex_and_grid_containers() {
    for display in ["flex", "grid", "inline-flex", "inline-grid"] {
        let engine = load(&format!(
            "<p>Before</p><div style='display:{display}'>Alpha word omega.</div><p>After</p>"
        ));
        let view = engine.render_view().unwrap();
        let selection = Selection::containing_element(view, find(&engine, "word").start).unwrap();
        assert_eq!(
            selection.text().plain.unwrap().trim(),
            "Alpha word omega.",
            "{display}"
        );
    }
}

#[test]
fn disjoint_line_export_deduplicates_and_preserves_source_structure() {
    let engine = load(
        "<p>one <b>two</b></p><p>skip</p><p>three</p><p style='visibility:hidden'>hidden</p><ol><li>item</li></ol>",
    );
    let content = engine.render_view().unwrap().export();
    let one = find(&engine, "one");
    let three = find(&engine, "three");
    let selected = export_queries::render_export_view_plain_text_ranges(
        content,
        [three.clone(), one.clone(), one],
    )
    .unwrap();
    assert!(selected.starts_with("one") && selected.ends_with("three"));
    assert!(!selected.contains("skip") && selected.contains('\n'));
    let view = engine.render_view().unwrap();
    let lines: Vec<_> = text_paint::render_text_view_spatial_order(view.text())
        .iter()
        .copied()
        .rev()
        .flat_map(|line| [line, line])
        .collect();
    let all = export_queries::text_for_lines(content, lines).unwrap();
    assert_eq!(all.matches("one").count(), 1);
    assert!(!all.contains("hidden") && !all.contains("1."));
}

#[test]
fn intrinsic_reading_regions_retain_cells_and_translate_without_columns() {
    let engine = load(
        "<table><tr><td>left</td><td>right</td></tr></table><p style='visibility:hidden'>hidden</p>",
    );
    let view = engine.render_view().unwrap();
    let line =
        text_paint::render_text_view_line_for_glyph(view.text(), find(&engine, "left").start)
            .unwrap();
    let geometry = engine.geometry().unwrap();
    let natural = geometry
        .placed_reading_line(
            LinePlacement {
                line,
                origin: view.text().line(line).unwrap().point(),
            },
            Rect::new(0.0, 0.0, 600.0, 200.0),
        )
        .unwrap();
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
    for reading in text_paint::render_text_view_spatial_order(view.text())
        .iter()
        .copied()
        .filter_map(|index| {
            geometry.placed_reading_line(
                LinePlacement {
                    line: index,
                    origin: view.text().line(index).unwrap().point(),
                },
                Rect::new(0.0, 0.0, 600.0, 200.0),
            )
        })
    {
        assert!(
            !export_queries::render_export_view_text(
                view.export(),
                reading.glyphs,
                TextExportFormat::Plain
            )
            .plain
            .unwrap_or_default()
            .contains("hidden")
        );
    }
}

#[test]
fn word_hits_snap_whitespace_and_select_across_an_unchanged_caret() {
    use crate::selection::{SelectionMode, SelectionState};
    let engine = load("<p style='font-size:100px'>a猫</p>");
    let geometry = engine.geometry().unwrap();
    let a = geometry.selection_rects(0..1)[0];
    let cat = geometry.selection_rects(1..2)[0];
    let start = Point::new(a.x0 + a.width() * 0.8, a.center().y);
    let end = Point::new(cat.x0 + cat.width() * 0.2, cat.center().y);
    let initial = geometry.word_hit_at(start, WordHitPolicy::Nearest).unwrap();
    let target = geometry.word_hit_at(end, WordHitPolicy::Nearest).unwrap();
    assert_eq!(initial.caret, target.caret);
    assert_eq!(initial.word, 0..1);
    assert_eq!(target.word, 1..2);
    let mut selection = SelectionState::default();
    selection.begin_at_word(initial, SelectionMode::Plain);
    assert!(selection.update_word_interaction(Some(target), SelectionMode::Plain, 4.0));
    assert_eq!(selection.range(), Some(0..2));

    let engine = load("<p style='font-size:100px;white-space:pre'>alpha   beta</p><p>outside</p>");
    let geometry = engine.geometry().unwrap();
    let spaces = geometry.selection_bounds(5..8).unwrap();
    assert_eq!(
        geometry
            .word_hit_at(
                Point::new(spaces.x0 + 1.0, spaces.center().y),
                WordHitPolicy::Nearest
            )
            .map(|hit| hit.word),
        Some(0..5)
    );
    assert_eq!(
        geometry
            .word_hit_at(
                Point::new(spaces.x1 - 1.0, spaces.center().y),
                WordHitPolicy::Nearest
            )
            .map(|hit| hit.word),
        Some(8..12)
    );
    assert_eq!(
        geometry
            .word_hit_at(spaces.center(), WordHitPolicy::InkOnly)
            .map(|hit| hit.word),
        None
    );
}

#[test]
fn nearest_word_ignores_clipped_intervening_text() {
    let engine = load("<p style='font-size:30px'>alpha <span style='display:inline-block;width:0;height:40px;overflow:hidden'>hidden</span> beta</p>");
    let geometry = engine.geometry().unwrap();
    let alpha = find(&engine, "alpha");
    let beta = find(&engine, "beta");
    assert!(geometry.selection_rects(find(&engine, "hidden")).is_empty());
    let before = geometry.selection_rects(alpha.end..alpha.end + 1)[0];
    let after = geometry.selection_rects(beta.start - 1..beta.start)[0];
    assert_eq!(
        geometry
            .word_hit_at(before.center(), WordHitPolicy::Nearest)
            .map(|hit| hit.word),
        Some(alpha)
    );
    assert_eq!(
        geometry
            .word_hit_at(after.center(), WordHitPolicy::Nearest)
            .map(|hit| hit.word),
        Some(beta)
    );
    assert_eq!(
        geometry
            .word_hit_at(before.center(), WordHitPolicy::InkOnly)
            .map(|hit| hit.word),
        None
    );
}

#[test]
fn word_hits_respect_ink_and_caller_selected_lines() {
    let engine = load("<p>first word</p><p>next</p>");
    let geometry = engine.geometry().unwrap();
    let range = find(&engine, "word");
    let rect = geometry.selection_rects(range.clone())[0];
    assert_eq!(
        geometry
            .word_hit_at(rect.center(), WordHitPolicy::InkOnly)
            .map(|hit| hit.word),
        Some(range.clone())
    );
    assert!(
        geometry
            .word_hit_at(Point::new(-100.0, -100.0), WordHitPolicy::InkOnly)
            .map(|hit| hit.word)
            .is_none()
    );
    let text = engine.render_view().unwrap().text();
    let line = text_paint::render_text_view_line_for_glyph(text, range.start).unwrap();
    let placements = [LinePlacement {
        line,
        origin: text.line(line).unwrap().point(),
    }];
    let nearest = geometry
        .word_hit_at_lines(
            Point::new(rect.x1 + 100.0, rect.y1 + 100.0),
            placements,
            WordHitPolicy::Nearest,
        )
        .unwrap()
        .word;
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
    let selection = engine.selection(selected).unwrap().with_semantics();
    let view = engine.render_view().unwrap();
    let table = (0..box_queries::len(view.boxes()))
        .map(layout_state::box_id_new)
        .find_map(|index| box_queries::render_box_view_table(view.boxes(), index))
        .unwrap();
    let cell_box = table.rows()[0].cells()[0].box_id();
    let cell_rect = Rect::from_origin_size(
        box_queries::render_box_view_point(view.boxes(), cell_box).unwrap(),
        box_queries::render_box_view_size(view.boxes(), cell_box).unwrap(),
    );
    let mut text_rects = Vec::new();
    let mut cells = Vec::new();
    engine.geometry().unwrap().collect_highlights(
        [],
        Some(SelectionHighlight {
            selection: selection.clone(),
            text: false,
            cells: true,
        }),
        None,
        view.text().lines().iter().map(|line| LinePlacement {
            line: line.id(),
            origin: line.point(),
        }),
        &mut text_rects,
        &mut cells,
    );
    assert!(text_rects.is_empty());
    let cell_rects: Vec<_> = cells
        .iter()
        .map(|&(_, index)| {
            Rect::from_origin_size(
                box_queries::render_box_view_point(view.boxes(), index).unwrap(),
                box_queries::render_box_view_size(view.boxes(), index).unwrap(),
            )
        })
        .collect();
    assert!(cells.iter().all(|&(cell, _)| cell));
    assert_eq!(cell_rects, vec![cell_rect]);
    assert!(engine.geometry().unwrap().selection_contains_lines(
        &selection,
        cell_rect.center(),
        [],
        |index| index == cell_box,
    ));
    let mut selected = TableSelectionState::default();
    selected.begin(
        engine
            .geometry()
            .unwrap()
            .table_at_where(|box_idx| box_idx == cell_box)
            .unwrap(),
    );
    let range = selected.range().unwrap();
    assert_eq!(
        range.markdown(&table),
        table.export(TableExportRequest::CellsMarkdown {
            rows: 0..2,
            columns: 0..1
        }),
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
        search_queries::render_text_view_find_matches(load(source)
            .render_view()
            .unwrap()
            .text(), "hidden", TextSearchOptions::default())
            .is_empty()
    );
    assert_eq!(
        index.statistics(),
        crate::search::TextStatistics::from_text(index.text())
    );
}

#[test]
fn structural_boundaries_include_empty_elements_and_authored_constraints() {
    let engine = load(
        "<p>alpha</p><div id='empty' style='break-before:page'></div><p id='flow' style='orphans:3;widows:4;break-inside:avoid'>beta</p>",
    );
    let view = engine.render_view().unwrap();
    let mut structure = Structure::default();
    let index = structure.for_lines(view, 0..view.text().line_count());
    let empty = (0..box_queries::len(view.boxes()))
        .map(layout_state::box_id_new)
        .find(|&box_idx| box_queries::attribute(view.boxes(), box_idx, "id") == Some("empty"))
        .unwrap();
    let flow = (0..box_queries::len(view.boxes()))
        .map(layout_state::box_id_new)
        .find(|&box_idx| box_queries::attribute(view.boxes(), box_idx, "id") == Some("flow"))
        .unwrap();
    let beta_line =
        text_paint::render_text_view_line_for_glyph(view.text(), find(&engine, "beta").start)
            .and_then(|line| {
                pagination_queries::render_view_pagination(view)
                    .locate(PaginationTarget::Line(line))
                    .map(|position| position.cursor)
            })
            .unwrap();
    assert_eq!(index.before(empty), Some(beta_line));
    assert!(
        index
            .authored_boundaries()
            .any(|boundary| boundary.trigger_line == beta_line && boundary.forced.is_some())
    );
    let constraints = index.constraints(flow);
    assert!(constraints.avoid_inside && constraints.owns_line_flow);
    assert_eq!((constraints.orphans, constraints.widows), (3, 4));
    let empty = structure.for_lines(view, 2..2);
    assert_eq!(empty.lines().count(), 0);
}

#[test]
fn table_dependencies_expose_span_groups_and_keep_header_inference_optional() {
    let engine = load(
        "<table><caption>caption</caption><tr><th>head</th><th>title</th></tr><tr><td rowspan='2'>span</td><td>one</td></tr><tr><td>two</td></tr></table>",
    );
    let view = engine.render_view().unwrap();
    let mut structure = Structure::default();
    let mut index = structure.for_lines(view, 0..view.text().line_count());
    let explicit = index.table_dependencies(HeaderPolicy::ExplicitOnly);
    assert!(explicit.iter().all(|group| !matches!(
        group.kind,
        TableGroupKind::RowGroup {
            header: Some(_),
            ..
        }
    )));
    let groups = index.table_dependencies(HeaderPolicy::InferLeadingHeaderCells);
    let whole = groups
        .iter()
        .find(|group| matches!(group.kind, TableGroupKind::WholeTable))
        .unwrap();

    let body = groups
        .iter()
        .find(|group| {
            matches!(
                group.kind,
                TableGroupKind::RowGroup {
                    header: Some(_),
                    ..
                }
            )
        })
        .unwrap();
    assert!(body.bottom > body.top && body.bottom <= whole.bottom);
    let span_line =
        text_paint::render_text_view_line_for_glyph(view.text(), find(&engine, "span").start)
            .unwrap();
    let two_line =
        text_paint::render_text_view_line_for_glyph(view.text(), find(&engine, "two").start)
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
    let mut structure = Structure::default();
    let index = structure.for_lines(view, 0..view.text().line_count());
    let pairs = index.sibling_pairs();
    assert!(pairs.iter().any(|pair| pair.definition_pair));
    assert!(pairs.iter().any(|pair| pair.caption_pair));
    assert!(pairs.iter().any(|pair| pair.heading_following));
}

#[test]
fn document_revisions_track_successful_changes_without_viewport_state() {
    let mut shaper = Shaper::default();
    let mut engine = Document::load(
        provider("<p>text</p>"),
        "document.html",
        DocumentOptions::new(600.0).unwrap(),
        &mut shaper,
    )
    .unwrap();
    let before = engine.revisions();
    let update = engine
        .apply(DocumentUpdate::default(), &mut shaper)
        .unwrap();
    assert_eq!(update, Update::Unchanged);
    assert_eq!(engine.revisions(), before);
    engine
        .apply(
            DocumentUpdate::default().reader_overrides(crate::style::ReaderStyleOverrides { foreground: Some(0x112233ff), ..Default::default() }),
            &mut shaper,
        )
        .unwrap();
    assert!(!engine.revisions().layout_changed_since(before));
    assert!(!engine.revisions().resources_changed_since(before));
    assert_ne!(engine.revisions(), before);
    let painted = engine.revisions();
    engine
        .apply(
            DocumentUpdate::default().source("<p>replacement</p>".into()),
            &mut shaper,
        )
        .unwrap();
    assert!(engine.revisions().layout_changed_since(painted));
    let revised = engine.revisions();
    assert!(engine.apply(DocumentUpdate::default().width(f64::NAN), &mut shaper).is_err());
    assert_eq!(engine.revisions(), revised);
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
    let (engine, timings) = Document::load_timed(
        provider,
        "document.html",
        DocumentOptions::new(600.0).unwrap(),
        &mut Shaper::default(),
    )
    .unwrap();
    assert!(engine.render_view().is_some());
    assert_eq!(timings.total(), timings.resource + timings.pipeline);
    let source = engine
        .image_source_request(dom_values::image_id_new(0))
        .unwrap()
        .read()
        .unwrap();
    assert_eq!(source.uri, "pic.svg");
    assert_eq!(source.bytes, b"<svg width='20' height='10'/>");
}

#[test]
fn image_queries_painting_and_scene_extent_agree_on_object_fit_and_placement() {
    for (fit, fitted_size) in [
        ("contain", Size::new(100.0, 20.0)),
        ("cover", Size::new(500.0, 100.0)),
    ] {
        let mut engine = load(&format!(
            "<body style='margin:0'><img src='wide.png' width='100' height='20' style='width:100px;height:100px;object-fit:{fit};position:relative;top:80px;vertical-align:top'></body>",
        ));
        // Object fitting uses natural image metrics, not HTML dimension hints.
        let mut metrics = html_pipeline::engine::ImageMetrics::default();
        metrics.set(0, 100, 20);
        engine
            .apply_changes(
                html_pipeline::PipelineChanges {
                    image_metrics: Some(metrics),
                    ..Default::default()
                },
                &mut Shaper::default(),
            )
            .unwrap();
        let view = engine.render_view().unwrap();
        let image = fragment_paint::render_fragment_view_images(view.fragments())
            .iter()
            .next()
            .unwrap();
        let line = view
            .text()
            .line(fragment_paint::render_image_fragment_line(&image))
            .unwrap();
        let rect = Rect::from_origin_size(
            line.point() + fragment_paint::render_image_fragment_offset(&image).to_vec2(),
            fragment_paint::render_image_fragment_size(&image),
        );
        let clip = fragment_paint::render_image_fragment_clip(&image) + line.point().to_vec2();
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
        assert_eq!(
            geometry
                .content_at(inside)
                .and_then(|hit| hit.image)
                .map(|image| image.image),
            Some(dom_values::image_id_new(0))
        );
        assert_eq!(
            geometry
                .content_at(padding)
                .and_then(|hit| hit.image)
                .map(|image| image.image),
            None
        );

        let delta = Vec2::new(17.25, -31.125);
        assert_eq!(
            geometry.image_at_lines(
                inside + delta,
                [LinePlacement {
                    line: line.id(),
                    origin: line.point() + delta
                }],
            ),
            Some(dom_values::image_id_new(0))
        );
        let mut painter = RecordingPainter::default();
        (&DocumentPainter::new(engine.render_view().unwrap()).with_images(engine.images())).paint(
            &render_core::RegionProjection {
                source: Rect::new(0.0, 0.0, 600.0, 300.0),
                destination: Point::ORIGIN + delta,
            },
            &mut painter,
        );
        assert_eq!(painter.resource_images[0].2, rect + delta);
        assert!(painter.clips.contains(&(clip + delta)));

        let expected_height = view
            .text()
            .lines()
            .iter()
            .map(|line| line.point().y + line.height())
            .fold(visible.y1, f64::max);
        let scene =
            render_core::render_scene_for_view(view, &SceneOptions::new(600.0, None).unwrap());
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
            .map(|index| text_paint::render_text_view_character_at(text, index as usize).unwrap())
            .collect::<String>()
    };
    let hidden = text_paint::render_text_view_text_runs(text)
        .find(|run| source_text(run.glyphs()) == "secret")
        .unwrap()
        .glyphs();
    let selection = engine.selection(hidden).unwrap().with_semantics();
    assert_eq!(selection.text(), TextExport::default());
    let visible: Vec<_> = text_paint::render_text_view_visible_source_runs(text)
        .map(|run| source_text(run.glyphs()))
        .filter(|text| !text.is_empty())
        .collect();
    assert_eq!(visible, ["visible", "Restored"]);
    let reading: String = reading_queries::reading_page(
        engine.render_view().unwrap().addressing(),
        0..text.glyph_count() as u32,
        text.glyph_count() as u32,
    )
    .fragments
    .into_iter()
    .map(|fragment| fragment.text)
    .collect();
    assert_eq!(reading, "visibleRestored");
}

#[test]
fn fragments_tables_and_search_previews_share_unicode_whitespace_normalization() {
    use html_pipeline::plain_text_from_fragment;

    let source = "<table><tr><td style='white-space:pre-wrap'>\t Café\u{a0} \n<span>漢\u{2003}字</span> \r\n🙂e\u{301}\u{202f}</td></tr></table>";
    let expected = "Café 漢 字 🙂e\u{301}";
    assert_eq!(plain_text_from_fragment(source), expected);
    let engine = load(source);
    let view = engine.render_view().unwrap();
    let boxes = view.boxes();
    let table = (0..box_queries::len(boxes))
        .map(layout_state::box_id_new)
        .find_map(|index| box_queries::render_box_view_table(boxes, index))
        .unwrap();
    assert_eq!(table.rows()[0].cells()[0].text(), expected);
    let search = engine.render_view().unwrap().export();
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
    let view = engine.render_view().unwrap();
    let search = view.export();
    let results = search_queries::render_text_view_find_matches(view.text(), "cafe", TextSearchOptions::default());
    assert_eq!(results.len(), 2);
    let first = results.first().unwrap().clone();
    assert_eq!(
        search.excerpt(first.clone(), 7, 8).as_deref(),
        Some("Before Café after")
    );
    assert_eq!(search.excerpt(first, 0, 0).as_deref(), Some("Café"));
    let second = results[1].clone();
    assert_eq!(
        second.end - second.start,
        5,
        "retain the combining mark's source position"
    );
    assert_eq!(search.excerpt(second, 0, 0).as_deref(), Some("Cafe\u{301}"));
    for query in ["secret", "concealed", "1"] {
        assert!(search_queries::render_text_view_find_matches(view.text(), query, TextSearchOptions::default()).is_empty());
    }
    assert!(search.excerpt(10..2, 0, 0).is_none());
    assert!(search.excerpt(0..u32::MAX, 0, 0).is_none());
    let preview = search
        .excerpt(results[0].clone(), u32::MAX, u32::MAX)
        .unwrap();
    assert!(
        !preview.contains("secret") && !preview.contains("concealed") && !preview.contains("1.")
    );
}

#[test]
fn table_hit_testing_retains_spans_and_accepts_host_projection() {
    let engine = load(
        "<table><tr><td rowspan='2' colspan='2'>spanned</td><td>other</td></tr>\
         <tr><td>last</td></tr></table>",
    );
    let view = engine.render_view().unwrap();
    let boxes = view.boxes();
    let table = (0..box_queries::len(boxes))
        .map(layout_state::box_id_new)
        .find_map(|index| box_queries::render_box_view_table(boxes, index))
        .unwrap();
    let cell = &table.rows()[0].cells()[0];
    let rect = Rect::from_origin_size(
        box_queries::render_box_view_point(boxes, cell.box_id()).unwrap(),
        box_queries::render_box_view_size(boxes, cell.box_id()).unwrap(),
    );
    let geometry = engine.geometry().unwrap();
    let hit = geometry.content_at(rect.center()).unwrap().table.unwrap();
    assert_eq!(hit.table_box(), table.box_id());
    let mut selected = TableSelectionState::default();
    selected.begin(hit);
    let range = selected.range().unwrap();
    // A host can test page fragments or any other arrangement of cell boxes.
    let projected = geometry
        .table_at_where(|box_idx| box_idx == cell.box_id())
        .unwrap();
    selected.begin(projected);
    assert_eq!(selected.range(), Some(range));
    assert_eq!(cell.row_range(), 0..2);
    assert_eq!(cell.column_range(), 0..2);
    let (mut text_rects, mut cells) = (Vec::new(), Vec::new());
    geometry.collect_highlights([], None, Some(((), range)), [], &mut text_rects, &mut cells);
    let highlighted: Vec<_> = cells
        .iter()
        .map(|&((), index)| {
            Rect::from_origin_size(
                box_queries::render_box_view_point(boxes, index).unwrap(),
                box_queries::render_box_view_size(boxes, index).unwrap(),
            )
        })
        .collect();
    assert!(text_rects.is_empty());
    assert_eq!(highlighted, vec![rect]);
    // A rectangle starting inside a merged cell still exports its text once.
    for (rows, columns) in [(0..2, 0..2), (1..2, 1..2)] {
        let markdown = table
            .export(TableExportRequest::CellsMarkdown {
                rows: rows,
                columns: columns,
            })
            .unwrap();
        assert_eq!(markdown.matches("spanned").count(), 1, "{markdown}");
        assert!(!markdown.contains("other") && !markdown.contains("last"));
    }
    assert!(geometry.table_at_where(|_| false).is_none());
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
    let prepared = Document::prepare(
        provider.clone(),
        "books/chapter.html",
        DocumentOptions::new(320.0).unwrap(),
    )
    .unwrap();
    assert_eq!(
        SearchIndex::load(provider.as_ref(), "books/chapter.html")
            .unwrap()
            .text(),
        "Café"
    );
    assert_eq!(prepared.font_faces()[0].family, "Custom");
    let prepared = prepared
        .finish(DocumentUpdate::default(), &mut Shaper::default())
        .unwrap();

    let loaded = Document::load(
        provider,
        "books/chapter.html",
        DocumentOptions::new(320.0).unwrap(),
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
            text_paint::paint_color_at(engine.render_view().unwrap().text(), range.start),
            Some(0xff0000ff)
        );
        assert_eq!(engine.configuration().height(), None);
    }
}

#[test]
fn resource_loading_returns_errors_for_invalid_options_and_missing_sources() {
    for width in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(DocumentOptions::new(width).is_err());
    }
    for size in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(
            DocumentOptions::new(320.0)
                .unwrap()
                .with_root_font_size(size)
                .is_err()
        );
    }
    let error = Document::load(
        provider("<p>text</p>"),
        "missing.html",
        DocumentOptions::new(320.0).unwrap(),
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
    let semantic = plain.with_semantics();
    let text = semantic.text();
    assert_eq!(text.plain.as_deref(), Some("chosen"));
    let markdown = text.markdown.unwrap();
    assert!(markdown.contains("First *chosen* tail"), "{markdown}");
    assert!(!markdown.contains("Other item"));
}

#[test]
fn code_is_character_selectable_until_the_selection_crosses_its_boundary() {
    let engine = load("<p>Before</p><pre><code>alpha beta</code></pre><p>After</p>");
    let inside = engine
        .selection(find(&engine, "beta"))
        .unwrap()
        .with_semantics();
    let view = engine.render_view().unwrap();
    let geometry = engine.geometry().unwrap();
    let (mut highlights, mut cells) = (Vec::new(), Vec::new());
    geometry.collect_highlights(
        [],
        Some(SelectionHighlight {
            selection: inside,
            text: (),
            cells: (),
        }),
        None,
        view.text().lines().iter().map(|line| LinePlacement {
            line: line.id(),
            origin: line.point(),
        }),
        &mut highlights,
        &mut cells,
    );
    assert!(
        cells.is_empty(),
        "code selection must not select table cells"
    );
    let highlights: Vec<_> = highlights.into_iter().map(|((), bounds)| bounds).collect();
    assert_eq!(highlights, geometry.selection_rects(find(&engine, "beta")));
    let across = engine
        .selection(find(&engine, "Before").start..find(&engine, "beta").end)
        .unwrap()
        .with_semantics();
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
    let markdown = selection.text().markdown.unwrap();
    assert!(
        markdown.contains("One") && markdown.contains("Two"),
        "{markdown}"
    );
    assert!(!markdown.contains("Three") && !markdown.contains("Six"));
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
        assert_eq!(selection.text(), TextExport::default());
        let view = engine.render_view().unwrap();
        let (mut rects, mut cells) = (Vec::new(), Vec::new());
        engine.geometry().unwrap().collect_highlights(
            [],
            Some(SelectionHighlight {
                selection,
                text: (),
                cells: (),
            }),
            None,
            view.text().lines().iter().map(|line| LinePlacement {
                line: line.id(),
                origin: line.point(),
            }),
            &mut rects,
            &mut cells,
        );
        assert!(rects.is_empty() && cells.is_empty());
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
            line: line.id(),
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
    let (start, end) = geometry.selection_endpoints_lines(
        range.clone(),
        view.text().lines().iter().map(|line| LinePlacement {
            line: line.id(),
            origin: line.point(),
        }),
    );
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
    (&painter).paint(
        &render_core::RegionProjection {
            source: source,
            destination: Point::ORIGIN,
        },
        &mut first,
    );
    (&painter).paint(
        &render_core::RegionProjection {
            source: source,
            destination: Point::ORIGIN + delta,
        },
        &mut second,
    );
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
    use crate::selection::{SelectionMode, SelectionState};
    let mut selection = SelectionState::default();
    selection.begin_at_word(
        WordHit {
            caret: 8,
            word: 6..10,
            position: Point::new(80.0, 0.0),
        },
        SelectionMode::Plain,
    );
    selection.update_word_interaction(
        Some(WordHit {
            caret: 3,
            word: 1..5,
            position: Point::new(30.0, 0.0),
        }),
        SelectionMode::Semantic,
        4.0,
    );
    let _ = selection.finish_interaction();
    assert_eq!(selection.range(), Some(1..10));
    let engine = load(
        "<table><tr><td colspan='2'>top</td><td rowspan='2'>side</td></tr>\
         <tr><td>a</td><td>b</td></tr><tr><td colspan='2'>bottom</td><td>last</td></tr></table>\
         <table><tr><td>other</td></tr></table>",
    );
    let view = engine.render_view().unwrap();
    let tables: Vec<_> = (0..box_queries::len(view.boxes()))
        .map(layout_state::box_id_new)
        .filter_map(|index| box_queries::render_box_view_table(view.boxes(), index))
        .collect();
    let geometry = engine.geometry().unwrap();
    let first = geometry
        .table_at_where(|box_idx| box_idx == tables[0].rows()[0].cells()[1].box_id())
        .unwrap();
    let last = geometry
        .table_at_where(|box_idx| box_idx == tables[0].rows()[2].cells()[0].box_id())
        .unwrap();
    let reverse = geometry
        .table_at_where(|box_idx| box_idx == tables[0].rows()[1].cells()[0].box_id())
        .unwrap();
    let other = geometry
        .table_at_where(|box_idx| box_idx == tables[1].rows()[0].cells()[0].box_id())
        .unwrap();
    let mut table = TableSelectionState::default();
    table.begin(first);
    let first_range = table.range().unwrap();
    assert!(table.begin_drag(first));
    assert_eq!(table.update_drag(Some(last)), Some(true));
    let range = table.range().unwrap();
    assert_eq!(
        range.markdown(&tables[0]),
        tables[0].export(TableExportRequest::CellsMarkdown {
            rows: 0..3,
            columns: 0..3
        }),
    );
    assert_eq!(table.update_drag(None), Some(false));
    assert_eq!(table.update_drag(Some(other)), Some(false));
    assert_eq!(table.range(), Some(range));
    assert_eq!(table.update_drag(Some(reverse)), Some(true));
    assert_eq!(
        table.range().unwrap().markdown(&tables[0]),
        tables[0].export(TableExportRequest::CellsMarkdown {
            rows: 0..2,
            columns: 0..3
        }),
    );
    assert_eq!(table.update_drag(Some(reverse)), Some(false));
    assert_eq!(table.update_drag(Some(first)), Some(true));
    assert_eq!(table.range(), Some(first_range));
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
    let mut engine =
        load("<p><img src='picture.svg' width='7' height='9' style='width:auto;height:auto'/></p>");
    engine.set_image_service(ImageService::with_loader(
        Arc::new(SvgLoader),
        Arc::new(InlineJobSpawner),
        1024 * 1024,
    ));
    engine.request_image_lines(
        (0..engine.render_view().unwrap().text().line_count()).map(LineId::new),
        [],
    );
    let poll = engine.poll_resources();
    assert!(poll.decoded);
    engine
        .apply(DocumentUpdate::default(), &mut Shaper::default())
        .unwrap();
    assert_eq!(
        decoded_image_dimensions(
            decoded_images_get(engine.images(), dom_values::image_id_new(0)).unwrap()
        ),
        (42, 31)
    );
    let mut content =
        DocumentPainter::new(engine.render_view().unwrap()).with_images(engine.images());
    let mut visible = RecordingPainter::default();
    (&content).paint(
        &render_core::RegionProjection {
            source: Rect::new(0.0, 0.0, 600.0, 600.0),
            destination: Point::ORIGIN,
        },
        &mut visible,
    );
    assert!(!visible.svgs.is_empty());
    content.image_policy = crate::render::SceneImagePolicy::Omit;
    let mut omitted = RecordingPainter::default();
    (&content).paint(
        &render_core::RegionProjection {
            source: Rect::new(0.0, 0.0, 600.0, 600.0),
            destination: Point::ORIGIN,
        },
        &mut omitted,
    );
    assert!(omitted.svgs.is_empty());
    assert_eq!(
        fragment_paint::render_fragment_view_images(engine.render_view().unwrap().fragments())
            .iter()
            .next()
            .map(|image| (
                fragment_paint::render_image_fragment_size(&image).width as u32,
                fragment_paint::render_image_fragment_size(&image).height as u32
            )),
        Some((42, 31))
    );
    let poll = engine.poll_resources();
    assert!(!poll.decoded && !poll.layout_needed);
    engine
        .apply(
            DocumentUpdate::default().source("<p><img src='replacement.svg'/></p>".into()),
            &mut Shaper::default(),
        )
        .unwrap();
    assert!(decoded_images_get(engine.images(), dom_values::image_id_new(0)).is_none());
    engine.request_image_lines(
        (0..engine.render_view().unwrap().text().line_count()).map(LineId::new),
        [],
    );
    // A host may defer an update, or fail to apply it. The proposed intrinsic
    // sizes remain pending until the document accepts them.
    assert!(engine.poll_resources().layout_needed);
    engine.request_image_lines([], []);
    assert!(engine.resources_pending());
    engine.poll_resources();
    engine
        .apply(DocumentUpdate::default(), &mut Shaper::default())
        .unwrap();
    assert!(!engine.resources_pending());
    assert_eq!(
        fragment_paint::render_fragment_view_images(engine.render_view().unwrap().fragments())
            .iter()
            .next()
            .map(|image| (
                fragment_paint::render_image_fragment_size(&image).width as u32,
                fragment_paint::render_image_fragment_size(&image).height as u32
            )),
        Some((90, 31))
    );
    engine.request_image_lines(
        (0..engine.render_view().unwrap().text().line_count()).map(LineId::new),
        [],
    );
    engine.poll_resources();
    engine
        .apply(DocumentUpdate::default(), &mut Shaper::default())
        .unwrap();
    assert_eq!(
        decoded_image_dimensions(
            decoded_images_get(engine.images(), dom_values::image_id_new(0)).unwrap()
        ),
        (90, 31)
    );
    engine.request_image_lines([], []);
    assert!(decoded_images_get(engine.images(), dom_values::image_id_new(0)).is_none());
    assert_eq!(
        fragment_paint::render_fragment_view_images(engine.render_view().unwrap().fragments())
            .iter()
            .next()
            .map(|image| (
                fragment_paint::render_image_fragment_size(&image).width as u32,
                fragment_paint::render_image_fragment_size(&image).height as u32
            )),
        Some((90, 31))
    );
}

#[test]
fn element_links_and_notes_resolve_without_navigation_policy() {
    let engine = load(
        "<p><a href='other.html#part'>linked</a></p><h2 id='part'>Heading</h2><aside id='note' role='doc-footnote'>Note</aside>",
    );
    let targets = engine.render_view().unwrap().addressing();
    let link = targets.link_for_glyph(find(&engine, "linked").start).unwrap();
    assert_eq!(link.href, "other.html#part");
    let resolved = engine.resolve_link(link.href).unwrap();
    assert_eq!(resolved.resource, "other.html");
    assert_eq!(resolved.fragment.as_deref(), Some("part"));
    assert!(targets.has_element("part"));
    assert!(matches!(
        targets.element("part"),
        Some(ElementTarget::Text { y: Some(_), .. } | ElementTarget::Position { .. })
    ));
    assert!(targets.element("note").unwrap().is_note());
    assert_eq!(
        engine.resolve_link("#part").unwrap().resource,
        "document.html"
    );
}

#[test]
fn sparse_paint_candidates_restore_css_order_and_include_inline_decorations() {
    use html_pipeline::engine::RenderPaintItem;
    let engine = load(
        "<div style='position:relative;background:red'><p style='background:blue'>one <b style='background:green'>two</b></p><span style='position:absolute;z-index:-1;background:yellow'>behind</span></div>",
    );
    let view = engine.render_view().unwrap();
    let mut blocks = Vec::new();
    fragment_paint::block_decoration_indices_intersecting(view.fragments(), -1e6, 1e6, &mut blocks);
    blocks.reverse();
    blocks.extend_from_within(..);
    blocks.push(usize::MAX);
    let absent = LineId::new(u32::MAX as usize - 1);
    let mut lines = text_paint::render_text_view_spatial_order(view.text())
        .iter()
        .copied()
        .rev()
        .collect::<Vec<_>>();
    lines.extend_from_within(..);
    lines.push(absent);
    let signature = |item| match item {
        RenderPaintItem::Line(line) => format!("line:{line:?}"),
        RenderPaintItem::Decoration(decoration) => format!("decoration:{decoration:?}"),
    };
    let full = fragment_paint::paint_order(view.fragments())
        .map(signature)
        .collect::<Vec<_>>();
    let selected = fragment_paint::paint_order_for(view.fragments(), &lines, &blocks);
    drop(lines);
    drop(blocks);
    let selected = selected.map(signature).collect::<Vec<_>>();
    assert_eq!(selected, full);
    let expected = fragment_paint::paint_order(view.fragments())
        .filter(|item| match item {
            RenderPaintItem::Line(line) => *line == LineId::new(0),
            RenderPaintItem::Decoration(decoration) => {
                fragment_paint::render_decoration_line(&decoration) == Some(LineId::new(0))
            }
        })
        .map(signature)
        .collect::<Vec<_>>();
    let partial = fragment_paint::paint_order_for(
        view.fragments(),
        &[LineId::new(0), absent, LineId::new(0)],
        &[],
    )
    .map(signature)
    .collect::<Vec<_>>();
    assert_eq!(partial, expected);

    assert!(
        fragment_paint::paint_order_for(view.fragments(), &[], &[])
            .next()
            .is_none()
    );
    assert!(
        fragment_paint::paint_order_for(view.fragments(), &[absent], &[usize::MAX])
            .next()
            .is_none()
    );
}

#[test]
fn bounded_region_paint_omits_text_entirely_outside_the_source_clip() {
    let engine = load(
        "<body style='margin:0'><p style='margin:0;line-height:20px'>One</p><p style='margin:0;line-height:20px'>Excluded</p></body>",
    );
    let mut painter = RecordingPainter::default();
    (&DocumentPainter::new(engine.render_view().unwrap()).with_images(engine.images())).paint(
        &render_core::RegionProjection {
            source: Rect::new(0.0, 0.0, 600.0, 20.0),
            destination: Point::ORIGIN,
        },
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
    let geometry = geometry_queries::render_view_geometry(view);
    let hit = engine
        .geometry()
        .unwrap()
        .hit_test(Point::new(4.0, 30.0))
        .unwrap();
    let text = view.text();
    let clipped =
        search_queries::render_text_view_find_matches(text, "Clipped", Default::default())[0]
            .clone();
    assert!(
        engine
            .geometry()
            .unwrap()
            .selection_rects(clipped.clone())
            .is_empty()
    );
    assert!(!clipped.contains(&hit.glyph));
    let last =
        search_queries::render_text_view_find_matches(text, "Last", Default::default())[0].clone();
    let bounds = engine.geometry().unwrap().selection_bounds(last).unwrap();
    let range = engine
        .geometry()
        .unwrap()
        .block_range_at(bounds.center())
        .unwrap();
    assert_eq!(
        export_queries::render_export_view_text(view.export(), range, TextExportFormat::Plain)
            .plain
            .as_deref(),
        Some("Last paragraph")
    );
    let all = 0..text.glyph_count() as u32;
    assert!(
        geometry_queries::render_geometry_view_line_selection_rects(geometry, hit.line, all)
            .all(|rect| rect.height() > 0.0)
    );
}

#[test]
fn plain_text_block_separator_preserves_authored_breaks_and_unicode() {
    let engine = load("<p>First &amp; café<br>next line</p><p>Second paragraph</p>");
    let view = engine.render_view().unwrap();
    let all = 0..view.text().glyph_count() as u32;
    let expected = Some("First & café\nnext line\nSecond paragraph");
    assert_eq!(export_queries::render_export_view_text(view.export(), all.clone(), TextExportFormat::Plain).plain.as_deref(), expected);
    assert_eq!(
        export_queries::render_export_view_text(view.export(), all.clone(), TextExportFormat::PlainAndMarkdown).plain.as_deref(),
        expected
    );
    assert_eq!(
        export_queries::render_export_view_plain_text_ranges(view.export(), [all.clone()]).as_deref(),
        expected
    );
    assert_eq!(
        export_queries::render_export_view_text(view.export(), all, TextExportFormat::PlainWithParagraphBreaks).plain
            .as_deref(),
        Some("First & café\nnext line\n\nSecond paragraph")
    );
}

mod document_operations {
    use crate::layout::BoxId;
    use super::*;
    use crate::export::{ExportPayload, ExportRequest, ImagePayload, TableExportFormat};
    use crate::search::DocumentSearch;
    use crate::selection::SelectionMode;

    #[test]
    fn scopes_share_owner_resources_and_invalidate_without_stale_selection() {
        let source = "<p>outside</p><section id='part'><p>Scoped <b>content</b> here.</p></section><aside id='note' role='doc-footnote'>Note content.</aside>";
        let mut shaper = Shaper::default();
        let mut engine = Document::load(
            provider(source),
            "document.html",
            DocumentOptions::new(600.0).unwrap(),
            &mut shaper,
        )
        .unwrap();
        let width = 160.0;
        let mut scope = engine.scope("part", width, &mut shaper).unwrap().unwrap();
        let view = scope.render_view(&engine).unwrap();
        let count = view.text().glyph_count() as u32;
        let exported = Selection::new(view, 0..count).plain_text().unwrap();
        assert!(exported.contains("Scoped content"));
        assert!(!exported.contains("outside") && !exported.contains("Note"));
        assert!(scope.scene(&engine).unwrap().content_height() > 0.0);
        let first = scope.geometry(&engine).unwrap().selection_rects(0..1)[0].center();
        assert!(
            scope
                .interact(
                    &engine,
                    SelectionInput::PointerDown {
                        position: first,
                        mode: SelectionMode::Plain,
                        select_text: true,
                        clicks: 1
                    }
                )
                .handled
        );
        let last = scope
            .geometry(&engine)
            .unwrap()
            .selection_rects(0..count)
            .last()
            .unwrap()
            .center();
        assert!(
            scope
                .interact(
                    &engine,
                    SelectionInput::PointerMove {
                        position: last,
                        mode: SelectionMode::Plain,
                        drag_distance: 4.0
                    }
                )
                .handled
        );
        assert!(
            scope
                .interact(
                    &engine,
                    SelectionInput::PointerUp {
                        position: last,
                        mode: SelectionMode::Plain,
                        drag_distance: 4.0
                    }
                )
                .finished
        );
        assert!(
            scope
                .selection(&engine)
                .unwrap()
                .text()
                .plain
                .unwrap()
                .contains("Scoped content")
        );
        let second = engine.scope("note", width, &mut shaper).unwrap().unwrap();
        assert!(second.render_view(&engine).is_some());
        assert!(
            scope.render_view(&engine).is_some(),
            "appending a second scope preserves the first"
        );
        assert!(
            engine
                .scope("missing", width, &mut shaper)
                .unwrap()
                .is_none()
        );
        assert!(
            scope.render_view(&load(source)).is_none(),
            "an unrelated owner cannot supply DOM or resources"
        );
        engine
            .apply(
                DocumentUpdate::default().source("<p id='part'>replacement</p>".into()),
                &mut shaper,
            )
            .unwrap();
        assert!(scope.render_view(&engine).is_none());
        assert!(scope.scene(&engine).is_none());
        assert!(scope.export(&engine).is_none());
        assert!(scope.selection(&engine).is_none());
        assert!(
            !scope
                .interact(
                    &engine,
                    SelectionInput::PointerMove {
                        position: first,
                        mode: SelectionMode::Plain,
                        drag_distance: 4.0
                    }
                )
                .handled
        );
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
                fragment_paint::render_fragment_view_images_for_line(view.fragments(), line.id())
                    .iter()
                    .next()
                    .map(|image| {
                        (fragment_paint::render_image_fragment_visible_rect(&image)
                            + line.point().to_vec2())
                        .center()
                    })
            })
            .unwrap();
        let hit = geometry.content_at(image_point).unwrap();
        assert_eq!(hit.link.unwrap().href, "#image");
        let image = hit.image.unwrap();
        assert_eq!(image.image, dom_values::image_id_new(0));
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
                fragment_paint::render_fragment_view_images_for_line(view.fragments(), line.id())
                    .iter()
                    .next()
                    .map(|image| {
                        (
                            line.id(),
                            fragment_paint::render_image_fragment_visible_rect(&image)
                                + line.point().to_vec2(),
                        )
                    })
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
    fn document_search_owns_query_options_and_refreshes_visibility_and_cursor() {
        let mut engine = load("<p>Café cafe cafeteria</p><p style='display:none'>cafe</p>");
        let mut search = DocumentSearch::default();
        let options = TextSearchOptions {
            whole_word: true,
            ..Default::default()
        };
        search.configure("cafe".into(), options);
        search.refresh(engine.render_view().unwrap());
        assert_eq!(search.query(), "cafe");
        assert_eq!(search.options(), options);
        assert_eq!(search.ranges().len(), 2);
        search.focus(1);
        assert_eq!(search.current(), Some((1, search.ranges()[1].clone())));
        engine
            .apply(
                DocumentUpdate::default().source("<p>cafe</p>".into()),
                &mut Shaper::default(),
            )
            .unwrap();
        search.refresh(engine.render_view().unwrap());
        assert_eq!(search.ranges().len(), 1);
        assert_eq!(search.current(), Some((0, search.ranges()[0].clone())));
        search.configure(
            "CAFÉ".into(),
            TextSearchOptions {
                match_case: true,
                match_diacritics: true,
                ..options
            },
        );
        search.refresh(engine.render_view().unwrap());
        assert!(search.ranges().is_empty());
        assert_eq!(search.current(), None);
    }

    #[test]
    fn unified_export_returns_text_tables_and_cells_without_clipboard_policy() {
        let engine = load(
            "<p>Some <b>bold</b> text.</p><table id='t'><tr><td>A</td><td>B</td></tr><tr><td>C</td><td>D</td></tr></table>",
        );
        let range = find(&engine, "Some bold text.");
        let selection = engine.selection(range).unwrap();
        let plain = selection.plain_text().unwrap();
        let markdown = selection.markdown().unwrap();
        assert_eq!(plain, "Some bold text.");
        assert!(markdown.contains("**bold**"));
        let view = engine.render_view().unwrap();
        let table = (0..box_queries::len(view.boxes()))
            .map(layout_state::box_id_new)
            .find_map(|index| box_queries::render_box_view_table(view.boxes(), index))
            .unwrap();
        for format in [
            TableExportFormat::Markdown,
            TableExportFormat::Html,
            TableExportFormat::AuthoredHtml,
        ] {
            let text = table.export(TableExportRequest::Whole(format)).unwrap();
            assert!(text.contains('A') && text.contains('D'));
        }
        let geometry = engine.geometry().unwrap();
        let first = geometry
            .table_at_where(|box_idx| box_idx == table.rows()[1].cells()[0].box_id())
            .unwrap();
        let last = geometry
            .table_at_where(|box_idx| box_idx == table.rows()[1].cells()[1].box_id())
            .unwrap();
        let mut selected = TableSelectionState::default();
        selected.begin(first);
        selected.begin_drag(first);
        selected.update_drag(Some(last));
        let cells = selected.range().unwrap();
        let text = cells.markdown(&table).unwrap();
        assert!(text.contains('C') && text.contains('D') && !text.contains('A'));
        assert!(
            box_queries::render_box_view_table(
                view.boxes(),
                layout_state::box_id_new(u32::MAX as usize),
            )
            .and_then(|table| table.export(TableExportRequest::Whole(TableExportFormat::Html)))
            .is_none()
        );
        assert!(
            engine
                .selection(u32::MAX - 1..u32::MAX)
                .unwrap()
                .plain_text()
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
                .request(ExportRequest::Image(dom_values::image_id_new(0)))
                .unwrap()
                .is_none()
        );
        engine.set_image_service(ImageService::with_loader(
            Arc::new(SvgLoader),
            Arc::new(InlineJobSpawner),
            1024 * 1024,
        ));
        engine.request_image_lines(
            (0..engine.render_view().unwrap().text().line_count()).map(LineId::new),
            [],
        );
        engine.poll_resources();
        engine
            .apply(DocumentUpdate::default(), &mut Shaper::default())
            .unwrap();
        assert!(
            matches!(engine.export().unwrap().request(ExportRequest::Image(dom_values::image_id_new(0))).unwrap(), Some(ExportPayload::Image(ImagePayload::Svg { bytes })) if bytes.starts_with(b"<svg"))
        );
        let view = engine.render_view().unwrap();
        let point = view
            .text()
            .lines()
            .iter()
            .find_map(|line| {
                fragment_paint::render_fragment_view_images_for_line(view.fragments(), line.id())
                    .iter()
                    .find(|image| image.image() == dom_values::image_id_new(1))
                    .map(|image| {
                        (fragment_paint::render_image_fragment_visible_rect(&image)
                            + line.point().to_vec2())
                        .center()
                    })
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
                .request(ExportRequest::EquationLatex(dom_values::image_id_new(1)))
                .is_err()
        );
        #[cfg(feature = "math-export")]
        {
            let formula = engine
                .export()
                .unwrap()
                .request(ExportRequest::EquationLatex(dom_values::image_id_new(1)))
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap();
            assert!(formula.contains('x') && formula.contains('1'));
            assert!(
                engine
                    .export()
                    .unwrap()
                    .request(ExportRequest::EquationLatex(dom_values::image_id_new(0)))
                    .unwrap()
                    .is_none()
            );
        }
    }
}

mod document_refinement {
    use super::*;
    use crate::configuration::DocumentUpdate;
    use crate::geometry::RangeRect;

    #[test]
    fn configuration_applies_batched_dimensions_and_typography() {
        let mut engine = load("<p>configuration</p>");
        let changes = DocumentUpdate::default().width(320.0).height(Some(480.0)).font_size(21.6);
        assert_eq!(engine.configuration().width(), 600.0);
        engine.apply(changes, &mut Shaper::default()).unwrap();
        assert_eq!(engine.configuration().width(), 320.0);
        assert_eq!(engine.configuration().height(), Some(480.0));
        assert_eq!(engine.configuration().font_size().get(), 22.0);
        engine.apply(DocumentUpdate::default().width(400.0), &mut Shaper::default()).unwrap();
        assert_eq!(engine.configuration().height(), Some(480.0));
        engine.apply(DocumentUpdate::default().height(None), &mut Shaper::default()).unwrap();
        assert_eq!(engine.configuration().height(), None);
        engine.apply(DocumentUpdate::default().height(Some(0.0)), &mut Shaper::default()).unwrap();
        assert_eq!(engine.configuration().height(), Some(0.0));
        for value in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(engine.apply(DocumentUpdate::default().width(value), &mut Shaper::default()).is_err());
            assert!(engine.apply(DocumentUpdate::default().height(Some(value)), &mut Shaper::default()).is_err());
        }
        for value in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(engine.apply(DocumentUpdate::default().font_size(value), &mut Shaper::default()).is_err());
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
        let placements: Vec<_> = text_paint::render_text_view_spatial_order(text)
            .iter()
            .copied()
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
                line: LineId::new(u32::MAX as usize - 1),
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
                .range_geometry_lines(
                    [("all", 0..u32::MAX)],
                    text.lines().iter().map(|line| LinePlacement {
                        line: line.id(),
                        origin: line.point()
                    })
                )
                .into_iter()
                .map(|rect| rect.bounds)
                .collect::<Vec<_>>(),
            geometry.selection_rects(0..u32::MAX)
        );
        let clipped = geometry.range_geometry_lines(
            [("first", first)],
            text.lines().iter().map(|line| LinePlacement {
                line: line.id(),
                origin: line.point(),
            }),
        );
        assert!(
            clipped
                .iter()
                .all(|rect| rect.bounds.width() <= 60.0 && rect.bounds.height() <= 18.0)
        );
    }

    #[test]
    fn resource_requests_follow_the_callers_lines_and_release_empty_sets() {
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
        let images: Vec<_> = fragment_paint::render_fragment_view_images(fragments)
            .iter()
            .collect();
        let first = &images[0];
        let first_line = fragment_paint::render_image_fragment_line(first);
        engine.request_image_lines(
            [first_line, first_line, LineId::new(u32::MAX as usize - 1)],
            [],
        );
        engine.poll_resources();
        assert!(decoded_images_get(engine.images(), dom_values::image_id_new(0)).is_some());
        assert!(decoded_images_get(engine.images(), dom_values::image_id_new(1)).is_none());
        engine.request_image_lines([], []);
        assert!(decoded_images_get(engine.images(), dom_values::image_id_new(1)).is_none());
        assert!(!engine.resources_pending());
    }
}

mod selection_state_fixtures {
    use super::Shaper;
    use html_engine_model::layout_state;
    use crate::export::TableExportFormat;
    use html_pipeline::engine::box_queries;

    use html_pipeline::engine::TableExportRequest;
    use crate::geometry::{Geometry, WordHit};
    use kurbo::Point;

    use crate::selection::{SelectionMode, SelectionState, TableSelectionState};
    use html_pipeline::engine::LayoutConstraints;
    use html_pipeline::engine::GlyphResourceStore;
    use html_pipeline::DocumentFactory;

    fn word_hit(caret: u32, word: std::ops::Range<u32>) -> WordHit {
        WordHit {
            caret,
            word,
            position: Point::new(caret as f64 * 10.0, 0.0),
        }
    }

    #[test]
    fn pointer_drag_can_cross_words_without_changing_caret_and_ignores_jitter() {
        let mut selection = SelectionState::default();
        selection.begin_at_word(word_hit(1, 0..1), SelectionMode::Plain);
        let jitter = WordHit {
            position: Point::new(12.0, 0.0),
            ..word_hit(1, 0..1)
        };
        assert!(!selection.update_word_interaction(Some(jitter), SelectionMode::Plain, 4.0));
        assert_eq!(selection.finish_interaction(), Some(1));

        selection.begin_at_word(word_hit(1, 0..1), SelectionMode::Plain);
        let next_word = WordHit {
            position: Point::new(30.0, 0.0),
            ..word_hit(1, 1..2)
        };
        assert!(selection.update_word_interaction(Some(next_word), SelectionMode::Plain, 4.0));
        assert_eq!(selection.range(), Some(0..2));
        assert_eq!(selection.finish_interaction(), None);

        selection.begin_at_word(word_hit(1, 0..1), SelectionMode::Plain);
        let same_word = WordHit {
            position: Point::new(30.0, 0.0),
            ..word_hit(1, 0..1)
        };
        assert!(selection.update_word_interaction(Some(same_word), SelectionMode::Plain, 4.0));
        assert_eq!(selection.range(), Some(0..1));
    }

    #[test]
    fn word_drag_preserves_clicks_and_snaps_in_both_directions() {
        let mut selection = SelectionState::default();
        selection.begin_at_word(word_hit(12, 10..15), SelectionMode::Plain);
        assert!(!selection.update_word_interaction(
            Some(word_hit(12, 10..15)),
            SelectionMode::Plain,
            4.0
        ));
        assert_eq!(selection.range(), None);
        assert_eq!(selection.finish_interaction(), Some(12));

        selection.begin_at_word(word_hit(12, 10..15), SelectionMode::Plain);
        assert!(selection.update_word_interaction(
            Some(word_hit(13, 10..15)),
            SelectionMode::Plain,
            4.0
        ));
        assert_eq!(selection.range(), Some(10..15));
        assert!(!selection.update_word_interaction(
            Some(word_hit(14, 10..15)),
            SelectionMode::Plain,
            4.0
        ));
        assert_eq!(selection.range(), Some(10..15));
        assert!(selection.update_word_interaction(
            Some(word_hit(22, 20..25)),
            SelectionMode::Plain,
            4.0
        ));
        assert_eq!(selection.range(), Some(10..25));
        assert!(selection.update_word_interaction(
            Some(word_hit(3, 0..5)),
            SelectionMode::Plain,
            4.0
        ));
        assert_eq!(selection.range(), Some(0..15));
        assert!(selection.update_word_interaction(
            Some(word_hit(12, 10..15)),
            SelectionMode::Plain,
            4.0
        ));
        assert_eq!(selection.range(), Some(10..15));
        assert!(selection.update_word_interaction(None, SelectionMode::Semantic, 4.0));
        assert_eq!(selection.range(), Some(10..15));
        assert!(!selection.update_word_interaction(None, SelectionMode::Semantic, 4.0));
        assert!(!selection.update_word_interaction(
            Some(word_hit(14, 10..15)),
            SelectionMode::Plain,
            4.0
        ));
        assert_eq!(selection.finish_interaction(), None);
    }

    #[test]
    fn selection_interaction_distinguishes_a_click_from_a_drag() {
        let mut selection = SelectionState::default();
        selection.begin_at_word(word_hit(12, 10..15), SelectionMode::Plain);

        assert_eq!(selection.finish_interaction(), Some(12));

        selection.begin_at_word(word_hit(12, 10..15), SelectionMode::Plain);
        assert!(selection.update_word_interaction(
            Some(word_hit(18, 17..22)),
            SelectionMode::Semantic,
            4.0
        ));
        assert_eq!(selection.finish_interaction(), None);
        assert_eq!(selection.range(), Some(10..22));
    }

    #[test]
    fn selection_update_outside_text_preserves_the_range_and_promotes_its_mode() {
        let mut selection = SelectionState::default();
        selection.begin_at_word(word_hit(7, 5..10), SelectionMode::Plain);

        assert!(!selection.update_word_interaction(None, SelectionMode::Semantic, 4.0));
        assert_eq!(selection.finish_interaction(), Some(7));
    }

    #[test]
    fn markdown_expands_spans_escapes_pipes_and_preserves_data_rows() {
        let mut factory = DocumentFactory::new();
        let prepared =
            factory.parse_with_new_pipeline("<table class='ledger' style='color:red' onclick='unsafe()'><tr><th colspan='2'>A | B</th></tr><tr><td rowspan='2'>One</td><td>Two &amp; more</td></tr><tr><td>Three</td></tr></table>", None);
        let mut shaper = Shaper::default();
        let mut shaped_resources = GlyphResourceStore::default();
        let shaped = prepared.shape(&mut shaper, &mut shaped_resources).unwrap();
        let document = shaped
            .layout(
                LayoutConstraints::new(500.0, 20.0).unwrap(),
                &shaped_resources,
            )
            .expect("test layout succeeds");
        let boxes = document.render_view(&shaped_resources).boxes();
        let table = (0..box_queries::len(boxes))
            .map(layout_state::box_id_new)
            .find(|&index| box_queries::is_table(boxes, index))
            .and_then(|index| box_queries::render_box_view_table(boxes, index))
            .unwrap();
        assert_eq!(
            table
                .export(TableExportRequest::Whole(TableExportFormat::Markdown))
                .as_deref(),
            Some("| A \\| B |  |\n| --- | --- |\n| One | Two & more |\n|  | Three |")
        );
        assert_eq!(
            table
                .export(TableExportRequest::Whole(TableExportFormat::Html))
                .as_deref(),
            Some(
                "<table><tr><th colspan=\"2\">A | B</th></tr><tr><td rowspan=\"2\">One</td><td>Two &amp; more</td></tr><tr><td>Three</td></tr></table>"
            )
        );
        assert!(table.authored_html().contains("class=\"ledger\""));
        assert!(table.authored_html().contains("style=\"color:red\""));
        assert!(!table.authored_html().contains("onclick"));

        let geometry = Geometry::new(document.render_view(&shaped_resources));
        let first = geometry
            .table_at_where(|box_idx| box_idx == table.rows()[1].cells()[1].box_id())
            .unwrap();
        let last = geometry
            .table_at_where(|box_idx| box_idx == table.rows()[2].cells()[0].box_id())
            .unwrap();
        let mut selected = TableSelectionState::default();
        selected.begin(first);
        selected.begin_drag(first);
        selected.update_drag(Some(last));
        let range = selected.range().unwrap();
        assert_eq!(
            range.markdown(&table).as_deref(),
            Some("|  |\n| --- |\n| Two & more |\n| Three |")
        );
    }
}
