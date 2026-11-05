//! Per-reader CSS aliases over private platform font families.
use html_engine_model::shaping_contracts;
use gpui::{Font, FontFallbacks, FontStyle, FontVariations, FontWeight, WindowTextSystem, font};
use html::style::ReaderStyleOverrides;
use html::text::FontFace;
use html_style_model::FontFaceStyle;
use html_style_model::FontSynthesisStyle;
use std::ops::RangeInclusive;
use html::text::FontSlant;
use html_fonts::{LoadedFontFace, face_key};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

/// A Latin reader override must still load publisher faces when CJK uses Book
/// font. Other roles can also retain a face replaced in only one CSS family.
pub fn publication_faces_for_overrides<'a>(
    faces: &'a [FontFace],
    overrides: &ReaderStyleOverrides,
) -> &'a [FontFace] {
    if overrides.uses_publication_fonts() {
        faces
    } else {
        &[]
    }
}

/// An active publication declaration paired with its registry key.
struct ActiveFace {
    face: FontFace,
    key: String,
}

/// Publication font selection owned by one text backend. Mutations report
/// whether shaped results may change so the owner can invalidate its caches.
#[derive(Default)]
pub(crate) struct PublicationFonts {
    active: Vec<ActiveFace>,
    loaded: HashMap<String, LoadedFontFace>,
    registered: HashSet<String>,
    native_widths: HashMap<String, RangeInclusive<f32>>,
    reader_font_lists: Vec<String>,
    global_reader_font: bool,
    cjk_reader_font: bool,
}

impl PublicationFonts {
    /// Returns whether the reader font policy changed.
    pub(crate) fn set_reader_overrides(&mut self, overrides: &ReaderStyleOverrides) -> bool {
        let lists = [
            overrides.font_family.as_deref(),
            overrides.serif_font_family.as_deref(),
            overrides.sans_serif_font_family.as_deref(),
            overrides.monospace_font_family.as_deref(),
        ];
        let global = overrides.font_family.is_some();
        let cjk_reader_font = overrides.cjk_font_family.is_some();
        if self.global_reader_font == global
            && self.cjk_reader_font == cjk_reader_font
            && self
                .reader_font_lists
                .iter()
                .map(String::as_str)
                .eq(lists.into_iter().flatten())
        {
            return false;
        }
        self.reader_font_lists.clear();
        self.reader_font_lists
            .extend(lists.into_iter().flatten().map(str::to_owned));
        self.global_reader_font = global;
        self.cjk_reader_font = cjk_reader_font;
        true
    }

    /// Reader override values are retained verbatim in computed font-family.
    /// Match the whole list so replacing serif leaves a publisher's sans-serif
    /// or monospace face available even when it shares the same family alias.
    pub(crate) fn allows_publication_family_list(&self, family_list: Option<&str>) -> bool {
        !self.global_reader_font && family_list.is_none_or(|list| !self.reader_font_lists.iter().any(|selected| selected.trim().eq_ignore_ascii_case(list.trim())))
    }

    pub(crate) fn allows_cjk_publication_fonts(&self) -> bool {
        !self.cjk_reader_font
    }

    /// Installed faces have an unrestricted CSS range, even if their cmap
    /// omits a glyph. Publication faces retain their authored unicode-range.
    pub(crate) fn has_restricted_unicode_range(&self, registered: &str) -> bool {
        self.active.iter().any(|ActiveFace { face, key }| {
            face.unicode_ranges != [0..=0x10ffff]
                && self.loaded.get(key).is_some_and(|loaded| loaded.family == registered)
        })
    }

    pub(crate) fn unicode_range_allows(&self, registered: &str, character: char) -> bool {
        let mut matched = false;
        for ActiveFace { face, key } in &self.active {
            if self.loaded.get(key).is_some_and(|loaded| loaded.family == registered) {
                matched = true;
                if face.unicode_ranges.iter().any(|range| range.contains(&u32::from(character))) {
                    return true;
                }
            }
        }
        !matched
    }

    /// Returns whether the active declarations changed.
    pub(crate) fn set_active(&mut self, faces: &[FontFace]) -> bool {
        if self.active.len() == faces.len()
            && self
                .active
                .iter()
                .zip(faces)
                .all(|(active, face)| active.face == *face)
        {
            return false;
        }
        self.active.clear();
        self.active.extend(faces.iter().map(|face| ActiveFace {
            key: face_key(face),
            face: face.clone(),
        }));
        true
    }

    /// Returns whether a new face was loaded.
    pub(crate) fn register(
        &mut self,
        loaded: LoadedFontFace,
        text_system: &WindowTextSystem,
    ) -> Result<bool, String> {
        let key = face_key(&loaded.declaration);
        if self.loaded.contains_key(&key) {
            return Ok(false);
        }
        if let Some(bytes) = &loaded.bytes
            && !self.registered.contains(&loaded.family)
        {
            text_system.add_fonts(vec![Cow::Owned(bytes.to_vec())]).map_err(|error| error.to_string())?;
            self.registered.insert(loaded.family.clone());
        }
        if let Some(range) = loaded.stretch_range() { self.native_widths.insert(loaded.family.clone(), range); }
        self.loaded.insert(key, loaded);
        Ok(true)
    }

    pub(crate) fn resolve(
        &self,
        family: &str,
        weight: u16,
        slant: FontSlant,
        stretch: f32,
        synthesis: FontSynthesisStyle,
    ) -> Option<Font> {
        let mut candidates = self
            .active
            .iter()
            .enumerate()
            .filter(|(_, active)| active.face.family.eq_ignore_ascii_case(family))
            .filter_map(|(index, ActiveFace { face, key })| {
                self.loaded.get(key).map(|loaded| (index, face, loaded))
            })
            .collect::<Vec<_>>();
        let width_range = |face: &FontFace, loaded: &LoadedFontFace| {
            if face.stretch_explicit { face.stretch.clone() }
            else { self.native_widths.get(&loaded.family).cloned().unwrap_or(0.0..=f32::MAX) }
        };
        let rank = |face: &FontFace, loaded: &LoadedFontFace| stretch_rank(&width_range(face, loaded), stretch);
        let style_rank = |style: &FontFaceStyle| {
            if synthesis == FontSynthesisStyle::ObliqueOnly && slant == FontSlant::Italic {
                match style { FontFaceStyle::Italic => 0, FontFaceStyle::Normal => 1, _ => 2 }
            } else { style_rank(style, slant) }
        };
        candidates.sort_by(|(a_index, a, a_loaded), (b_index, b, b_loaded)| {
            rank(a, a_loaded)
                .partial_cmp(&rank(b, b_loaded))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| style_rank(&a.style).cmp(&style_rank(&b.style)))
                .then_with(|| {
                    oblique_distance(&a.style, slant).total_cmp(&oblique_distance(&b.style, slant))
                })
                .then_with(|| {
                    weight_rank(a, weight)
                        .partial_cmp(&weight_rank(b, weight))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| b_index.cmp(a_index))
        });
        let (_, selected, loaded) = *candidates.first()?;
        let mut resolved = font(loaded.family.clone());
        resolved.weight = FontWeight(if loaded.variable {
            f32::from(weight).clamp(*selected.weight.start(), *selected.weight.end())
        } else {
            f32::from(loaded.weight)
        });
        let (effective_style, angle) = if selected.style_explicit {
            match &selected.style {
                FontFaceStyle::Oblique(range) => {
                    let angle = match slant {
                        FontSlant::Normal => 0.0,
                        FontSlant::Italic => 14.0,
                        FontSlant::Oblique { angle } => shaping_contracts::oblique_angle_degrees(angle),
                    };
                    (
                        FontStyle::Oblique,
                        Some(angle.clamp(*range.start(), *range.end())),
                    )
                }
                FontFaceStyle::Italic => (FontStyle::Italic, None),
                FontFaceStyle::Normal => native_slant(slant),
            }
        } else {
            native_slant(slant)
        };
        resolved.style = effective_style;
        let width = width_range(selected, loaded);
        resolved.variations = FontVariations::new(stretch.clamp(*width.start(), *width.end()), angle)
            .unwrap_or_default()
            .with_slant_axis(!(selected.style_explicit && selected.style == FontFaceStyle::Normal))
            .with_synthesis(synthesis != FontSynthesisStyle::None
                && (!selected.style_explicit || selected.style == FontFaceStyle::Normal)
                && !(slant == FontSlant::Italic && synthesis == FontSynthesisStyle::ObliqueOnly));
        let fallback = candidates
            .iter()
            .skip(1)
            .map(|(_, _, loaded)| loaded.family.clone())
            .collect::<Vec<_>>();
        if !fallback.is_empty() {
            resolved.fallbacks = Some(FontFallbacks::from_fonts(fallback));
        }
        Some(resolved)
    }
}

// CSS width matching searches narrower first at <=100%, wider first above it.
fn stretch_rank(stretch: &RangeInclusive<f32>, requested: f32) -> (u8, f32) {
    if stretch.contains(&requested) {
        return (0, 0.0);
    }
    let (start, end) = (*stretch.start(), *stretch.end());
    if requested <= 100.0 {
        if end < requested {
            (1, requested - end)
        } else {
            (2, start - requested)
        }
    } else if start > requested {
        (1, start - requested)
    } else {
        (2, requested - end)
    }
}

/// Translate the engine's resolved posture at the native font boundary.
pub(crate) fn native_slant(slant: FontSlant) -> (FontStyle, Option<f32>) {
    match slant {
        FontSlant::Normal => (FontStyle::Normal, None),
        FontSlant::Italic => (FontStyle::Italic, None),
        FontSlant::Oblique { angle } => (FontStyle::Oblique, Some(shaping_contracts::oblique_angle_degrees(angle))),
    }
}

fn oblique_distance(style: &FontFaceStyle, slant: FontSlant) -> f32 {
    if let (FontSlant::Oblique { angle }, FontFaceStyle::Oblique(range)) = (slant, style) {
        let angle = shaping_contracts::oblique_angle_degrees(angle);
        (angle - angle.clamp(*range.start(), *range.end())).abs()
    } else {
        0.0
    }
}

fn style_rank(style: &FontFaceStyle, requested: FontSlant) -> u8 {
    match (requested, style) {
        (FontSlant::Normal, FontFaceStyle::Normal)
        | (FontSlant::Italic, FontFaceStyle::Italic)
        | (FontSlant::Oblique { .. }, FontFaceStyle::Oblique(_)) => 0,
        (FontSlant::Normal | FontSlant::Italic, FontFaceStyle::Oblique(_))
        | (FontSlant::Oblique { .. }, FontFaceStyle::Normal) => 1,
        _ => 2,
    }
}

// CSS Fonts weight matching prefers 500 over lighter faces for a 400 request.
fn weight_rank(face: &FontFace, requested: u16) -> (u8, f32) {
    let target = f32::from(requested);
    let (start, end) = (*face.weight.start(), *face.weight.end());
    if face.weight.contains(&target) {
        return (0, 0.0);
    }
    if (400.0..=500.0).contains(&target) {
        if start > target && start <= 500.0 {
            (1, start - target)
        } else if end < target {
            (2, target - end)
        } else {
            (3, start - target)
        }
    } else if target < 400.0 {
        if end < target { (1, target - end) } else { (2, start - target) }
    } else if start > target {
        (1, start - target)
    } else {
        (2, target - end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(name: &str, weight: u16) -> FontFace {
        FontFace { family: name.into(), sources: vec![], weight: f32::from(weight)..=f32::from(weight), style: FontFaceStyle::Normal, stretch: 100.0..=100.0, unicode_ranges: vec![0..=0x10ffff], style_explicit: false, stretch_explicit: false, additional_descriptors: vec![] }
    }

    #[test]
    fn global_reader_fonts_disable_all_publication_faces() {
        let faces = vec![face("Publisher", 400), face("Chosen Reader", 700)];
        assert_eq!(publication_faces_for_overrides(&faces, &ReaderStyleOverrides::default()), faces.as_slice());
        let overrides = ReaderStyleOverrides { font_family: Some("Chosen Reader, serif".into()), ..Default::default() };
        assert!(publication_faces_for_overrides(&faces, &overrides).is_empty());
    }

    #[test]
    fn publisher_cjk_faces_remain_available_with_a_latin_reader_font() {
        let faces = vec![face("Publisher", 400)];
        let overrides = ReaderStyleOverrides { font_family: Some("Latin".into()), cjk_font_fallback: Some("Embedded".into()), ..Default::default() };
        assert_eq!(publication_faces_for_overrides(&faces, &overrides), faces.as_slice());
        assert!(publication_faces_for_overrides(&faces, &ReaderStyleOverrides { cjk_font_family: Some("Selected CJK".into()), ..overrides }).is_empty());
    }

    #[test]
    fn role_overrides_are_scoped_to_the_replaced_font_lists() {
        let faces = vec![face("Reader Serif", 400), face("Publisher", 400)];
        let overrides = ReaderStyleOverrides { serif_font_family: Some("Reader Serif, serif".into()), ..Default::default() };
        assert_eq!(publication_faces_for_overrides(&faces, &overrides), faces.as_slice());
        let mut registry = PublicationFonts::default();
        registry.set_reader_overrides(&overrides);
        assert!(!registry.allows_publication_family_list(Some("Reader Serif, serif")));
        assert!(registry.allows_publication_family_list(Some("Reader Serif, sans-serif")));
        assert!(registry.allows_publication_family_list(Some("Publisher, monospace")));
        registry.set_reader_overrides(&ReaderStyleOverrides::default());
        assert!(registry.allows_publication_family_list(Some("Reader Serif, serif")));
    }

    #[test]
    fn aliases_are_document_scoped_and_match_declared_weights() {
        let regular = face("Publisher", 400);
        let bold = face("Publisher", 700);
        let mut registry = PublicationFonts::default();
        for (declaration, family) in [(regular.clone(), "PrivateRegular"), (bold.clone(), "PrivateBold")] {
            registry.loaded.insert(face_key(&declaration), LoadedFontFace { declaration, family: family.into(), bytes: None, weight: 400, slant: FontSlant::Normal, variable: false });
        }
        registry.set_active(&[regular, bold]);
        assert_eq!(
            registry
                .resolve(
                    "publisher",
                    400,
                    FontSlant::Normal,
                    100.0,
                    FontSynthesisStyle::Auto
                )
                .unwrap()
                .family
                .as_ref(),
            "PrivateRegular"
        );
        assert_eq!(
            registry
                .resolve(
                    "Publisher",
                    700,
                    FontSlant::Normal,
                    100.0,
                    FontSynthesisStyle::Auto
                )
                .unwrap()
                .family
                .as_ref(),
            "PrivateBold"
        );
        registry.set_active(&[]);
        assert!(
            registry
                .resolve(
                    "Publisher",
                    400,
                    FontSlant::Normal,
                    100.0,
                    FontSynthesisStyle::Auto
                )
                .is_none()
        );
    }

    #[test]
    fn css_weight_search_order_handles_the_400_to_500_interval() {
        assert!(weight_rank(&face("A", 500), 400) < weight_rank(&face("A", 300), 400));
        assert!(weight_rank(&face("A", 300), 400) < weight_rank(&face("A", 600), 400));
        assert!(weight_rank(&face("A", 800), 700) < weight_rank(&face("A", 600), 700));
    }
}
