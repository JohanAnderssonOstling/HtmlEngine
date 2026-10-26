#[path = "../common.rs"]
mod common;

use anyhow::{Context, Result, bail, ensure};
use common::{Options, blank_html, emit};
use data_url::DataUrl;
use gpui::{TextSystem, WindowTextSystem};
use gpui_wgpu::CosmicTextSystem;
use html::engine::Engine;
use html::pipeline::{
    FontEnvironmentRevision, ImageMetricsRevision, ImageSizingPolicy, LayoutConstraints,
    MarkupSyntax, MediaEnvironment, PaintSettingsRevision, PipelineInputs, ResourceRevision,
    SourceRevision, StyleEnvironment, StylesheetRevision, TextCompositionPolicy,
};
use html::resources::{FileSystemProvider, ImagePipeline, ResourceProvider};
use html_gpui::backend::GpuiTextCache;
use libc::{CLOCK_THREAD_CPUTIME_ID, clock_gettime, timespec};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{self, BufRead};
use std::sync::Arc;
use std::time::Instant;

struct LocalResources(FileSystemProvider);

impl ResourceProvider for LocalResources {
    fn read_bytes(&self, uri: &str) -> io::Result<Vec<u8>> {
        if uri.starts_with("data:") {
            return DataUrl::process(uri)
                .map_err(|error| io::Error::other(format!("invalid data URL: {error:?}")))?
                .decode_to_vec()
                .map(|(bytes, _)| bytes)
                .map_err(|error| io::Error::other(format!("invalid image data: {error:?}")));
        }
        self.0.read_bytes(uri)
    }

    fn resolve(&self, base: &str, href: &str) -> String {
        if href.starts_with("data:") {
            href.to_owned()
        } else {
            self.0.resolve(base, href)
        }
    }

    fn exists(&self, uri: &str) -> bool {
        if uri.starts_with("data:") {
            self.read_bytes(uri).is_ok()
        } else {
            self.0.exists(uri)
        }
    }

    fn list_html_candidates(&self, root: &str) -> io::Result<Vec<String>> {
        self.0.list_html_candidates(root)
    }
}

fn thread_cpu_ms() -> Result<f64> {
    let mut value = timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // The OS initializes the supplied timespec for this calling thread.
    if unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut value) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(value.tv_sec as f64 * 1000.0 + value.tv_nsec as f64 / 1_000_000.0)
}

struct LayoutHost {
    engine: Engine,
    shaper: GpuiTextCache,
    provider: Arc<dyn ResourceProvider>,
    decoded: Option<ImagePipeline>,
    source_revision: SourceRevision,
}

impl LayoutHost {
    fn new() -> Self {
        // WindowTextSystem owns text layout caches; constructing it does not
        // create a platform window, GPU device, atlas or renderer.
        let text = Arc::new(TextSystem::new(Arc::new(CosmicTextSystem::new(
            "DejaVu Sans",
        ))));
        let provider: Arc<dyn ResourceProvider> =
            Arc::new(LocalResources(FileSystemProvider::new()));
        Self {
            engine: Engine::new(provider.clone()),
            shaper: GpuiTextCache::new(Arc::new(WindowTextSystem::new(text))),
            provider,
            decoded: None,
            source_revision: SourceRevision::INITIAL,
        }
    }

    fn load(&mut self, source: &str, options: &Options) -> Result<Value> {
        let began = Instant::now();
        let cpu_began = thread_cpu_ms()?;
        self.source_revision = self.source_revision.next();
        let mut inputs = PipelineInputs {
            source: source.into(),
            markup_syntax: MarkupSyntax::Html,
            user_styles: Vec::new(),
            reader_overrides: Default::default(),
            note_flow: Default::default(),
            source_revision: self.source_revision,
            base_uri: options.fixture.to_string_lossy().into_owned(),
            resource_revision: ResourceRevision::INITIAL,
            stylesheet_revision: StylesheetRevision::INITIAL,
            style_environment: StyleEnvironment {
                root_font_size: 16,
                media: MediaEnvironment::screen(
                    f64::from(options.width),
                    Some(f64::from(options.height)),
                )
                .context("invalid viewport")?,
                direction: 0,
            },
            font_environment: FontEnvironmentRevision::INITIAL,
            image_metrics_revision: ImageMetricsRevision::INITIAL,
            layout: LayoutConstraints {
                viewport_width: f64::from(options.width),
                viewport_height: Some(f64::from(options.height)),
                line_height: 16.0,
                image_sizing_policy: ImageSizingPolicy::WebCompatible,
                text_composition_policy: TextCompositionPolicy::WebCompatible,
            },
            image_metrics: Default::default(),
            paint: PaintSettingsRevision::INITIAL,
        };
        let wall = Instant::now();
        let stage = thread_cpu_ms()?;
        self.engine.prepare_through_style(inputs.clone())?;
        let parse_style_cpu_ms = thread_cpu_ms()? - stage;
        let parse_style_elapsed_ms = wall.elapsed().as_secs_f64() * 1000.0;
        let wall = Instant::now();
        let stage = thread_cpu_ms()?;
        self.engine.rehydrate_glyphs(&mut self.shaper)?;
        let shape_layout_cpu_ms = thread_cpu_ms()? - stage;
        let shape_layout_elapsed_ms = wall.elapsed().as_secs_f64() * 1000.0;
        let stage = thread_cpu_ms()?;
        let resources = self
            .engine
            .document()
            .context("missing document")?
            .render_view()
            .images()
            .to_vec();
        let desired = (0..resources.len() as u32).collect::<HashSet<_>>();
        let mut decoded = ImagePipeline::new(Arc::new(resources), self.provider.clone());
        decoded.ensure_window(&desired);
        decoded.poll();
        let mut intrinsic_images = Vec::new();
        for index in &desired {
            let image = decoded
                .get_decoded(*index)
                .with_context(|| format!("image {index} failed to decode"))?;
            let (width, height) = image.dimensions();
            inputs.image_metrics.set(*index, width, height);
            intrinsic_images.push(json!({"index":index,"width":width,"height":height}));
        }
        intrinsic_images.sort_by_key(|image| image["index"].as_u64());
        self.decoded = Some(decoded);
        let image_preparation_cpu_ms = thread_cpu_ms()? - stage;
        let wall = Instant::now();
        let stage = thread_cpu_ms()?;
        if !desired.is_empty() {
            inputs.image_metrics_revision = inputs.image_metrics_revision.next();
            self.engine.update(inputs, &mut self.shaper)?;
        }
        let image_relayout_cpu_ms = thread_cpu_ms()? - stage;
        let image_relayout_elapsed_ms = wall.elapsed().as_secs_f64() * 1000.0;
        let total_cpu_ms = thread_cpu_ms()? - cpu_began;
        let elapsed_ms = began.elapsed().as_secs_f64() * 1000.0;
        let document = self
            .engine
            .document()
            .context("missing laid-out document")?;
        let view = document.render_view();
        let text = view.text();
        let boxes = view.boxes();
        let table_rows = (0..boxes.len())
            .filter(|index| boxes.is_table_row(*index))
            .count();
        let mut image_rects = Vec::new();
        for (index, line) in text.lines().iter().enumerate() {
            for image in view.fragments().images_for_line(index).iter() {
                image_rects.push(json!({"index":image.image_idx(),
                    "x":line.point().x + image.offset().x, "y":line.point().y + image.offset().y,
                    "width":image.size().width, "height":image.size().height}));
            }
        }
        let diagnostic_lines = std::env::var_os("SKRIFT_BENCH_INSPECT").map(|_| {
            text.lines()
                .iter()
                .map(|line| {
                    let characters: Vec<_> = line
                        .glyphs()
                        .filter_map(|index| {
                            let metric = text.glyph_metric(text.glyph_at(index as usize)?)?;
                            Some(json!({"character":metric.ch().to_string(),
                        "advance":text.character_advance(index),
                        "metric_advance":metric.advance()}))
                        })
                        .collect();
                    let owner = line.owner_box_idx();
                    json!({"index":line.index(),"x":line.point_x(),"y":line.point_y(),
                    "height":line.height(),"owner":owner,"tag":boxes.tag(owner),
                    "owner_width":boxes.size(owner).map(|size| size.width),
                    "font_size":boxes.text_format(owner).font_size,
                    "characters":characters})
                })
                .collect::<Vec<_>>()
        });
        Ok(json!({
            "engine_elapsed_ms":parse_style_elapsed_ms + shape_layout_elapsed_ms + image_relayout_elapsed_ms,
            "parse_style_elapsed_ms":parse_style_elapsed_ms,"shape_layout_elapsed_ms":shape_layout_elapsed_ms,
            "image_relayout_elapsed_ms":image_relayout_elapsed_ms,
            "engine_cpu_ms":parse_style_cpu_ms + shape_layout_cpu_ms + image_relayout_cpu_ms,
            "parse_style_cpu_ms":parse_style_cpu_ms, "shape_layout_cpu_ms":shape_layout_cpu_ms,
            "image_relayout_cpu_ms":image_relayout_cpu_ms, "image_preparation_cpu_ms":image_preparation_cpu_ms,
            "total_main_thread_cpu_ms":total_cpu_ms, "load_to_layout_ms":elapsed_ms,
            "diagnostic_lines":diagnostic_lines,
            "layout":{"width":options.width,"height":options.height,"scale":1,
                "lines":text.lines().len(),"table_rows":table_rows,
                "intrinsic_images":intrinsic_images,"image_rects":image_rects},
            "painting":false,"gpu_device_created":false,"platform_window_created":false
        }))
    }
}

fn main() -> Result<()> {
    let options = Options::parse()?;
    ensure!(
        options.screenshot.is_none(),
        "layout benchmark does not paint or save screenshots"
    );
    let mut host = LayoutHost::new();
    let mut baseline = host.load(blank_html(), &options)?;
    baseline["event"] = "layout_ready".into();
    baseline["phase"] = "blank".into();
    emit(baseline);
    let mut loaded = false;
    for command in io::stdin().lock().lines() {
        match command?.as_str() {
            "load" if !loaded => {
                let began = Instant::now();
                let source = std::fs::read_to_string(&options.fixture)?;
                let mut document = host.load(&source, &options)?;
                document["load_to_layout_ms"] = (began.elapsed().as_secs_f64() * 1000.0).into();
                document["event"] = "layout_ready".into();
                document["phase"] = "document".into();
                emit(document);
                loaded = true;
            }
            "quit" => break,
            command => bail!("unexpected command: {command}"),
        }
    }
    Ok(())
}
