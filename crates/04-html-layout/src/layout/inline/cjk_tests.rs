use crate::parser::DocumentFactory;
use crate::test_support::TestGlyphShaper;
use crate::{
    FontMetricsRequest, FontRelativeMetrics, FontSlant, GlyphId, GlyphResourceStore, GlyphShaper,
    LaidOutDocument, LayoutConstraints, ShapeError, ShapedTextRun, TextCompositionPolicy,
    TextRunShapeRequest,
};
use std::sync::Arc;

#[derive(Default)]
struct RecordingShaper {
    fallback: TestGlyphShaper,
    languages: Vec<(String, Option<String>)>,
    metric_languages: Vec<Option<String>>,
    cluster_text: Option<&'static str>,
}

impl GlyphShaper for RecordingShaper {
    fn font_relative_metrics(
        &mut self,
        request: FontMetricsRequest<'_>,
    ) -> Result<FontRelativeMetrics, ShapeError> {
        self.metric_languages.push(
            request
                .language()
                .map(|language| language.as_str().to_owned()),
        );
        Ok(FontRelativeMetrics::fallback())
    }
    fn reset(&mut self) {
        self.fallback.reset();
    }
    fn begin_append_shaping(&mut self) -> Result<(), ShapeError> {
        self.fallback.begin_append_shaping()
    }
    fn commit_append_shaping(&mut self) {
        self.fallback.commit_append_shaping();
    }
    fn rollback_append_shaping(&mut self) {
        self.fallback.rollback_append_shaping();
    }
    fn glyph_resources(&mut self) -> &mut GlyphResourceStore {
        self.fallback.glyph_resources()
    }
    fn shape_glyph(
        &mut self,
        ch: char,
        size: f32,
        weight: u16,
        slant: FontSlant,
        color: u32,
        family: Option<&str>,
    ) -> Result<GlyphId, ShapeError> {
        self.fallback
            .shape_glyph(ch, size, weight, slant, color, family)
    }
    fn shape_text_run(
        &mut self,
        request: TextRunShapeRequest<'_>,
    ) -> Result<Option<ShapedTextRun>, ShapeError> {
        self.languages.push((
            request.text().to_owned(),
            request
                .style()
                .language()
                .map(|language| language.as_str().to_owned()),
        ));
        if self.cluster_text == Some(request.text()) {
            let count = request.text().chars().count();
            let mut boundaries = vec![true; count + 1];
            boundaries[1] = false;
            return Ok(ShapedTextRun::new(
                Arc::from(vec![10.0; count]),
                Arc::from(boundaries),
            ));
        }
        Ok(None)
    }
}

fn layout(body: &str, width: f64, policy: TextCompositionPolicy) -> LaidOutDocument {
    let html = format!("<body style='margin:0;font-size:20px'>{body}</body>");
    DocumentFactory::new()
        .parse_with_new_pipeline(&html, None)
        .shape(&mut TestGlyphShaper::new())
        .unwrap()
        .layout(
            LayoutConstraints::new(width, 16.0)
                .unwrap()
                .with_text_composition_policy(policy),
        )
}

fn lines(document: &LaidOutDocument) -> Vec<String> {
    let text = document.render_view().text();
    text.lines()
        .iter()
        .map(|line| {
            line.glyphs()
                .map(|index| {
                    text.glyph_metric(text.glyph_at(index as usize).unwrap())
                        .unwrap()
                        .ch()
                })
                .collect()
        })
        .collect()
}

fn web(body: &str, width: f64) -> Vec<String> {
    lines(&layout(body, width, TextCompositionPolicy::WebCompatible))
}

#[test]
fn cjk_horizontal_paragraphs_wrap_without_spaces() {
    for (text, expected) in [
        ("中文排版測試", vec!["中文排", "版測試"]),
        ("日本語の文章", vec!["日本語", "の文章"]),
        ("한국어문장", vec!["한국어", "문장"]),
        ("𠀀𰀀𲎰漢字", vec!["𠀀𰀀𲎰", "漢字"]),
    ] {
        assert_eq!(web(&format!("<p>{text}</p>"), 30.0), expected, "{text}");
    }
}

#[test]
fn cjk_punctuation_stays_with_its_contents_even_when_it_overflows() {
    assert_eq!(web("<p>甲（乙）丙</p>", 20.0), ["甲", "（乙）", "丙"]);
    assert_eq!(
        web("<p>甲<span>（乙</span>）丙</p>", 20.0),
        ["甲", "（乙）", "丙"]
    );
    assert_eq!(web("<p>甲 （乙） 丙</p>", 20.0), ["甲", "（乙）", "丙"]);
}

#[test]
fn cjk_normal_keeps_latin_words_and_numbers_together() {
    assert_eq!(web("<p>中文abc測試</p>", 30.0), ["中文", "abc", "測試"]);
    assert_eq!(web("<p>中文123測試</p>", 30.0), ["中文", "123", "測試"]);
    assert_eq!(
        web("<p style='word-break:break-all'>中文abc測試</p>", 30.0),
        ["中文a", "bc測", "試"]
    );
}

#[test]
fn cjk_keep_all_suppresses_letter_breaks_but_preserves_punctuation_breaks() {
    assert_eq!(
        web("<p style='word-break:keep-all'>한국어문장</p>", 20.0),
        ["한국어문장"]
    );
    assert_eq!(
        web("<p style='word-break:keep-all'>中文，文本</p>", 20.0),
        ["中文，", "文本"]
    );
    assert_eq!(
        web("<p style='word-break:keep-all'>한국어 문장</p>", 30.0),
        ["한국어", "문장"]
    );
}

#[test]
fn cjk_small_kana_and_iteration_marks_obey_line_break_strictness() {
    for text in ["あぁい", "山々川"] {
        let characters = text
            .chars()
            .map(|character| character.to_string())
            .collect::<Vec<_>>();
        for strictness in ["auto", "normal", "strict"] {
            assert_eq!(
                web(
                    &format!("<p lang='ja' style='line-break:{strictness}'>{text}</p>"),
                    10.0
                ),
                [
                    format!("{}{}", characters[0], characters[1]),
                    characters[2].clone()
                ]
            );
        }
        assert_eq!(
            web(
                &format!("<p lang='ja' style='line-break:loose'>{text}</p>"),
                10.0
            ),
            characters
        );
    }
}

#[test]
fn cjk_punctuation_tailoring_uses_inherited_content_language() {
    for character in ['〜', '゠', '・', '！', '？', '：'] {
        let strictness = if matches!(character, '〜' | '゠') {
            "normal"
        } else {
            "loose"
        };
        let body =
            format!("<div lang='ja'><p style='line-break:{strictness}'>山{character}川</p></div>");
        assert_eq!(
            web(&body, 10.0),
            ["山".to_owned(), character.to_string(), "川".to_owned()]
        );
        let strict = format!("<p lang='ja' style='line-break:strict'>山{character}川</p>");
        assert_eq!(
            web(&strict, 10.0),
            [format!("山{character}"), "川".to_owned()]
        );
        let english = format!("<p lang='en' style='line-break:{strictness}'>山{character}川</p>");
        assert_eq!(
            web(&english, 10.0),
            [format!("山{character}"), "川".to_owned()]
        );
    }
}

#[test]
fn cjk_tailoring_uses_structured_scripts_and_respects_language_resets() {
    for language in [
        "zh-Hant-TW-u-nu-hanidec-x-reader",
        "und-Hans",
        "ja-Jpan",
        "yue-Hant-HK",
    ] {
        assert_eq!(
            web(
                &format!("<p lang='{language}' style='line-break:loose'>山・川</p>"),
                10.0
            ),
            ["山", "・", "川"],
            "{language}"
        );
    }
    for language in ["ja-Latn", "en_US", ""] {
        assert_eq!(
            web(
                &format!(
                    "<div lang='ja'><p lang='{language}' style='line-break:loose'>山・川</p></div>"
                ),
                10.0
            ),
            ["山・", "川"],
            "{language}"
        );
    }
}

#[test]
fn cjk_anywhere_overrides_punctuation_and_keep_all_restrictions() {
    assert_eq!(
        web(
            "<p style='word-break:keep-all;line-break:anywhere'>（中文）</p>",
            10.0
        ),
        ["（", "中", "文", "）"]
    );
    assert_eq!(
        web("<p style='line-break:anywhere'>a&nbsp;b</p>", 10.0),
        ["a", "\u{a0}", "b"]
    );
}

#[test]
fn cjk_break_all_retains_punctuation_restrictions_for_latin_too() {
    assert_eq!(
        web("<p style='word-break:break-all'>a(b)c</p>", 10.0),
        ["a", "(b)", "c"]
    );
}

#[test]
fn cjk_graphemes_survive_different_inline_styles_and_anywhere_wrapping() {
    assert_eq!(
        web(
            "<p style='line-break:anywhere'>か<span style='color:red'>゙</span>な</p>",
            10.0
        ),
        ["が", "な"]
    );
    assert_eq!(
        web("<p style='line-break:anywhere'>👩‍👩‍👧‍👦中文</p>", 10.0),
        ["👩‍👩‍👧‍👦", "中", "文"]
    );
    assert_eq!(
        web("<p style='line-break:anywhere'>한글</p>", 10.0),
        ["한", "글"]
    );
}

#[test]
fn cjk_respects_preserved_and_forced_line_breaks() {
    assert_eq!(
        web("<p style='white-space:nowrap'>中文測試</p>", 10.0),
        ["中文測試"]
    );
    assert_eq!(
        web("<p style='white-space:pre'>中文\n測試</p>", 10.0),
        ["中文", "測試"]
    );
    assert_eq!(web("<p>中文<br>測試</p>", 30.0), ["中文", "測試"]);
    assert_eq!(
        web("<p style='white-space:pre-line'>中文\n測試</p>", 10.0),
        ["中", "文", "測", "試"]
    );
}

#[test]
fn cjk_inline_boundaries_use_the_common_ancestors_white_space() {
    for (body, expected) in [
        (
            "<p><span style='white-space:nowrap'>中</span><span style='white-space:nowrap'>文</span></p>",
            vec!["中", "文"],
        ),
        (
            "<p style='white-space:nowrap'><span style='white-space:normal'>中</span><span style='white-space:normal'>文</span></p>",
            vec!["中文"],
        ),
        (
            "<p style='white-space:nowrap'><span style='white-space:normal'><em style='white-space:nowrap'>中</em><b style='white-space:nowrap'>文</b></span></p>",
            vec!["中", "文"],
        ),
        (
            "<p style='white-space:nowrap'><span style='white-space:normal'>中文</span></p>",
            vec!["中", "文"],
        ),
        (
            "<p style='white-space:nowrap'><span style='white-space:normal'>中 </span>文</p>",
            vec!["中", "文"],
        ),
        (
            "<p><span style='white-space:nowrap'>中 </span>文</p>",
            vec!["中 文"],
        ),
    ] {
        for policy in [
            TextCompositionPolicy::WebCompatible,
            TextCompositionPolicy::BookOptimized,
        ] {
            assert_eq!(lines(&layout(body, 10.0, policy)), expected, "{body}");
        }
    }
}

#[test]
fn cjk_inline_boundary_min_content_uses_the_common_ancestors_white_space() {
    for (css, child_css, expected) in [
        ("white-space:normal", "white-space:nowrap", 10.0),
        ("white-space:nowrap", "white-space:normal", 20.0),
    ] {
        let document = layout(
            &format!(
                "<div id='target' style='display:inline-block;width:min-content;{css}'><span style='{child_css}'>中</span><span style='{child_css}'>文</span></div>"
            ),
            300.0,
            TextCompositionPolicy::WebCompatible,
        );
        let view = document.render_view();
        let target = (0..view.boxes().len())
            .find(|&index| view.boxes().id(index).map(|id| view.string(id)) == Some("target"))
            .unwrap();
        assert_eq!(view.boxes().size(target).unwrap().width, expected, "{css}");
    }
}

#[test]
fn cjk_opportunities_cross_inline_runs_without_creating_breaks_between_blocks() {
    assert_eq!(
        web(
            "<p>中<span style='color:red'>文</span><em>排</em>版</p><p>測試</p>",
            20.0
        ),
        ["中文", "排版", "測試"]
    );
}

#[test]
fn cjk_intrinsic_min_content_uses_the_same_unicode_boundaries() {
    for container in [
        "display:inline-block;width:min-content",
        "display:grid;grid-template-columns:min-content;width:min-content",
        "display:flex;width:min-content",
    ] {
        let body = format!("<div id='target' style='{container}'><div>中文測試</div></div>");
        let document = layout(&body, 300.0, TextCompositionPolicy::WebCompatible);
        let view = document.render_view();
        let target = (0..view.boxes().len())
            .find(|&index| view.boxes().id(index).map(|id| view.string(id)) == Some("target"))
            .unwrap();
        assert!(
            (view.boxes().size(target).unwrap().width - 10.0).abs() < 0.01,
            "{container}: {:?}",
            view.boxes().size(target)
        );
        // Inline-block carriers also publish a line containing the atomic
        // box; it has no source glyphs of its own.
        assert_eq!(
            lines(&document)
                .into_iter()
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>(),
            ["中", "文", "測", "試"],
            "{container}"
        );
    }
}

#[test]
fn cjk_book_composition_preserves_characters_and_punctuation_rules() {
    let document = layout(
        "<p>中文排版，測試文章。日本語の文章。</p>",
        50.0,
        TextCompositionPolicy::BookOptimized,
    );
    let lines = lines(&document);
    assert!(lines.len() > 1, "{lines:?}");
    assert_eq!(lines.concat(), "中文排版，測試文章。日本語の文章。");
    assert!(
        lines
            .iter()
            .all(|line| !line.starts_with(['，', '。', '）']) && !line.ends_with('（')),
        "{lines:?}"
    );
}

#[test]
fn cjk_native_clusters_are_respected_by_layout_and_intrinsic_sizing() {
    for word_break in ["normal", "break-all"] {
        let body = format!(
            "<body style='margin:0;font-size:20px'><div id='target' style='display:inline-block;width:min-content;word-break:{word_break}'>中文測試</div></body>"
        );
        let prepared = DocumentFactory::new().parse_with_new_pipeline(&body, None);
        let mut shaper = RecordingShaper {
            cluster_text: Some("中文測試"),
            ..Default::default()
        };
        let document = prepared
            .shape(&mut shaper)
            .unwrap()
            .layout(LayoutConstraints::new(10.0, 16.0).unwrap());
        let view = document.render_view();
        let target = (0..view.boxes().len())
            .find(|&index| view.boxes().id(index).map(|id| view.string(id)) == Some("target"))
            .unwrap();
        assert_eq!(view.boxes().size(target).unwrap().width, 20.0);
        assert_eq!(
            lines(&document)
                .into_iter()
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>(),
            ["中文", "測試"]
        );
    }
}

#[test]
fn cjk_font_metrics_cache_keeps_region_languages_separate() {
    let html = "<p lang='ja'><span style='background:red'>日</span></p>\
        <p lang='zh-Hant'><span style='background:red'>漢</span></p>\
        <p lang='ko'><span style='background:red'>한</span></p>";
    let prepared = DocumentFactory::new().parse_with_new_pipeline(html, None);
    let mut shaper = RecordingShaper::default();
    prepared.shape(&mut shaper).unwrap();
    for language in ["ja", "zh-Hant", "ko"] {
        assert!(
            shaper
                .metric_languages
                .iter()
                .any(|tag| tag.as_deref() == Some(language)),
            "{language}: {:?}",
            shaper.metric_languages
        );
    }
}

#[test]
fn cjk_shaping_receives_language_for_each_region_and_first_line() {
    let body = "<body lang='ja'><p id='first'>日本語</p><p lang='zh-Hant'>漢字</p><p lang='ko'>한국어</p><p lang='en'>Latin</p></body>";
    let prepared = DocumentFactory::new()
        .parse_with_new_pipeline(body, Some("#first::first-line {font-size:24px}"));
    let mut shaper = RecordingShaper::default();
    let document = prepared
        .shape(&mut shaper)
        .unwrap()
        .layout_with_metrics_and_shaper(
            LayoutConstraints::new(300.0, 16.0).unwrap(),
            &crate::ImageMetrics::default(),
            &mut shaper,
        )
        .unwrap();
    assert!(document.line_count() >= 4);
    for (text, language) in [
        ("日本語", "ja"),
        ("漢字", "zh-Hant"),
        ("한국어", "ko"),
        ("Latin", "en"),
    ] {
        assert!(
            shaper
                .languages
                .iter()
                .any(|(run, tag)| run == text && tag.as_deref() == Some(language)),
            "{text}: {:?}",
            shaper.languages
        );
    }
    assert!(
        shaper
            .languages
            .iter()
            .filter(|(text, _)| text == "日本語")
            .count()
            >= 2,
        "first-line must be reshaped"
    );
    assert!(
        shaper
            .languages
            .iter()
            .filter(|(text, _)| text == "日本語")
            .all(|(_, tag)| tag.as_deref() == Some("ja"))
    );
}

#[test]
fn cjk_source_newlines_join_without_inserting_spaces_while_korean_and_latin_keep_them() {
    assert_eq!(web("<p>中文\n測試</p>", 300.0), ["中文測試"]);
    assert_eq!(web("<p>中 \n <span>文</span></p>", 300.0), ["中文"]);
    assert_eq!(web("<p>中 文</p>", 300.0), ["中 文"]);
    assert_eq!(web("<p>한국어\n문장</p>", 300.0), ["한국어 문장"]);
    assert_eq!(web("<p>hello\nworld</p>", 300.0), ["hello world"]);
    assert_eq!(web("<p>中文\nLatin</p>", 300.0), ["中文 Latin"]);
    assert_eq!(web("<p>漢&#xE0100;\n字</p>", 300.0), ["漢\u{e0100}字"]);
    assert_eq!(web("<p lang='ko'>漢\n字</p>", 300.0), ["漢 字"]);
}

#[test]
fn cjk_newline_context_stops_at_atomic_boxes_and_other_blocks() {
    for previous in ["中", "A"] {
        assert_eq!(
            web(
                &format!(
                    "<p>{previous}</p><p><span style='display:inline-block;width:10px'></span>\n文</p>"
                ),
                300.0
            ),
            [previous, " 文"]
        );
    }
    for content in ["中", "A"] {
        let document = DocumentFactory::new().parse_with_new_pipeline(
            &format!("<p><span style='display:inline-block'>{content}</span>\n文</p>"),
            None,
        );
        let text: String = document
            .inline_content()
            .characters()
            .iter()
            .filter_map(|&glyph| char::from_u32(glyph))
            .collect();
        assert_eq!(text, format!("{content} 文"));
    }
}

#[test]
fn cjk_newline_normalization_ignores_bidi_controls_and_retains_their_order() {
    for control in [
        '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
        '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
    ] {
        for (source, expected) in [
            (format!("中{control}\n文"), format!("中{control}文")),
            (format!("中\n{control}文"), format!("中{control}文")),
            (
                format!("中 {control}\n {control}文"),
                format!("中{control}{control}文"),
            ),
            (format!("A\n{control}B"), format!("A {control}B")),
            (format!("A {control} B"), format!("A {control}B")),
            (format!(" {control}A"), format!("{control}A")),
            (format!("A {control} "), format!("A{control}")),
            (format!("A {control}<br>B"), format!("A{control}B")),
            (format!("A {control}</p><p>B"), format!("A{control}B")),
        ] {
            let document =
                DocumentFactory::new().parse_with_new_pipeline(&format!("<p>{source}</p>"), None);
            let text: String = document
                .inline_content()
                .characters()
                .iter()
                .filter_map(|&glyph| char::from_u32(glyph))
                .collect();
            assert_eq!(text, expected, "{source:?}");
        }
    }
}

#[test]
fn cjk_newline_normalization_respects_explicit_writing_systems() {
    for (language, expected) in [
        ("ko-Hani", "漢字"),
        ("ko-Hans", "漢字"),
        ("ko-Jpan", "漢字"),
        ("en-Hang", "漢 字"),
        ("zh-Kore", "漢 字"),
        ("und-Jamo", "漢 字"),
        ("ko", "漢 字"),
        ("zh", "漢字"),
    ] {
        assert_eq!(
            web(&format!("<p lang='{language}'>漢\n字</p>"), 300.0),
            [expected],
            "{language}"
        );
    }
}

#[test]
fn cjk_min_content_keeps_punctuation_groups_and_max_content_remains_unwrapped() {
    for (css, text, expected) in [
        ("width:min-content", "甲（乙）丙", 30.0),
        ("width:min-content;word-break:keep-all", "中文測試", 40.0),
        ("width:max-content", "中文測試", 40.0),
        ("width:min-content;word-break:break-all", "abcd", 10.0),
        ("width:min-content;white-space:nowrap", "中文測試", 40.0),
    ] {
        let document = layout(
            &format!("<div id='target' style='display:inline-block;{css}'>{text}</div>"),
            300.0,
            TextCompositionPolicy::WebCompatible,
        );
        let view = document.render_view();
        let target = (0..view.boxes().len())
            .find(|&index| view.boxes().id(index).map(|id| view.string(id)) == Some("target"))
            .unwrap();
        assert_eq!(
            view.boxes().size(target).unwrap().width,
            expected,
            "{css}, {text}"
        );
    }
}

#[test]
fn cjk_latin_preparation_keeps_the_existing_wrap_metadata_and_empty_unicode_path() {
    use crate::layout_model::WhitespaceWrapOverride;
    for text in [
        "ordinary English text",
        "Français, español, svenska: café, naïve, Ångström",
    ] {
        let prepared =
            DocumentFactory::new().parse_with_new_pipeline(&format!("<p>{text}</p>"), None);
        assert!(!prepared.inline_content().has_unicode_wraps());
        for index in 0..prepared.inline_content().characters().len() {
            assert!(!matches!(
                prepared.inline_content().whitespace_wrap_before(index),
                WhitespaceWrapOverride::UnicodeAllow | WhitespaceWrapOverride::UnicodeSuppress
            ));
        }
    }
}

#[test]
fn cjk_loose_hyphens_suffixes_and_prefixes_keep_the_required_context() {
    for character in ['‐', '–'] {
        assert_eq!(
            web(
                &format!("<p style='line-break:loose'>山{character}川</p>"),
                10.0
            ),
            ["山".to_owned(), character.to_string(), "川".to_owned()]
        );
        assert_eq!(
            web(
                &format!("<p style='line-break:strict'>山{character}川</p>"),
                10.0
            ),
            [format!("山{character}"), "川".to_owned()]
        );
    }
    assert_eq!(
        web("<p lang='ja' style='line-break:loose'>￥山％</p>", 10.0),
        ["￥", "山", "％"]
    );
    assert_eq!(
        web("<p lang='ja' style='line-break:strict'>￥山％</p>", 10.0),
        ["￥山％"]
    );
    assert_eq!(
        web("<p style='line-break:normal'>山……川</p>", 10.0),
        ["山……", "川"]
    );
    assert_eq!(
        web("<p style='line-break:loose'>山……川</p>", 10.0),
        ["山…", "…", "川"]
    );
}

#[test]
fn cjk_breaks_inside_tables_and_around_atomic_inline_and_floated_content() {
    let document = layout(
        "<table style='border-spacing:0;width:1px'><tr><td id='target' style='padding:0'>中文測試</td></tr></table>",
        300.0,
        TextCompositionPolicy::WebCompatible,
    );
    let view = document.render_view();
    let target = (0..view.boxes().len())
        .find(|&index| view.boxes().id(index).map(|id| view.string(id)) == Some("target"))
        .unwrap();
    assert_eq!(view.boxes().size(target).unwrap().width, 10.0);
    assert_eq!(lines(&document), ["中", "文", "測", "試"]);
    let document = layout(
        "<p>中文<span style='display:inline-block'>甲乙</span>測試</p>",
        20.0,
        TextCompositionPolicy::WebCompatible,
    );
    assert_eq!(lines(&document).concat(), "中文甲乙測試");
    let document = layout(
        "<div><span style='float:left;width:10px;height:20px'></span>中文測試</div>",
        30.0,
        TextCompositionPolicy::WebCompatible,
    );
    assert_eq!(lines(&document).concat(), "中文測試");
    assert!(document.line_count() >= 2);
}

#[test]
fn cjk_bidi_wrapping_preserves_source_characters() {
    let document = layout(
        "<p dir='rtl'>中文אבג測試</p>",
        30.0,
        TextCompositionPolicy::WebCompatible,
    );
    assert_eq!(lines(&document), ["中文", "אבג", "測試"]);
    let text = document.render_view().text();
    assert_eq!(text.character_direction(0), crate::TextDirection::Ltr);
    assert_eq!(text.character_direction(2), crate::TextDirection::Rtl);
}
