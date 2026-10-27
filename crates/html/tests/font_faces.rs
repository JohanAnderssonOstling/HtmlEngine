use html::engine::Engine;
use html::pipeline::{PipelineInputs, SourceRevision, StyleEnvironment};
use html::resources::FileSystemProvider;
use html::style::{FontFaceSource, FontFaceStyle};
use std::sync::Arc;

#[test]
fn engine_exposes_font_faces_before_a_text_backend_is_attached() {
    let mut engine = Engine::new(Arc::new(FileSystemProvider::new()));
    assert!(engine.font_faces().is_empty());
    let inputs = PipelineInputs {
        source: r#"<!doctype html><html><head><style>
            @font-face { font-family: Reader; src: url(fonts/reader.ttf); font-style: italic; }
        </style></head><body><p>Hello</p></body></html>"#
            .into(),
        base_uri: "/books/chapter.html".into(),
        ..inputs_defaults()
    };
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

fn inputs_defaults() -> PipelineInputs {
    use html::pipeline::{
        FontEnvironmentRevision, ImageMetricsRevision, LayoutConstraints, MarkupSyntax,
        PaintSettingsRevision, ResourceRevision, StylesheetRevision,
    };
    PipelineInputs {
        source: String::new().into(),
        source_revision: SourceRevision::INITIAL,
        base_uri: String::new(),
        markup_syntax: MarkupSyntax::Html,
        user_styles: Vec::new(),
        reader_overrides: Default::default(),
        note_flow: Default::default(),
        style_environment: StyleEnvironment::default(),
        resource_revision: ResourceRevision::INITIAL,
        stylesheet_revision: StylesheetRevision::INITIAL,
        font_environment: FontEnvironmentRevision::INITIAL,
        image_metrics_revision: ImageMetricsRevision::INITIAL,
        layout: LayoutConstraints {
            viewport_width: 600.0,
            viewport_height: None,
            line_height: 16.0,
            image_sizing_policy: html::layout::ImageSizingPolicy::WebCompatible,
            text_composition_policy: html::layout::TextCompositionPolicy::WebCompatible,
        },
        image_metrics: Default::default(),
        paint: PaintSettingsRevision::INITIAL,
    }
}

#[test]
fn independent_reader_fonts_reach_portable_shaping_for_mixed_text() {
    use html::pipeline::{DocumentFactory, ReaderStyleOverrides};
    use html::text::{
        FontSlant, GlyphId, GlyphMetric, GlyphResourceGeneration, GlyphResourceStore, GlyphShaper,
        ShapeError,
    };

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
        fn shape_glyph(
            &mut self,
            character: char,
            size: f32,
            _: u16,
            _: FontSlant,
            _: u32,
            family: Option<&str>,
            resources: &mut GlyphResourceStore,
        ) -> Result<GlyphId, ShapeError> {
            self.selected.push((character, family.map(str::to_owned)));
            let metric =
                GlyphMetric::try_new(character, size * 0.5, size * 0.8, size * 0.2, size * 0.8)
                    .map_err(ShapeError::rejected_metric)?;
            resources.register(metric)
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
        let prepared = factory.parse_with_new_pipeline(
            "<p style='font-family:Publisher'>Latin 漢字 かな 한국</p>",
            None,
        );
        let mut shaper = Recorder::default();
        let mut resources = GlyphResourceStore::default();
        prepared.shape(&mut shaper, &mut resources).unwrap();
        for character in ['L', 'a'] {
            assert!(shaper.selected.iter().any(
                |(ch, family)| *ch == character && family.as_deref() == Some("Selected Latin")
            ));
        }
        let expected = format!("{}, Embedded", cjk.unwrap_or("Publisher"));
        for character in ['漢', '字', 'か', '한'] {
            assert!(
                shaper
                    .selected
                    .iter()
                    .any(|(ch, family)| *ch == character
                        && family.as_deref() == Some(expected.as_str())),
                "{character}: {:?}",
                shaper.selected
            );
        }
    }
    let mut engine = Engine::new(Arc::new(FileSystemProvider::new()));
    let mut inputs = inputs_defaults();
    inputs.source = "<p style='font-family:Publisher'>Latin 漢字</p>".into();
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
