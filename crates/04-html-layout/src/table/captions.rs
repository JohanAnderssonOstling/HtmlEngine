use crate::layout::{BoxLayoutRequest, LayoutEngine, ResolvedBoxModel, measure_box_isolated};
use kurbo::Point;

#[derive(Clone, Copy)]
pub(super) enum CaptionMeasurement {
    BorderBoxes,
    MarginStack,
}

pub(super) fn measure_caption_stack(
    session: &mut LayoutEngine<'_, '_>,
    captions: &[u32],
    content_width: f64,
    parent_content_height: Option<f64>,
    measurement: CaptionMeasurement,
) -> f64 {
    captions
        .iter()
        .map(|&caption_idx| {
            let size = measure_box_isolated(
                session,
                BoxLayoutRequest::normal(
                    caption_idx as usize,
                    content_width.max(0.0),
                    parent_content_height,
                ),
            )
            .size_with_floats();
            let margins = match measurement {
                CaptionMeasurement::BorderBoxes => 0.0,
                CaptionMeasurement::MarginStack => {
                    let box_model = ResolvedBoxModel::new(
                        session.reader.style(caption_idx as usize),
                        content_width,
                    );
                    box_model.margin_top + box_model.margin_bottom
                }
            };
            size.height + margins
        })
        .sum()
}

pub(super) fn layout_caption_stack(
    session: &mut LayoutEngine<'_, '_>,
    captions: &[u32],
    origin: Point,
    content_width: f64,
    parent_content_height: Option<f64>,
) -> f64 {
    let mut height = 0.0;
    for &caption_idx in captions {
        let caption_idx = caption_idx as usize;
        let box_model = ResolvedBoxModel::new(session.reader.style(caption_idx), content_width);
        session.geometry.set_point(
            caption_idx,
            Point::new(
                origin.x + box_model.margin_left,
                origin.y + height + box_model.margin_top,
            ),
        );
        let caption_size = session
            .layout_box(crate::layout::BoxLayoutRequest::normal(
                caption_idx,
                content_width,
                parent_content_height,
            ))
            .size;
        height += box_model.margin_top + caption_size.height + box_model.margin_bottom;
    }
    height
}

pub(super) fn minimum_width(
    session: &LayoutEngine<'_, '_>,
    captions_top: &[u32],
    captions_bottom: &[u32],
) -> f64 {
    captions_top
        .iter()
        .chain(captions_bottom)
        .map(|&caption| crate::layout::box_intrinsic_widths(session, caption as usize).0)
        .fold(0.0, f64::max)
}
