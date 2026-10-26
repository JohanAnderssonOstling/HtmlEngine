#[path = "../../common.rs"]
mod common;

#[cfg(feature = "memory-profile")]
#[path = "../../memory_probe.rs"]
mod memory_probe;

use anyhow::{Context as _, Result, bail, ensure};
use common::{Options, blank_html, emit};
use data_url::DataUrl;
use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, ContentMask, Context, IntoElement, Render, Window,
    WindowBounds, WindowOptions, canvas, div, prelude::*, px, rgb, size,
};
use html::engine::Engine;
use html::pipeline::{
    FontEnvironmentRevision, ImageMetricsRevision, ImageSizingPolicy, LayoutConstraints,
    MarkupSyntax, MediaEnvironment, PaintSettingsRevision, PipelineInputs, ResourceRevision,
    SourceRevision, StyleEnvironment, StylesheetRevision, TextCompositionPolicy,
};
use html::render::{
    FragmentImagePolicy, FragmentRenderOptions, Painter, RenderCommand, RenderScene,
    paint_line_text_runs,
};
use html::resources::{DecodedImage, FileSystemProvider, ImagePipeline, ResourceProvider};
use html_gpui::backend::{
    GlyphStore, GpuiCommandPainter, GpuiTextCache, ImageCache, PaintCommand, paint_commands,
};
use serde_json::json;
use std::collections::HashSet;
use std::io::{self, BufRead};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct LocalResources(FileSystemProvider);

impl ResourceProvider for LocalResources {
    fn read_bytes(&self, uri: &str) -> io::Result<Vec<u8>> {
        if uri.starts_with("data:") {
            return DataUrl::process(uri)
                .map_err(|error| io::Error::other(format!("invalid data URL: {error:?}")))?
                .decode_to_vec()
                .map(|(bytes, _)| bytes)
                .map_err(|error| io::Error::other(format!("invalid data URL bytes: {error:?}")));
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

struct DocumentView {
    engine: Engine,
    source_revision: SourceRevision,
    shaper: GpuiTextCache,
    provider: Arc<dyn ResourceProvider>,
    decoded: Option<ImagePipeline>,
    images: ImageCache,
    commands: Arc<Vec<PaintCommand>>,
    content_height: f64,
    image_count: usize,
}

impl DocumentView {
    #[cfg(feature = "memory-profile")]
    fn memory_checkpoint(&self, stage: &str) {
        let heap = memory_probe::sample();
        let entries = self
            .engine
            .document()
            .map(|document| {
                document.memory_usage_report().entries.into_iter().map(|entry| {
                json!({"label":entry.label,"bytes":entry.bytes,"count":entry.count})
            }).collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let stats = self.shaper.shape_cache_stats();
        emit(
            json!({"event":"memory_checkpoint","stage":stage,"heap":heap,
            "engine_entries":entries,
            "paint_commands":self.commands.len(),
            "paint_command_storage_bytes":self.commands.capacity() * size_of::<PaintCommand>(),
            "shaper":{"retained_glyphs":stats.retained_glyphs,"retained_lines":stats.retained_lines,
                "registered_glyphs":stats.registered_glyphs,"backend_glyph_records":stats.backend_glyph_records,
                "font_setups":stats.retained_font_setups}}),
        );
        memory_probe::pause();
    }

    fn new(window: &Window) -> Self {
        let provider: Arc<dyn ResourceProvider> =
            Arc::new(LocalResources(FileSystemProvider::new()));
        Self {
            engine: Engine::new(provider.clone()),
            source_revision: SourceRevision::INITIAL,
            shaper: GpuiTextCache::new(window.text_system().clone()),
            provider,
            decoded: None,
            images: ImageCache::default(),
            commands: Arc::default(),
            content_height: 0.0,
            image_count: 0,
        }
    }

    fn load(&mut self, source: &str, options: &Options) -> Result<()> {
        #[cfg(feature = "memory-profile")]
        self.memory_checkpoint("before_load");
        let settings =
            FragmentRenderOptions::new(f64::from(options.width), Some(f64::from(options.height)))?
                .with_base_uri(options.fixture.to_string_lossy().into_owned())?
                .with_image_policy(FragmentImagePolicy::Reference);
        self.source_revision = self.source_revision.next();
        let mut inputs = PipelineInputs {
            source: source.into(),
            markup_syntax: MarkupSyntax::Html,
            user_styles: Vec::new(),
            reader_overrides: Default::default(),
            note_flow: Default::default(),
            source_revision: self.source_revision,
            base_uri: settings.base_uri().to_owned(),
            resource_revision: ResourceRevision::INITIAL,
            stylesheet_revision: StylesheetRevision::INITIAL,
            style_environment: StyleEnvironment {
                root_font_size: 16,
                media: MediaEnvironment::screen(
                    f64::from(options.width),
                    Some(f64::from(options.height)),
                )
                .context("invalid media dimensions")?,
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
        self.engine.update(inputs.clone(), &mut self.shaper)?;
        #[cfg(feature = "memory-profile")]
        self.memory_checkpoint("after_layout");
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
        for index in &desired {
            let image = decoded
                .get_decoded(*index)
                .with_context(|| format!("image {index} did not decode"))?;
            let (width, height) = image.dimensions();
            inputs.image_metrics.set(*index, width, height);
        }
        if !desired.is_empty() {
            inputs.image_metrics_revision = inputs.image_metrics_revision.next();
            self.engine.update(inputs, &mut self.shaper)?;
        }
        let document = self
            .engine
            .document()
            .context("missing laid-out document")?;
        let scene = RenderScene::for_document(document, &settings);
        self.content_height = scene.content_height();
        let mut painter = GpuiCommandPainter::new(
            &mut self.images,
            self.shaper.store(),
            self.shaper.run_store(),
            Vec::new(),
        );
        self.image_count = 0;
        for command in scene.commands() {
            match command {
                RenderCommand::PushClip { rect } => painter.push_clip(*rect),
                RenderCommand::PopClip => painter.pop_clip(),
                RenderCommand::Fill { rect, color } => painter.fill_rect(*rect, *color),
                RenderCommand::RoundedFill { rect, radii, color } => {
                    painter.fill_rounded_rect(*rect, *radii, *color)
                }
                RenderCommand::RoundedBorder {
                    rect,
                    radii,
                    width,
                    color,
                } => painter.stroke_rounded_rect(*rect, *radii, *width, *color),
                RenderCommand::Glyph { .. } => {}
                RenderCommand::Image { image, hash, rect } => {
                    painter.draw_image(image, hash, *rect)
                }
                RenderCommand::Svg {
                    bytes,
                    hash,
                    intrinsic_size,
                    rect,
                } => painter.draw_svg(bytes, hash, *intrinsic_size, *rect),
                RenderCommand::ResourceImage {
                    image_idx, rect, ..
                } => {
                    match decoded
                        .get_decoded(*image_idx)
                        .context("missing decoded image")?
                    {
                        DecodedImage::Raster { image, hash, .. } => {
                            painter.draw_image(image, hash, *rect)
                        }
                        DecodedImage::Svg {
                            bytes,
                            hash,
                            width,
                            height,
                        } => painter.draw_svg(bytes, hash, (*width, *height), *rect),
                    }
                    self.image_count += 1;
                }
            }
        }
        let text = document.render_view().text();
        for index in text.paint_order_indices() {
            let index = *index as usize;
            let Some(line) = text.line(index) else {
                continue;
            };
            if line.point().y >= f64::from(options.height) {
                continue;
            }
            ensure!(
                paint_line_text_runs(document, index, line.point(), 1.0, None, &mut painter),
                "native text run painting unavailable for line {index}"
            );
        }
        self.commands = Arc::new(painter.into_commands());
        ensure!(
            self.commands
                .iter()
                .filter(|command| matches!(command, PaintCommand::Image { .. }))
                .count()
                >= self.image_count,
            "an image failed to rasterize"
        );
        self.decoded = Some(decoded);
        #[cfg(feature = "memory-profile")]
        self.memory_checkpoint("after_paint_commands");
        Ok(())
    }
}

impl Render for DocumentView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let commands = self.commands.clone();
        let glyphs: GlyphStore = self.shaper.store();
        div().size_full().bg(rgb(0xffffff)).overflow_hidden().child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        paint_commands(&commands, &glyphs, bounds, window, cx);
                    });
                },
            )
            .size_full(),
        )
    }
}

fn snapshot(
    handle: AnyWindowHandle,
    phase: &str,
    began: Instant,
    options: &Options,
    cx: &mut impl AppContext,
) -> Result<()> {
    handle.update(cx, |root, window, cx| -> Result<()> {
        window.draw(cx).clear(cx);
        let image = window.render_to_image()?;
        let elapsed = began.elapsed().as_secs_f64() * 1000.0;
        let viewport = window.viewport_size();
        let scale = window.scale_factor();
        let dimensions = image.dimensions();
        let view = root
            .downcast::<DocumentView>()
            .map_err(|_| anyhow::anyhow!("unexpected root view"))?;
        let state = view.read(cx);
        let content_height = state.content_height;
        let image_count = state.image_count;
        if phase == "document"
            && let Some(path) = &options.screenshot
        {
            image.save(path)?;
        }
        #[cfg(feature = "memory-profile")]
        state.memory_checkpoint("snapshot_pixels_alive");
        drop(image);
        #[cfg(feature = "memory-profile")]
        state.memory_checkpoint("after_snapshot");
        emit(json!({
            "event":"ready", "phase":phase, "engine":"skrift-gpui",
            "gpu":window.gpu_specs(),
            "pid":std::process::id(), "document_to_snapshot_ms":elapsed,
            "snapshot_width":dimensions.0, "snapshot_height":dimensions.1,
            "page":{"width":f32::from(viewport.width), "height":f32::from(viewport.height),
                    "scale":scale, "layout_width":f32::from(viewport.width),
                    "content_height":content_height, "images_painted":image_count},
            "endpoint":"viewport snapshot complete; includes readback; excludes PNG encoding"
        }));
        Ok(())
    })??;
    Ok(())
}

fn main() -> Result<()> {
    let options = Options::parse()?;
    let (sender, receiver) = async_channel::unbounded();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            match line {
                Ok(line) => {
                    if sender.send_blocking(line).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    eprintln!("stdin failed: {error}");
                    return;
                }
            }
        }
    });
    gpui_platform::application().run(move |cx: &mut App| {
        let result = (|| -> Result<()> {
            let bounds = Bounds::centered(
                None,
                size(px(options.width as f32), px(options.height as f32)),
                cx,
            );
            let window = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: None,
                    is_resizable: false,
                    ..Default::default()
                },
                |window, cx| cx.new(|_| DocumentView::new(window)),
            )?;
            window.update(cx, |view, _, _| view.load(blank_html(), &options))??;
            cx.activate(true);
            cx.spawn(async move |cx| {
                let result = async {
                    // Allow the compositor's initial configure and scale to
                    // arrive before capturing the untimed empty-window baseline.
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                    snapshot(window.into(), "blank", Instant::now(), &options, cx)?;
                    let mut loaded = false;
                    while let Ok(command) = receiver.recv().await {
                        match command.as_str() {
                            #[cfg(feature = "memory-profile")]
                            "memory" => {
                                window.update(cx, |view, _, _| view.memory_checkpoint("settled"))?
                            }
                            #[cfg(feature = "memory-profile")]
                            "trim" => window.update(cx, |view, _, _| {
                                unsafe { libc::malloc_trim(0) };
                                view.memory_checkpoint("after_malloc_trim");
                            })?,
                            #[cfg(feature = "memory-profile")]
                            "clear" | "reset" => {
                                let reset = command == "reset";
                                window.update(cx, |view, window, cx| {
                                    if reset {
                                        *view = DocumentView::new(window);
                                    }
                                    view.load(blank_html(), &options)?;
                                    cx.notify();
                                    Ok::<_, anyhow::Error>(())
                                })??;
                                snapshot(
                                    window.into(),
                                    if reset { "reset" } else { "clear" },
                                    Instant::now(),
                                    &options,
                                    cx,
                                )?;
                            }
                            "load" if !loaded => {
                                #[cfg(feature = "memory-profile")]
                                memory_probe::reset_peak();
                                let began = Instant::now();
                                let source = std::fs::read_to_string(&options.fixture)?;
                                window.update(cx, |view, _, cx| {
                                    view.load(&source, &options)?;
                                    cx.notify();
                                    Ok::<_, anyhow::Error>(())
                                })??;
                                snapshot(window.into(), "document", began, &options, cx)?;
                                loaded = true;
                            }
                            "quit" => break,
                            _ => bail!("unexpected command: {command}"),
                        }
                    }
                    Ok::<_, anyhow::Error>(())
                }
                .await;
                if let Err(error) = result {
                    emit(json!({"event":"error", "message":error.to_string()}));
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
            Ok(())
        })();
        if let Err(error) = result {
            emit(json!({"event":"error", "message":error.to_string()}));
            cx.quit();
        }
    });
    Ok(())
}
