use std::collections::{HashMap, HashSet};

use html::layout::ThemeAdaptation;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};
use roxmltree::{Document, Node};

/// Adapt a normalized SVG from `ParsedSvg::to_svg`. Normalization resolves
/// stylesheets, inherited/default paints, named colors and text into explicit
/// paint attributes, so this does not need a second CSS implementation.
///
/// Mask, clip and filter paint is functional input, not visible diagram ink.
/// Keep it and any referenced paint servers unchanged. Embedded image bytes,
/// opacity, geometry and the absence of a background are also preserved.
pub(crate) fn themed_svg(bytes: &[u8], theme: ThemeAdaptation) -> Option<Vec<u8>> {
    let document = Document::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    let protected = protected_resources(&document);
    let mut reader = Reader::from_reader(bytes);
    let mut writer = Writer::new(Vec::with_capacity(bytes.len()));
    let mut scopes = Vec::new();
    loop {
        let event = match reader.read_event().ok()? {
            Event::Start(start) => {
                let preserve = preserve_element(&start, &protected, &scopes)?;
                scopes.push(preserve);
                Event::Start(rewrite_paint(&start, theme, preserve)?)
            }
            Event::Empty(start) => {
                let preserve = preserve_element(&start, &protected, &scopes)?;
                Event::Empty(rewrite_paint(&start, theme, preserve)?)
            }
            Event::End(end) => {
                scopes.pop()?;
                Event::End(end.into_owned())
            }
            Event::Eof => break,
            event => event.into_owned(),
        };
        writer.write_event(event).ok()?;
    }
    scopes.is_empty().then(|| writer.into_inner())
}

pub(crate) fn themed_svg_cache_key(hash: &[u8], width: u32, height: u32, theme: ThemeAdaptation) -> Vec<u8> {
    let mut key = Vec::with_capacity(hash.len() + 17);
    key.push(b't');
    key.extend_from_slice(&width.to_le_bytes());
    key.extend_from_slice(&height.to_le_bytes());
    key.extend_from_slice(&theme.ink.to_be_bytes());
    key.extend_from_slice(&theme.surface.to_be_bytes());
    key.extend_from_slice(hash);
    key
}

fn functional_element(name: &[u8]) -> bool {
    matches!(name, b"mask" | b"clipPath" | b"filter")
}

fn preserve_element(start: &BytesStart<'_>, protected: &HashSet<String>, scopes: &[bool]) -> Option<bool> {
    if scopes.last() == Some(&true) || functional_element(start.local_name().as_ref()) {
        return Some(true);
    }
    for attribute in start.attributes() {
        let attribute = attribute.ok()?;
        if attribute.key.as_ref() == b"id"
            && protected.contains(std::str::from_utf8(attribute.value.as_ref()).ok()?)
        {
            return Some(true);
        }
    }
    Some(false)
}

fn rewrite_paint(start: &BytesStart<'_>, theme: ThemeAdaptation, preserve: bool) -> Option<BytesStart<'static>> {
    if preserve {
        return Some(start.to_owned());
    }
    let name = std::str::from_utf8(start.name().as_ref()).ok()?.to_owned();
    let mut rewritten = BytesStart::new(name);
    for attribute in start.attributes() {
        let attribute = attribute.ok()?;
        if matches!(attribute.key.as_ref(), b"fill" | b"stroke" | b"stop-color")
            && let Some(color) = normalized_color(attribute.value.as_ref())
        {
            let color = format!("#{:06x}", theme.adapt(color) >> 8);
            rewritten.push_attribute((attribute.key.as_ref(), color.as_bytes()));
        } else {
            rewritten.push_attribute(attribute);
        }
    }
    Some(rewritten.into_owned())
}

fn normalized_color(value: &[u8]) -> Option<u32> {
    let value = value.strip_prefix(b"#")?;
    if value.len() != 6 {
        return None;
    }
    u32::from_str_radix(std::str::from_utf8(value).ok()?, 16).ok().map(|rgb| rgb << 8 | 0xff)
}

fn paint_references<'a, 'input>(node: Node<'a, 'input>) -> impl Iterator<Item = &'a str> {
    node.descendants().flat_map(|node| node.attributes()).filter_map(|attribute| {
        let value = attribute.value();
        value.strip_prefix("url(#").and_then(|id| id.strip_suffix(')'))
            .or_else(|| (attribute.name() == "href").then(|| value.strip_prefix('#')).flatten())
    })
}

fn protected_resources(document: &Document<'_>) -> HashSet<String> {
    let nodes: HashMap<_, _> = document.descendants().filter_map(|node| node.attribute("id").map(|id| (id, node))).collect();
    let mut pending: Vec<_> = document.descendants()
        .filter(|node| functional_element(node.tag_name().name().as_bytes()))
        .flat_map(paint_references)
        .map(str::to_owned)
        .collect();
    let mut protected = HashSet::new();
    while let Some(id) = pending.pop() {
        if protected.insert(id.clone())
            && let Some(node) = nodes.get(id.as_str())
        {
            pending.extend(paint_references(*node).map(str::to_owned));
        }
    }
    protected
}

#[cfg(test)]
mod tests {
    use super::*;
    use resvg::tiny_skia::{Pixmap, Transform};
    use resvg::usvg::{Options, Tree, WriteOptions};

    const DARK: ThemeAdaptation = ThemeAdaptation { ink: 0xe8e7e4ff, surface: 0x2d2b28ff };
    const LIGHT: ThemeAdaptation = ThemeAdaptation { ink: 0x272624ff, surface: 0xfef8edff };

    fn render(source: &str, theme: Option<ThemeAdaptation>) -> Pixmap {
        let options = Options::default();
        let original = Tree::from_data(source.as_bytes(), &options).unwrap();
        let normalized = original.to_string(&WriteOptions::default());
        let bytes = match theme {
            Some(theme) => themed_svg(normalized.as_bytes(), theme).unwrap(),
            None => normalized.into_bytes(),
        };
        let tree = Tree::from_data(&bytes, &options).unwrap();
        let size = tree.size().to_int_size();
        let mut pixels = Pixmap::new(size.width(), size.height()).unwrap();
        resvg::render(&tree, Transform::identity(), &mut pixels.as_mut());
        pixels
    }

    fn pixel(pixels: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * pixels.width() + x) * 4) as usize;
        pixels.data()[offset..offset + 4].try_into().unwrap()
    }

    #[test]
    fn css_named_current_and_default_colors_follow_theme_without_painting_transparency() {
        let source = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="2">
          <style>.background {fill:white}.warning {fill:rgb(161,48,41)}</style>
          <path d="M0 0h2v2H0z"/>
          <path class="background" d="M2 0h2v2H2z"/>
          <path class="warning" d="M4 0h2v2H4z"/>
          <g color="black"><path fill="currentColor" d="M6 0h2v2H6z"/></g>
        </svg>"##;
        for theme in [DARK, LIGHT] {
            let pixels = render(source, Some(theme));
            assert_eq!(pixel(&pixels, 0, 0), theme.adapt(0x000000ff).to_be_bytes());
            assert_eq!(pixel(&pixels, 2, 0), theme.adapt(0xffffffff).to_be_bytes());
            assert_eq!(pixel(&pixels, 4, 0), theme.adapt(0xa13029ff).to_be_bytes());
            assert_eq!(pixel(&pixels, 6, 0), theme.adapt(0x000000ff).to_be_bytes());
            assert_eq!(pixel(&pixels, 8, 0), [0; 4]);
        }
    }

    #[test]
    fn strokes_gradients_and_opacity_keep_their_geometry_and_alpha() {
        let source = r##"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="4">
          <defs><linearGradient id="ramp"><stop stop-color="black"/><stop offset="1" stop-color="white"/></linearGradient></defs>
          <path fill="url(#ramp)" d="M0 0h8v4H0z"/>
          <path fill="none" stroke="black" stroke-width="2" d="M9 0v4"/>
          <path opacity="0.5" d="M10 0h2v4h-2z"/>
        </svg>"##;
        let original = render(source, None);
        let themed = render(source, Some(DARK));
        assert!(pixel(&original, 0, 1)[0] < pixel(&original, 7, 1)[0]);
        assert!(pixel(&themed, 0, 1)[0] > pixel(&themed, 7, 1)[0]);
        assert_eq!(pixel(&themed, 8, 1), DARK.ink.to_be_bytes());
        assert_eq!(pixel(&themed, 11, 1)[3], pixel(&original, 11, 1)[3]);
        assert_eq!(pixel(&themed, 11, 1)[3], 128);
    }

    #[test]
    fn luminosity_masks_and_their_transitive_shared_paint_servers_keep_original_coverage() {
        let source = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="2">
          <defs>
            <linearGradient id="ramp"><stop stop-color="white"/><stop offset="1" stop-color="black"/></linearGradient>
            <pattern id="pattern" width="1" height="1" patternContentUnits="objectBoundingBox"><path fill="url(#ramp)" d="M0 0h1v1H0z"/></pattern>
            <mask id="mask"><path fill="url(#pattern)" d="M0 0h8v2H0z"/></mask>
          </defs>
          <path fill="red" mask="url(#mask)" d="M0 0h8v2H0z"/>
        </svg>"##;
        let original = render(source, None);
        let themed = render(source, Some(DARK));
        assert!(original.data().chunks_exact(4).any(|p| p[3] > 0));
        for (before, after) in original.data().chunks_exact(4).zip(themed.data().chunks_exact(4)) {
            assert_eq!(before[3], after[3], "theme must not invert mask luminance");
        }
    }

    #[test]
    fn filters_keep_functional_colors() {
        let source = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><defs><filter id="f" x="0" y="0" width="1" height="1"><feFlood flood-color="white" flood-opacity=".5"/></filter></defs><path filter="url(#f)" d="M0 0h4v4H0z"/></svg>"##;
        assert_eq!(render(source, None).data(), render(source, Some(DARK)).data());
    }

    #[test]
    fn embedded_bitmap_pixels_remain_original_while_vector_colors_adapt() {
        let source = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"><path fill="red" d="M0 0h2v2H0z"/><image x="2" width="2" height="2" href="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="/></svg>"##;
        let original = render(source, None);
        let themed = render(source, Some(DARK));
        assert_eq!(pixel(&themed, 0, 0), DARK.adapt(0xff0000ff).to_be_bytes());
        assert_eq!(pixel(&original, 2, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&themed, 2, 0), pixel(&original, 2, 0));
    }

    #[test]
    fn cache_keys_include_both_theme_endpoints_size_and_content() {
        let key = themed_svg_cache_key(b"svg", 20, 30, DARK);
        for different in [
            themed_svg_cache_key(b"svg", 20, 30, LIGHT),
            themed_svg_cache_key(b"svg", 20, 30, ThemeAdaptation { surface: 0x101010ff, ..DARK }),
            themed_svg_cache_key(b"svg", 21, 30, DARK),
            themed_svg_cache_key(b"other", 20, 30, DARK),
        ] {
            assert_ne!(key, different);
        }
    }
}
