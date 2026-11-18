use crate::measurement::measure_box_isolated;
use crate::{BoxLayoutRequest, LayoutEngine, ResolvedBoxModel};
use kurbo::Point;

pub(super) fn measure_caption_stack(
    engine: &mut LayoutEngine<'_, '_>,
    captions: &[u32],
    content_width: f64,
    parent_content_height: Option<f64>,
) -> f64 {
    captions
        .iter()
        .map(|&caption_idx| {
            let size = measure_box_isolated(
                engine,
                BoxLayoutRequest::normal(
                    caption_idx as usize,
                    content_width.max(0.0),
                    parent_content_height,
                ),
            )
            .size_with_floats();
            size.height
        })
        .sum()
}

pub(super) fn layout_caption_stack(
    engine: &mut LayoutEngine<'_, '_>,
    captions: &[u32],
    origin: Point,
    content_width: f64,
    parent_content_height: Option<f64>,
) -> f64 {
    let mut height = 0.0;
    for &caption_idx in captions {
        let caption_idx = caption_idx as usize;
        let box_model = ResolvedBoxModel::new(engine.read.reader.style(caption_idx), content_width);
        engine.geometry.set_point(
            caption_idx,
            Point::new(
                origin.x + f64::from(box_model.margin_left),
                origin.y + height + f64::from(box_model.margin_top),
            ),
        );
        let caption_size = engine
            .layout_box(BoxLayoutRequest::normal(
                caption_idx,
                content_width,
                parent_content_height,
            ))
            .size;
        height += box_model.vertical_margin() + caption_size.height;
    }
    height
}
