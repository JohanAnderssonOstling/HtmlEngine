use html_engine_model::shaping_contracts;
use html_pipeline::PipelineSession;
use html_pipeline::{PipelineInputs, StyleEnvironment};
use html_pipeline::MarkupSyntax;
use crate::resources::FileSystemProvider;
use html_pipeline::engine::LayoutConstraints;
use html_style_model::FontFaceSource;
use html_style_model::FontFaceStyle;
use std::sync::Arc;

#[test]
fn pipeline_exposes_font_faces_before_a_text_backend_is_attached() {
    let mut engine = PipelineSession::new(Arc::new(FileSystemProvider::new()));
    assert!(engine.font_faces().is_empty());
    let inputs = PipelineInputs::new(
        r#"<!doctype html><html><head><style>
            @font-face { font-family: Reader; src: url(fonts/reader.ttf); font-style: italic; }
        </style></head><body><p>Hello</p></body></html>"#
            .into(),
        MarkupSyntax::Html,
        "/books/chapter.html".into(),
        StyleEnvironment::default(),
        LayoutConstraints::new(600.0, 16.0).unwrap(),
    );
    engine
        .prepare_through_style(inputs)
        .expect("preparation must not load the missing font file");
    let face = &engine.font_faces()[0];
    assert_eq!(face.family, "Reader");
    assert_eq!(face.style, FontFaceStyle::Italic);
    assert!(
        matches!(&face.sources[0], FontFaceSource::Url { url, .. } if url == "/books/fonts/reader.ttf")
    );
    assert!(engine.document().is_none());
}

#[test]
fn independent_reader_fonts_reach_portable_shaping_for_mixed_text() {
    use html_pipeline::DocumentFactory;
use crate::style::ReaderStyleOverrides;
    use crate::text::{
        GlyphRequest, GlyphResourceGeneration, GlyphShaper, ShapeError, FontMetrics, GlyphHandle,
        ShapedGlyph,
    };
    use html_pipeline::engine::GlyphResourceStore;

    #[derive(Default)]
    struct Recorder {
        generation: GlyphResourceGeneration,
        selected: Vec<(char, Option<String>)>,
    }
    impl GlyphShaper for Recorder {
        fn reset(&mut self) {
            self.generation = GlyphResourceGeneration::default();
            self.selected.clear();
        }
        fn resource_generation(&self) -> GlyphResourceGeneration {
            self.generation
        }
        fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
            self.selected.push((
                request.character,
                shaping_contracts::shaping_style_font_family_for_character(&request
                    .style, request.character)
                    .map(str::to_owned),
            ));
            let advance = request.style.font_size * 0.5;
            let metrics = FontMetrics {
                ascent: request.style.font_size * 0.8,
                descent: request.style.font_size * 0.2,
            };
            Ok(ShapedGlyph::new(
                GlyphHandle(request.character as u32),
                advance,
                metrics,
            ))
        }
    }
    for cjk in [None, Some("Selected CJK")] {
        let mut factory = DocumentFactory::new();
        factory.set_reader_overrides(ReaderStyleOverrides {
            font_family: Some("Selected Latin".into()),
            cjk_font_family: cjk.map(str::to_owned),
            cjk_font_fallback: Some("Embedded".into()),
            ..Default::default()
        });
        for source in [
            "<p style='font-family:Publisher'>Latin 漢字 かな 한국</p>",
            "<pre style='font-family:Publisher'>Latin\n漢字 かな 한국</pre>",
        ] {
            let prepared = factory.parse_with_new_pipeline(source, None);
            let mut shaper = Recorder::default();
            let mut resources = GlyphResourceStore::default();
            prepared.shape(&mut shaper, &mut resources).unwrap();
            assert_eq!(
                shaper
                    .selected
                    .iter()
                    .any(|(character, _)| *character == '\n'),
                source.contains('\n'),
            );
            for character in ['L', 'a'] {
                assert!(
                    shaper.selected.iter().any(|(ch, family)| *ch == character
                        && family.as_deref() == Some("Selected Latin"))
                );
            }
            let expected = format!("{}, Embedded", cjk.unwrap_or("Publisher"));
            for character in ['漢', '字', 'か', '한'] {
                assert!(
                    shaper.selected.iter().any(|(ch, family)| *ch == character
                        && family.as_deref() == Some(expected.as_str())),
                    "{character}: {:?}",
                    shaper.selected
                );
            }
        }
    }
    let mut engine = PipelineSession::new(Arc::new(FileSystemProvider::new()));
    let mut inputs = PipelineInputs::new(
        "<p style='font-family:Publisher'>Latin 漢字</p>".into(),
        MarkupSyntax::Html,
        String::new(),
        StyleEnvironment::default(),
        LayoutConstraints::new(600.0, 16.0).unwrap(),
    );
    let mut shaper = Recorder::default();
    for (latin, cjk) in [
        ("First Latin", None),
        ("First Latin", Some("Selected CJK")),
        ("Second Latin", None),
    ] {
        inputs.reader_overrides = ReaderStyleOverrides {
            font_family: Some(latin.into()),
            cjk_font_family: cjk.map(str::to_owned),
            cjk_font_fallback: Some("Embedded".into()),
            ..Default::default()
        };
        engine.update(inputs.clone(), &mut shaper).unwrap();
        let expected = format!("{}, Embedded", cjk.unwrap_or("Publisher"));
        assert!(
            shaper
                .selected
                .iter()
                .any(|(ch, family)| *ch == 'L' && family.as_deref() == Some(latin))
        );
        assert!(
            shaper
                .selected
                .iter()
                .any(|(ch, family)| *ch == '漢' && family.as_deref() == Some(expected.as_str())),
            "restyling: {:?}",
            shaper.selected
        );
    }
}

#[test]
fn mirrored_cjk_glyphs_keep_selected_fonts_and_first_line_typography() {
    use html_pipeline::DocumentFactory;
use crate::style::ReaderStyleOverrides;
    use crate::text::FontSlant;
    use crate::text::GlyphRequest;
    use html_engine_model::GlyphWriter;
    use html_engine_model::ResourceShaper;
    use crate::text::FontMetrics;
    use crate::text::GlyphHandle;
    use crate::text::ShapedGlyph;
    use html_pipeline::engine::GlyphResourceStore;

    type Selection = (char, f32, u16, FontSlant, Option<String>);
    let mut shaper = ResourceShaper::new(
        |request: GlyphRequest<'_>, native: &mut GlyphWriter<'_, Selection>| {
            let handle = native.insert((
                request.character,
                request.style.font_size,
                request.style.font_weight,
                request.style.font_slant,
                shaping_contracts::shaping_style_font_family_for_character(&request
                    .style, request.character)
                    .map(str::to_owned),
            ))?;
            let advance = request.style.font_size * 0.5;
            let metrics = FontMetrics {
                ascent: request.style.font_size * 0.8,
                descent: request.style.font_size * 0.2,
            };
            Ok(ShapedGlyph::new(handle, advance, metrics))
        },
    );
    let mut factory = DocumentFactory::new();
    factory.set_reader_overrides(ReaderStyleOverrides {
        font_family: Some("Latin".into()),
        cjk_font_family: Some("CJK".into()),
        ..Default::default()
    });
    let prepared = factory.parse_with_new_pipeline(
        "<p dir='rtl' style='font-size:16px'>《אב</p>",
        Some("p::first-line { font-size:32px; font-weight:700; font-style:italic }"),
    );
    let mut resources = GlyphResourceStore::default();
    let shaped = prepared.shape(&mut shaper, &mut resources).unwrap();
    let selected = |shaper: &ResourceShaper<Selection, _>, size, weight, slant| {
        (0..)
            .map_while(|id| shaper.resources().get(GlyphHandle(id)))
            .any(|(character, font_size, font_weight, font_slant, family)| {
                *character == '》'
                    && *font_size == size
                    && *font_weight == weight
                    && *font_slant == slant
                    && family.as_deref() == Some("CJK")
            },
        )
    };
    assert!(selected(&shaper, 16.0, 400, FontSlant::Normal));
    shaped
        .layout_with_shaper(
            LayoutConstraints::new(600.0, 16.0).unwrap(),
            &mut shaper,
            &mut resources,
        )
        .unwrap();
    assert!(selected(&shaper, 32.0, 700, FontSlant::Italic));
}
