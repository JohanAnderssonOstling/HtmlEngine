use html_style::{
    AuthorStylesheetInput, FontFaceDescriptor, FontFaceSource, FontFaceStyle, MediaEnvironment,
    MediaType, StyleProgram, StyleProgramCache, resolve_font_face_urls, style_document,
    style_document_with_cached_program_and_timings, style_document_with_environment,
};

fn document() -> html_dom::Document {
    html_parse::parse_dom_document("<!doctype html><html><body><p>Hello</p></body></html>")
        .expect("valid DOM")
}

fn environment(width: f64) -> MediaEnvironment {
    MediaEnvironment::new(MediaType::Screen, width, None).expect("valid viewport")
}

#[test]
fn font_faces_expose_decoded_sources_and_matching_descriptors() {
    let styled = style_document(
        document(),
        &[r#"
        @font-face {
            font-family: "Reader\20 Serif";
            src: local("Reader\20 Installed"),
                 url("fonts/reader\20 regular.woff2") format(woff2) tech(variations),
                 url(reader.ttf) format("truetype");
            font-weight: bold;
            font-style: italic;
            font-stretch: condensed;
            unicode-range: U+0-7F, U+4??;
            font-display: swap;
        }
    "#],
    );
    let face = &styled.font_faces()[0];
    assert_eq!(face.family, "Reader Serif");
    assert_eq!(face.weight, 700.0..=700.0);
    assert_eq!(face.style, FontFaceStyle::Italic);
    assert_eq!(face.stretch, 75.0..=75.0);
    assert_eq!(face.unicode_ranges, [0..=0x7f, 0x400..=0x4ff]);
    assert_eq!(
        face.sources,
        [
            FontFaceSource::Local("Reader Installed".into()),
            FontFaceSource::Url {
                url: "fonts/reader regular.woff2".into(),
                format: Some("woff2".into()),
                technologies: vec!["variations".into()],
            },
            FontFaceSource::Url {
                url: "reader.ttf".into(),
                format: Some("truetype".into()),
                technologies: vec![],
            },
        ]
    );
    assert_eq!(
        face.additional_descriptors,
        [FontFaceDescriptor {
            name: "font-display".into(),
            value: "swap".into()
        }]
    );
}

#[test]
fn font_faces_keep_variable_ranges_and_unknown_format_hints() {
    let styled = style_document(
        document(),
        &[r#"
        @font-face {
            font-family: Variable Reader;
            src: url(variable.bin) format("future-format");
            font-weight: 123.5 850;
            font-stretch: 80% 120%;
            font-style: oblique 0.05turn 30deg;
        }
    "#],
    );
    let face = &styled.font_faces()[0];
    assert_eq!(face.family, "Variable Reader");
    assert_eq!(face.weight, 123.5..=850.0);
    assert!((*face.stretch.start() - 80.0).abs() < 0.0001);
    assert!((*face.stretch.end() - 120.0).abs() < 0.0001);
    assert_eq!(face.style, FontFaceStyle::Oblique(18.0..=30.0));
    assert!(
        matches!(&face.sources[0], FontFaceSource::Url { format: Some(format), .. } if format == "future-format")
    );
}

#[test]
fn font_faces_use_defaults_and_the_last_valid_descriptor() {
    let styled = style_document(
        document(),
        &[r#"
        @font-face { font-family: Defaults; src: url(defaults.ttf); }
        @font-face {
            font-family: Old; font-family: "serif"; font-family: serif;
            src: url(old.ttf); src: local("New"), url(new.ttf); src: bogus;
            font-weight: 500; font-weight: bolder; font-weight: 900 100;
            font-style: italic; font-style: oblique 120deg;
            font-stretch: 80%; font-stretch: -10%;
            unicode-range: U+41; unicode-range: bogus;
        }
    "#],
    );
    let faces = styled.font_faces();
    assert_eq!(faces.len(), 2);
    assert_eq!(faces[0].weight, 400.0..=400.0);
    assert_eq!(faces[0].style, FontFaceStyle::Normal);
    assert_eq!(faces[0].stretch, 100.0..=100.0);
    assert_eq!(faces[0].unicode_ranges, [0..=0x10ffff]);
    assert_eq!(
        faces[1].family, "serif",
        "quoted generic names are valid custom names"
    );
    assert_eq!(faces[1].weight, 500.0..=500.0);
    assert_eq!(faces[1].style, FontFaceStyle::Italic);
    assert_eq!(faces[1].stretch, 80.0..=80.0);
    assert_eq!(faces[1].unicode_ranges, [0x41..=0x41]);
    assert!(matches!(&faces[1].sources[0], FontFaceSource::Local(name) if name == "New"));
}

#[test]
fn font_faces_skip_unusable_rules_and_preserve_duplicate_family_faces() {
    let styled = style_document(
        document(),
        &[r#"
        @font-face { src: url(no-family.ttf); }
        @font-face { font-family: NoSource; }
        @font-face { font-family: serif; src: url(generic.ttf); }
        @font-face { font-family: ""; src: url(empty.ttf); }
        @font-face { font-family: Reader; src: url(regular.ttf); }
        @font-face { font-family: Reader; src: url(bold.ttf); font-weight: bold; }
    "#],
    );
    assert_eq!(styled.font_faces().len(), 2);
    assert_eq!(styled.font_faces()[0].family, "Reader");
    assert_eq!(styled.font_faces()[1].family, "Reader");
    assert_eq!(styled.font_faces()[1].weight, 700.0..=700.0);
}

#[test]
fn font_only_media_rules_invalidate_style_and_respect_supports() {
    let css = r#"
        @media (min-width: 700px) {
            @layer fonts {
                @font-face { font-family: Wide; src: url(wide.ttf); }
            }
        }
        @supports (display: block) {
            @font-face { font-family: Supported; src: local(Supported); }
        }
        @supports (unknown-property: bogus) {
            @font-face { font-family: Unsupported; src: url(no.ttf); }
        }
    "#;
    let (small, queries) = style_document_with_environment(document(), &[css], environment(600.0));
    let (large, _) = style_document_with_environment(document(), &[css], environment(800.0));
    assert_eq!(
        small
            .font_faces()
            .iter()
            .map(|face| face.family.as_str())
            .collect::<Vec<_>>(),
        ["Supported"]
    );
    assert_eq!(
        large
            .font_faces()
            .iter()
            .map(|face| face.family.as_str())
            .collect::<Vec<_>>(),
        ["Wide", "Supported"]
    );
    assert_ne!(
        queries.match_key(environment(600.0), 16.0),
        queries.match_key(environment(800.0), 16.0)
    );
}

#[test]
fn font_faces_survive_both_program_cache_paths() {
    let css =
        "@media (min-width: 700px) { @font-face { font-family: Reader; src: url(reader.woff2) } }";
    let inputs = [AuthorStylesheetInput {
        css,
        implicit_scope_root: None,
    }];
    let program = StyleProgram::compile(&[css], environment(800.0), 16.0);
    let mut cache = StyleProgramCache::default();
    for width in [800.0, 600.0, 800.0] {
        let (cached, _, _) = style_document_with_cached_program_and_timings(
            &mut cache,
            document(),
            &inputs,
            environment(width),
        );
        let (fresh, _) = style_document_with_environment(document(), &[css], environment(width));
        assert_eq!(cached.font_faces(), fresh.font_faces());
        if width == 800.0 {
            assert_eq!(cached.font_faces(), program.font_faces());
        }
    }
    let stats = cache.stats();
    assert_eq!((stats.hits, stats.misses, stats.parsed_hits), (1, 2, 1));
}

#[test]
fn font_url_resolution_preserves_css_escaping_and_source_metadata() {
    let css = r#"
        @media screen { @supports (display: block) { @layer fonts {
            @font\2d face {
                font-family: "Reader";
                src: local("Installed"), url("font\20 file.woff2") format(woff2) tech(variations);
            }
        } } }
        p { background-image: url(unchanged.png); }
    "#;
    let resolved = resolve_font_face_urls(css, |url| format!("book/\"quoted\"/{url}"));
    assert!(resolved.contains("unchanged.png"));
    let styled = style_document(document(), &[&resolved]);
    let sources = &styled.font_faces()[0].sources;
    assert_eq!(sources[0], FontFaceSource::Local("Installed".into()));
    assert!(
        matches!(&sources[1], FontFaceSource::Url { url, format: Some(format), technologies }
        if url == "book/\"quoted\"/font file.woff2" && format == "woff2" && technologies == &["variations"])
    );
    let ordinary = "p { color: red }";
    assert!(matches!(
        resolve_font_face_urls(ordinary, |_| panic!("no font URL")),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn absolute_and_data_font_urls_are_preserved_without_rebasing() {
    let css = r#"@font-face { font-family: Reader;
        src: url("data:font/woff2;base64,AA==") format(woff2),
             url("https://example.com/reader.ttf#face"), url(relative.ttf);
    }"#;
    let mut seen = Vec::new();
    let resolved = resolve_font_face_urls(css, |url| {
        seen.push(url.to_owned());
        format!("book/{url}")
    });
    assert_eq!(seen, ["relative.ttf"]);
    let styled = style_document(document(), &[&resolved]);
    let urls = styled.font_faces()[0]
        .sources
        .iter()
        .filter_map(|source| match source {
            FontFaceSource::Url { url, .. } => Some(url.as_str()),
            FontFaceSource::Local(_) => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        urls,
        [
            "data:font/woff2;base64,AA==",
            "https://example.com/reader.ttf#face",
            "book/relative.ttf"
        ]
    );
}
