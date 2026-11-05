//! Basic Latin text and rectangles, using HtmlEngine and GPUI directly.
//! Shapes afresh each frame; images and native text-run shaping are omitted.

use html_engine_model::shaping_contracts;
use gpui::{
    App, Bounds, Context, Font, FontStyle, FontVariations, FontWeight, PaintGlyph, Pixels, Point,
    ShapedLine, TextRun, Window, WindowOptions, WindowTextSystem, canvas, div, fill, point,
    prelude::*, px, rgb, rgba, size,
};
use html::render::{Painter, SceneImagePolicy};
use html::{Document, DocumentOptions};
use html::text::FontMetrics;
use html::text::FontSlant;
use html::text::GlyphHandle;
use html::text::GlyphRequest;
use html_engine_model::GlyphResources;
use html_engine_model::GlyphWriter;
use html_engine_model::ResourceShaper;
use html::text::ShapeError;
use html::text::ShapedGlyph;
use kurbo::{Point as HtmlPoint, Rect};
use peniko::{Color, Image};

fn shape_glyph(
    system: &WindowTextSystem,
    request: GlyphRequest<'_>,
    glyphs: &mut GlyphWriter<'_, ShapedLine>,
) -> Result<ShapedGlyph, ShapeError> {
    let (style, angle) = match request.style.font_slant {
        FontSlant::Normal => (FontStyle::Normal, None),
        FontSlant::Italic => (FontStyle::Italic, None),
        FontSlant::Oblique { angle } => (FontStyle::Oblique, Some(shaping_contracts::oblique_angle_degrees(angle))),
    };
    let font = Font {
        weight: FontWeight(f32::from(request.style.font_weight)),
        style,
        variations: FontVariations::new(request.style.font_stretch, angle).unwrap_or_default(),
        ..Font::default() // System font for this minimal adapter.
    };
    let text = request.character.to_string();
    let run = TextRun {
        len: text.len(),
        font,
        ..Default::default()
    };
    let line = if request.style.font_size == 0.0 || request.character.is_control() {
        ShapedLine::default()
    } else {
        system.shape_line(text.into(), px(request.style.font_size), &[run], None)
    };
    let metrics = FontMetrics {
        ascent: f32::from(line.ascent),
        descent: f32::from(line.descent),
    };
    let advance = f32::from(line.width());
    let handle = glyphs.insert(line)?;
    Ok(ShapedGlyph::new(handle, advance, metrics))
}

struct GpuiPainter<'a> {
    glyphs: &'a GlyphResources<ShapedLine>,
    origin: Point<Pixels>,
    window: &'a mut Window,
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

    fn draw_glyph(&mut self, glyph: GlyphHandle, origin: HtmlPoint, color: u32) {
        let line = self.glyphs.get(glyph).expect("native glyph handle");
        let origin = self.origin + point(px(origin.x as f32), px(origin.y as f32));
        let color = rgba(color).into();
        let glyphs = line.runs.iter().flat_map(|run| {
            run.glyphs.iter().map(move |glyph| PaintGlyph {
                origin: origin + glyph.position,
                font_id: run.font_id,
                glyph_id: glyph.id,
                font_size: line.font_size,
                color,
                is_emoji: glyph.is_emoji,
            })
        });
        self.window.paint_glyphs(glyphs).expect("paint glyph");
    }

    fn draw_image(&mut self, _: &Image, _: &[u8], _: Rect) {}
}

struct Example;

impl Render for Example {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_4().bg(rgb(0xffffff)).child(
            canvas(
                |bounds, window, _| {
                    let mut shaper = ResourceShaper::new(|request, glyphs| {
                        shape_glyph(window.text_system(), request, glyphs)
                    });
                    let options =
                        DocumentOptions::new(f64::from(f32::from(bounds.size.width)).max(1.0))
                            .expect("valid width");
                    let scene = Document::from_source(
                        "<p>Hello <strong>HtmlEngine</strong>!</p>".to_owned(),
                        options,
                        &mut shaper,
                    )
                    .expect("render HTML")
                    .scene(SceneImagePolicy::Omit)
                    .expect("positive width");
                    (scene, shaper.into_resources())
                },
                |bounds, (scene, glyphs), window, _| {
                    scene.paint(&mut GpuiPainter {
                        glyphs: &glyphs,
                        origin: bounds.origin,
                        window,
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
