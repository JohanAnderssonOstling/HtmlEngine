//! MathML is retained in the DOM and painted through the ordinary SVG image path.

use formulary::{Item, Layout, LayoutOptions, MathFont, layout, parse, svg};
use html_dom::{Document, ImageSource};
use html_style_model::ComputedStyles;
use quick_xml::{Reader, events::Event};
use std::sync::{Arc, OnceLock};

fn math_font() -> &'static MathFont<'static> {
    static FONT: OnceLock<MathFont<'static>> = OnceLock::new();
    FONT.get_or_init(|| MathFont::new(include_bytes!("../../assets/math/STIXTwoMath-Regular.otf"), 0).expect("bundled STIX Two Math must have a valid MATH table"))
}

pub(super) struct MathMlUndo {
    index: usize,
    width: u32,
    height: u32,
    svg: Arc<[u8]>,
    baseline: f32,
}

pub(super) fn prepare(document: &mut Document, styles: &ComputedStyles) {
    prepare_with_undo(document, styles);
}

pub(super) fn prepare_with_undo(document: &mut Document, styles: &ComputedStyles) -> Vec<MathMlUndo> {
    let equations: Vec<_> = document.node_ids().filter_map(|node| {
        let image_idx = document.element_ref(node)?.image_idx()?;
        if !matches!(document.image(image_idx)?.source, ImageSource::MathMl { .. }) { return None; }
        let style = styles.view(styles.style_for_node(node)?)?;
        Some((image_idx as usize, style.font_size(), style.color()))
    }).collect();
    let mut undo = Vec::with_capacity(equations.len());
    for (index, size, color) in equations {
        let ImageSource::MathMl { markup, .. } = &document.images()[index].source else { unreachable!() };
        let (svg, width, height, baseline) = render(markup, size, color);
        let image = &mut document.images_mut()[index];
        let ImageSource::MathMl { svg: old_svg, baseline: old_baseline, .. } = &mut image.source else { unreachable!() };
        undo.push(MathMlUndo {
            index, width: std::mem::replace(&mut image.width, width), height: std::mem::replace(&mut image.height, height),
            svg: std::mem::replace(old_svg, svg.into()), baseline: std::mem::replace(old_baseline, baseline),
        });
    }
    undo
}

pub(super) fn restore(document: &mut Document, undo: Vec<MathMlUndo>) {
    for old in undo {
        let image = &mut document.images_mut()[old.index];
        image.width = old.width; image.height = old.height;
        let ImageSource::MathMl { svg, baseline, .. } = &mut image.source else { unreachable!() };
        *svg = old.svg; *baseline = old.baseline;
    }
}

fn render(markup: &str, font_size: f32, color: u32) -> (Vec<u8>, u32, u32, f32) {
    let options = LayoutOptions { font_size: font_size.max(0.001) };
    let font = math_font();
    let laid = bounded_markup(markup).then(|| parse(markup).ok()).flatten().map(|root| layout(&root, font, &options)).filter(valid_layout).unwrap_or_else(|| {
        // Retain the source even when it cannot be rendered. Never pass
        // non-finite dimensions or excessive allocations to the SVG painter.
        let root = parse("<math><mtext>Equation unavailable</mtext></math>").expect("constant MathML fallback");
        layout(&root, font, &options)
    });
    let width = (laid.width + 4.0).ceil().max(1.0) as u32;
    let height = (laid.ascent + laid.descent + 4.0).ceil().max(1.0) as u32;
    let baseline = ((laid.ascent + 2.0) / (laid.ascent + laid.descent + 4.0)).clamp(0.0, 1.0);
    let svg = svg::to_svg(&laid, font).replacen("<svg ", &format!("<svg data-html-mathml=\"true\" color=\"#{color:08x}\" "), 1).replacen("fill=\"black\"", "fill=\"currentColor\"", 1);
    (svg.into_bytes(), width, height, baseline)
}

fn bounded_markup(markup: &str) -> bool {
    if markup.len() > 256 * 1024 {
        return false;
    }
    let mut reader = Reader::from_str(markup);
    let (mut depth, mut nodes) = (0usize, 0usize);
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) => {
                depth += 1;
                nodes += 1;
            }
            Ok(Event::Empty(_)) => nodes += 1,
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) => return true,
            Err(_) => return false,
            _ => {}
        }
        if depth > 128 || nodes > 8192 {
            return false;
        }
    }
}

fn valid_layout(laid: &Layout) -> bool {
    let dimension = |v: f32| v.is_finite() && (0.0..=1_000_000.0).contains(&v);
    dimension(laid.width)
        && dimension(laid.ascent)
        && dimension(laid.descent)
        && laid.items.len() <= 100_000
        && laid.items.iter().all(|item| match item {
            Item::Glyph { x, y, size, advance, .. } => x.is_finite() && y.is_finite() && dimension(*size) && advance.is_finite(),
            Item::Rule { x, y, w, h, .. } | Item::Background { x, y, w, h, .. } => x.is_finite() && y.is_finite() && dimension(*w) && dimension(*h),
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LayoutConstraints;
    use crate::test_support::{DocumentFactory, TestGlyphShaper};

    fn laid_out(source: &str, width: f64) -> crate::LaidOutDocument {
        let prepared = DocumentFactory::new().parse_with_new_pipeline(source, None);
        prepared.shape(&mut TestGlyphShaper::new()).unwrap().layout(LayoutConstraints::new(width, 24.0).unwrap())
    }

    #[test]
    fn source_is_retained_and_math_descendants_do_not_render_as_text() {
        let document = laid_out("<!doctype html><p>Before <math><mfrac><mi>x</mi><mn>2</mn></mfrac></math> after</p>", 400.0);
        let view = document.render_view();
        assert!(view.image_mathml(0).unwrap().contains("<mfrac>"));
        assert!(view.image_mathml(0).unwrap().contains("http://www.w3.org/1998/Math/MathML"));
        assert_eq!(view.fragments().images().len(), 1);
        let text = view.text();
        assert!(!(0..text.glyph_count()).any(|index| { text.glyph_at(index).and_then(|glyph| text.glyph_metric(glyph)).is_some_and(|metric| metric.ch() == 'x') }), "formula source is represented by an atomic image");
    }

    #[test]
    fn wide_equation_fits_column_and_fraction_extends_below_baseline() {
        let document = laid_out("<!doctype html><p>A <math><mfrac><mi>abcdefghijklmnopqrstuvwxyz</mi><mn>2</mn></mfrac></math> B</p>", 90.0);
        let view = document.render_view();
        let images = view.fragments().images();
        let image = images.iter().next().unwrap();
        assert!(image.size().width <= 74.01, "equation must fit inside body margins: {:?}", image.size());
        let line = view.text().line(image.line_idx()).unwrap();
        let bottom = image.offset().y + image.size().height;
        assert!(image.offset().y < line.baseline() && bottom > line.baseline());
    }

    #[test]
    fn block_equation_is_atomic_and_separate_from_surrounding_text() {
        let document = laid_out("<!doctype html><p>Before<math display='block'><msup><mi>x</mi><mn>2</mn></msup></math>after</p>", 400.0);
        let view = document.render_view();
        assert_eq!(view.fragments().images().len(), 1);
        assert!(view.text().lines().len() >= 3);
    }

    #[test]
    fn namespaced_xhtml_mathml_keeps_source_and_reader_font_size() {
        let source = r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:m="http://www.w3.org/1998/Math/MathML"><body><m:math><m:mi>x</m:mi></m:math></body></html>"#;
        let mut widths = Vec::new();
        for size in [16.0, 32.0] {
            let mut document = html_parse::parse_xml_document(source).unwrap().build_dom();
            document.set_root_font_size(html_dom::RootFontSize::new(size).unwrap());
            let styled = html_style::style_document(document, &[]);
            let (document, styles) = styled.into_parts();
            let prepared = crate::PreparedDocument::try_new(document, styles).unwrap();
            let document = prepared.shape(&mut TestGlyphShaper::new()).unwrap().layout(LayoutConstraints::new(400.0, 40.0).unwrap());
            let view = document.render_view();
            assert!(view.image_mathml(0).unwrap().contains("<mi>x</mi>"));
            widths.push(view.images()[0].width);
        }
        assert!((widths[1] as i64 - 4 - 2 * (widths[0] as i64 - 4)).abs() <= 2);
    }

    #[test]
    fn paint_override_updates_equation_color_without_changing_svg_geometry() {
        let prepared = DocumentFactory::new().parse_with_new_pipeline("<!doctype html><p><math><mi>x</mi></math></p>", None);
        let themed = prepared.with_reader_paint_overrides(&crate::ReaderStyleOverrides { foreground: Some(0xeeeeeeff), ..Default::default() }).unwrap();
        let layout = |prepared: &crate::PreparedDocument| prepared.shape(&mut TestGlyphShaper::new()).unwrap().layout(LayoutConstraints::new(400.0, 24.0).unwrap());
        let ordinary = layout(&prepared);
        let themed = layout(&themed);
        let view = themed.render_view();
        assert_eq!(view.fragments().images().iter().next().unwrap().text_color(), 0xeeeeeeff);
        let ImageSource::MathMl { svg: old, .. } = &ordinary.render_view().images()[0].source else { panic!("math source") };
        let ImageSource::MathMl { svg: new, .. } = &view.images()[0].source else { panic!("math source") };
        assert_eq!(old, new, "theme changes must reuse formula geometry");
    }

    #[test]
    fn svg_keeps_explicit_colors_and_has_an_inheritable_theme_color() {
        let (bytes, width, height, baseline) = render("<math><mi>x</mi><mo>+</mo><mi mathcolor=\"red\">y</mi></math>", 24.0, 0xabcdefFF);
        let svg = String::from_utf8(bytes).unwrap();
        assert!(svg.contains("color=\"#abcdefff\""));
        assert!(svg.contains("fill=\"currentColor\""));
        assert!(svg.contains("fill=\"#ff0000\""));
        assert!(svg.contains("<path"));
        assert!(!svg.contains("<text"));
        assert!(width > 1 && height > 1 && baseline > 0.0 && baseline < 1.0);
    }

    #[test]
    fn svg_scales_with_reading_size_and_tracks_fraction_baseline() {
        let markup = "<math><mfrac><mn>1</mn><mi>x</mi></mfrac></math>";
        let (_, w16, h16, baseline) = render(markup, 16.0, 0x000000ff);
        let (_, w32, h32, _) = render(markup, 32.0, 0x000000ff);
        assert!((w32 as i64 - 4 - 2 * (w16 as i64 - 4)).abs() <= 2);
        assert!((h32 as i64 - 4 - 2 * (h16 as i64 - 4)).abs() <= 2);
        assert!(baseline < 0.9, "fraction denominator must extend below the text baseline");
    }

    #[test]
    fn excessive_or_nonfinite_input_uses_a_finite_fallback() {
        let huge = format!("<math><mspace width=\"{}px\"/></math>", "9".repeat(80));
        let deep = format!("<math>{}<mi>x</mi>{}</math>", "<mrow>".repeat(140), "</mrow>".repeat(140));
        assert!(!bounded_markup(&deep));
        for markup in [huge, deep] {
            let (svg, width, height, baseline) = render(&markup, 16.0, 0x000000ff);
            assert!(width < 1000 && height < 1000 && baseline.is_finite());
            assert!(!String::from_utf8(svg).unwrap().contains("inf"));
        }
    }
}
