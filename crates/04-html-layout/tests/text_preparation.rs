use html_pipeline::engine::LayoutRequest;
use html_pipeline::engine::{FontMetrics, GlyphHandle, ShapedRun, TextRunShapeRequest, ShapedGlyph};
use html_pipeline::engine::{
    FormattingDocument, GlyphResourceGeneration, GlyphResourceStore, GlyphShaper,
    LayoutConstraints, PreparedDocument};

#[derive(Default)]
struct Shaper(GlyphResourceGeneration);

impl GlyphShaper for Shaper {
    fn resource_generation(&self) -> GlyphResourceGeneration {
        self.0
    }

    fn shape_run(&mut self, request: TextRunShapeRequest<'_>) -> ShapedRun {
        ShapedRun::characters(request, |character, style| {
            ShapedGlyph::new(
                GlyphHandle(character as u32),
                style.font_size / 2.0,
                FontMetrics {
                    ascent: style.font_size,
                    descent: 0.0,
                },
            )
        })
    }
}

fn styled() -> (html_dom::Document, html_style_model::ComputedStyles) {
    let document = html_parse::parse_dom_document(
        "<!doctype html><p>ß<span>  world</span><i>hidden</i></p><pre>x\n y</pre>",
    )
    .unwrap();
    html_style::style_document(document, &["p {text-transform:uppercase} i {display:none}"])
        .into_parts()
}

fn text(prepared: PreparedDocument<'_>) -> String {
    let mut document_resources_shaper = Shaper::default();

    let mut document_resources = GlyphResourceStore::default();
    let document = prepared
        .shape(&mut document_resources_shaper, &mut document_resources)
        .unwrap()
        .layout(
            LayoutRequest::new(LayoutConstraints::new(600.0, 16.0).unwrap()),
            &mut document_resources_shaper,
            &mut document_resources,
        )
        .expect("layout requires the shaper-aware API for first-line styles");
    let view = document.render_view(&document_resources).text();
    (0..view.glyph_count())
        .map(|index| {
            view.glyph_metric(view.glyph_at(index).unwrap())
                .unwrap()
                .ch()
        })
        .collect()
}

#[test]
fn explicit_pipeline_stages_apply_css_before_text_preparation() {
    let (document, styles) = styled();
    let formatting = FormattingDocument::try_new(document, styles).unwrap();
    let prepared = formatting.prepare_text();
    assert_eq!(text(prepared), "SS WORLDx\n y");
}

#[test]
fn convenience_preparation_uses_the_same_text_stage() {
    let (document, styles) = styled();
    let explicit = FormattingDocument::try_new(document, styles)
        .unwrap()
        .prepare_text();
    let (document, styles) = styled();
    let convenience = PreparedDocument::try_new(document, styles).unwrap();
    assert_eq!(text(explicit), text(convenience));
}
