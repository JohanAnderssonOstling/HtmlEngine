use html_dom::{DomNodeId, RootFontSize};
use html_style::{
    AuthorStylesheetInput, MediaEnvironment, StyleProgramCache, StyledDocument,
    style_document_with_cached_program_and_reader_overrides,
};
use html_style_model::{LengthPct, PreferredSize, ReaderStyleOverrides, StyleIndices};

fn styled(source: &str, desired: f32, canonical: Option<f32>) -> StyledDocument {
    let mut document = html_parse::parse_dom_document(source).unwrap();
    document.set_root_font_size(RootFontSize::new(desired).unwrap());
    let overrides = ReaderStyleOverrides {
        canonical_font_size: canonical,
        ..Default::default()
    };
    let css = source
        .split_once("<style>")
        .and_then(|(_, rest)| rest.split_once("</style>"))
        .map_or("", |(css, _)| css);
    let inputs = [AuthorStylesheetInput {
        css,
        implicit_scope_root: None,
    }];
    style_document_with_cached_program_and_reader_overrides(
        &mut StyleProgramCache::default(),
        document,
        &inputs,
        MediaEnvironment::default(),
        &overrides,
    )
    .0
}

fn node(document: &StyledDocument, id: &str) -> DomNodeId {
    document
        .document()
        .node_ids()
        .find(|&node| document.document().get_dom_id(node) == Some(id))
        .unwrap()
}

fn style(document: &StyledDocument, id: &str) -> StyleIndices {
    document.style_for_node(node(document, id)).unwrap()
}

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.001,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn random_math_keeps_canonical_typography_coordinates() {
    let source = "<style>body {font-size:20px;line-height:40px} #target {font-size:random(fixed 0.5,10px,30px);line-height:random(fixed 0.5,1lh,2lh)}</style><p id='target'>Text</p>";
    for (desired, size, line) in [(16.0, 20.0, 60.0), (32.0, 40.0, 120.0)] {
        let document = styled(source, desired, Some(16.0));
        let view = document.style_view(style(&document, "target")).unwrap();
        close(view.font.font_size, size);
        close(view.text.line_height, line);
    }
}

#[test]
fn contextual_sign_products_keep_canonical_typography_coordinates() {
    let source = "<style>body {font-size:20px;line-height:40px} #target {font-size:calc(sign(1rem - 20px) * 10px + 30px);line-height:calc(sign(1lh - 30px) * 10px + 20px)}</style><p id='target'>Text</p>";
    for (desired, expected_size, expected_line) in [(16.0, 20.0, 30.0), (32.0, 40.0, 60.0)] {
        let document = styled(source, desired, Some(16.0));
        let indices = style(&document, "target");
        let view = document.style_view(indices).unwrap();
        close(view.font.font_size, expected_size);
        close(view.text.line_height, expected_line);
    }
}

#[test]
fn fixed_and_nested_relative_sizes_preserve_publisher_proportions() {
    let source = r#"<!doctype html><html><body>
        <p id="regular">Regular</p>
        <div id="fixed" style="font-size: 12px; line-height: 18px; margin-top: 2em; width: 120px">
            <span id="inherited">Inherited</span>
            <span id="em" style="font-size: 1.5em">
                <span id="percent" style="font-size: 50%">Nested</span>
            </span>
            <span id="calc" style="font-size: calc(50% + 4px)">Mixed</span>
            <span id="rem" style="font-size: 2rem">Root</span>
            <span id="keyword" style="font-size: large">Keyword</span>
            <span id="smaller" style="font-size: smaller">Smaller</span>
            <span id="points" style="font-size: 9pt">Points</span>
            <span id="shorthand" style="font: italic 10px/15px serif">Shorthand</span>
            <span id="initial" style="font-size: initial">Initial</span>
            <span id="zero" style="font-size: 0">Zero</span>
        </div>
        <style>#important { font-size: 8px !important }</style>
        <p id="important" style="font-size: 40px">Important</p>
        <font id="legacy" size="5">Legacy</font>
    </body></html>"#;
    let baseline = styled(source, 16.0, None);
    for desired in [8.0, 16.0, 24.0, 32.0] {
        let document = styled(source, desired, Some(16.0));
        let ratio = desired / 16.0;
        for id in [
            "regular",
            "fixed",
            "inherited",
            "em",
            "percent",
            "calc",
            "rem",
            "keyword",
            "smaller",
            "points",
            "shorthand",
            "initial",
            "zero",
            "important",
            "legacy",
        ] {
            let original = style(&baseline, id);
            let resized = style(&document, id);
            close(
                document.font_style(resized).unwrap().font_size,
                baseline.font_style(original).unwrap().font_size * ratio,
            );
            close(
                document.text_style(resized).unwrap().line_height,
                baseline.text_style(original).unwrap().line_height * ratio,
            );
        }
        let box_model = document.box_model_style(style(&document, "fixed")).unwrap();
        assert_eq!(box_model.width, PreferredSize::Px(120.0));
        assert_eq!(box_model.margin_top, LengthPct::Px(24.0 * ratio));
    }
}

#[test]
fn authored_root_and_generated_content_scale_once() {
    let source = r#"<!doctype html><html style="font-size: 20px; line-height: 30px"><body>
        <p id="target" style="font-size: 12px; line-height: 1.5">Text</p>
        <p id="rem" style="font-size: 2rem">Root relative</p>
        <p id="lh" style="font-size: 1lh">Line relative</p>
        <p id="rlh" style="font-size: 1rlh">Root line relative</p>
        <style>
            #target::before { content: "Before"; font-size: 8px; line-height: 12px }
            #target::after { content: "After"; font-size: 150% }
            #target::first-letter { font-size: 2em }
            #target::first-line { font-size: 18px }
        </style>
    </body></html>"#;
    let baseline = styled(source, 16.0, None);
    let document = styled(source, 24.0, Some(16.0));
    for id in ["target", "rem", "lh", "rlh"] {
        close(
            document.font_style(style(&document, id)).unwrap().font_size,
            baseline.font_style(style(&baseline, id)).unwrap().font_size * 1.5,
        );
    }
    let pseudos = |document: &StyledDocument| {
        let styles = document.styles();
        let target = node(document, "target");
        [
            styles.before_style_for_node(target).unwrap().0,
            styles.after_style_for_node(target).unwrap().0,
            styles.first_letter_style_for_node(target).unwrap(),
            styles.first_line_style_for_node(target).unwrap(),
        ]
    };
    for (original, resized) in pseudos(&baseline).into_iter().zip(pseudos(&document)) {
        close(
            document.font_style(resized).unwrap().font_size,
            baseline.font_style(original).unwrap().font_size * 1.5,
        );
        close(
            document.text_style(resized).unwrap().line_height,
            baseline.text_style(original).unwrap().line_height * 1.5,
        );
    }
}

#[test]
fn canonical_size_is_configurable_and_invalid_values_preserve_css_sizing() {
    let source = "<!doctype html><p id='target' style='font-size: 10px'>Text</p>";
    let document = styled(source, 30.0, Some(20.0));
    close(
        document
            .font_style(style(&document, "target"))
            .unwrap()
            .font_size,
        15.0,
    );
    for canonical in [
        None,
        Some(0.0),
        Some(-16.0),
        Some(f32::NAN),
        Some(f32::INFINITY),
        Some(f32::MIN_POSITIVE),
    ] {
        let document = styled(source, 24.0, canonical);
        close(
            document
                .font_style(style(&document, "target"))
                .unwrap()
                .font_size,
            10.0,
        );
    }
}

#[test]
fn viewport_font_sizes_retain_their_resize_dependency() {
    let mut document = html_parse::parse_dom_document(
        "<!doctype html><p id='target' style='font-size: 10vw'>Viewport</p>",
    )
    .unwrap();
    document.set_root_font_size(RootFontSize::new(24.0).unwrap());
    let overrides = ReaderStyleOverrides {
        canonical_font_size: Some(16.0),
        ..Default::default()
    };
    let (document, _, queries) = style_document_with_cached_program_and_reader_overrides(
        &mut StyleProgramCache::default(),
        document,
        &[],
        MediaEnvironment::screen(600.0, None).unwrap(),
        &overrides,
    );
    close(
        document
            .font_style(style(&document, "target"))
            .unwrap()
            .font_size,
        90.0,
    );
    assert!(queries.uses_viewport_units());
}

#[test]
fn font_metric_contributions_scale_but_root_metric_coefficients_do_not() {
    let source = r#"<!doctype html><html><body>
        <p id="calc" style="font-size: calc(10px + 2ex + 3ch + 4cap); line-height: 2ex">Metrics</p>
        <p id="cap" style="font-size: 2cap">Cap</p>
        <p id="rch" style="font-size: 2rch">Root ch</p>
        <p id="rcap" style="font-size: 2rcap">Root cap</p>
        <p id="rlh" style="font-size: 2rlh">Root line</p>
    </body></html>"#;
    let baseline = styled(source, 16.0, None);
    let document = styled(source, 24.0, Some(16.0));
    for id in ["calc", "cap", "rch", "rcap", "rlh"] {
        let original = baseline.font_style(style(&baseline, id)).unwrap();
        let resized = document.font_style(style(&document, id)).unwrap();
        close(resized.font_size, original.font_size * 1.5);
        close(
            resized.font_size_x_height_px,
            original.font_size_x_height_px * 1.5,
        );
        close(
            resized.font_size_ch_advance_px,
            original.font_size_ch_advance_px * 1.5,
        );
        close(
            resized.font_size_cap_height_px,
            original.font_size_cap_height_px * 1.5,
        );
        close(resized.font_size_root_ch, original.font_size_root_ch);
        close(
            resized.font_size_root_cap_height,
            original.font_size_root_cap_height,
        );
        close(
            resized.font_size_root_line_height,
            original.font_size_root_line_height,
        );
    }
}
