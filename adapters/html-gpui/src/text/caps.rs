//! Synthetic caps retain logical source indices, including case expansions.
use super::*;
use unicode_categories::UnicodeCategories;

const SMALL_CAPS_SCALE: f32 = 0.7;

impl GpuiTextCache {
    fn has_native_caps(&mut self, font: &Font, character: char, tag: &'static str) -> bool {
        let key = (self.text_system.resolve_font(font), character, tag);
        if let Some(supported) = self.native_caps.get(&key) {
            return *supported;
        }
        let mut bytes = [0; 4];
        let text = character.encode_utf8(&mut bytes);
        let probe = |enabled| {
            let mut font = font.clone();
            let mut features = font.features.0.as_ref().clone();
            features.retain(|(name, _)| name != tag);
            features.push((tag.to_owned(), enabled));
            font.features = gpui::FontFeatures(Arc::new(features));
            self.text_system.layout_line(
                text,
                px(16.0),
                &[TextRun {
                    len: text.len(),
                    font,
                    ..Default::default()
                }],
                None,
            )
        };
        let enabled = probe(1);
        let disabled = probe(0);
        let supported = enabled
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .map(|glyph| glyph.id)
            .ne(disabled
                .runs
                .iter()
                .flat_map(|run| &run.glyphs)
                .map(|glyph| glyph.id));
        if self.native_caps.len() == 1024 {
            self.native_caps.clear();
        }
        self.native_caps.insert(key, supported);
        supported
    }

    pub(super) fn shape_caps_line(
        &mut self,
        text: &str,
        styles: &[TextStyleSpan<'_>],
        placement: Option<&TextShapeRequest<'_>>,
    ) -> Result<Option<(ShapedLine, Arc<[f32]>)>, ShapeError> {
        let source_bytes = CharacterBytes::new(text);
        let mut display = String::with_capacity(text.len());
        let mut display_styles = Vec::new();
        let mut display_bytes = Vec::with_capacity(source_bytes.len());
        let mut display_characters = Vec::with_capacity(source_bytes.len());
        let mut display_count = 0;
        let mut ascent = 0.0f32;
        let mut descent = 0.0f32;
        for style in styles {
            let prepared = self.prepared_font(style);
            let id = self.text_system.resolve_font(&prepared.font);
            ascent = ascent.max(f32::from(
                self.text_system.ascent(id, px(style.font_size())),
            ));
            descent = descent.max(-f32::from(
                self.text_system.descent(id, px(style.font_size())),
            ));
            let mut group_start = display.len();
            let mut group_size = style.font_size();
            let mut previous_small = false;
            for character in text[style.byte_range()].chars() {
                display_bytes.push(display.len());
                display_characters.push(display_count);
                let caps = style.caps_synthesis();
                let tag = if character.is_lowercase() {
                    Some("smcp")
                } else if caps == Some(true) && character.is_uppercase() {
                    Some("c2sc")
                } else {
                    None
                };
                let font = if is_cjk_character(character) {
                    prepared.cjk_font.as_ref().unwrap_or(&prepared.font)
                } else {
                    &prepared.font
                };
                let small = caps.is_some() && tag.is_some_and(|tag| !self.has_native_caps(font, character, tag))
                    // Keep marks with the preceding synthesized base glyph.
                    || (caps.is_some() && previous_small && character.is_mark());
                // Resolve synthesized sizes on the CSS 1/64px grid before
                // shaping, just as a separately sized inline font would be.
                let size = if small {
                    (style.font_size() * SMALL_CAPS_SCALE * 64.0).floor() / 64.0
                } else {
                    style.font_size()
                };
                if size != group_size && display.len() > group_start {
                    let Some(display_style) = style.clone().with_caps_synthesis(None).with_display_range(group_start..display.len(), group_size) else {
                        return Ok(None);
                    };
                    display_styles.push(display_style);
                    group_start = display.len();
                }
                group_size = size;
                if small {
                    for upper in character.to_uppercase() {
                        display.push(upper);
                        display_count += 1;
                    }
                } else {
                    display.push(character);
                    display_count += 1;
                }
                previous_small = small;
            }
            if display.len() > group_start {
                let Some(display_style) = style.clone().with_caps_synthesis(None).with_display_range(group_start..display.len(), group_size) else {
                    return Ok(None);
                };
                display_styles.push(display_style);
            }
        }
        display_bytes.push(display.len());
        display_characters.push(display_count);
        let Some((mut line, _)) = self.shape_line_geometry(&display, &display_styles, None)? else {
            return Ok(None);
        };
        let native = &self.runs[line.run as usize];
        let mut caret_cursor = native.caret_stops.cursor();
        let Some(mut glyphs) = (0..native.glyphs.len())
            .map(|index| {
                let mut glyph = native.native_glyph(index, &mut caret_cursor)?;
                let character = display_bytes
                    .partition_point(|byte| *byte <= glyph.index)
                    .saturating_sub(1);
                glyph.index = source_bytes.get(character)?;
                Some(glyph)
            })
            .collect::<Option<Vec<_>>>() else {
            return Ok(None);
        };
        let mut fonts = native
            .metadata
            .font_spans_in_range(0..glyphs.len(), glyphs.len())
            .collect::<FontSpans>();
        let Some(mut carets) = display_characters.iter().map(|index| line.caret_stops.get(*index)).collect::<Option<Arc<[f32]>>>() else {
            return Ok(None);
        };
        let boundaries: Arc<[bool]> = display_characters
            .iter()
            .map(|index| line.cluster_boundaries[*index])
            .collect();
        let advances = advances_from_carets(&carets, &boundaries);
        if let Some(request) = placement {
            let Some(placed) = apply_line_placements(
                request,
                source_bytes.slice(0..source_bytes.len()),
                &carets,
                &mut glyphs,
                &mut fonts,
            ) else {
                return Ok(None);
            };
            carets = placed;
            line.line_index = request.line_index();
            line.text_range = request.text_range();
        } else {
            line.text_range = 0..(source_bytes.len() - 1) as u32;
        }
        line.ascent = ascent;
        let carets = if placement.is_none() {
            CaretPositions::from_advances(advances.clone(), carets)
        } else {
            carets.into()
        };
        line.caret_stops = carets.clone();
        line.cluster_boundaries = boundaries;
        let colors = styles
            .iter()
            .map(|style| (style.byte_range(), Hsla::from(rgba(style.color()))))
            .collect();
        let paint_bounds = GpuiTextRun::native_paint_bounds(&glyphs, &fonts, &self.text_system);
        let mut native = GpuiTextRun::new(
            glyphs,
            fonts,
            px(ascent),
            px(descent),
            source_bytes,
            colors,
            carets,
        );
        native.paint_bounds = paint_bounds;
        Rc::make_mut(&mut self.runs)[line.run as usize] = Arc::new(native);
        Ok(Some((
            line,
            if placement.is_some() {
                Arc::default()
            } else {
                advances
            },
        )))
    }
}
