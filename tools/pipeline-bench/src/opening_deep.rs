use crate::opening::{style_details, summary};
use crate::{Shaper, cpu_seconds, layout_fingerprint};
use html::layout::{
    LayoutConstraints,
    profiling::{capture, capture_with_measurements, scope},
};
use html::pipeline::{DocumentFactory, PipelineInputs, parse_document};
use html::resources::FileSystemProvider;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn cpu_clock() -> Duration {
    Duration::from_secs_f64(cpu_seconds())
}

fn no_cpu_clock() -> Duration {
    Duration::ZERO
}

pub(super) fn run(path: &str, case: &str, count: usize, inputs: &PipelineInputs) {
    assert!(count > 0);
    let detailed = case == "open_solver_book";
    let cpu_measured = !matches!(case, "open_deep_wall_book" | "open_solver_book");
    let clock = if cpu_measured {
        cpu_clock
    } else {
        no_cpu_clock
    };
    let mut samples = BTreeMap::<String, Vec<f64>>::new();
    let mut scopes = BTreeMap::<String, BTreeMap<&str, Vec<f64>>>::new();
    let mut measurements = BTreeMap::<String, Vec<Vec<BTreeMap<&str, usize>>>>::new();
    let constraints = LayoutConstraints::new(600.0, 16.0)
        .unwrap()
        .with_text_composition_policy(inputs.layout.text_composition_policy);
    let mut output = Value::Null;
    for iteration in 0..count + 3 {
        let mut factory = DocumentFactory::new();
        factory.set_resource_context(Arc::new(FileSystemProvider), path);
        factory.set_reader_overrides(inputs.reader_overrides.layout_overrides());
        factory.set_media_environment(inputs.style_environment.media);
        let cpu = cpu_seconds();
        let wall = Instant::now();
        let ((document, preparation, source_bytes), entries) = capture_with_measurements(clock, detailed, || {
            let _total = scope("opening.total");
            let source = {
                let _scope = scope("opening.source_read");
                std::fs::read_to_string(path).unwrap()
            };
            let parsed = {
                let _scope = scope("opening.parse");
                parse_document(&source, inputs.markup_syntax).unwrap()
            };
            let (base, preparation) = {
                let _scope = scope("opening.preparation");
                factory.build_pipeline_from_parsed_with_timings(&parsed, &[])
            };
            let prepared = {
                let _scope = scope("opening.paint_prepare");
                base.with_reader_paint_overrides(&inputs.reader_overrides)
                    .unwrap()
            };
            let mut shaper = Shaper::default();
            let shaped = {
                let _scope = scope("opening.shape");
                prepared.shape(&mut shaper).unwrap()
            };
            let document = {
                let _scope = scope("opening.layout");
                shaped
                    .layout_with_metrics_and_shaper(constraints, &inputs.image_metrics, &mut shaper)
                    .unwrap()
            };
            (document, preparation, source.len())
        });
        let elapsed = wall.elapsed();
        let cpu_ms = (cpu_seconds() - cpu) * 1000.0;
        if iteration >= 3 {
            // Absent scopes (e.g. hyphenation) contribute zero in each sample.
            for series in scopes.values_mut() {
                for values in series.values_mut() {
                    values.push(0.0);
                }
            }
            for entry in &entries {
                if !entry.measurements.is_empty() {
                    measurements.entry(entry.name.to_owned()).or_default().push(entry.measurements.clone());
                }
                let series = scopes.entry(entry.name.to_owned()).or_default();
                for (metric, value) in [
                    (
                        "inclusive_wall",
                        entry.inclusive_wall.as_secs_f64() * 1000.0,
                    ),
                    (
                        "exclusive_wall",
                        entry.exclusive_wall.as_secs_f64() * 1000.0,
                    ),
                    ("inclusive_cpu", entry.inclusive_cpu.as_secs_f64() * 1000.0),
                    ("exclusive_cpu", entry.exclusive_cpu.as_secs_f64() * 1000.0),
                    ("calls", entry.calls as f64),
                ] {
                    let values = series
                        .entry(metric)
                        .or_insert_with(|| vec![0.0; iteration - 2]);
                    *values.last_mut().unwrap() = value;
                }
            }
            let mut record = |name: &str, value: Duration| {
                samples
                    .entry(name.to_owned())
                    .or_default()
                    .push(value.as_secs_f64() * 1000.0);
            };
            for entry in &entries {
                if let Some(name) = entry.name.strip_prefix("opening.") {
                    if name != "total" {
                        record(name, entry.inclusive_wall);
                    }
                }
            }
            for (name, value) in [
                ("prepare.dom", preparation.dom),
                ("prepare.stylesheets", preparation.stylesheets),
                ("prepare.style", preparation.style),
                ("prepare.box_tree", preparation.prepare),
                ("total_wall", elapsed),
            ] {
                record(name, value);
            }
            for (name, value) in style_details(&preparation.style_details) {
                record(&format!("style.{name}"), value);
            }
            samples.entry("total_cpu".into()).or_default().push(cpu_ms);
            let sum_exclusive_cpu: Duration = entries.iter().map(|entry| entry.exclusive_cpu).sum();
            let root = entries
                .iter()
                .find(|entry| entry.name == "opening.total")
                .unwrap();
            assert_eq!(
                sum_exclusive_cpu, root.inclusive_cpu,
                "exclusive CPU times must partition the root"
            );
            let sum_exclusive_wall: Duration =
                entries.iter().map(|entry| entry.exclusive_wall).sum();
            assert_eq!(
                sum_exclusive_wall, root.inclusive_wall,
                "exclusive wall times must partition the root"
            );
            output = json!({
                "case": case, "path": path, "samples": count, "warmups": 3,
                "source_bytes": source_bytes, "font_backend": "deterministic mock", "heap_instrumented": cfg!(feature="heap"),
                "composition": format!("{:?}", inputs.layout.text_composition_policy),
                "glyphs": document.render_view().text().glyph_count(), "lines": document.render_view().text().line_count(),
                "render_fingerprint": format!("{:016x}", layout_fingerprint(&document)),
                "exclusive_times_partition_root": true,
                "scope_cpu_measured": cpu_measured,
            });
        }
    }
    output["timings"] = samples
        .iter()
        .map(|(key, values)| (key.clone(), summary(values)))
        .collect::<serde_json::Map<_, _>>()
        .into();
    output["measurements"] = json!(measurements);
    output["scopes"] = scopes
        .iter()
        .map(|(key, series)| {
            (
                key.clone(),
                json!(
                    series
                        .iter()
                    .map(|(metric, values)| {
                        if !cpu_measured && metric.ends_with("_cpu") {
                            return ((*metric).to_owned(), Value::Null);
                        }
                        let value = summary(values);
                        let value = if *metric == "calls" {
                            json!({"median": value["median_ms"], "min": value["min_ms"], "max": value["max_ms"], "samples": values})
                        } else { value };
                        ((*metric).to_owned(), value)
                    })
                        .collect::<BTreeMap<_, _>>()
                ),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into();
    let iterations = 10000;
    let started = cpu_seconds();
    capture(clock, || {
        for _ in 0..iterations {
            drop(scope("empty"));
        }
    });
    output["empty_scope_cpu_ns"] = json!((cpu_seconds() - started) * 1e9 / f64::from(iterations));
    println!("{output}");
}
