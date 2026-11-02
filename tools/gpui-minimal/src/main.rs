//! Basic Latin text and rectangles, using HtmlEngine and GPUI directly.
//! Shapes afresh each frame; images and native text-run shaping are omitted.

use gpui::{
    App, Bounds, Context, Font, FontStyle, FontWeight, PaintGlyph, Pixels, Point, ShapedLine,
    TextRun, Window, WindowOptions, WindowTextSystem, canvas, div, fill, point, prelude::*, px,
    rgb, rgba, size,
};
use html::render::{FragmentImagePolicy, FragmentRenderOptions, FragmentRenderer, Painter};
use html::text::{
    FontMetrics, FontSlant, GlyphHandle, GlyphRequest, GlyphResources, ResourceShaper, ShapeError,
    ShapedGlyph,
};
use kurbo::{Point as HtmlPoint, Rect};
use peniko::{Color, Image};

fn shape_glyph(
    system: &WindowTextSystem,
    request: GlyphRequest<'_>,
    glyphs: &mut GlyphResources<ShapedLine>,
) -> Result<ShapedGlyph, ShapeError> {
    let font = Font {
        weight: FontWeight(f32::from(request.font_weight)),
        style: match request.font_slant {
            FontSlant::Normal => FontStyle::Normal,
            FontSlant::Italic => FontStyle::Italic,
            FontSlant::Oblique => FontStyle::Oblique,
        },
        ..Font::default() // System font for this minimal adapter.
    };
    let text = request.character.to_string();
    let run = TextRun {
        len: text.len(),
        font,
        ..Default::default()
    };
    let line = if request.font_size == 0.0 || request.character.is_control() {
        ShapedLine::default()
    } else {
        system.shape_line(text.into(), px(request.font_size), &[run], None)
    };
    let metrics = FontMetrics::new(f32::from(line.ascent), f32::from(line.descent))?;
    let advance = f32::from(line.width());
    let handle = glyphs.insert(line)?;
    Ok(ShapedGlyph::new(handle, advance, metrics)?)
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
                    let mut renderer = FragmentRenderer::new();
                    let mut shaper = ResourceShaper::new(|request, glyphs| {
                        shape_glyph(window.text_system(), request, glyphs)
                    });
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
