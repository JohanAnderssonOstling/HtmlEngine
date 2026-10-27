use crate::{Shaper, cpu_seconds, layout_fingerprint};
use html::layout::{LayoutConstraints, LayoutTimings};
use html::pipeline::ResourceContext;
use html::pipeline::{DocumentFactory, PipelineInputs, parse_document};
use html::resources::FileSystemProvider;
use html::style::StyleTimings;
use html::text::GlyphResourceStore;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::{Duration, Instant};

pub(super) fn summary(values: &[f64]) -> Value {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    json!({
        "median_ms": sorted[sorted.len() / 2],
        "p95_ms": sorted[(sorted.len() * 95).div_ceil(100) - 1],
        "min_ms": sorted[0],
        "max_ms": sorted[sorted.len() - 1],
        "samples_ms": values,
    })
}

pub(super) fn style_details(t: &StyleTimings) -> [(&'static str, Duration); 9] {
    [
        ("parse_default_css", t.parse_default_css),
        ("parse_author_css", t.parse_author_css),
        ("prepare_rules", t.prepare_rules),
        ("selector_index", t.selector_index),
        ("resolve_styles", t.resolve_styles),
        ("resolver_setup", t.resolver_setup),
        ("selector_matching", t.selector_matching),
        ("cascade", t.cascade),
        ("style_store", t.style_store),
    ]
}

fn layout_details(t: &LayoutTimings) -> [(&'static str, Duration); 25] {
    [
        ("clear_layout_output", t.clear_layout_output),
        ("layout_tree_traversal", t.layout_tree_traversal),
        ("root_box_layout", t.root_box_layout),
        ("finalize_layout", t.finalize_layout),
        ("sort_lines_and_remap_images", t.sort_lines_and_remap_images),
        ("collect_inline_decorations", t.collect_inline_decorations),
        (
            "rebuild_image_fragments_by_line",
            t.rebuild_image_fragments_by_line,
        ),
        ("layout_box_total", t.layout_box_total),
        ("layout_block_children", t.layout_block_children),
        ("layout_table", t.layout_table),
        ("layout_table_row", t.layout_table_row),
        ("layout_runs", t.layout_runs),
        (
            "layout_runs_around_float_exclusions",
            t.layout_runs_around_float_exclusions,
        ),
        ("build_inline_tokens", t.build_inline_tokens),
        (
            "build_inline_tokens_from_runs",
            t.build_inline_tokens_from_runs,
        ),
        ("break_lines", t.break_lines),
        ("break_lines_knuth", t.break_lines_knuth),
        ("emit_lines", t.emit_lines),
        ("measure_line", t.measure_line),
        ("write_line_fragments", t.write_line_fragments),
        ("place_float_anchor", t.place_float_anchor),
        ("layout_flex_grid", t.layout_flex_grid),
        ("measure_flex_grid_item", t.measure_flex_grid_item),
        // Deliberately recorded separately: nested timers are not additive.
        (
            "tree_outside_root",
            t.layout_tree_traversal.saturating_sub(t.root_box_layout),
        ),
        (
            "finalize_other",
            t.finalize_layout.saturating_sub(
                t.sort_lines_and_remap_images
                    + t.collect_inline_decorations
                    + t.rebuild_image_fragments_by_line,
            ),
        ),
    ]
}

/// Repeats a fresh document load. Read time is reported separately because
/// the actual session API accepts source text that the host already loaded.
pub fn run(path: &str, case: &str, count: usize, inputs: &PipelineInputs) {
    assert!(count > 0, "sample count must be positive");
    let mut samples = BTreeMap::<String, Vec<f64>>::new();
    let mut output = Value::Null;
    let constraints = LayoutConstraints::new(600.0, 16.0)
        .unwrap()
        .with_text_composition_policy(inputs.layout.text_composition_policy);
    for iteration in 0..count + 3 {
        let mut factory = DocumentFactory::new();
        let factory_provider = FileSystemProvider;
        let factory_resources = ResourceContext::new(&factory_provider, (path).as_ref());
        factory.set_reader_overrides(inputs.reader_overrides.layout_overrides());
        factory.set_media_environment(inputs.style_environment.media);
        let cpu = cpu_seconds();
        let start = Instant::now();
        let source = std::fs::read_to_string(path).unwrap();
        let read_at = Instant::now();
        let parsed = parse_document(&source, inputs.markup_syntax).unwrap();
        let parsed_at = Instant::now();
        let (base, preparation) = factory
            .with_resources(factory_resources)
            .build_pipeline_from_parsed_with_timings(&parsed, &[]);
        let prepared_at = Instant::now();
        let prepared = base
            .with_reader_paint_overrides(&inputs.reader_overrides)
            .unwrap();
        let painted_at = Instant::now();
        let mut shaper = Shaper::default();
        let mut shaped_resources = GlyphResourceStore::default();
        let shaped = prepared
            .shape_borrowed(&mut shaper, &mut shaped_resources)
            .unwrap();
        let shaped_at = Instant::now();
        let document = shaped
            .layout_with_metrics_and_shaper(
                constraints,
                &inputs.image_metrics,
                &mut shaper,
                &mut shaped_resources,
            )
            .unwrap();
        let finished = Instant::now();
        let total = finished - start;
        let total_cpu_ms = (cpu_seconds() - cpu) * 1000.0;
        black_box(&document);

        // Detailed per-line timers perturb layout. Collect them in a second
        // pass and check its output against the production layout entry point.
        let mut detail_shaper = Shaper::default();
        let mut detail_input_resources = GlyphResourceStore::default();
        let detail_input = prepared
            .shape_borrowed(&mut detail_shaper, &mut detail_input_resources)
            .unwrap();
        let detail_start = Instant::now();
        let (detailed, layout) = detail_input
            .layout_with_timings_and_metrics_and_shaper(
                constraints,
                &inputs.image_metrics,
                &mut detail_shaper,
                &mut detail_input_resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles");
        let detail_total = detail_start.elapsed();
        let fingerprint = layout_fingerprint(&document, &shaped_resources);
        assert_eq!(
            fingerprint,
            layout_fingerprint(&detailed, &detail_input_resources),
            "instrumented layout differs: {path}"
        );
        if iteration >= 3 {
            let mut record = |name: &str, value: Duration| {
                samples
                    .entry(name.to_owned())
                    .or_default()
                    .push(value.as_secs_f64() * 1000.0);
            };
            for (name, value) in [
                ("source_read", read_at - start),
                ("parse", parsed_at - read_at),
                ("preparation", prepared_at - parsed_at),
                ("paint_prepare", painted_at - prepared_at),
                ("shape", shaped_at - painted_at),
                ("layout", finished - shaped_at),
                ("total_wall", total),
                ("prepare.dom", preparation.dom),
                ("prepare.stylesheets", preparation.stylesheets),
                ("prepare.style", preparation.style),
                ("prepare.box_tree", preparation.prepare),
                ("detail_layout.total", detail_total),
            ] {
                record(name, value);
            }
            for (name, value) in style_details(&preparation.style_details) {
                record(&format!("style.{name}"), value);
            }
            for (name, value) in layout_details(&layout) {
                record(&format!("detail_layout.{name}"), value);
            }
            samples
                .entry("total_cpu".into())
                .or_default()
                .push(total_cpu_ms);
            output = json!({
                "case": case,
                "path": path,
                "samples": count,
                "warmups": 3,
                "source_bytes": source.len(),
                "font_backend": "deterministic mock",
                "heap_instrumented": cfg!(feature = "heap"),
                "composition": format!("{:?}", inputs.layout.text_composition_policy),
                "glyphs": document.render_view(&shaped_resources).text().glyph_count(),
                "lines": document.render_view(&shaped_resources).text().line_count(),
                "render_fingerprint": format!("{fingerprint:016x}"),
                "detail_layout_matches": true,
            });
        }
    }
    output["timings"] = samples
        .iter()
        .map(|(name, values)| (name.clone(), summary(values)))
        .collect::<serde_json::Map<_, _>>()
        .into();
    println!("{output}");
}
