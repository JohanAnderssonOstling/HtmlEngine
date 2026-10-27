//! Focused workloads for the clone/lock/Arc audit. Setup and integrity checks
//! are outside the measured region; heap and timing builds run separately.

use crate::{Shaper, cpu_seconds, heap, layout_fingerprint, median};
use html::document::{ImageResource, ImageSource};
use html::layout::LayoutConstraints;
use html::pipeline::{BookStylesheetCache, DocumentFactory, parse_html_document};
use html::resources::{FileSystemProvider, ImagePipeline};
use html::text::GlyphResourceStore;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

pub(super) fn measure(count: usize, mut operation: impl FnMut()) -> Value {
    let mut wall = Vec::new();
    let mut cpu = Vec::new();
    let mut allocations = Vec::new();
    let mut bytes = Vec::new();
    let mut instructions = Vec::new();
    let mut counter = (!cfg!(feature = "heap"))
        .then(crate::counter::InstructionCounter::new)
        .flatten();
    for index in 0..count + 3 {
        heap::reset();
        if let Some(counter) = &mut counter {
            counter.start();
        }
        let cpu_start = cpu_seconds();
        let start = Instant::now();
        operation();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let consumed = (cpu_seconds() - cpu_start) * 1000.0;
        let instruction_count = counter
            .as_mut()
            .and_then(crate::counter::InstructionCounter::stop);
        let (_, _, allocs, allocated) = heap::sample();
        if index >= 3 {
            wall.push(elapsed);
            cpu.push(consumed);
            allocations.push(allocs as f64);
            bytes.push(allocated as f64);
            if let Some(value) = instruction_count {
                instructions.push(value as f64);
            }
        }
    }
    json!({"wall_median_ms": median(&mut wall), "cpu_median_ms": median(&mut cpu),
        "allocations": median(&mut allocations), "allocated_bytes": median(&mut bytes),
        "user_instructions": (!instructions.is_empty()).then(|| median(&mut instructions)),
        "measured_samples": count})
}

pub(super) fn run(path: &str, case: &str, count: usize) {
    let source = std::fs::read_to_string(path).unwrap();
    let mut result_resources = GlyphResourceStore::default();
    let mut result = match case {
        "access_images" => {
            let images = (0..256)
                .map(|index| ImageResource {
                    source: ImageSource::InlineSvg {
                        bytes: format!(
                            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"1\"/>",
                            index + 1
                        )
                        .into_bytes()
                        .into(),
                        base_uri: String::new(),
                    },
                    width: 1,
                    height: 1,
                    width_attr: None,
                    height_attr: None,
                })
                .collect::<Vec<_>>();
            let mut pipeline = ImagePipeline::new(Arc::new(FileSystemProvider));
            pipeline.ensure_window(&images, &(0..256).collect::<HashSet<_>>());
            assert!(pipeline.poll().decoded);
            let mut result = measure(count, || {
                for _ in 0..100 {
                    assert!(!black_box(pipeline.poll()).decoded);
                }
            });
            let dimensions: Vec<_> = (0..256)
                .map(|index| pipeline.get_decoded(index).unwrap().dimensions())
                .collect();
            result["integrity"] = json!(dimensions);
            result["operations_per_sample"] = json!(100);
            result
        }
        "access_style" => {
            let parsed = parse_html_document(&source);
            let cache = BookStylesheetCache::default();
            let mut factory = DocumentFactory::new();
            factory.set_book_stylesheet_cache(cache);
            let mut prepared = None;
            let mut result = measure(count, || {
                prepared = Some(black_box(factory.build_pipeline_from_parsed(&parsed, &[])));
            });
            let mut shaper = Shaper::default();
            let document = prepared
                .unwrap()
                .shape(&mut shaper, &mut result_resources)
                .unwrap()
                .layout_with_metrics_and_shaper(
                    LayoutConstraints::new(600.0, 16.0).unwrap(),
                    &Default::default(),
                    &mut shaper,
                    &mut result_resources,
                )
                .unwrap();
            result["integrity"] = json!(format!(
                "{:016x}",
                layout_fingerprint(&document, &result_resources)
            ));
            result
        }
        "access_first_line" => {
            let parsed = parse_html_document(&source);
            let mut factory = DocumentFactory::new();
            let prepared = factory.build_pipeline_from_parsed(&parsed, &[]);
            let mut document = None;
            let mut result = measure(count, || {
                let mut shaper = Shaper {
                    register_runs: true,
                    ..Default::default()
                };
                document = Some(black_box(
                    prepared
                        .shape_borrowed(&mut shaper, &mut result_resources)
                        .unwrap()
                        .layout_with_metrics_and_shaper(
                            LayoutConstraints::new(600.0, 16.0).unwrap(),
                            &Default::default(),
                            &mut shaper,
                            &mut result_resources,
                        )
                        .unwrap(),
                ));
            });
            result["integrity"] = json!(format!(
                "{:016x}",
                layout_fingerprint(document.as_ref().unwrap(), &result_resources)
            ));
            result
        }
        "access_controls" => {
            let parsed = parse_html_document(&source);
            let mut factory = DocumentFactory::new();
            let prepared = factory.build_pipeline_from_parsed(&parsed, &[]);
            let mut shaped = None;
            // Keep the final backend alive for first-line layout after timing.
            let mut shaper = Shaper::default();
            let mut result = measure(count, || {
                shaper = Shaper {
                    register_runs: true,
                    ..Default::default()
                };
                shaped = Some(black_box(
                    prepared
                        .shape_borrowed(&mut shaper, &mut result_resources)
                        .unwrap(),
                ));
            });
            let document = shaped
                .unwrap()
                .layout_with_shaper(
                    LayoutConstraints::new(600.0, 16.0).unwrap(),
                    &mut shaper,
                    &mut result_resources,
                )
                .expect("access fixture lays out with its active shaper");
            result["integrity"] = json!(format!(
                "{:016x}",
                layout_fingerprint(&document, &result_resources)
            ));
            result
        }
        "access_style_parallel" => {
            let cache = BookStylesheetCache::default();
            let mut result = measure(count, || {
                std::thread::scope(|scope| {
                    let jobs: Vec<_> = (0..4)
                        .map(|_| {
                            let cache = cache.clone();
                            let source = &source;
                            scope.spawn(move || {
                                let parsed = parse_html_document(source);
                                let mut factory = DocumentFactory::new();
                                factory.set_book_stylesheet_cache(cache);
                                black_box(factory.build_pipeline_from_parsed(&parsed, &[]));
                            })
                        })
                        .collect();
                    for job in jobs {
                        job.join().unwrap();
                    }
                });
            });
            let parsed = parse_html_document(&source);
            let mut factory = DocumentFactory::new();
            factory.set_book_stylesheet_cache(cache);
            let prepared = factory.build_pipeline_from_parsed(&parsed, &[]);
            let mut shaper = Shaper::default();
            let document = prepared
                .shape(&mut shaper, &mut result_resources)
                .unwrap()
                .layout_with_metrics_and_shaper(
                    LayoutConstraints::new(600.0, 16.0).unwrap(),
                    &Default::default(),
                    &mut shaper,
                    &mut result_resources,
                )
                .unwrap();
            result["integrity"] = json!(format!(
                "{:016x}",
                layout_fingerprint(&document, &result_resources)
            ));
            result["workers"] = json!(4);
            result
        }
        "access_parallel" => {
            let parsed = parse_html_document(&source);
            let mut factory = DocumentFactory::new();
            let prepared = factory.build_pipeline_from_parsed(&parsed, &[]);
            let mut shaper = Shaper::default();
            let shaped = prepared
                .shape_borrowed(&mut shaper, &mut result_resources)
                .unwrap();
            let constraints = LayoutConstraints::new(600.0, 16.0)
                .unwrap()
                .with_parallel_workers(4);
            let mut document = Some(
                shaped
                    .layout_with_metrics_and_shaper(
                        constraints,
                        &Default::default(),
                        &mut shaper,
                        &mut result_resources,
                    )
                    .unwrap(),
            );
            let mut result = measure(count, || {
                document = Some(black_box(
                    document
                        .take()
                        .unwrap()
                        .into_shaped()
                        .layout_with_metrics_and_shaper(
                            constraints,
                            &Default::default(),
                            &mut shaper,
                            &mut result_resources,
                        )
                        .unwrap(),
                ));
            });
            result["integrity"] = json!(format!(
                "{:016x}",
                layout_fingerprint(document.as_ref().unwrap(), &result_resources)
            ));
            result["workers"] = json!(constraints.parallel_workers());
            result
        }
        _ => panic!("unknown access workload"),
    };
    result["case"] = json!(case);
    println!("{result}");
}
