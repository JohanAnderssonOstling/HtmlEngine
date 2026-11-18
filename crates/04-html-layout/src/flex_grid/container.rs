//! Resolve container sizing policy before executing the flex/grid pipeline.
use super::sizing::ContainerFacts;
use super::{FlexGridInput, TaffyContainerKind, intrinsic_widths, layout};
use crate::BoxLayoutRequest;
use crate::boxes::ResolvedBoxSizing;
use crate::measurement::{measure_box_isolated, with_isolated_measurement};
use crate::{ContentLayoutResult, LayoutEngine};
use html_style_model::FlexWrap;
use kurbo::Point;

pub(crate) fn layout_container(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    content_point: Point,
    children: &[u32],
    kind: TaffyContainerKind,
) -> ContentLayoutResult {
    let container = ContainerFacts::new(&engine.read.reader, resolved.box_idx(), kind);
    let mut natural_height = None;
    let mut measure_natural_height = |engine: &mut LayoutEngine<'_, '_>| {
        *natural_height.get_or_insert_with(|| {
            measure_natural_taffy_height(engine, resolved, children, container)
        })
    };
    let (min_height, max_height) = if kind == TaffyContainerKind::Flex
        && (resolved.vertical.min.is_intrinsic() || resolved.vertical.max.is_intrinsic())
    {
        // An intrinsic min/max block-size is a constraint on the flex
        // container's natural block-size. Measuring the final pass at the
        // authored height first would make `height: 0; min-height: max-content`
        // appear to have a zero intrinsic minimum (and similarly preserve an
        // oversized authored height through an intrinsic maximum).
        //
        // Probe an auto-height Flex formatting context in disposable output,
        // then feed that natural size back to Taffy as the root min/max
        // constraint. Keeping it as a constraint rather than an exact height
        // also preserves the definiteness rules used for descendant
        // percentage heights.
        let wrapped_column = !container.flex_row() && container.wrap != FlexWrap::NoWrap;
        let natural_height = if wrapped_column && resolved.vertical.min.is_intrinsic() {
            let mut maximum = 0.0f64;
            for &child in children {
                let child_idx = child as usize;
                if engine.read.reader.box_facts(child_idx).absolute {
                    continue;
                }
                let size = measure_box_isolated(
                    engine,
                    BoxLayoutRequest::normal(child_idx, resolved.horizontal.content.max(0.0), None),
                )
                .size_with_floats();
                let style = engine.read.reader.style(child_idx);
                let outer_height = size.height
                    + style.margin_top().resolve(resolved.horizontal.content)
                    + style.margin_bottom().resolve(resolved.horizontal.content);
                maximum = maximum.max(outer_height);
            }
            maximum
        } else {
            measure_natural_height(engine)
        };
        (
            resolved.vertical.min.resolve(Some(natural_height)),
            resolved.vertical.max.resolve(Some(natural_height)),
        )
    } else {
        (
            resolved.vertical.min.resolve(None),
            resolved.vertical.max.resolve(None),
        )
    };
    let mut height_basis = resolved.vertical.descendant_basis;
    if resolved.used_aspect_ratio.is_some()
        && resolved.vertical.explicit.is_none()
        && resolved.vertical.ratio_auto_min_uses_content
        && let Some(preferred_height) = height_basis
    {
        // A ratio makes the preferred block size definite, but an automatic
        // minimum may still expand it to fit content. Measure that minimum
        // without the preferred height before sizing tracks or flex lines.
        let natural_height = measure_natural_height(engine);
        let minimum = natural_height.min(max_height.unwrap_or(f64::INFINITY));
        height_basis = Some(preferred_height.max(minimum));
    }
    let layout_width = if resolved.is_float && resolved.horizontal.auto_width {
        let (min_content, max_content) = intrinsic_widths(
            engine,
            resolved.request.box_idx,
            children,
            kind,
            None,
            height_basis,
        );
        let horizontal_noncontent =
            resolved.horizontal.margin_padding + resolved.box_model.horizontal_border();
        let available_content = (resolved.request.available_width - horizontal_noncontent).max(0.0);
        max_content
            .min(available_content.max(min_content))
            .clamp(resolved.horizontal.min, resolved.horizontal.max)
            .max(0.0)
    } else {
        resolved.horizontal.content
    };
    layout(
        engine,
        FlexGridInput {
            container_idx: resolved.request.box_idx,
            children,
            content_origin: content_point,
            content_width: layout_width,
            explicit_height: height_basis,
            min_height,
            max_height,
            container,
        },
    )
}

fn measure_natural_taffy_height(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    children: &[u32],
    container: ContainerFacts,
) -> f64 {
    with_isolated_measurement(engine, |engine| {
        layout(
            engine,
            FlexGridInput {
                container_idx: resolved.box_idx(),
                children,
                content_origin: Point::ZERO,
                content_width: resolved.horizontal.content,
                explicit_height: None,
                min_height: None,
                max_height: None,
                container,
            },
        )
        .size
        .height
        .max(0.0)
    })
}
