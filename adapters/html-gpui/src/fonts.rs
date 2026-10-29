//! Per-reader CSS aliases over private platform font families.
use gpui::{Font, FontFallbacks, FontStyle, FontWeight, WindowTextSystem, font};
use html::pipeline::ReaderStyleOverrides;
use html::style::{FontFace, FontFaceStyle};
use html::text::FontSlant;
use html_fonts::{LoadedFontFace, face_key};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// A Latin reader override must still load publisher faces when CJK uses Book
/// font. Other roles can also retain a face replaced in only one CSS family.
pub fn publication_faces_for_overrides(faces: &[FontFace], overrides: &ReaderStyleOverrides) -> Vec<FontFace> {
    if overrides.font_family.is_some() && !(overrides.cjk_font_family.is_none() && overrides.cjk_font_fallback.is_some()) { Vec::new() } else { faces.to_vec() }
}

pub type SharedPublicationFonts = Rc<RefCell<PublicationFonts>>;

#[derive(Default)]
pub struct PublicationFonts {
    active: Vec<FontFace>,
    active_keys: Vec<String>,
    loaded: HashMap<String, LoadedFontFace>,
    registered: HashSet<String>,
    generation: u64,
    reader_font_lists: Vec<String>,
    global_reader_font: bool,
    cjk_reader_font: bool,
}

impl PublicationFonts {
    pub fn set_reader_overrides(&mut self, overrides: &ReaderStyleOverrides) {
        let lists =
            [overrides.font_family.as_deref(), overrides.serif_font_family.as_deref(), overrides.sans_serif_font_family.as_deref(), overrides.monospace_font_family.as_deref()].into_iter().flatten().map(str::to_owned).collect::<Vec<_>>();
        let global = overrides.font_family.is_some();
        let cjk_reader_font = overrides.cjk_font_family.is_some();
        if self.reader_font_lists != lists || self.global_reader_font != global || self.cjk_reader_font != cjk_reader_font {
            self.reader_font_lists = lists;
            self.global_reader_font = global;
            self.cjk_reader_font = cjk_reader_font;
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// Reader override values are retained verbatim in computed font-family.
    /// Match the whole list so replacing serif leaves a publisher's sans-serif
    /// or monospace face available even when it shares the same family alias.
    pub fn allows_publication_family_list(&self, family_list: Option<&str>) -> bool {
        !self.global_reader_font && family_list.is_none_or(|list| !self.reader_font_lists.iter().any(|selected| selected.trim().eq_ignore_ascii_case(list.trim())))
    }

    pub(crate) fn allows_cjk_publication_fonts(&self) -> bool {
        !self.cjk_reader_font
    }

    pub fn has_loaded_active_faces(&self) -> bool {
        self.active_keys.iter().any(|key| self.loaded.contains_key(key))
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn set_active(&mut self, faces: &[FontFace]) -> bool {
        if self.active == faces {
            return false;
        }
        self.active = faces.to_vec();
        self.active_keys = faces.iter().map(face_key).collect();
        self.generation = self.generation.wrapping_add(1);
        true
    }

    pub fn register(&mut self, loaded: LoadedFontFace, text_system: &WindowTextSystem) -> Result<bool, String> {
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
        self.loaded.insert(key, loaded);
        self.generation = self.generation.wrapping_add(1);
        Ok(true)
    }

    pub(crate) fn resolve(&self, family: &str, weight: u16, slant: FontSlant) -> Option<Font> {
        let mut candidates = self
            .active
            .iter()
            .zip(&self.active_keys)
            .enumerate()
            .filter(|(_, (face, _))| face.family.eq_ignore_ascii_case(family))
            .filter_map(|(index, (face, key))| self.loaded.get(key).map(|loaded| (index, face, loaded)))
            .collect::<Vec<_>>();
        candidates.sort_by(|(a_index, a, _), (b_index, b, _)| {
            style_rank(&a.style, slant).cmp(&style_rank(&b.style, slant)).then_with(|| weight_rank(a, weight).partial_cmp(&weight_rank(b, weight)).unwrap_or(std::cmp::Ordering::Equal)).then_with(|| b_index.cmp(a_index))
        });
        let (_, selected, loaded) = *candidates.first()?;
        let mut resolved = font(loaded.family.clone());
        resolved.weight = FontWeight(if loaded.variable { f32::from(weight).clamp(*selected.weight.start(), *selected.weight.end()) } else { f32::from(loaded.weight) });
        resolved.style = match if loaded.bytes.is_none() { slant } else { loaded.slant } {
            FontSlant::Normal => FontStyle::Normal,
            FontSlant::Italic => FontStyle::Italic,
            FontSlant::Oblique => FontStyle::Oblique,
        };
        let fallback = candidates.iter().skip(1).map(|(_, _, loaded)| loaded.family.clone()).collect::<Vec<_>>();
        if !fallback.is_empty() {
            resolved.fallbacks = Some(FontFallbacks::from_fonts(fallback));
        }
        Some(resolved)
    }
}

fn style_rank(style: &FontFaceStyle, requested: FontSlant) -> u8 {
    match (requested, style) {
        (FontSlant::Normal, FontFaceStyle::Normal) | (FontSlant::Italic, FontFaceStyle::Italic) | (FontSlant::Oblique, FontFaceStyle::Oblique(_)) => 0,
        (FontSlant::Normal | FontSlant::Italic, FontFaceStyle::Oblique(_)) | (FontSlant::Oblique, FontFaceStyle::Italic) => 1,
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
        FontFace { family: name.into(), sources: vec![], weight: f32::from(weight)..=f32::from(weight), style: FontFaceStyle::Normal, stretch: 100.0..=100.0, unicode_ranges: vec![0..=0x10ffff], additional_descriptors: vec![] }
    }

    #[test]
    fn global_reader_fonts_disable_all_publication_faces() {
        let faces = vec![face("Publisher", 400), face("Chosen Reader", 700)];
        assert_eq!(publication_faces_for_overrides(&faces, &ReaderStyleOverrides::default()), faces);
        let overrides = ReaderStyleOverrides { font_family: Some("Chosen Reader, serif".into()), ..Default::default() };
        assert!(publication_faces_for_overrides(&faces, &overrides).is_empty());
    }

    #[test]
    fn publisher_cjk_faces_remain_available_with_a_latin_reader_font() {
        let faces = vec![face("Publisher", 400)];
        let overrides = ReaderStyleOverrides { font_family: Some("Latin".into()), cjk_font_fallback: Some("Embedded".into()), ..Default::default() };
        assert_eq!(publication_faces_for_overrides(&faces, &overrides), faces);
        assert!(publication_faces_for_overrides(&faces, &ReaderStyleOverrides { cjk_font_family: Some("Selected CJK".into()), ..overrides }).is_empty());
    }

    #[test]
    fn role_overrides_are_scoped_to_the_replaced_font_lists() {
        let faces = vec![face("Reader Serif", 400), face("Publisher", 400)];
        let overrides = ReaderStyleOverrides { serif_font_family: Some("Reader Serif, serif".into()), ..Default::default() };
        assert_eq!(publication_faces_for_overrides(&faces, &overrides), faces);
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
        assert_eq!(registry.resolve("publisher", 400, FontSlant::Normal).unwrap().family.as_ref(), "PrivateRegular");
        assert_eq!(registry.resolve("Publisher", 700, FontSlant::Normal).unwrap().family.as_ref(), "PrivateBold");
        registry.set_active(&[]);
        assert!(registry.resolve("Publisher", 400, FontSlant::Normal).is_none());
    }

    #[test]
    fn css_weight_search_order_handles_the_400_to_500_interval() {
        assert!(weight_rank(&face("A", 500), 400) < weight_rank(&face("A", 300), 400));
        assert!(weight_rank(&face("A", 300), 400) < weight_rank(&face("A", 600), 400));
        assert!(weight_rank(&face("A", 800), 700) < weight_rank(&face("A", 600), 700));
    }
}
