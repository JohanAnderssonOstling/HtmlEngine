//! Basic Latin text and rectangles, using HtmlEngine and GPUI directly.
//! Shapes afresh each frame; images and native text-run shaping are omitted.

use gpui::{
    App, Bounds, Context, Font, FontStyle, FontWeight, Pixels, Point, ShapedLine, TextAlign,
    TextRun, Window, WindowOptions, WindowTextSystem, canvas, div, fill, point, prelude::*, px,
    rgb, rgba, size,
};
use html::render::{FragmentImagePolicy, FragmentRenderOptions, FragmentRenderer, Painter};
use html::text::{
    FontSlant, GlyphId, GlyphMetric, GlyphResourceGeneration, GlyphResourceStore, GlyphShaper,
    ShapeError,
};
use kurbo::{Point as HtmlPoint, Rect};
use peniko::{Color, Image};

struct Shaper<'a> {
    system: &'a WindowTextSystem,
    generation: GlyphResourceGeneration,
    glyphs: Vec<ShapedLine>,
}

impl GlyphShaper for Shaper<'_> {
    fn resource_generation(&self) -> GlyphResourceGeneration {
        self.generation
    }

    fn reset(&mut self) {
        self.glyphs.clear();
        self.generation = GlyphResourceGeneration::default();
    }

    fn shape_glyph(
        &mut self,
        ch: char,
        font_size: f32,
        weight: u16,
        slant: FontSlant,
        color: u32,
        _family: Option<&str>,
        resources: &mut GlyphResourceStore,
    ) -> Result<GlyphId, ShapeError> {
        let font = Font {
            weight: FontWeight(f32::from(weight)),
            style: match slant {
                FontSlant::Normal => FontStyle::Normal,
                FontSlant::Italic => FontStyle::Italic,
                FontSlant::Oblique => FontStyle::Oblique,
            },
            ..Font::default() // System font for this minimal adapter.
        };
        let text = ch.to_string();
        let run = TextRun {
            len: text.len(),
            font,
            color: rgba(color).into(),
            ..Default::default()
        };
        let line = if font_size == 0.0 || ch.is_control() {
            ShapedLine::default()
        } else {
            self.system
                .shape_line(text.into(), px(font_size), &[run], None)
        };
        let metric = GlyphMetric::try_new(
            ch,
            f32::from(line.width()),
            f32::from(line.ascent),
            f32::from(line.descent),
            f32::from(line.ascent),
        )
        .map_err(ShapeError::rejected_metric)?;
        let id = resources.register(metric)?;
        debug_assert_eq!(id as usize, self.glyphs.len());
        self.glyphs.push(line);
        Ok(id)
    }
}

struct GpuiPainter<'a> {
    glyphs: &'a [ShapedLine],
    origin: Point<Pixels>,
    window: &'a mut Window,
    cx: &'a mut App,
}

impl Painter for GpuiPainter<'_> {
    fn fill_rect(&mut self, rect: Rect, color: Color) {
        let bounds = Bounds {
            origin: self.origin + point(px(rect.x0 as f32), px(rect.y0 as f32)),
            size: size(px(rect.width() as f32), px(rect.height() as f32)),
        };
        self.window.paint_quad(fill(
            bounds,
            rgba(u32::from_be_bytes([color.r, color.g, color.b, color.a])),
        ));
    }

    fn draw_glyph(&mut self, glyph: GlyphId, origin: HtmlPoint) {
        let line = &self.glyphs[glyph as usize];
        if line.len() == 0 {
            return;
        }
        let origin = self.origin + point(px(origin.x as f32), px(origin.y as f32));
        line.paint(
            origin,
            line.ascent + line.descent,
            TextAlign::Left,
            None,
            self.window,
            self.cx,
        )
        .expect("paint glyph");
    }

    fn draw_image(&mut self, _: &Image, _: &[u8], _: Rect) {}
}

struct Example;

impl Render for Example {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_4().bg(rgb(0xffffff)).child(
            canvas(
                |bounds, window, _| {
                    let mut renderer = FragmentRenderer::new();
                    let mut shaper = Shaper {
                        system: window.text_system(),
                        generation: GlyphResourceGeneration::default(),
                        glyphs: Vec::new(),
                    };
                    let options = FragmentRenderOptions::new(
                        f64::from(f32::from(bounds.size.width)).max(1.0),
                        None,
                    )
                    .expect("valid width")
                    .with_image_policy(FragmentImagePolicy::Omit);
                    let scene = renderer
                        .render(
                            "<p>Hello <strong>HtmlEngine</strong>!</p>",
                            &options,
                            &mut shaper,
                        )
                        .expect("render HTML")
                        .into_scene();
                    (scene, shaper.glyphs)
                },
                |bounds, (scene, glyphs), window, cx| {
                    scene.paint(&mut GpuiPainter {
                        glyphs: &glyphs,
                        origin: bounds.origin,
                        window,
                        cx,
                    });
                },
            )
            .size_full(),
        )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| Example))
            .expect("open window");
        cx.activate(true);
    });
}
