//! Untimed geometry snapshots for checking precision changes between builds.

use html::text::GlyphResourceStore;

use html::layout::LaidOutDocument;
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
                .map(|index| f64::from(text.character_advance(index).unwrap_or_default()))
                .collect();
            if let Some(runs) = text.line_glyph_offsets(line.index()) {
                for run in runs.iter() {
                    for index in run.range() {
                        offsets[(index - line.start()) as usize] = run.offset();
                    }
                }
            }
            if let Some(runs) = text.line_glyph_advances(line.index()) {
                for run in runs.iter() {
                    for index in run.range() {
                        advances[(index - line.start()) as usize] = run.advance();
                    }
                }
            }
            let fragments: Vec<_> = text
                .line_text_fragments(line.index())
                .into_iter()
                .flatten()
                .map(|fragment| {
                    json!({
                        "glyphs": [fragment.glyphs().start, fragment.glyphs().end],
                        "offset_x": fragment.offset_x(),
                        "paint_order": fragment.paint_order(),
                        "visible": fragment.visible(),
                    })
                })
                .collect();
            let clip = text.line_overflow_clip(line.index()).map(|clip| {
                let rect = clip.rect();
                json!({"rect": [rect.x0, rect.y0, rect.x1, rect.y1],
                "axes": [clip.clips_x(), clip.clips_y()]})
            });
            json!({
                "glyphs": [line.start(), line.end()],
                "geometry": [line.point().x, line.point().y, line.height(), line.baseline(),
                    line.word_spacing(), line.letter_spacing(), line.optical_offset_x()],
                "offsets": offsets,
                "advances": advances,
                "fragments": fragments,
                "clip": clip,
            })
        })
        .collect();
    let images: Vec<_> = view
        .fragments()
        .images()
        .iter()
        .map(|image| {
            let point = image.offset();
            let size = image.size();
            let clip = image.clip();
            json!({"image_idx": image.image_idx(), "line_idx": image.line_idx(),
            "geometry": [point.x, point.y, size.width, size.height,
                clip.x0, clip.y0, clip.x1, clip.y1]})
        })
        .collect();
    let decorations: Vec<_> = view
        .fragments()
        .decorations()
        .iter()
        .map(|decoration| {
            let rect = decoration.rect();
            json!({"rect": [rect.x0, rect.y0, rect.x1, rect.y1],
            "line_idx": decoration.line_idx(), "color": decoration.color(),
            "paint_order": decoration.paint_order()})
        })
        .collect();
    json!({"lines": lines, "images": images, "decorations": decorations})
}
