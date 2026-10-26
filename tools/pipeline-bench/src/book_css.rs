//! Compare fresh chapter factories with and without a book-owned CSS cache.

use crate::counter::CacheCounters;
use crate::opening::{style_details, summary};
use crate::{Shaper, cpu_seconds, input, layout_fingerprint};
use html::layout::{LayoutConstraints, TextCompositionPolicy};
use html::pipeline::{BookStylesheetCache, DocumentFactory, MarkupSyntax, parse_document};
use html::resources::FileSystemProvider;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

pub(super) fn run(manifest: &str, count: usize, detailed: bool, cache_counters: bool) {
    assert!(count > 0);
    let paths: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string(manifest).unwrap()).unwrap();
    assert!(!paths.is_empty());
    let mut inputs = input(String::new());
    inputs.layout.text_composition_policy = TextCompositionPolicy::BookOptimized;
    inputs.reader_overrides.link_foreground = None;
    let constraints = LayoutConstraints::new(600.0, 16.0)
        .unwrap()
        .with_text_composition_policy(inputs.layout.text_composition_policy);
    let mut samples = BTreeMap::<String, Vec<f64>>::new();
    let mut identities = vec![Value::Null; paths.len()];
    let mut cache_records = Vec::new();
    let mut counter_records = Vec::new();
    let mut counters = cache_counters
        .then(|| CacheCounters::new().expect("AMD data-cache PMU counters unavailable"));
    for iteration in 0..count + 3 {
        // Reset the shared book cache for every traversal, including warmups.
        // Each measured traversal therefore pays its initial compilation cost.
        for shared in if iteration % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let variant = if shared { "shared" } else { "isolated" };
            let cache = BookStylesheetCache::default();
            let mut book_cpu = 0.0;
            let mut book_wall = 0.0;
            for (index, path) in paths.iter().enumerate() {
                let mut factory = DocumentFactory::new();
                factory.set_resource_context(Arc::new(FileSystemProvider), path);
                factory.set_reader_overrides(inputs.reader_overrides.layout_overrides());
                factory.set_media_environment(inputs.style_environment.media);
                if shared {
                    factory.set_book_stylesheet_cache(cache.clone());
                }
                if let Some(counters) = &mut counters {
                    counters.start();
                }
                let cpu = cpu_seconds();
                let started = Instant::now();
                let source = std::fs::read_to_string(path).unwrap();
                let parsed = parse_document(&source, MarkupSyntax::from_uri(path)).unwrap();
                let (base, preparation) = if detailed {
                    let (base, timings) =
                        factory.build_pipeline_from_parsed_with_timings(&parsed, &[]);
                    (base, Some(timings))
                } else {
                    (factory.build_pipeline_from_parsed(&parsed, &[]), None)
                };
                let prepared = base
                    .with_reader_paint_overrides(&inputs.reader_overrides)
                    .unwrap();
                let mut shaper = Shaper::default();
                let shaped = prepared.shape(&mut shaper).unwrap();
                let document = shaped
                    .layout_with_metrics_and_shaper(constraints, &inputs.image_metrics, &mut shaper)
                    .unwrap();
                let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
                let cpu_ms = (cpu_seconds() - cpu) * 1000.0;
                if let Some(counters) = &mut counters {
                    let values: BTreeMap<_, _> = counters.stop().into_iter().collect();
                    if iteration >= 3 {
                        counter_records.push(json!({"sample": iteration - 3, "variant": variant,
                                                    "section": index, "values": values}));
                    }
                }
                book_cpu += cpu_ms;
                book_wall += wall_ms;
                // Hashing and cache statistics are outside the timed load.
                let identity = json!({
                    "render_fingerprint": format!("{:016x}", layout_fingerprint(&document)),
                    "glyphs": document.render_view().text().glyph_count(),
                    "lines": document.render_view().text().line_count(),
                });
                if identities[index].is_null() {
                    identities[index] = identity;
                } else {
                    assert_eq!(
                        identities[index], identity,
                        "output differs: {variant} {path}"
                    );
                }
                if iteration >= 3 {
                    let mut record = |metric: &str, value: f64| {
                        samples
                            .entry(format!("{variant}/{index}/{metric}"))
                            .or_default()
                            .push(value);
                    };
                    record("cpu", cpu_ms);
                    record("wall", wall_ms);
                    if let Some(timings) = preparation {
                        record("prepare.style", timings.style.as_secs_f64() * 1000.0);
                        record(
                            "prepare.stylesheets",
                            timings.stylesheets.as_secs_f64() * 1000.0,
                        );
                        for (name, duration) in style_details(&timings.style_details) {
                            record(&format!("style.{name}"), duration.as_secs_f64() * 1000.0);
                        }
                    }
                    if shared && detailed {
                        let stats = cache.stats();
                        cache_records.push(json!({
                            "sample": iteration - 3, "section": index,
                            "entries": stats.entries, "bytes": stats.bytes,
                            "compiled_entries": stats.compiled_entries,
                            "compiled_hits": stats.compiled_hits, "compiled_misses": stats.compiled_misses,
                            "parsed_entries": stats.parsed_entries,
                            "parsed_hits": stats.parsed_hits, "parsed_misses": stats.parsed_misses,
                            "inline_entries": stats.inline_entries,
                            "inline_hits": stats.inline_hits, "inline_misses": stats.inline_misses,
                        }));
                    }
                }
            }
            if iteration >= 3 {
                samples
                    .entry(format!("{variant}/book/cpu"))
                    .or_default()
                    .push(book_cpu);
                samples
                    .entry(format!("{variant}/book/wall"))
                    .or_default()
                    .push(book_wall);
            }
        }
    }
    let timings: BTreeMap<_, _> = samples
        .iter()
        .map(|(name, values)| (name, summary(values)))
        .collect();
    println!(
        "{}",
        json!({
            "samples": count, "warmups": 3, "detailed": detailed,
            "paths": paths, "identities": identities, "timings": timings,
            "cache_records": cache_records, "all_outputs_identical": true,
            "counter_records": counter_records,
            "cache_reset_per_book_traversal": true,
            "font_backend": "deterministic mock", "composition": "BookOptimized",
        })
    );
}
