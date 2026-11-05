use html_pipeline::engine::box_queries;
use html_dom as dom_values;
use html_engine_model::layout_state;
use html_pipeline::engine::geometry_queries;
use html_render_core as render_core;
use html_resources::resources::images as image_resources;
use html_pipeline::engine::fragment_paint;
use html_pipeline::engine::text_paint;
use crate::common::Options;
use anyhow::{Context as _, Result};
use data_url::DataUrl;
use gpui::{ContentMask, IntoElement, Window, canvas, div, prelude::*, px, rgb};
use html::document::ImageId;
use html_pipeline::MarkupSyntax;
use html::layout::BoxId;
use html::render::DocumentPainter;
use html_pipeline::engine::LayoutConstraints;
use html_pipeline::{
    MediaEnvironment, PipelineChanges, PipelineInputs, PipelineSession, StyleEnvironment,
};
use html::resources::{FileSystemProvider, ResourceProvider};
use html_resources::ImagePipeline;
use html_gpui::backend::{GpuiCommandPainter, GpuiScene, GpuiTextCache, ImageCache};
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
    engine: PipelineSession,
    shaper: GpuiTextCache,
    provider: Arc<dyn ResourceProvider>,
    decoded: Option<ImagePipeline>,
    images: ImageCache,
    scene: Option<GpuiScene>,
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
            engine: PipelineSession::new(provider.clone()),
            shaper: GpuiTextCache::new(window.text_system().clone()),
            provider,
            decoded: None,
            images: ImageCache::default(),
            scene: None,
            content_height: 0.0,
            image_count: 0,
            paint_all: false,
            default_font_family: None,
            markup_syntax: MarkupSyntax::Html,
        }
    }

    pub(super) fn load(&mut self, source: &str, options: &Options) -> Result<()> {
        let base_uri = options.fixture.to_string_lossy().into_owned();
        let mut inputs = PipelineInputs::new(
            source.into(),
            self.markup_syntax,
            base_uri,
            StyleEnvironment {
                root_font_size: Default::default(),
                media: MediaEnvironment::screen(
                    f64::from(options.width),
                    Some(f64::from(options.height)),
                )
                .context("invalid media dimensions")?,
            },
            LayoutConstraints::new(f64::from(options.width), 16.0)?
                // Screen media keeps the window height above. An unbounded
                // flow height disables page fragmentation for native embedding.
                .with_viewport_height(
                    (!options.native_window).then_some(f64::from(options.height)),
                )?,
        );
        inputs.user_styles = self
            .default_font_family
            .as_ref()
            .map(|family| format!(":where(html) {{ font-family: {family:?}; }}"))
            .into_iter()
            .collect();
        self.engine.attach(&mut self.shaper).update(inputs)?;

        let resources =
            text_paint::render_view_images(&self.engine.render_view().context("missing document")?);
        let desired = (0..resources.len() as u32)
            .map(dom_values::image_id_new)
            .collect::<HashSet<_>>();
        let mut decoded = ImagePipeline::new(self.provider.clone());
        decoded.ensure_window(resources, &desired);
        decoded.poll(resources);
        let mut image_metrics = self
            .engine
            .inputs()
            .context("missing inputs")?
            .image_metrics
            .clone();
        for &index in &desired {
            let image = image_resources::decoded_images_get(decoded.decoded_images(), index)
                .with_context(|| format!("image {} did not decode", index.raw()))?;
            let (width, height) = image.dimensions();
            image_metrics.set(index.raw(), width, height);
        }
        if !desired.is_empty() {
            self.engine
                .attach(&mut self.shaper)
                .apply_changes(PipelineChanges {
                    image_metrics: Some(image_metrics),
                    ..Default::default()
                })?;
        }
        let document = self
            .engine
            .render_view()
            .context("missing laid-out document")?;
        // Keep native text and decorations in the engine's shared CSS paint
        // order. Painting all text after all backgrounds lets text cover
        // positioned descendants that should paint above it.
        self.content_height = document
            .text()
            .lines()
            .iter()
            .filter_map(|line| {
                geometry_queries::render_geometry_view_line_vertical_bounds(
                    geometry_queries::render_view_geometry(document),
                    line.id(),
                )
            })
            .map(|bounds| bounds.end)
            .chain(
                fragment_paint::decorations(document.fragments())
                    .iter()
                    .map(|decoration| decoration.rect().y1),
            )
            .fold(0.0_f64, f64::max);
        let height = if self.paint_all {
            self.content_height.max(f64::from(options.height))
        } else {
            f64::from(options.height)
        };
        let mut painter = GpuiCommandPainter::new(&mut self.images, self.shaper.resources());
        render_core::document_painter_paint_region(
            &DocumentPainter::new(document).with_images(decoded.decoded_images()),
            Rect::new(0.0, 0.0, f64::from(options.width), height),
            Point::ORIGIN,
            &mut painter,
        );
        let scene = painter.finish();
        self.image_count = scene.image_count();
        self.scene = Some(scene);
        self.decoded = Some(decoded);

        Ok(())
    }
}

impl DocumentView {
    pub(super) fn element(&self) -> impl IntoElement {
        self.element_at(0.0)
    }

    pub(super) fn element_at(&self, scroll_y: f32) -> impl IntoElement {
        let scene = self.scene.clone();
        div().size_full().bg(rgb(0xffffff)).overflow_hidden().child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        let mut shifted = bounds;
                        shifted.origin.y -= px(scroll_y);
                        if let Some(scene) = &scene {
                            scene.paint(shifted, window, cx);
                        }
                    });
                },
            )
            .size_full(),
        )
    }

    pub(super) fn inspect(&self, full_layout: bool) -> serde_json::Value {
        let view = self.engine.render_view().expect("loaded document");
        let boxes = view.boxes();
        let entries: Vec<_> = (0..if full_layout { box_queries::len(boxes) } else { box_queries::len(boxes).min(45) })
            .map(|raw_index| {
                let index = layout_state::box_id_new(raw_index);
                let format = box_queries::text_format(boxes, index);
                let point = box_queries::render_box_view_point(boxes, index);
                let size = box_queries::render_box_view_size(boxes, index);
                let lines = box_queries::content_line_range(boxes, index);
                serde_json::json!({"index":raw_index,"tag":box_queries::tag(boxes, index),
                "class":box_queries::attribute(boxes, index,"class"),"parent":box_queries::parent(boxes, index).map(layout_state::box_id_index),
                "x":point.map(|p|p.x),"y":point.map(|p|p.y),
                "width":size.map(|s|s.width),"height":size.map(|s|s.height),
                "lines":[lines.start.index(), lines.end.index()],
                "table":box_queries::is_table(boxes, index),"font_size":format.font_size,
                "font_weight":format.font_weight,"font_style":format!("{:?}",format.font_style),
                "font_family":format.font_family})
            })
            .collect();
        let text = view.text();
        let lines = full_layout.then(|| text.lines().iter().map(|line| {
            let value = text_paint::render_line_glyphs(&line).filter_map(|index| text_paint::render_text_view_character_at(text, index as usize)).collect::<String>();
            let advances = text_paint::render_line_glyphs(&line).map(|index| text_paint::character_advance(text, index)
                .or_else(|| text_paint::glyph_at(text, index as usize).and_then(|glyph| text_paint::glyph_metric(text, glyph)).map(|metric| metric.advance())))
                .collect::<Vec<_>>();
            let font_extents = text_paint::render_line_glyphs(&line).map(|index| text_paint::glyph_at(text, index as usize)
                .and_then(|glyph| text_paint::glyph_metric(text, glyph))
                .map(|metric| [metric.ascent(), metric.descent()]))
                .collect::<Vec<_>>();
            serde_json::json!({"index":line.id().index(),"owner":line.owner_box().index(),
                "x":line.point().x,"y":line.point().y,"height":line.height(),"baseline":text_paint::line_baseline(&line),
                "text":value,"advances":advances,"font_extents":font_extents,"hyphen":text_paint::hyphen_for_line(text, line.id()).is_some()})
        }).collect::<Vec<_>>());
        serde_json::json!({"boxes":entries,"line_count":text.lines().len(),"lines":lines})
    }
}
