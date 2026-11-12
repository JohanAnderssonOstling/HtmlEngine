//! Pipeline and layout wall-time benchmarks for external perf sampling.
//! The pipeline case includes preparation, shaping and first layout.
use html_pipeline::engine::LayoutRequest;
use html::layout::{ImageSizingPolicy, TextCompositionPolicy};
use html::resources::FileSystemProvider;
use html::pagination::{PageGeometry, PageRequest, PageView, PaginationMode, PaginationScratch, Paginator};
use html::render::PaintProjection;
use html_engine_model::layout_state::line_id_index;
use html_pipeline::engine::{
    FontMetrics, GlyphHandle, GlyphResourceGeneration, GlyphResourceStore, GlyphShaper,
    LayoutConstraints, ShapedGlyph, ShapedRun, TextRunShapeRequest};
use html_pipeline::{DocumentFactory, ResourceContext, PipelineSession, PipelineInputs, PipelineChanges,
    MediaEnvironment, StyleEnvironment, MarkupSyntax};
use std::sync::Arc;
use serde_json::json;
use std::collections::HashMap;
use std::hint::black_box;
use std::io::{Read, Write};

#[path = "../geometry.rs"]
mod geometry;
use std::time::Instant;

#[derive(Default)]
struct Shaper {
    generation: GlyphResourceGeneration,
    cache: HashMap<(char, u32), GlyphHandle>,
}

impl GlyphShaper for Shaper {
    fn resource_generation(&self) -> GlyphResourceGeneration {
        self.generation
    }

    fn shape_run(&mut self, request: TextRunShapeRequest<'_>) -> ShapedRun {
        ShapedRun::characters(request, |character, style| {
            let next = GlyphHandle(self.cache.len() as u32);
            let glyph = *self
                .cache
                .entry((character, style.font_size.to_bits()))
                .or_insert(next);
            ShapedGlyph::new(
                glyph,
                style.font_size * 0.5,
                FontMetrics {
                    ascent: style.font_size * 0.75,
                    descent: style.font_size * 0.25,
                },
            )
        })
    }
}

fn perf_control(command: &str) -> std::io::Result<()> {
    if let Some(path) = std::env::var_os("LAYOUT_PERF_CONTROL") {
        let mut fifo = std::fs::OpenOptions::new().write(true).open(path)?;
        fifo.write_all(command.as_bytes())?;
        if let Some(path) = std::env::var_os("LAYOUT_PERF_ACK") {
            let mut ack = std::fs::File::open(path)?;
            let mut response = [0; 5];
            ack.read_exact(&mut response)?;
            if &response != b"ack\n\0" {
                return Err(std::io::Error::other("unexpected perf acknowledgement"));
            }
        }
    }
    Ok(())
}

fn run_pipeline(
    source: &str,
    css: &str,
    path: &str,
    constraints: LayoutConstraints,
    iterations: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let provider = FileSystemProvider::new();
    let mut totals = Vec::with_capacity(iterations);
    let mut preparation = Vec::with_capacity(iterations);
    let mut shaping = Vec::with_capacity(iterations);
    let mut layout = Vec::with_capacity(iterations);
    let mut counts = json!(null);
    let mut memory = json!(null);
    for iteration in 0..iterations {
        perf_control("enable\n")?;
        let started = Instant::now();
        // Use the normal owned parse path, including CSS preparation, style
        // resolution and layout-tree construction, without cloning a DOM.
        let prepared = DocumentFactory::new()
            .with_resources(ResourceContext::new(&provider, path))
            .parse_with_new_pipeline(source, Some(css));
        let prepared_at = Instant::now();
        let mut resources = GlyphResourceStore::default();
        let mut shaper = Shaper::default();
        let shaped = prepared.shape_borrowed(&mut shaper, &mut resources)?;
        let shaped_at = Instant::now();
        let document =
            shaped.layout(LayoutRequest::new(constraints), &mut shaper, &mut resources)?;
        let finished = Instant::now();
        perf_control("disable\n")?;
        totals.push((finished - started).as_secs_f64() * 1_000.0);
        preparation.push((prepared_at - started).as_secs_f64() * 1_000.0);
        shaping.push((shaped_at - prepared_at).as_secs_f64() * 1_000.0);
        layout.push((finished - shaped_at).as_secs_f64() * 1_000.0);
        let view = document.render_view(&resources);
        counts = json!({
            "boxes": document.box_count(),
            "lines": view.text().line_count(), "glyphs": view.text().glyph_count(),
        });
        if iteration + 1 == iterations {
            let report = document.memory_usage_report_with_resources(&resources);
            memory = json!({
                "reported_document_bytes": report.total_bytes(),
                "entries": report.entries.iter().map(|entry| json!({
                    "label": entry.label, "bytes": entry.bytes, "count": entry.count,
                })).collect::<Vec<_>>(),
            });
        }
        if iteration + 1 == iterations
            && let Some(path) = std::env::var_os("LAYOUT_SNAPSHOT")
        {
            let links: Vec<_> = (0..view.text().glyph_count())
                .map(|glyph| {
                    view.addressing()
                        .link_for_glyph(glyph as u32)
                        .map(|link| json!([link.href, link.is_note_reference]))
                })
                .collect();
            let dom = document.document();
            let anchors: Vec<_> = dom.node_ids().filter_map(|node| {
                let element = dom.try_element_ref(node)?;
                Some(json!([element.id()?, document.anchor_glyph(element.id_idx()?)]))
            }).collect();
            let snapshot = json!({
                "geometry": geometry::snapshot(&document, &resources),
                "links": links,
                "anchors": anchors,
            });
            std::fs::write(path, serde_json::to_vec(&snapshot)?)?;
        }
        black_box(&document);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "case": "pipeline", "input": path, "iterations": iterations,
            "backend": "mock fixed-width glyph metrics", "workers": 1,
            "width": constraints.viewport_width(), "height": constraints.viewport_height(), "counts": counts, "memory": memory,
            "samples_ms": totals,
            "stages_ms": { "parse_prepare": preparation, "shape": shaping, "layout": layout },
            "scope": "HTML/CSS already in memory; fresh parse, CSS/style/tree preparation, shaping and first layout. Excludes output inspection, teardown and GUI rendering.",
        }))?
    );
    Ok(())
}

fn run_pagination_benchmark(
    paginator: Paginator<'_>,
    line_count: usize,
    iterations: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut results = Vec::new();
    for backward in [false, true] {
        for cached in [false, true] {
            let mut scratch = PaginationScratch::default();
            let mut samples = Vec::new();
            let mut totals = Vec::new();
            let mut page_counts = Vec::new();
            let mut slowest = (0.0_f64, 0.0_f64);
            for iteration in 0..iterations + usize::from(cached) {
                if !cached { scratch = PaginationScratch::default(); }
                let mut request = if backward {
                    PageRequest::EndingAt(line_count.saturating_sub(1))
                } else { PageRequest::StartingAtOffset(0.0) };
                let mut buffer = Vec::new();
                let mut pages = 0;
                let mut elapsed = 0.0;
                loop {
                    let started = Instant::now();
                    let page = paginator.compose(request, buffer, &mut scratch);
                    let ms = started.elapsed().as_secs_f64() * 1000.0;
                    let recording = !cached || iteration > 0;
                    if recording {
                        samples.push(ms);
                        elapsed += ms;
                        if ms > slowest.0 { slowest = (ms, page.paint_start_offset_y); }
                    }
                    pages += 1;
                    let next = if backward {
                        paginator.previous_page_anchor(Some(&page), page.paint_start_offset_y)
                    } else {
                        paginator.next_page_anchor(Some(&page), page.paint_start_offset_y)
                    };
                    black_box(&page);
                    let Some(next) = next else { break };
                    if pages > line_count.max(1) * 4 { return Err("pagination did not terminate".into()); }
                    request = if backward {
                        next.cursor.map_or(PageRequest::EndingAtOffset(next.offset_y), PageRequest::EndingAt)
                    } else {
                        next.cursor.map_or(PageRequest::StartingAtOffset(next.offset_y), PageRequest::StartingAt)
                    };
                    buffer = page.positions.into_buffer();
                }
                if !cached || iteration > 0 { totals.push(elapsed); page_counts.push(pages); }
            }
            samples.sort_by(f64::total_cmp);
            totals.sort_by(f64::total_cmp);
            let percentile = |values: &[f64], fraction: f64| values[((values.len() - 1) as f64 * fraction) as usize];
            results.push(json!({
                "direction": if backward { "backward" } else { "forward" },
                "cache": if cached { "warm" } else { "fresh" },
                "pages": page_counts, "traversal_median_ms": percentile(&totals, 0.5),
                "page_median_ms": percentile(&samples, 0.5), "page_p95_ms": percentile(&samples, 0.95),
                "page_max_ms": slowest.0, "slowest_source_y": slowest.1,
            }));
        }
    }
    println!("{}", serde_json::to_string_pretty(&json!({
        "backend": "mock fixed-width glyph metrics; composition only, excluding JSON and paint submission",
        "iterations": iterations, "results": results,
    }))?);
    Ok(())
}

fn run_resize_pagination(
    source: &str,
    css: &str,
    path: &str,
    width: f64,
    height: f64,
    iterations: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if iterations == 0 { return Err("iterations must be positive".into()); }
    let constraints = |w, h| -> Result<LayoutConstraints, Box<dyn std::error::Error>> {
        Ok(LayoutConstraints::new(w, 16.0)?.with_viewport_height(Some(h))?
            .with_text_composition_policy(TextCompositionPolicy::BookOptimized)
            .with_image_sizing_policy(ImageSizingPolicy::SmartStandalone))
    };
    let environment = |w, h| StyleEnvironment {
        media: MediaEnvironment::screen(w, Some(h)).expect("valid benchmark viewport"),
        ..Default::default()
    };
    let mut results = Vec::new();
    for (scenario, target_width, target_height) in [
        ("height_only", width, height + 200.0),
        ("width_only", width * 0.8, height),
        ("width_and_height", width * 0.8, height + 200.0),
    ] {
        for middle in [false, true] {
            let mut session = PipelineSession::new(Arc::new(FileSystemProvider::new()));
            let mut shaper = Shaper::default();
            let mut inputs = PipelineInputs::new(source.to_owned(), MarkupSyntax::Html,
                path.to_owned(), environment(width, height), constraints(width, height)?);
            inputs.user_styles.push(css.to_owned());
            session.update(inputs, &mut shaper)?;
            let view = session.render_view().expect("initial layout");
            let glyph = if middle { view.text().glyph_count() as u32 / 2 } else { 0 };
            let height_dependent = session.document().expect("initial layout").layout_depends_on_viewport_height();
            let mut revision = view.boxes().pagination_ownership().revision;
            let mut scratch = PaginationScratch::default();
            let mut buffer = Vec::new();
            let mut samples = Vec::new();
            let mut stages: HashMap<String, usize> = HashMap::new();
            let mut reused = 0;
            perf_control("enable\n")?;
            for iteration in 0..iterations + 4 {
                let (w, h) = if iteration % 2 == 0 { (target_width, target_height) } else { (width, height) };
                let changes = PipelineChanges {
                    layout: Some(constraints(w, h)?),
                    style_environment: Some(environment(w, h)),
                    ..Default::default()
                };
                let started = Instant::now();
                let stage = session.apply_changes(changes, &mut shaper)?;
                let updated = Instant::now();
                let view = session.render_view().expect("resized layout");
                // Preserve the same logical text position after width reflow.
                let offset = if middle {
                    view.text().lines().iter().find(|line| line.glyphs().contains(&glyph))
                        .map_or(0.0, |line| line.point().y)
                } else { 0.0 };
                let geometry = PageGeometry { col_width: w, col_gap: 0.0, col_count: 1.0,
                    size: (w, h).into() };
                let paginator = Paginator::new(view, geometry, PaginationMode::Book);
                let anchored = Instant::now();
                let page = paginator.compose(PageRequest::StartingAtOffset(offset), buffer, &mut scratch);
                let finished = Instant::now();
                black_box(&page);
                let next_revision = view.boxes().pagination_ownership().revision;
                if iteration >= 4 {
                    if revision == next_revision { reused += 1; }
                    *stages.entry(format!("{stage:?}")).or_default() += 1;
                    samples.push(json!({
                        "width": w, "height": h,
                        "update_ms": (updated - started).as_secs_f64() * 1000.0,
                        "anchor_ms": (anchored - updated).as_secs_f64() * 1000.0,
                        "pagination_ms": (finished - anchored).as_secs_f64() * 1000.0,
                        "total_ms": (finished - started).as_secs_f64() * 1000.0,
                    }));
                }
                revision = next_revision;
                buffer = page.positions.into_buffer();
            }
            perf_control("disable\n")?;
            let mut summary = serde_json::Map::new();
            for field in ["update_ms", "anchor_ms", "pagination_ms", "total_ms"] {
                let mut values: Vec<_> = samples.iter().map(|row| row[field].as_f64().unwrap()).collect();
                values.sort_by(f64::total_cmp);
                summary.insert(field.to_owned(), json!({
                    "median": values[(values.len() - 1) / 2],
                    "p95": values[((values.len() - 1) as f64 * 0.95) as usize],
                    "max": values[values.len() - 1],
                }));
            }
            results.push(json!({"scenario": scenario, "anchor": if middle { "middle_glyph" } else { "start" },
                "from": [width, height], "to": [target_width, target_height],
                "height_dependent": height_dependent, "geometry_reused": reused, "stages": stages,
                "summary": summary, "samples": samples}));
        }
    }
    println!("{}", serde_json::to_string_pretty(&json!({"input": path, "iterations": iterations,
        "backend": "mock fixed-width glyph metrics", "columns": 1,
        "scope": "PipelineSession resize update, text-anchor lookup, and one visible page composition; excludes initial load and desktop painting",
        "results": results}))?);
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 4 {
        return Err(
            "usage: layout-profile HTML CSS pipeline|cold|repeat|resize|pagination|pagination-bench|resize-pagination [iterations=100] [width=600] [height=800]".into(),
        );
    }
    let iterations: usize = args.get(4).map(|v| v.parse()).transpose()?.unwrap_or(100);
    let width: f64 = args.get(5).map(|v| v.parse()).transpose()?.unwrap_or(600.0);
    let height: f64 = args.get(6).map(|v| v.parse()).transpose()?.unwrap_or(800.0);
    let source = std::fs::read_to_string(&args[1])?;
    let css = std::fs::read_to_string(&args[2])?;
    let provider = FileSystemProvider::new();
    let constraints = LayoutConstraints::new(width, 16.0)?
        .with_viewport_height(Some(height))?
        .with_text_composition_policy(TextCompositionPolicy::BookOptimized)
        .with_image_sizing_policy(ImageSizingPolicy::SmartStandalone);
    let resized = LayoutConstraints::new(640.0, 16.0)?
        .with_viewport_height(Some(720.0))?
        .with_text_composition_policy(TextCompositionPolicy::BookOptimized)
        .with_image_sizing_policy(ImageSizingPolicy::SmartStandalone);
    let case = args[3].as_str();
    if case == "resize-pagination" {
        return run_resize_pagination(&source, &css, &args[1], width, height, iterations);
    }
    if case == "pipeline" {
        return run_pipeline(&source, &css, &args[1], constraints, iterations);
    }
    if !matches!(case, "cold" | "repeat" | "resize" | "pagination" | "pagination-bench") {
        return Err("case must be pipeline, cold, repeat, resize, pagination or pagination-bench".into());
    }
    let prepared = DocumentFactory::new()
        .with_resources(ResourceContext::new(&provider, &args[1]))
        .parse_with_new_pipeline(&source, Some(&css));
    let mut resources = GlyphResourceStore::default();
    let mut shaper = Shaper::default();
    let mut laid_out = prepared
        .shape_borrowed(&mut shaper, &mut resources)?
        .layout(LayoutRequest::new(constraints), &mut shaper, &mut resources)?;
    if case == "pagination-bench" {
        if iterations == 0 { return Err("iterations must be positive".into()); }
        let view = laid_out.render_view(&resources);
        let geometry = PageGeometry { col_width: width, col_gap: 0.0, col_count: 1.0,
            size: (width, height).into() };
        let paginator = Paginator::new(view, geometry, PaginationMode::Book);
        perf_control("enable\n")?;
        let result = run_pagination_benchmark(paginator, view.pagination().len(), iterations);
        perf_control("disable\n")?;
        return result;
    }
    if case == "pagination" {
        let view = laid_out.render_view(&resources);
        let geometry = PageGeometry { col_width: width, col_gap: 0.0, col_count: 1.0,
            size: (width, height).into() };
        let paginator = Paginator::new(view, geometry, PaginationMode::Book);
        let mut scratch = PaginationScratch::default();
        let mut request = PageRequest::StartingAtOffset(0.0);
        let mut pages = Vec::new();
        let started = Instant::now();
        loop {
            let page = paginator.compose(request, Vec::new(), &mut scratch);
            let positioned: Vec<_> = page.positions.iter().map(|(id, screen)| {
                let line = view.text().line(id).expect("positioned source line");
                let text: String = line.glyphs().filter_map(|index| view.text().glyph_at(index as usize)
                    .and_then(|glyph| view.text().glyph_metric(glyph)).map(|metric| metric.ch())).collect();
                let group = view.pagination().locate(html_pipeline::engine::PaginationTarget::Line(id)).unwrap().group;
                json!({"id": line_id_index(id), "source": [line.point().x, line.point().y, line.height()],
                    "group": [group.point().y, group.height()],
                    "screen": [screen.point.x, screen.point.y], "clip": screen.visible_y, "text": text})
            }).collect();
            let projection = PageView::new(view, &page);
            let mut projected = Vec::new();
            let decorations: Vec<_> = view.fragments().decorations().iter().flat_map(|decoration| {
                projected.clear();
                projection.project_decoration(decoration.owner_box(), decoration.rect(), decoration.line(), &mut projected);
                projected.iter().map(|fragment| json!({
                    "source": [fragment.source.x0, fragment.source.y0, fragment.source.x1, fragment.source.y1],
                    "destination": [fragment.destination.x, fragment.destination.y],
                })).collect::<Vec<_>>()
            }).collect();
            pages.push(json!({"start": page.paint_start_offset_y, "next": page.next_start_offset_y,
                "end": page.reached_end, "lines": positioned, "cuts": page.offset_breaks, "decorations": decorations }));
            let Some(next) = paginator.next_page_anchor(Some(&page), page.paint_start_offset_y) else { break };
            if next.offset_y <= page.paint_start_offset_y || pages.len() > view.text().line_count().max(1) * 4 {
                return Err(format!("pagination failed to advance: page={} start={} cursor={} last={:?} next={:?} end={} lines={} cuts={:?} group={:?}",
                    pages.len(), page.paint_start_offset_y, page.start_line, page.last_line, next, page.reached_end, page.positions.iter().count(), page.offset_breaks,
                    view.pagination().locate(html_pipeline::engine::PaginationTarget::Cursor(page.start_line)).map(|p| (p.group.point(), p.group.height()))).into());
            }
            request = next.cursor.map_or(PageRequest::StartingAtOffset(next.offset_y), PageRequest::StartingAt);
        }
        let mut backward = Vec::new();
        let mut request = PageRequest::EndingAt(view.pagination().len().saturating_sub(1));
        loop {
            let page = paginator.compose(request, Vec::new(), &mut scratch);
            let visible: Vec<_> = page.positions.iter().filter_map(|(id, screen)| {
                let line = view.text().line(id)?;
                (screen.point.y < screen.visible_bottom(height) - 0.001
                    && screen.point.y + line.height() > screen.visible_top() + 0.001)
                    .then_some(line_id_index(id))
            }).collect();
            backward.push(json!({"start": page.paint_start_offset_y, "lines": visible}));
            let Some(previous) = paginator.previous_page_anchor(Some(&page), page.paint_start_offset_y) else { break };
            if backward.len() > view.text().line_count().max(1) * 4 {
                return Err("backward pagination failed to advance".into());
            }
            request = previous.cursor.map_or(PageRequest::EndingAtOffset(previous.offset_y), PageRequest::EndingAt);
        }
        println!("{}", serde_json::to_string(&json!({"input": args[1], "width": width, "height": height,
            "backend": "mock fixed-width glyph metrics", "pagination_ms": started.elapsed().as_secs_f64() * 1000.0,
            "geometry": geometry::snapshot(&laid_out, &resources), "pages": pages, "backward": backward}))?);
        return Ok(());
    }
    for _ in 0..3 {
        laid_out.relayout(LayoutRequest::new(constraints), &mut shaper, &mut resources)?;
    }
    let mut samples = Vec::with_capacity(iterations);
    if case != "cold" {
        perf_control("enable\n")?;
    }
    for iteration in 0..iterations {
        if case == "cold" {
            let mut fresh_resources = GlyphResourceStore::default();
            let mut fresh_shaper = Shaper::default();
            let shaped = prepared.shape_borrowed(&mut fresh_shaper, &mut fresh_resources)?;
            perf_control("enable\n")?;
            let started = Instant::now();
            let fresh = shaped.layout(
                LayoutRequest::new(constraints),
                &mut fresh_shaper,
                &mut fresh_resources,
            )?;
            samples.push(started.elapsed().as_secs_f64() * 1_000.0);
            perf_control("disable\n")?;
            black_box(&fresh);
        } else {
            let next = if case == "resize" && iteration % 2 == 0 {
                resized
            } else {
                constraints
            };
            let started = Instant::now();
            laid_out.relayout(LayoutRequest::new(next), &mut shaper, &mut resources)?;
            samples.push(started.elapsed().as_secs_f64() * 1_000.0);
            black_box(&laid_out);
        }
    }
    if case != "cold" {
        perf_control("disable\n")?;
    }
    let view = laid_out.render_view(&resources);
    let counts = json!({
        "boxes": laid_out.box_count(),
        "lines": view.text().line_count(), "glyphs": view.text().glyph_count(),
    });
    let report = laid_out.memory_usage_report_with_resources(&resources);
    let inline_plan_bytes: usize = report
        .entries
        .iter()
        .filter(|entry| entry.label.contains(".inline_plans"))
        .map(|entry| entry.bytes)
        .sum();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "input": args[1], "css": args[2], "width": width, "height": height,
            "backend": "mock fixed-width glyph metrics", "workers": 1,
            "iterations": iterations, "counts": counts, "case": case, "samples_ms": samples,
            "reported_document_bytes": report.total_bytes(), "inline_plan_bytes": inline_plan_bytes,
        }))?
    );
    Ok(())
}
