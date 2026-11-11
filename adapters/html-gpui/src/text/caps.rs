//! Synthetic caps retain logical source indices, including case expansions.
use super::*;
use unicode_categories::UnicodeCategories;
use html_text::{append_css_uppercase, greek_uppercase_mapping};

const SMALL_CAPS_SCALE: f32 = 0.7;

impl GpuiTextCache {
    fn caps_font(&mut self, font: &Font, character: char) -> FontId {
        let key = (self.text_system.resolve_font(font), character);
        if let Some(&font) = self.caps_fonts.get(&key) {
            return font;
        }
        let font = self.range_filtered_font(font, character);
        let primary = self.text_system.resolve_font(&font);
        let selected = if self
            .text_system
            .advance(primary, px(16.0), character)
            .is_ok()
        {
            primary
        } else {
            // Capability belongs to the face that actually supplies the glyph,
            // including implicit native fallback beyond the authored family list.
            let mut utf8 = [0; 4];
            let text = character.encode_utf8(&mut utf8);
            let run = TextRun {
                len: text.len(),
                font,
                ..Default::default()
            };
            let layout = self
                .text_system
                .layout_line_uncached(text, px(16.0), &[run]);
            let selected = layout
                .runs
                .iter()
                .find(|run| !run.glyphs.is_empty())
                .map_or(primary, |run| run.font_id);
            self.text_system.recycle_line_clusters(layout.clusters);
            selected
        };
        if self.caps_fonts.len() == 1024 {
            self.caps_fonts.clear();
        }
        self.caps_fonts.insert(key, selected);
        selected
    }

    fn has_native_caps(&mut self, font: &Font, character: char, tag: [u8; 4]) -> bool {
        let font = self.caps_font(font, character);
        let key = (font, tag);
        if let Some(&supported) = self.native_caps.get(&key) {
            return supported;
        }
        let supported = self.text_system.font_has_substitution_feature(font, tag);
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
    ) -> Option<(ShapedTextRun, GpuiTextRun)> {
        let source_bytes = CharacterBytes::new(text);
        let mut display = String::with_capacity(text.len());
        let mut display_styles = Vec::new();
        let mut display_bytes = Vec::with_capacity(source_bytes.len());
        let mut display_characters = Vec::with_capacity(source_bytes.len());
        let mut display_count = 0;
        let mut ascent = 0.0f32;
        let mut descent = 0.0f32;
        for style in styles {
            let prepared = self.prepared_font(&style.style);
            let id = self.text_system.resolve_font(&prepared.font);
            ascent = ascent.max(f32::from(
                self.text_system.ascent(id, px(style.style.font_size)),
            ));
            descent = descent.max(-f32::from(
                self.text_system.descent(id, px(style.style.font_size)),
            ));
            let source = &text[style.byte_range()];
            let greek_uppercase = style
                .style
                .language
                .and_then(language::content_language_tag)
                .is_some_and(|tag| tag.primary_language() == "el");
            let mut uppercase = None;
            let mut group_start = display.len();
            let mut group_size = style.style.font_size;
            let mut previous_small = false;
            for (index, character) in source.chars().enumerate() {
                display_bytes.push(display.len());
                display_characters.push(display_count);
                let caps = style.style.caps_synthesis;
                let tag = if character.is_lowercase() {
                    Some(*b"smcp")
                } else if caps == CapsSynthesis::AllSmallCaps && character.is_uppercase() {
                    Some(*b"c2sc")
                } else {
                    None
                };
                let font = if is_cjk_character(character) {
                    prepared.cjk_font.as_ref().unwrap_or(&prepared.font)
                } else {
                    &prepared.font
                };
                let small = caps != CapsSynthesis::None && tag.is_some_and(|tag| !self.has_native_caps(font, character, tag))
                    // Keep marks with the preceding synthesized base glyph.
                    || (caps != CapsSynthesis::None && previous_small && character.is_mark());
                // Resolve synthesized sizes on the CSS 1/64px grid before
                // shaping, just as a separately sized inline font would be.
                let size = if small {
                    (style.style.font_size * SMALL_CAPS_SCALE * 64.0).floor() / 64.0
                } else {
                    style.style.font_size
                };
                if size != group_size && display.len() > group_start {
                    let mut properties = style.style;
                    properties.caps_synthesis = CapsSynthesis::None;
                    properties.font_size = group_size;
                    let display_style = TextStyleSpan::new(group_start..display.len(), properties);
                    display_styles.push(display_style);
                    group_start = display.len();
                }
                group_size = size;
                if small {
                    let mapping = if greek_uppercase {
                        uppercase
                            .get_or_insert_with(|| greek_uppercase_mapping(source))
                            .as_ref()
                    } else {
                        None
                    };
                    let mut append = |upper| {
                        display.push(upper);
                        display_count += 1;
                    };
                    if let Some(mapping) = mapping {
                        mapping[index].iter().copied().for_each(append);
                    } else {
                        append_css_uppercase(character, &mut append);
                    }
                } else {
                    display.push(character);
                    display_count += 1;
                }
                previous_small = small;
            }
            if display.len() > group_start {
                let mut properties = style.style;
                properties.caps_synthesis = CapsSynthesis::None;
                properties.font_size = group_size;
                let display_style = TextStyleSpan::new(group_start..display.len(), properties);
                display_styles.push(display_style);
            }
        }
        display_bytes.push(display.len());
        display_characters.push(display_count);
        self.ensure_text_fonts(&display);
        let Some((line, native)) = self.shape_line_geometry(&display, &display_styles) else {
            return None;
        };
        // Preserve the full-size primary strut and include the actual fallback
        // faces at their shaped sizes, including the independent CJK font.
        ascent = ascent.max(f32::from(native.ascent));
        descent = descent.max(f32::from(native.descent).abs());
        let mut caret_cursor = native.caret_stops.cursor();
        let Some(glyphs) = (0..native.glyphs.len())
            .map(|index| {
                let mut glyph = native.native_glyph(index, &mut caret_cursor)?;
                let character = display_bytes
                    .partition_point(|byte| *byte <= glyph.index)
                    .saturating_sub(1);
                glyph.index = source_bytes.get(character)?;
                Some(glyph)
            })
            .collect::<Option<Vec<_>>>() else {
            return None;
        };
        let fonts = native
            .metadata
            .font_spans_in_range(0..glyphs.len(), glyphs.len())
            .collect::<FontSpans>();
        let Some(carets) = display_characters
            .iter()
            .map(|index| native.caret_stops.get(*index))
            .collect::<Option<Arc<[f32]>>>()
        else {
            return None;
        };
        let boundaries: Arc<[bool]> = display_characters
            .iter()
            .map(|index| line.cluster_boundaries()[*index])
            .collect();
        let advances = advances_from_carets(&carets, &boundaries);
        let carets = CaretPositions::from_advances(&advances, carets);
        let geometry = ShapedTextRun::with_carets(advances, boundaries, carets.clone(), ascent);
        let paint_bounds = GpuiTextRun::native_paint_bounds(&glyphs, &fonts, &self.text_system);
        let mut native = GpuiTextRun::new(
            glyphs,
            fonts,
            px(ascent),
            px(descent),
            source_bytes,
            carets,
        );
        native.paint_bounds = paint_bounds;
        Some((geometry, native))
    }
}
