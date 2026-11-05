//! Untimed geometry snapshots for checking precision changes between builds.

use html_engine_model::layout_state;
use html_pipeline::engine::GlyphResourceStore;
use html_pipeline::engine::fragment_paint;
use html_pipeline::engine::text_paint;

use html_pipeline::engine::LaidOutDocument;
use serde_json::{Value, json};

pub(super) fn snapshot(document: &LaidOutDocument, resources: &GlyphResourceStore) -> Value {
    let view = document.render_view(resources);
    let text = view.text();
    let lines: Vec<_> = text
        .lines()
        .iter()
        .map(|line| {
            let mut offsets = vec![0.0; (line.end() - line.start()) as usize];
            let mut advances: Vec<_> = (line.start()..line.end())
                .map(|index| {
                    f64::from(text_paint::character_advance(text, index).unwrap_or_default())
                })
                .collect();
            if let Some(runs) = text_paint::line_glyph_offsets(text, line.id()) {
                for run in runs.iter() {
                    for index in run.range() {
                        offsets[(index - line.start()) as usize] = run.offset();
                    }
                }
            }
            if let Some(runs) = text_paint::line_glyph_advances(text, line.id()) {
                for run in runs.iter() {
                    for index in run.range() {
                        advances[(index - line.start()) as usize] = run.advance();
                    }
                }
            }
            let fragments: Vec<_> = text_paint::render_text_view_line_text_fragments(text, line.id())
                .into_iter()
                .flatten()
                .map(|fragment| {
                    json!({
                        "glyphs": [fragment.glyphs().start, fragment.glyphs().end],
                        "offset_x": text_paint::fragment_offset_x(&fragment),
                        "paint_order": text_paint::fragment_paint_order(&fragment),
                        "visible": fragment.visible(),
                    })
                })
                .collect();
            let clip = text_paint::line_overflow_clip(text, line.id()).map(|clip| {
                let rect = clip.rect();
                json!({"rect": [rect.x0, rect.y0, rect.x1, rect.y1],
                "axes": [clip.clips_x(), clip.clips_y()]})
            });
            json!({
                "glyphs": [line.start(), line.end()],
                "geometry": [line.point().x, line.point().y, line.height(), text_paint::line_baseline(&line),
                    text_paint::line_word_spacing(&line), text_paint::line_letter_spacing(&line), text_paint::line_optical_offset_x(&line)],
                "offsets": offsets,
                "advances": advances,
                "fragments": fragments,
                "clip": clip,
            })
        })
        .collect();
    let images: Vec<_> = fragment_paint::render_fragment_view_images(view
        .fragments())
        .iter()
        .map(|image| {
            let point = fragment_paint::render_image_fragment_offset(&image);
            let size = fragment_paint::render_image_fragment_size(&image);
            let clip = fragment_paint::render_image_fragment_clip(&image);
            json!({"image_idx": image.image().raw(), layout_state::line_id_index("line_idx": image.line()),
            "geometry": [point.x, point.y, size.width, size.height,
                clip.x0, clip.y0, clip.x1, clip.y1]})
        })
        .collect();
    let decorations: Vec<_> = fragment_paint::decorations(view
        .fragments())
        .iter()
        .map(|decoration| {
            let rect = decoration.rect();
            json!({"rect": [rect.x0, rect.y0, rect.x1, rect.y1],
            "line_idx": fragment_paint::render_decoration_line(&decoration).map(|line| layout_state::line_id_index(line)), "color": fragment_paint::decoration_color(&decoration),
            "paint_order": fragment_paint::decoration_paint_order(&decoration)})
        })
        .collect();
    json!({"lines": lines, "images": images, "decorations": decorations})
}
