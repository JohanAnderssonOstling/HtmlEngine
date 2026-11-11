//! Measure parse, style, real GPUI shaping and screen layout, excluding painting.
use anyhow::{Context, Result, ensure};
use gpui::WindowTextSystem;
use html::resources::FileSystemProvider;
use html_engine_model::LayoutConstraints;
use html_gpui::backend::GpuiTextCache;
use html_pipeline::{
    MarkupSyntax, MediaEnvironment, PipelineInputs, PipelineSession, StyleEnvironment,
};
use serde_json::json;
use std::{hint::black_box, sync::Arc, time::Instant};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() >= 2,
        "usage: gpui-pipeline-profile HTML [iterations=40] [retained|fresh]"
    );
    let iterations: usize = args.get(2).map(|v| v.parse()).transpose()?.unwrap_or(40);
    ensure!(iterations > 0, "iterations must be positive");
    let cache_mode = args.get(3).map(String::as_str).unwrap_or("retained");
    ensure!(
        matches!(cache_mode, "retained" | "fresh"),
        "cache mode must be retained or fresh"
    );
    let path = std::fs::canonicalize(&args[1])?;
    let source = std::fs::read_to_string(&path)?;
    let app = gpui_platform::headless();
    let text_system = app.text_system();
    let provider = Arc::new(FileSystemProvider::new());
    let create_shaper = || GpuiTextCache::new(Arc::new(WindowTextSystem::new(text_system.clone())));
    let mut shaper = (cache_mode == "retained").then(create_shaper);
    let mut samples = Vec::with_capacity(iterations);
    let mut counts = json!(null);
    for iteration in 0..iterations + 3 {
        // Source transfer and host text-system initialization are outside timing,
        // matching the browser's already-transferred input and running process.
        let inputs = PipelineInputs::new(
            source.clone(),
            MarkupSyntax::Html,
            path.to_string_lossy().into_owned(),
            StyleEnvironment {
                root_font_size: Default::default(),
                media: MediaEnvironment::screen(600.0, Some(800.0))
                    .context("invalid screen dimensions")?,
            },
            LayoutConstraints::new(600.0, 16.0)?,
        );
        let started = Instant::now();
        let current_shaper = shaper.get_or_insert_with(create_shaper);
        let mut session = PipelineSession::new(provider.clone());
        session.attach(current_shaper).update(inputs)?;
        let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
        if iteration >= 3 {
            samples.push(elapsed);
        }
        let view = session.render_view().context("missing layout output")?;
        counts = json!({"lines": view.text().line_count(), "glyphs": view.text().glyph_count()});
        black_box(&session);
        if cache_mode == "fresh" {
            shaper = None;
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "case": "pipeline", "input": path, "iterations": iterations,
            "backend": "GpuiTextCache with the native GPUI platform text system",
            "width": 600, "media_height": 800, "flow_height": null,
            "composition": "WebCompatible", "images": "no decoding or network",
            "warmup_iterations": 3, "text_cache_mode": cache_mode, "counts": counts, "samples_ms": samples,
            "scope": "Fresh session: HTML/CSS parse, styling, box construction, real font shaping and first continuous layout. Excludes source transfer, platform initialization, output inspection, teardown and painting."
        }))?
    );
    Ok(())
}
