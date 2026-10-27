use html_pipeline::engine::{
    FormattingDocument, GlyphMetric, GlyphResourceGeneration, GlyphResourceStore, GlyphShaper,
    LayoutConstraints, PreparedDocument, ShapeError,
};
use html_pipeline::{GlyphRequest, ShapedGlyph};

#[derive(Default)]
struct Shaper(GlyphResourceGeneration);

impl GlyphShaper for Shaper {
    fn reset(&mut self) {
        self.0 = GlyphResourceGeneration::default();
    }

    fn resource_generation(&self) -> GlyphResourceGeneration {
        self.0
    }

    fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
        Ok(ShapedGlyph::from_metric(
            request.character as u32,
            GlyphMetric::try_new(
                request.character,
                request.font_size / 2.0,
                request.font_size,
                0.0,
            )
            .unwrap(),
        ))
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
    let mut document_resources = GlyphResourceStore::default();
    let document = prepared
        .shape(&mut Shaper::default(), &mut document_resources)
        .unwrap()
        .layout(
            LayoutConstraints::new(600.0, 16.0).unwrap(),
            &document_resources,
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
