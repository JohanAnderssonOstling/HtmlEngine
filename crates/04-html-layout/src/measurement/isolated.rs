use crate::engine::LayoutPassKind;
use crate::engine::execute_layout;
use crate::{BoxLayoutRequest, BoxLayoutResult, LayoutEngine, ResolvedBoxSizing};
use kurbo::Point;

/// Run the ordinary layout algorithm against owned temporary output.
/// The active pass's geometry, fragments, floats and placement are never replaced.
pub fn with_isolated_measurement<T>(
    engine: &mut LayoutEngine<'_, '_>,
    measure: impl FnOnce(&mut LayoutEngine<'_, '_>) -> T,
) -> T {
    let mut measurement = engine.scratch.measurement.take().unwrap_or_default();
    engine
        .scratch
        .transfer_measurement_caches(&mut measurement.scratch);
    let result = execute_layout(
        engine.read,
        measurement.outputs(),
        LayoutPassKind::Measurement,
        measure,
    );
    measurement
        .scratch
        .transfer_measurement_caches(engine.scratch);
    engine.scratch.measurement = Some(measurement);
    result
}

/// Probes run the ordinary box pipeline against isolated storage and consume
/// its result. Final layout consumes the same result's output ranges.
pub fn measure_box_isolated(
    engine: &mut LayoutEngine<'_, '_>,
    request: BoxLayoutRequest,
) -> BoxLayoutResult<()> {
    let resolved = engine.resolve_box_sizing(request);
    measure_resolved_box_isolated(engine, resolved)
}

pub fn measure_resolved_box_isolated(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: ResolvedBoxSizing,
) -> BoxLayoutResult<()> {
    with_isolated_measurement(engine, |engine| {
        engine.geometry.set_point(resolved.box_idx(), Point::ZERO);
        engine.layout_resolved_box(resolved)
    })
    .without_output()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::LayoutReadContext;
    use crate::engine::PassContext;
    use crate::engine::{LayoutInputs, LayoutOutputs, LayoutScratch};
    use crate::engine::{LayoutInputs, LayoutOutputs, LayoutScratch};
    use crate::{InlinePlanGeneration, PlacementState};
    use html_dom::Document;
    use html_dom::Document;
    use html_engine_model::{
        BoxFontMetrics, BoxGeometry, GlyphMetrics, GlyphSlice, ImageMetrics, InlineContent,
        LayoutState, Line, PaintView, ShapedFontMetrics,
    };
    use html_style_model::{ComputedStylesBuilder, ReaderStyleOverrides};
    use rustc_data_structures::fx::FxHashMap;

    #[test]
    fn nested_measurements_keep_active_output_and_reuse_their_measurement() {
        let document = Document::default();
        let styles = ComputedStylesBuilder::new(&document).finish().unwrap();
        let topology = html_engine_model::LayoutTree::default();
        let inline = InlineContent::default();
        let glyph_metrics = GlyphMetrics::default();
        let font_metrics = ShapedFontMetrics::new(
            BoxFontMetrics::default(),
            Default::default(),
            8.0,
            10.0,
            16.0,
        );
        let image_metrics = ImageMetrics::default();
        let glyph_map = FxHashMap::default();
        let paint_map = FxHashMap::default();
        let overrides = ReaderStyleOverrides::default();
        let first_letter_styles = html_text::FirstLetterStyles::default();
        let inputs = LayoutInputs {
            document: &document,
            styles: &styles,
            topology: &topology,
            inline_content: &inline,
            first_letter_styles: &first_letter_styles,
            glyphs: GlyphSlice::new(&[], &[]),
            glyph_metrics: &glyph_metrics,
            font_metrics: &font_metrics,
            text_geometry: None,
            word_index: None,
            ellipsis_glyphs: &glyph_map,
            hyphen_glyphs: &glyph_map,
            inline_plan_generation: InlinePlanGeneration::new(),
            image_metrics: &image_metrics,
            paint: PaintView::new(&overrides, &[], &[], &paint_map, &paint_map),
            text_style_ranges: &[],
            resource_generation: html_engine_model::GlyphResourceGeneration::default(),
        };
        let mut geometry = BoxGeometry::default();
        let mut state = LayoutState::default();
        let mut placement = PlacementState::default();
        let mut scratch = LayoutScratch::default();
        let pass_context = PassContext::new(&inputs);
        let read = LayoutReadContext::new(
            inputs,
            html_engine_model::LayoutConstraints::new(100.0, 16.0).unwrap(),
            &pass_context,
        );
        execute_layout(
            read,
            LayoutOutputs {
                geometry: &mut geometry,
                state: &mut state,
                placement: &mut placement,
                scratch: &mut scratch,
            },
            LayoutPassKind::Final,
            |engine| {
                engine.geometry.reset(1);
                let point = Point::new(13.0, 17.0);
                engine.geometry.set_point(0, point);
                engine
                    .fragments
                    .state_mut()
                    .line_output
                    .lines
                    .push(Line::default());
                engine.scratch.floats.reset();
                engine
                    .scratch
                    .floats
                    .current_float_context_mut()
                    .add_band(FloatBand {
                        left: 0.0,
                        right: 20.0,
                        top: 0.0,
                        bottom: 30.0,
                        side: FloatSide::Left,
                    });
                let probe = |probe: &mut LayoutEngine<'_, '_>| {
                    assert_eq!(probe.fragments.line_len(), 0);
                    assert!(probe.scratch.floats.current_float_context().is_empty());
                    probe.geometry.reset(1);
                    probe.geometry.set_point(0, Point::new(99.0, 99.0));
                    probe
                        .fragments
                        .state_mut()
                        .line_output
                        .lines
                        .push(Line::default());
                    let nested =
                        with_isolated_measurement(probe, |nested| nested.fragments.line_len());
                    assert_eq!(nested, 0);
                    assert_eq!(probe.fragments.line_len(), 1);
                    assert_eq!(probe.geometry.point(0), Point::new(99.0, 99.0));
                };
                with_isolated_measurement(engine, probe);
                let measurement = engine.scratch.measurement.as_deref().unwrap() as *const _;
                with_isolated_measurement(engine, probe);
                assert_eq!(
                    measurement,
                    engine.scratch.measurement.as_deref().unwrap() as *const _
                );
                assert_eq!(engine.geometry.point(0), point);
                assert_eq!(engine.fragments.line_len(), 1);
                assert_eq!(
                    engine.scratch.floats.current_float_context().max_bottom(),
                    30.0
                );
            },
        );
    }
}
