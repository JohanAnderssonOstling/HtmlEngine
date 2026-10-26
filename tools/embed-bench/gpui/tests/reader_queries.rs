//! Reader integration through the application's native GPUI shaping adapter.
//! No window or OS clipboard is required: the host records clipboard writes.

use gpui::{TextSystem, WindowTextSystem};
use gpui_wgpu::CosmicTextSystem;
use html::pipeline::ReaderStyleOverrides;
use html::resources::ResourceProvider;
use html::style::FontFace;
use html::text::TextSearchOptions;
use html_gpui::backend::{GpuiTextCache, SharedPublicationFonts, load_font_face};
use html_view_core::{
    AnnotationStyle, RecordingPainter, RendererAnnotation, RendererCommand, RendererEvent,
    RendererHost, RendererInitialConfig, RendererSession, cfi,
};
use kurbo::{Point, Size};
use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

struct Fixture(String);
impl ResourceProvider for Fixture {
    fn read_bytes(&self, uri: &str) -> io::Result<Vec<u8>> {
        if uri == "chapter.html" {
            Ok(self.0.as_bytes().to_vec())
        } else if uri == "native.ttf" {
            Ok(include_bytes!("../../../../../Html-Prototype/apps/desktop-gpui/assets/fonts/NV_Garamond-Regular.ttf").to_vec())
        } else {
            Err(io::Error::from(io::ErrorKind::NotFound))
        }
    }
    fn resolve(&self, _: &str, href: &str) -> String {
        href.to_owned()
    }
    fn exists(&self, uri: &str) -> bool {
        uri == "chapter.html"
    }
    fn list_html_candidates(&self, _: &str) -> io::Result<Vec<String>> {
        Ok(vec!["chapter.html".into()])
    }
}

#[derive(Default)]
struct Host {
    clipboard: RefCell<String>,
    events: RefCell<Vec<RendererEvent>>,
    fonts: SharedPublicationFonts,
    faces: RefCell<Vec<FontFace>>,
}
impl RendererHost for Host {
    fn set_font_overrides(&self, overrides: &ReaderStyleOverrides) {
        self.fonts.borrow_mut().set_reader_overrides(overrides);
    }
    fn prepare_font_faces(&self, faces: &[FontFace], _: Arc<dyn ResourceProvider>) {
        self.fonts.borrow_mut().set_active(faces);
        *self.faces.borrow_mut() = faces.to_vec();
    }
    fn request_repaint(&self) {}
    fn request_style(&self) {}
    fn schedule(&self, _: Duration, _: Box<dyn FnOnce() + Send>) {}
    fn schedule_repaint(&self, _: Duration) {}
    fn resource_waker(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        None
    }
    fn set_clipboard(&self, text: &str) -> Result<(), String> {
        *self.clipboard.borrow_mut() = text.to_owned();
        Ok(())
    }
    fn set_clipboard_image(&self, _: usize, _: usize, _: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    fn emit(&self, event: RendererEvent) {
        self.events.borrow_mut().push(event);
    }
}

fn reader(
    body: &str,
) -> (
    RendererSession<GpuiTextCache>,
    Rc<Host>,
    Arc<WindowTextSystem>,
) {
    let text = Arc::new(TextSystem::new(Arc::new(CosmicTextSystem::new(
        "DejaVu Serif",
    ))));
    let text = Arc::new(WindowTextSystem::new(text));
    let host = Rc::new(Host::default());
    let shaper = GpuiTextCache::new(text.clone()).with_publication_fonts(host.fonts.clone());
    let provider = Arc::new(Fixture(format!(
        "<!doctype html><html><head><style>@font-face{{font-family:'NV Garamond';src:url(native.ttf)}}body{{margin:0;font-family:'NV Garamond','DejaVu Serif'}}p{{margin:0;line-height:1.5}}</style></head><body>{body}</body></html>"
    )));
    let mut reader = RendererSession::from_provider_with_nav(
        host.clone(),
        shaper,
        provider,
        vec!["chapter.html".into()],
        0,
        None,
        RendererInitialConfig {
            font_size: 20.0,
            column_width: 400.0,
            max_column_count: Some(1),
            ..Default::default()
        },
    );
    frame(&mut reader, 400.0);
    assert!(reader.glyph_shaper().shape_cache_stats().line_misses > 0);
    (reader, host, text)
}

fn frame(reader: &mut RendererSession<GpuiTextCache>, width: f64) {
    reader
        .prepare_frame(Size::new(width, 1000.0))
        .paint(&mut RecordingPainter::default());
}

fn select(reader: &mut RendererSession<GpuiTextCache>, query: &str, expected: &str) -> String {
    let range = reader
        .document()
        .render_view()
        .text()
        .find_matches(query, TextSearchOptions::default())[0]
        .clone();
    // Scan the displayed line to exercise native hit testing rather than
    // constructing a selection by assigning logical offsets directly.
    let lines = reader.visible_reading_lines();
    for line in lines
        .iter()
        .filter(|line| line.glyphs.start < range.end && line.glyphs.end > range.start)
    {
        let y = (line.bounds.y0 + line.bounds.y1) / 2.0;
        for x in (line.bounds.x0.floor() as i32)..(line.bounds.x1.ceil() as i32) {
            if reader.select_word_at(Point::new(f64::from(x) + 0.5, y))
                && let Some((cfi, plain)) = reader.selected_passage()
                && plain == expected
            {
                let (start, end) = reader.selection_endpoints();
                assert!(start.is_some() && end.is_some());
                assert!(reader.selection_bounds().is_some());
                return cfi;
            }
        }
    }
    panic!("native hit testing could not select {expected:?}");
}

#[test]
fn native_words_copy_and_annotate_ligatures_emoji_rtl_tracking_and_hyphens() {
    for (body, query, plain, markdown) in [
        (
            "<p><strong>office</strong></p>",
            "office",
            "office",
            "**office**",
        ),
        ("<p>👩‍👩‍👧‍👦</p>", "👩‍👩‍👧‍👦", "👩‍👩‍👧‍👦", "👩‍👩‍👧‍👦"),
        ("<p dir='rtl'>שלום</p>", "שלום", "שלום", "שלום"),
        (
            "<p style='letter-spacing:2px'>tracking</p>",
            "tracking",
            "tracking",
            "tracking",
        ),
        (
            "<p style='width:65px;hyphens:manual'>hy&shy;phenation</p>",
            "hy\u{ad}phenation",
            "hyphenation",
            "hyphenation",
        ),
    ] {
        let (mut reader, host, _) = reader(body);
        let view = reader.document().render_view();
        if plain == "office" || plain == "👩‍👩‍👧‍👦" {
            assert!(
                (1..view.text().glyph_count() as u32)
                    .any(|glyph| !view.text().is_character_cluster_boundary(glyph)),
                "the fixture must exercise a joined native cluster"
            );
        }
        for line in view.text().lines().iter() {
            let (left, right) = view
                .geometry()
                .caret_span(line.index(), line.glyphs(), &[])
                .unwrap();
            for x in (left.floor() as i32)..=(right.ceil() as i32) {
                let glyph = view
                    .geometry()
                    .hit_test_line(line.index(), f64::from(x), &[])
                    .unwrap();
                assert!(
                    view.text().is_character_cluster_boundary(glyph),
                    "native hit testing split a cluster in {plain:?}"
                );
            }
        }
        let cfi = select(&mut reader, query, plain);
        assert!(reader.copy_selection());
        assert_eq!(*host.clipboard.borrow(), markdown);
        reader.apply(RendererCommand::UpsertAnnotation(RendererAnnotation {
            id: "native".into(),
            cfi_range: cfi,
            exact_text: plain.into(),
            prefix: None,
            suffix: None,
            style: AnnotationStyle::Highlight,
            color: [200, 120, 40, 100],
        }));
        frame(&mut reader, 400.0);
        assert!(reader.annotation_bounds("native").is_some());
    }
}

#[test]
fn native_source_and_annotations_survive_reflow_and_font_registration() {
    let (mut reader, host, text) = reader("<p>Before office after more words for reflow.</p>");
    let cfi = select(&mut reader, "office", "office");
    reader.apply(RendererCommand::UpsertAnnotation(RendererAnnotation {
        id: "stable".into(),
        cfi_range: cfi.clone(),
        exact_text: "office".into(),
        prefix: None,
        suffix: None,
        style: AnnotationStyle::Underline,
        color: [20, 80, 160, 255],
    }));
    let point = cfi::source_position_cfi(&cfi::parse_source_position(&cfi).unwrap());
    assert!(reader.set_cfi_position(&point));
    frame(&mut reader, 400.0);
    let position = reader.current_cfi().unwrap();
    reader.set_column_width(170.0);
    reader.set_root_font_size(27.0);
    frame(&mut reader, 170.0);
    assert_eq!(reader.current_cfi().as_deref(), Some(position.as_str()));
    assert!(reader.annotation_bounds("stable").is_some());

    let width_before = (0..reader.document().render_view().text().glyph_count() as u32)
        .map(|glyph| {
            reader
                .document()
                .render_view()
                .text()
                .character_advance(glyph)
                .unwrap_or_default()
        })
        .sum::<f32>();
    let declaration = host.faces.borrow()[0].clone();
    let loaded = pollster::block_on(load_font_face(
        declaration,
        Arc::new(Fixture(String::new())),
        &text.all_font_names(),
    ))
    .unwrap();
    assert!(host.fonts.borrow_mut().register(loaded, &text).unwrap());
    reader.apply(RendererCommand::RefreshFonts);
    frame(&mut reader, 170.0);
    let width_after = (0..reader.document().render_view().text().glyph_count() as u32)
        .map(|glyph| {
            reader
                .document()
                .render_view()
                .text()
                .character_advance(glyph)
                .unwrap_or_default()
        })
        .sum::<f32>();
    assert!(
        (width_before - width_after).abs() > 1.0,
        "registered publication fonts must replace fallback geometry"
    );
    assert_eq!(reader.current_cfi().as_deref(), Some(position.as_str()));
    assert!(reader.annotation_bounds("stable").is_some());
    select(&mut reader, "office", "office");
}
