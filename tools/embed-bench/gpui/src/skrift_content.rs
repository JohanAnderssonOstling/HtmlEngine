use crate::common::Options;
use anyhow::{Context as _, Result};
use data_url::DataUrl;
use gpui::{ContentMask, IntoElement, Window, canvas, div, prelude::*, px, rgb};
use html::engine::Engine;
use html::{
    document::MarkupSyntax,
    layout::LayoutConstraints,
    pipeline::{
        FontEnvironmentRevision, ImageMetricsRevision, PaintSettingsRevision, PipelineChanges,
        PipelineInputs, ResourceRevision, SourceRevision, StyleEnvironment, StylesheetRevision,
    },
    style::MediaEnvironment,
};
use html::resources::{FileSystemProvider, ImagePipeline, ResourceProvider};
use html_gpui::backend::{
    GlyphStore, GpuiCommandPainter, GpuiTextCache, ImageCache, PaintCommand, paint_commands,
};
use kurbo::{Point, Rect};
use std::collections::HashSet;
use std::io;
use std::sync::Arc;

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

pub(super) struct DocumentView {
    engine: Engine,
    source_revision: SourceRevision,
    shaper: GpuiTextCache,
    provider: Arc<dyn ResourceProvider>,
    decoded: Option<ImagePipeline>,
    images: ImageCache,
    commands: Arc<Vec<PaintCommand>>,
    pub(super) content_height: f64,
    pub(super) image_count: usize,
    pub(super) paint_all: bool,
    pub(super) default_font_family: Option<String>,
    pub(super) markup_syntax: MarkupSyntax,
}

impl DocumentView {
    pub(super) fn new(window: &Window) -> Self {
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
            paint_all: false,
            default_font_family: None,
            markup_syntax: MarkupSyntax::Html,
        }
    }

    pub(super) fn load(&mut self, source: &str, options: &Options) -> Result<()> {

        let settings =
            FragmentRenderOptions::new(f64::from(options.width), Some(f64::from(options.height)))?
                .with_base_uri(options.fixture.to_string_lossy().into_owned())?
                .with_image_policy(FragmentImagePolicy::Reference);
        self.source_revision = self.source_revision.next();
        let inputs = PipelineInputs {
            source: source.into(),
            markup_syntax: self.markup_syntax,
            user_styles: self
                .default_font_family
                .as_ref()
                .map(|family| format!(":where(html) {{ font-family: {family:?}; }}"))
                .into_iter()
                .collect(),
            reader_overrides: Default::default(),
            note_flow: Default::default(),
            source_revision: self.source_revision,
            base_uri: settings.base_uri().to_owned(),
            resource_revision: ResourceRevision::INITIAL,
            stylesheet_revision: StylesheetRevision::INITIAL,
            style_environment: StyleEnvironment {
                root_font_size: Default::default(),
                media: MediaEnvironment::screen(
                    f64::from(options.width),
                    Some(f64::from(options.height)),
                )
                .context("invalid media dimensions")?,
            },
            font_environment: FontEnvironmentRevision::INITIAL,
            image_metrics_revision: ImageMetricsRevision::INITIAL,
            layout: LayoutConstraints::new(f64::from(options.width), 16.0)?
                // Screen media keeps the window height above. An unbounded
                // flow height disables page fragmentation for native embedding.
                .with_viewport_height(
                    (!options.native_window).then_some(f64::from(options.height)),
                )?,
            image_metrics: Default::default(),
            paint: PaintSettingsRevision::INITIAL,
        };
        self.engine.update(inputs, &mut self.shaper)?;

        let resources = self
            .engine
            .render_view()
            .context("missing document")?
            .images();
        let desired = (0..resources.len() as u32).collect::<HashSet<_>>();
        let mut decoded = ImagePipeline::new(self.provider.clone());
        decoded.ensure_window(resources, &desired);
        decoded.poll();
        let mut image_metrics = self
            .engine
            .inputs()
            .context("missing inputs")?
            .image_metrics
            .clone();
        for index in &desired {
            let image = decoded
                .get_decoded(*index)
                .with_context(|| format!("image {index} did not decode"))?;
            let (width, height) = image.dimensions();
            image_metrics.set(*index, width, height);
        }
        if !desired.is_empty() {
            let revision = self
                .engine
                .inputs()
                .context("missing inputs")?
                .image_metrics_revision
                .next();
            self.engine.apply_changes(
                PipelineChanges {
                    image_metrics: Some(image_metrics),
                    image_metrics_revision: Some(revision),
                    ..Default::default()
                },
                &mut self.shaper,
            )?;
        }
        let document = self
            .engine
            .render_view()
            .context("missing laid-out document")?;
        // Keep native text and decorations in the engine's shared CSS paint
        // order. Painting all text after all backgrounds lets text cover
        // positioned descendants that should paint above it.
        self.content_height = document.text().lines().iter()
            .filter_map(|line| document.geometry().line_vertical_bounds(line.index()))
            .map(|bounds| bounds.end)
            .chain(document.fragments().decorations().iter().map(|decoration| decoration.rect().y1))
            .fold(0.0_f64, f64::max);
        let height = if self.paint_all {
            self.content_height.max(f64::from(options.height))
        } else {
            f64::from(options.height)
        };
        let mut painter = GpuiCommandPainter::new(
            &mut self.images,
            self.shaper.store(),
            self.shaper.run_store(),
            Vec::new(),
        );
        DocumentPainter::new(document).with_images(&decoded).paint_region(
            Rect::new(0.0, 0.0, f64::from(options.width), height),
            Point::ORIGIN,
            &mut painter,
        );
        self.commands = Arc::new(painter.into_commands());
        self.image_count = self.commands.iter()
            .filter(|command| matches!(command, PaintCommand::Image { .. }))
            .count();
        self.decoded = Some(decoded);

        Ok(())
    }
}

impl DocumentView {
    pub(super) fn element(&self) -> impl IntoElement {
        self.element_at(0.0)
    }

    pub(super) fn element_at(&self, scroll_y: f32) -> impl IntoElement {
        let commands = self.commands.clone();
        let glyphs: GlyphStore = self.shaper.store();
        div().size_full().bg(rgb(0xffffff)).overflow_hidden().child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        let mut shifted = bounds;
                        shifted.origin.y -= px(scroll_y);
                        paint_commands(&commands, &glyphs, shifted, window, cx);
                    });
                },
            )
            .size_full(),
        )
    }

    pub(super) fn inspect(&self, full_layout: bool) -> serde_json::Value {
        let view = self.engine.render_view().expect("loaded document");
        let boxes = view.boxes();
        let entries: Vec<_> = (0..if full_layout { boxes.len() } else { boxes.len().min(45) })
            .map(|index| {
                let format = boxes.text_format(index);
                let point = boxes.point(index);
                let size = boxes.size(index);
                let lines = boxes.content_line_range(index);
                serde_json::json!({"index":index,"tag":boxes.tag(index),
                "class":boxes.attribute(index,"class"),"parent":boxes.parent(index),
                "x":point.map(|p|p.x),"y":point.map(|p|p.y),
                "width":size.map(|s|s.width),"height":size.map(|s|s.height),
                "lines":[lines.start, lines.end],
                "table":boxes.is_table(index),"font_size":format.font_size,
                "font_weight":format.font_weight,"font_style":format!("{:?}",format.font_style),
                "font_family":format.font_family.and_then(|id|view.style_string(id))})
            })
            .collect();
        let text = view.text();
        let lines = full_layout.then(|| text.lines().iter().map(|line| {
            let value = line.glyphs().filter_map(|index| text.character_at(index as usize)).collect::<String>();
            let advances = line.glyphs().map(|index| text.character_advance(index)
                .or_else(|| text.glyph_at(index as usize).and_then(|glyph| text.glyph_metric(glyph)).map(|metric| metric.advance())))
                .collect::<Vec<_>>();
            let font_extents = line.glyphs().map(|index| text.glyph_at(index as usize)
                .and_then(|glyph| text.glyph_metric(glyph))
                .map(|metric| [metric.ascent(), metric.descent()]))
                .collect::<Vec<_>>();
            serde_json::json!({"index":line.index(),"owner":line.owner_box_idx(),
                "x":line.point_x(),"y":line.point_y(),"height":line.height(),"baseline":line.baseline(),
                "text":value,"advances":advances,"font_extents":font_extents,"hyphen":text.hyphen_for_line(line.index()).is_some()})
        }).collect::<Vec<_>>());
        serde_json::json!({"boxes":entries,"line_count":text.lines().len(),"lines":lines})
    }
}
