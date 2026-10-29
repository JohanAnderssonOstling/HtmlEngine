use std::collections::VecDeque;

use html::layout::ThemeAdaptation;
use image::{
    RgbaImage,
    imageops::{FilterType, resize},
};

const SAMPLE_SIDE: usize = 64;
const REGION_SIDE: usize = 8;
const CLASSIFICATION_CACHE_SIZE: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RasterKind {
    Preserve,
    NeutralLineArt,
}

/// Image-content decisions survive reader theme and display-size changes.
/// The resource hash identifies immutable decoded pixels, as in ImageCache.
#[derive(Default)]
pub(crate) struct RasterClassifier {
    entries: VecDeque<(Vec<u8>, RasterKind)>,
}

impl RasterClassifier {
    pub(crate) fn classify(&mut self, hash: &[u8], pixels: &[u8], width: u32, height: u32) -> RasterKind {
        if let Some(index) = self.entries.iter().position(|(key, _)| key == hash)
            && let Some(entry) = self.entries.remove(index)
        {
            let kind = entry.1;
            self.entries.push_back(entry);
            return kind;
        }
        let kind = classify_raster(pixels, width, height);
        if self.entries.len() == CLASSIFICATION_CACHE_SIZE {
            self.entries.pop_front();
        }
        self.entries.push_back((hash.to_vec(), kind));
        kind
    }
}

#[derive(Clone, Copy, Default)]
struct Samples {
    total: u32,
    visible: u32,
    dark: u32,
    light: u32,
    colored: u32,
    midtones: u32,
    isolated: u32,
}

impl Samples {
    fn add(&mut self, pixel: &[u8], isolated: bool) {
        self.total += 1;
        if pixel[3] < 32 {
            return;
        }
        self.visible += 1;
        let (minimum, maximum) = extrema(pixel);
        if maximum - minimum > 24 {
            self.colored += 1;
        }
        if maximum - minimum <= 16 && maximum <= 80 {
            self.dark += 1;
            self.isolated += u32::from(isolated);
        } else if maximum - minimum <= 16 && minimum >= 224 {
            self.light += 1;
        } else {
            self.midtones += 1;
        }
    }
}

/// Fixed, conservative rules over at most 4096 source pixels. No model,
/// random state, file-extension assumptions, or work proportional to the
/// full decoded image is needed for classification. Uncertainty preserves
/// the original, including grayscale photographs and mixed screenshots.
pub(crate) fn classify_raster(pixels: &[u8], width: u32, height: u32) -> RasterKind {
    if !valid_pixels(pixels, width, height) {
        return RasterKind::Preserve;
    }
    let columns = (width as usize).min(SAMPLE_SIDE);
    let rows = (height as usize).min(SAMPLE_SIDE);
    let mut all = Samples::default();
    let mut regions = [Samples::default(); REGION_SIDE * REGION_SIDE];
    for gy in 0..rows {
        for gx in 0..columns {
            // Deterministic jitter within each sample cell avoids sampling
            // only the gaps of regularly spaced text or halftone patterns.
            let x0 = gx as u64 * u64::from(width) / columns as u64;
            let x1 = (gx + 1) as u64 * u64::from(width) / columns as u64;
            let y0 = gy as u64 * u64::from(height) / rows as u64;
            let y1 = (gy + 1) as u64 * u64::from(height) / rows as u64;
            let mut seed = (gx as u32 + 1).wrapping_mul(0x9e3779b9) ^ (gy as u32 + 1).wrapping_mul(0x85ebca6b);
            seed ^= seed >> 16;
            let x = (x0 + u64::from(seed) % (x1 - x0)) as u32;
            let y = (y0 + u64::from(seed.rotate_left(13)) % (y1 - y0)) as u32;
            let pixel = at(pixels, width, x, y);
            let (minimum, maximum) = extrema(pixel);
            let isolated = pixel[3] >= 32 && maximum <= 80 && maximum - minimum <= 16 && !has_ink_neighbor(pixels, width, height, x, y);
            all.add(pixel, isolated);
            regions[(gy * REGION_SIDE / rows) * REGION_SIDE + gx * REGION_SIDE / columns].add(pixel, isolated);
        }
    }

    // Color is potentially meaningful. Allow only a trace of chromatic
    // noise, and reject small colorful or shaded regions even when a large
    // white page would otherwise dilute their contribution to the total.
    if all.visible < 8 || all.colored * 200 > all.visible || regions.iter().any(|region| region.visible >= 16 && (region.colored * 50 > region.visible || region.midtones * 100 > region.visible * 35)) {
        return RasterKind::Preserve;
    }

    let transparent = all.total - all.visible;
    if transparent * 10 >= all.total {
        // Dark neutral ink on transparency is a strong signal. Light ink
        // already intended for dark backgrounds and shaded cutouts stay as-is.
        return if all.dark * 100 >= all.visible * 95 { RasterKind::NeutralLineArt } else { RasterKind::Preserve };
    }

    // Opaque text/line art must have a dominant light field, limited ink,
    // little shading, and connected strokes. Dense patches and isolated
    // halftone dots are treated as ambiguous, not as text to invert.
    if all.total >= 64
        && transparent * 100 <= all.total * 2
        && all.light * 100 >= all.visible * 70
        && all.dark * 200 >= all.visible
        && all.dark * 4 <= all.visible
        && all.midtones * 100 <= all.visible * 8
        && all.isolated * 10 <= all.dark
        && !regions.iter().any(|region| region.total >= 16 && region.dark * 100 > region.total * 60)
    {
        RasterKind::NeutralLineArt
    } else {
        RasterKind::Preserve
    }
}

fn valid_pixels(pixels: &[u8], width: u32, height: u32) -> bool {
    width > 0 && height > 0 && (width as usize).checked_mul(height as usize).and_then(|size| size.checked_mul(4)) == Some(pixels.len())
}

fn extrema(pixel: &[u8]) -> (u8, u8) {
    (pixel[0].min(pixel[1]).min(pixel[2]), pixel[0].max(pixel[1]).max(pixel[2]))
}

fn at(pixels: &[u8], width: u32, x: u32, y: u32) -> &[u8] {
    let offset = (y as usize * width as usize + x as usize) * 4;
    &pixels[offset..offset + 4]
}

fn has_ink_neighbor(pixels: &[u8], width: u32, height: u32, x: u32, y: u32) -> bool {
    for ny in y.saturating_sub(1)..=y.saturating_add(1).min(height - 1) {
        for nx in x.saturating_sub(1)..=x.saturating_add(1).min(width - 1) {
            if nx == x && ny == y {
                continue;
            }
            let pixel = at(pixels, width, nx, ny);
            let (minimum, maximum) = extrema(pixel);
            if pixel[3] >= 32 && maximum <= 160 && maximum - minimum <= 24 {
                return true;
            }
        }
    }
    false
}

/// Resize in RGBA, then map accepted neutral pixels through a 256-entry
/// reader-palette lookup. Alpha and any small colored accents are retained.
/// The source buffer remains immutable; adaptation never compounds.
pub(crate) fn raster_buffer(pixels: &[u8], width: u32, height: u32, target_width: u32, target_height: u32, theme: Option<ThemeAdaptation>) -> Option<RgbaImage> {
    if !valid_pixels(pixels, width, height) || target_width == 0 || target_height == 0 {
        return None;
    }
    let source = RgbaImage::from_raw(width, height, pixels.to_vec())?;
    let mut buffer = if width == target_width && height == target_height { source } else { resize(&source, target_width, target_height, FilterType::Triangle) };
    if let Some(theme) = theme {
        let palette: [[u8; 4]; 256] = std::array::from_fn(|gray| theme.adapt((gray as u32 * 0x01010100) | 0xff).to_be_bytes());
        for pixel in buffer.pixels_mut() {
            let (minimum, maximum) = extrema(&pixel.0);
            if pixel[3] == 0 || maximum - minimum > 16 {
                continue;
            }
            let gray = (u32::from(pixel[0]) * 54 + u32::from(pixel[1]) * 183 + u32::from(pixel[2]) * 19 + 128) >> 8;
            pixel.0[..3].copy_from_slice(&palette[gray as usize][..3]);
        }
    }
    Some(buffer)
}

pub(crate) fn raster_cache_key(hash: &[u8], width: u32, height: u32, theme: Option<ThemeAdaptation>) -> Vec<u8> {
    let mut key = Vec::with_capacity(hash.len() + 17);
    key.push(if theme.is_some() { b'l' } else { b'r' });
    key.extend_from_slice(&width.to_le_bytes());
    key.extend_from_slice(&height.to_le_bytes());
    if let Some(theme) = theme {
        key.extend_from_slice(&theme.ink.to_be_bytes());
        key.extend_from_slice(&theme.surface.to_be_bytes());
    }
    key.extend_from_slice(hash);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK: ThemeAdaptation = ThemeAdaptation { ink: 0xe8e7e4ff, surface: 0x2d2b28ff };
    const SEPIA: ThemeAdaptation = ThemeAdaptation { ink: 0x272624ff, surface: 0xfef8edff };

    fn image(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| image::Rgba(pixel(x, y)))
    }

    fn text_page(width: u32, height: u32, transparent: bool) -> RgbaImage {
        image(width, height, |x, y| {
            if x > width / 12 && x < width * 11 / 12 && y % 16 >= 5 && y % 16 <= 6 && x % 11 < 8 {
                [0, 0, 0, 255]
            } else if transparent {
                [0, 0, 0, 0]
            } else {
                [255; 4]
            }
        })
    }

    fn kind(image: &RgbaImage) -> RasterKind {
        classify_raster(image.as_raw(), image.width(), image.height())
    }

    #[test]
    fn clear_opaque_and_transparent_text_adapts_at_small_and_large_sizes() {
        for (width, height) in [(64, 64), (720, 960)] {
            for transparent in [false, true] {
                assert_eq!(kind(&text_page(width, height, transparent)), RasterKind::NeutralLineArt);
            }
        }
    }

    #[test]
    fn connected_diagram_strokes_and_mild_compression_noise_adapt() {
        let diagram = image(64, 64, |x, y| {
            if (x > 8 && x < 56 && (y == 16 || y == 48)) || (y > 16 && y < 48 && (x == 8 || x == 56)) {
                [12, 10, 14, 255]
            } else {
                let noise = ((x * 7 + y * 3) % 8) as u8;
                [247 + noise, 250, 248 + noise / 2, 255]
            }
        });
        assert_eq!(kind(&diagram), RasterKind::NeutralLineArt);
    }

    #[test]
    fn grayscale_and_colored_continuous_tone_images_stay_original() {
        let grayscale = image(64, 64, |x, y| {
            let value = ((x * 3 + y * 5) % 256) as u8;
            [value, value, value, 255]
        });
        let colorful = image(64, 64, |x, y| [(x * 4) as u8, (y * 4) as u8, ((x + y) * 2) as u8, 255]);
        assert_eq!(kind(&grayscale), RasterKind::Preserve);
        assert_eq!(kind(&colorful), RasterKind::Preserve);
    }

    #[test]
    fn small_photographic_region_on_an_otherwise_white_page_stays_original() {
        let mixed = image(64, 64, |x, y| {
            if x >= 48 && y >= 48 {
                let value = (96 + (x * 3 + y * 7) % 96) as u8;
                [value, value, value, 255]
            } else {
                [255; 4]
            }
        });
        assert_eq!(kind(&mixed), RasterKind::Preserve);
    }

    #[test]
    fn colored_chart_accents_are_not_mistaken_for_grayscale() {
        let mut chart = text_page(64, 64, false);
        for x in 5..59 {
            chart.put_pixel(x, 32, image::Rgba([190, 0, 0, 255]));
        }
        assert_eq!(kind(&chart), RasterKind::Preserve);
    }

    #[test]
    fn already_dark_or_light_transparent_ink_stays_original() {
        let dark_page = image(64, 64, |x, y| if x > 4 && x < 60 && y % 16 == 6 { [255; 4] } else { [0, 0, 0, 255] });
        let light_ink = image(64, 64, |x, y| if x > 4 && x < 60 && y == 30 { [255; 4] } else { [0; 4] });
        assert_eq!(kind(&dark_page), RasterKind::Preserve);
        assert_eq!(kind(&light_ink), RasterKind::Preserve);
    }

    #[test]
    fn solid_fields_dense_patches_and_isolated_halftone_dots_stay_original() {
        assert_eq!(kind(&image(64, 64, |_, _| [255; 4])), RasterKind::Preserve);
        assert_eq!(kind(&image(64, 64, |_, _| [0, 0, 0, 255])), RasterKind::Preserve);
        let patch = image(64, 64, |x, y| if x < 16 && y < 32 { [0, 0, 0, 255] } else { [255; 4] });
        let dots = image(64, 64, |x, y| if x % 3 == 0 && y % 3 == 0 { [0, 0, 0, 255] } else { [255; 4] });
        assert_eq!(kind(&patch), RasterKind::Preserve);
        assert_eq!(kind(&dots), RasterKind::Preserve);
    }

    #[test]
    fn malformed_empty_and_insufficient_images_stay_original() {
        assert_eq!(classify_raster(&[], 0, 0), RasterKind::Preserve);
        assert_eq!(classify_raster(&[255; 4], u32::MAX, u32::MAX), RasterKind::Preserve);
        assert_eq!(kind(&image(64, 64, |_, _| [0; 4])), RasterKind::Preserve);
        assert_eq!(kind(&image(2, 2, |x, _| if x == 0 { [0, 0, 0, 255] } else { [255; 4] })), RasterKind::Preserve);
    }

    #[test]
    fn pixel_mapping_preserves_alpha_accents_source_and_theme_independence() {
        let pixels = [0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 128, 0, 0, 0, 0, 200, 0, 0, 255];
        let original = pixels;
        for theme in [DARK, SEPIA] {
            let result = raster_buffer(&pixels, 5, 1, 5, 1, Some(theme)).unwrap();
            assert_eq!(result.get_pixel(0, 0).0, theme.adapt(0x000000ff).to_be_bytes());
            assert_eq!(result.get_pixel(1, 0).0, theme.adapt(0xffffffff).to_be_bytes());
            assert_eq!(result.get_pixel(2, 0).0[3], 128);
            assert_eq!(result.get_pixel(3, 0).0, [0; 4]);
            assert_eq!(result.get_pixel(4, 0).0, [200, 0, 0, 255]);
        }
        assert_eq!(pixels, original);
        assert_eq!(raster_buffer(&pixels, 5, 1, 5, 1, None).unwrap().as_raw(), &pixels);
    }

    #[test]
    fn resized_photographic_content_without_theme_keeps_existing_resize_behavior() {
        let source = image(32, 32, |x, y| [(x * 7) as u8, (y * 5) as u8, 120, 255]);
        let actual = raster_buffer(source.as_raw(), 32, 32, 17, 19, None).unwrap();
        assert_eq!(actual, resize(&source, 17, 19, FilterType::Triangle));
    }

    #[test]
    fn classification_and_cache_keys_are_deterministic_and_theme_aware() {
        let source = text_page(720, 960, false);
        let mut cache = RasterClassifier::default();
        for _ in 0..3 {
            assert_eq!(cache.classify(b"source", source.as_raw(), 720, 960), RasterKind::NeutralLineArt);
        }
        assert_eq!(cache.entries.len(), 1);
        let key = raster_cache_key(b"source", 200, 300, Some(DARK));
        assert_ne!(key, raster_cache_key(b"source", 200, 300, Some(SEPIA)));
        assert_ne!(key, raster_cache_key(b"source", 200, 300, None));
        assert_ne!(key, raster_cache_key(b"source", 201, 300, Some(DARK)));
        assert_ne!(key, raster_cache_key(b"other", 200, 300, Some(DARK)));
    }
}
