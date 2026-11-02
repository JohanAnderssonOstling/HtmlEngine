use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};

/// Override only the inherited ink of generated equations. Ordinary SVGs,
/// explicit MathML colors and backgrounds retain their publication styling.
pub(crate) fn themed_math_svg(bytes: &[u8], color: u32) -> Option<Vec<u8>> {
    let mut reader = Reader::from_reader(bytes);
    let mut writer = Writer::new(Vec::with_capacity(bytes.len() + 32));
    let color = format!("#{color:08x}");
    let mut rewritten = false;
    loop {
        let event = match reader.read_event().ok()? {
            Event::Start(start) if !rewritten => {
                if start.local_name().as_ref() != b"svg" {
                    return None;
                }
                let attributes = start.attributes().collect::<Result<Vec<_>, _>>().ok()?;
                if !attributes.iter().any(|attribute| attribute.key.as_ref() == b"data-html-mathml" && attribute.value.as_ref() == b"true") {
                    return None;
                }
                let name = std::str::from_utf8(start.name().as_ref()).ok()?.to_owned();
                let mut root = BytesStart::new(name);
                for attribute in attributes {
                    if attribute.key.as_ref() != b"color" {
                        root.push_attribute(attribute);
                    }
                }
                root.push_attribute(("color", color.as_str()));
                rewritten = true;
                Event::Start(root.into_owned())
            }
            Event::Eof => break,
            event => event.into_owned(),
        };
        writer.write_event(event).ok()?;
    }
    rewritten.then(|| writer.into_inner())
}

#[cfg(test)]
mod math_svg_tests {
    use super::*;

    #[test]
    fn theme_changes_inherited_equation_ink_without_changing_explicit_colors() {
        let bytes = br##"<svg data-html-mathml="true" color="#000000"><g fill="currentColor"><path d="M0 0h2v2H0z"/><path fill="#ff0000" d="M2 0h2v2H2z"/></g></svg>"##;
        let themed = themed_math_svg(bytes, 0xeeeeeeff).unwrap();
        let source = String::from_utf8(themed).unwrap();
        assert!(source.contains("color=\"#eeeeeeff\""));
        assert!(source.contains("fill=\"#ff0000\""));
        assert!(source.contains("fill=\"currentColor\""));
    }

    #[test]
    fn ordinary_svg_keeps_its_color() {
        assert!(themed_math_svg(br##"<svg color="#ff0000"><path/></svg>"##, 0xffffffff).is_none());
    }

    #[test]
    fn rasterizer_uses_theme_ink_preserves_explicit_ink_and_leaves_background_transparent() {
        let bytes = br##"<svg xmlns="http://www.w3.org/2000/svg" width="6" height="2" data-html-mathml="true" color="#000000"><g fill="currentColor"><path d="M0 0h2v2H0z"/><path fill="#ff0000" d="M2 0h2v2H2z"/></g></svg>"##;
        let themed = themed_math_svg(bytes, 0xabcdefFF).unwrap();
        let tree = resvg::usvg::Tree::from_data(&themed, &resvg::usvg::Options::default()).unwrap();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(6, 2).unwrap();
        resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
        assert_eq!(&pixmap.data()[0..4], &[0xab, 0xcd, 0xef, 0xff]);
        assert_eq!(&pixmap.data()[8..12], &[0xff, 0, 0, 0xff]);
        assert_eq!(&pixmap.data()[16..20], &[0, 0, 0, 0]);
    }
}
