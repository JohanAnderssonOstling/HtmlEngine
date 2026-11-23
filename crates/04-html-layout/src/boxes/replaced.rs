use html_style_model::{
    AspectRatio, BoxSizing, ObjectFit, UsedObjectPosition, UsedPreferredSize as PreferredSize,
    UsedStyleView,
};
use kurbo::{Point, Rect, Size};

use super::constraints::resolve_definite_content_size;

use crate::{LayoutEngine, LayoutReader};
use html_dom::ImageSource;
use html_engine_model::ImageSizingPolicy;

#[derive(Clone, Copy, Debug)]
pub struct ImageIntrinsic {
    pub size: Size,
    pub aspect_ratio: Option<f64>,
}

impl LayoutReader<'_> {
    fn image_is_mathml(&self, box_idx: usize) -> bool {
        self.image_idx(box_idx)
            .and_then(|index| self.document().image(index))
            .is_some_and(|image| matches!(image.source, ImageSource::MathMl { .. }))
    }

    pub fn image_intrinsic(&self, box_idx: usize) -> Option<ImageIntrinsic> {
        let image_idx = self.image_idx(box_idx)?;
        let (width, height) = self.image_display_size(image_idx);
        let size = Size::new(width, height);
        Some(ImageIntrinsic {
            size,
            aspect_ratio: self.image_intrinsic_ratio(box_idx, size),
        })
    }

    fn image_intrinsic_ratio(&self, box_idx: usize, intrinsic: Size) -> Option<f64> {
        if let Some(element) = self.layout_tree().element_for_box(box_idx, self.document())
            && element.tag().eq_ignore_ascii_case("svg")
        {
            if let Some(view_box) = element.attr("viewBox").or_else(|| element.attr("viewbox")) {
                let mut parts = view_box
                    .split(|character: char| character.is_ascii_whitespace() || character == ',')
                    .filter(|value| !value.is_empty());
                let mut values = [0.0; 4];
                for value in &mut values {
                    *value = parts.next()?.parse::<f64>().ok()?;
                }
                if parts.next().is_some() || values[2] <= 0.0 || values[3] <= 0.0 {
                    return None;
                }
                return Some(values[2] / values[3]);
            }
            return None;
        }
        (intrinsic.height > 0.0).then_some(intrinsic.width / intrinsic.height)
    }
}

impl LayoutEngine<'_, '_> {
    /// Returns a renderer-supplied column-filling width for an eligible
    /// standalone image in content pixels, capped to the viewport height.
    pub(crate) fn smart_standalone_image_width(
        &self,
        box_idx: usize,
        intrinsic: Size,
        available_width: f64,
        horizontal_padding_border: f64,
        vertical_outer_inset: f64,
        standalone: bool,
    ) -> Option<f64> {
        const MIN_SOURCE_WIDTH: f64 = 96.0;

        if self.read.constraints.image_sizing_policy() != ImageSizingPolicy::SmartStandalone
            || !standalone
            || intrinsic.width < MIN_SOURCE_WIDTH
            || self.read.reader.image_is_mathml(box_idx)
        {
            return None;
        }
        let style = self.read.reader.style(box_idx);
        if style.height() != PreferredSize::Auto
            || style.min_height() != PreferredSize::Auto
            || style.max_height() != PreferredSize::Auto
        {
            return None;
        }

        let element = self
            .read
            .reader
            .layout_tree()
            .element_for_box(box_idx, self.read.reader.document())?;
        if element.attr("height").is_some() {
            return None;
        }

        let mut expanded_content_width = (available_width - horizontal_padding_border).max(0.0);
        if intrinsic.height > 0.0
            && let Some(viewport_height) = self.read.constraints.viewport_height()
        {
            let available_content_height = (viewport_height - vertical_outer_inset).max(0.0);
            expanded_content_width = expanded_content_width
                .min(available_content_height * intrinsic.width / intrinsic.height);
        }
        Some(expanded_content_width)
    }

    /// Resolve reader image fitting before line breaking and float placement.
    /// Images already inside the limits retain their exact resolved geometry.
    pub(crate) fn fit_image(
        &self,
        box_idx: usize,
        size: Size,
        available_width: f64,
        outer: Size,
    ) -> Size {
        if self.read.constraints.image_sizing_policy() != ImageSizingPolicy::SmartStandalone
            || size.width <= 0.0
            || size.height <= 0.0
        {
            return size;
        }
        let width =
            (available_width.min(self.read.constraints.viewport_width()) - outer.width).max(0.0);
        let height = self
            .read
            .constraints
            .viewport_height()
            .filter(|height| *height > 0.0)
            .map_or(f64::INFINITY, |height| (height - outer.height).max(0.0));
        if (size.width <= width && size.height <= height)
            || self.read.reader.image_is_mathml(box_idx)
        {
            return size;
        }
        let scale = (width / size.width).min(height / size.height).min(1.0);
        Size::new(size.width * scale, size.height * scale)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ReplacedSizeInput {
    pub horizontal: ReplacedAxis,
    pub vertical: ReplacedAxis,
    pub aspect_ratio: Option<f64>,
    pub box_sizing: BoxSizing,
}

impl ReplacedSizeInput {
    pub fn from_style(
        style: UsedStyleView<'_>,
        intrinsic: Size,
        aspect_ratio: Option<f64>,
        available_width: f64,
        available_height: Option<f64>,
    ) -> Self {
        Self {
            horizontal: ReplacedAxis {
                size: style.width(),
                min: style.min_width(),
                max: style.max_width(),
                intrinsic: intrinsic.width,
                basis: Some(available_width),
                margin: 0.0,
                padding_border: 0.0,
            },
            vertical: ReplacedAxis {
                size: style.height(),
                min: style.min_height(),
                max: style.max_height(),
                intrinsic: intrinsic.height,
                basis: available_height,
                margin: 0.0,
                padding_border: 0.0,
            },
            aspect_ratio,
            box_sizing: style.box_sizing(),
        }
    }

    pub fn with_box_model(
        mut self,
        horizontal_margin: f64,
        vertical_margin: f64,
        horizontal_padding_border: f64,
        vertical_padding_border: f64,
    ) -> Self {
        self.horizontal.margin = horizontal_margin;
        self.vertical.margin = vertical_margin;
        self.horizontal.padding_border = horizontal_padding_border;
        self.vertical.padding_border = vertical_padding_border;
        self
    }

    pub fn with_width_constraints(
        mut self,
        width: PreferredSize,
        min_width: PreferredSize,
        max_width: PreferredSize,
    ) -> Self {
        self.horizontal.size = width;
        self.horizontal.min = min_width;
        self.horizontal.max = max_width;
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ReplacedAxis {
    pub size: PreferredSize,
    pub min: PreferredSize,
    pub max: PreferredSize,
    pub intrinsic: f64,
    pub basis: Option<f64>,
    pub margin: f64,
    pub padding_border: f64,
}

impl ReplacedAxis {
    fn resolve(self, value: PreferredSize, box_sizing: BoxSizing) -> Option<f64> {
        resolve_definite_content_size(
            value,
            self.basis,
            self.margin,
            self.padding_border,
            box_sizing,
        )
    }

    fn bounds(
        self,
        intrinsic: f64,
        box_sizing: BoxSizing,
        [minimum, maximum]: [Option<f64>; 2],
    ) -> (f64, f64) {
        let bound = |value, auto| match value {
            PreferredSize::Auto => auto,
            PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent => {
                intrinsic
            }
            _ => self.resolve(value, box_sizing).unwrap_or(auto),
        };
        let min = minimum.unwrap_or_else(|| bound(self.min, 0.0));
        (
            min,
            maximum
                .unwrap_or_else(|| bound(self.max, f64::INFINITY))
                .max(min),
        )
    }
}

pub fn preferred_aspect_ratio(authored: AspectRatio, intrinsic: Option<f64>) -> Option<f64> {
    if authored.uses_intrinsic() {
        intrinsic.or_else(|| authored.preferred().map(f64::from))
    } else {
        authored.preferred().map(f64::from)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReplacedObjectGeometry {
    /// Painted image rectangle relative to the content-box origin.
    pub rect: Rect,
    /// The replaced content is always clipped to its content box.
    pub clip: Rect,
}

pub fn replaced_object_geometry(
    content: Size,
    intrinsic: Size,
    fit: ObjectFit,
    position: UsedObjectPosition,
) -> ReplacedObjectGeometry {
    let content = Size::new(content.width.max(0.0), content.height.max(0.0));
    let valid_intrinsic = intrinsic.width.is_finite()
        && intrinsic.height.is_finite()
        && intrinsic.width > 0.0
        && intrinsic.height > 0.0;
    let fitted = if !valid_intrinsic || matches!(fit, ObjectFit::Fill) {
        content
    } else {
        let contain_scale =
            (content.width / intrinsic.width).min(content.height / intrinsic.height);
        let cover_scale = (content.width / intrinsic.width).max(content.height / intrinsic.height);
        match fit {
            ObjectFit::Fill => content,
            ObjectFit::Contain => intrinsic * contain_scale,
            ObjectFit::Cover => intrinsic * cover_scale,
            ObjectFit::CoverScaleDown if cover_scale >= 1.0 => intrinsic,
            ObjectFit::CoverScaleDown => intrinsic * cover_scale,
            ObjectFit::None => intrinsic,
            ObjectFit::ScaleDown
                if intrinsic.width <= content.width && intrinsic.height <= content.height =>
            {
                intrinsic
            }
            ObjectFit::ScaleDown => intrinsic * contain_scale,
        }
    };
    let offset = Point::new(
        position.x.resolve(content.width - fitted.width),
        position.y.resolve(content.height - fitted.height),
    );
    ReplacedObjectGeometry {
        rect: Rect::from_origin_size(offset, fitted),
        clip: Rect::from_origin_size(Point::ZERO, content),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplacedMainAxis {
    Horizontal,
    Vertical,
}

/// Resolve the constraint value Taffy should receive for `min-size: auto` in
/// the main axis of a replaced flex item.
/// Cross constraints apply before ratio transfer; main constraints and a
/// definite preferred main size then cap the resulting suggestion.
pub fn resolve_replaced_flex_auto_min_main_size(
    input: ReplacedSizeInput,
    main_axis: ReplacedMainAxis,
    stretch_cross_size: bool,
) -> f64 {
    let (main, mut cross) = match main_axis {
        ReplacedMainAxis::Horizontal => (input.horizontal, input.vertical),
        ReplacedMainAxis::Vertical => (input.vertical, input.horizontal),
    };
    let ratio = input
        .aspect_ratio
        .filter(|ratio| ratio.is_finite() && *ratio > 0.0);
    if stretch_cross_size && matches!(cross.size, PreferredSize::Auto | PreferredSize::Stretch) {
        cross.size = PreferredSize::Stretch;
    }
    let (cross_min, cross_max) =
        cross.bounds(cross.intrinsic.max(0.0), input.box_sizing, [None; 2]);
    let cross_size = cross
        .resolve(cross.size, input.box_sizing)
        .unwrap_or(cross.intrinsic.max(0.0))
        .clamp(cross_min, cross_max);
    let transferred_main = ratio.map_or(main.intrinsic.max(0.0), |ratio| match main_axis {
        ReplacedMainAxis::Horizontal => cross_size * ratio,
        ReplacedMainAxis::Vertical => cross_size / ratio,
    });

    let (main_min, main_max) = main.bounds(main.intrinsic.max(0.0), input.box_sizing, [None; 2]);
    let mut automatic_minimum = transferred_main.clamp(main_min, main_max);
    if let Some(specified) = main.resolve(main.size, input.box_sizing) {
        automatic_minimum = automatic_minimum.min(specified.clamp(main_min, main_max));
    }

    match input.box_sizing {
        BoxSizing::ContentBox => automatic_minimum,
        BoxSizing::BorderBox => automatic_minimum + main.padding_border,
    }
}

/// Convert an intrinsic sizing keyword used as a replaced-element constraint
/// into the numeric value expected by a formatting-context adapter. The
/// intrinsic dimensions stored by the renderer are content-box dimensions;
/// an adapter whose sizing model consumes border-box constraints must include
/// the corresponding padding and border exactly once.
pub fn resolve_replaced_intrinsic_constraint(
    value: PreferredSize,
    intrinsic_content_size: f64,
    padding_border: f64,
    box_sizing: BoxSizing,
) -> Option<f64> {
    if !value.is_intrinsic_keyword() {
        return None;
    }
    let intrinsic_content_size = intrinsic_content_size.max(0.0);
    Some(match box_sizing {
        BoxSizing::ContentBox => intrinsic_content_size,
        BoxSizing::BorderBox => intrinsic_content_size + padding_border.max(0.0),
    })
}

/// Clamp a definite preferred size by intrinsic min/max constraints before it
/// is exposed as an intrinsic contribution. This is observable for grid auto
/// tracks: passing an unclamped preferred size lets the track grow from a
/// value the item itself can never use.
pub fn clamp_replaced_definite_size_by_intrinsic_constraints(
    preferred: PreferredSize,
    intrinsic_minimum: Option<f64>,
    intrinsic_maximum: Option<f64>,
) -> Option<f64> {
    let PreferredSize::Px(preferred) = preferred else {
        return None;
    };
    let minimum = intrinsic_minimum.unwrap_or(0.0).max(0.0);
    let maximum = intrinsic_maximum.unwrap_or(f64::INFINITY).max(minimum);
    Some((preferred.max(0.0) as f64).clamp(minimum, maximum))
}

/// Resolve the content-box size of a replaced element before its formatting
/// context lays out descendants. Intrinsic sizing keywords on replaced
/// elements behave like `auto`, but can transfer a definite size through the
/// preferred aspect ratio. `stretch` is different: it fills the available
/// margin box, regardless of `box-sizing`. Optional content widths override the
/// preferred, minimum, and maximum widths after numeric resolution.
pub fn resolve_replaced_content_size(
    input: ReplacedSizeInput,
    [preferred_content_width, minimum_width, maximum_width]: [Option<f64>; 3],
) -> Size {
    let ratio = input
        .aspect_ratio
        .filter(|ratio| ratio.is_finite() && *ratio > 0.0);
    let definite_width = preferred_content_width.or_else(|| {
        input
            .horizontal
            .resolve(input.horizontal.size, input.box_sizing)
    });
    let definite_height = input
        .vertical
        .resolve(input.vertical.size, input.box_sizing);

    let (width, height) = match (definite_width, definite_height, ratio) {
        (Some(width), Some(height), _) => (width, height),
        (Some(width), None, Some(ratio)) => (width, width / ratio),
        (None, Some(height), Some(ratio)) => (height * ratio, height),
        (Some(width), None, None) => (width, input.vertical.intrinsic.max(0.0)),
        (None, Some(height), None) => (input.horizontal.intrinsic.max(0.0), height),
        (None, None, _) => {
            let intrinsic_width = input.horizontal.intrinsic.max(0.0);
            let available_content = (input.horizontal.basis.unwrap_or(0.0)
                - input.horizontal.margin
                - input.horizontal.padding_border)
                .max(0.0);
            let width = if matches!(
                input.horizontal.size,
                PreferredSize::Auto | PreferredSize::FitContent
            ) {
                intrinsic_width.min(available_content)
            } else {
                intrinsic_width
            };
            let height =
                ratio.map_or_else(|| input.vertical.intrinsic.max(0.0), |ratio| width / ratio);
            (width, height)
        }
    };

    let transferred_width = definite_height
        .and_then(|height| ratio.map(|ratio| height * ratio))
        .unwrap_or(input.horizontal.intrinsic.max(0.0));
    let transferred_height = definite_width
        .and_then(|width| ratio.map(|ratio| width / ratio))
        .unwrap_or(input.vertical.intrinsic.max(0.0));
    let (min_width, max_width) = input.horizontal.bounds(
        transferred_width,
        input.box_sizing,
        [minimum_width, maximum_width],
    );
    let (min_height, max_height) =
        input
            .vertical
            .bounds(transferred_height, input.box_sizing, [None; 2]);

    let mut size = Size::new(width, height);
    constrain_replaced_size(
        &mut size,
        Size::new(min_width, min_height),
        Size::new(max_width, max_height),
        ratio,
        (definite_width.is_some(), definite_height.is_some()),
    );
    Size::new(size.width.max(0.0), size.height.max(0.0))
}

fn constrain_replaced_size(
    size: &mut Size,
    minimum: Size,
    maximum: Size,
    fallback_ratio: Option<f64>,
    definite: (bool, bool),
) {
    let original = *size;
    // Constraint transfer on a replaced element follows its preferred aspect
    // ratio. Using the tentative ratio would make a clamped `width: 1px`
    // distort a definite stretched height instead of restoring the intrinsic
    // ratio.
    let ratio = fallback_ratio.unwrap_or_else(|| {
        if original.width > 0.0 && original.height > 0.0 {
            original.width / original.height
        } else {
            1.0
        }
    });
    let width_low = original.width < minimum.width;
    let width_high = original.width > maximum.width;
    let height_low = original.height < minimum.height;
    let height_high = original.height > maximum.height;

    match (width_low, width_high, height_low, height_high) {
        (false, false, false, false) => {}
        (false, true, false, false) => {
            size.width = maximum.width;
            if !definite.1 {
                size.height = (maximum.width / ratio).max(minimum.height);
            }
        }
        (true, false, false, false) => {
            size.width = minimum.width;
            if !definite.1 {
                size.height = (minimum.width / ratio).min(maximum.height);
            }
        }
        (false, false, false, true) => {
            if !definite.0 {
                size.width = (maximum.height * ratio).max(minimum.width);
            }
            size.height = maximum.height;
        }
        (false, false, true, false) => {
            if !definite.0 {
                size.width = (minimum.height * ratio).min(maximum.width);
            }
            size.height = minimum.height;
        }
        (false, true, false, true) => {
            if maximum.width / original.width <= maximum.height / original.height {
                size.width = maximum.width;
                size.height = (maximum.width / ratio).max(minimum.height);
            } else {
                size.width = (maximum.height * ratio).max(minimum.width);
                size.height = maximum.height;
            }
        }
        (true, false, true, false) => {
            size.width = minimum.width.max(minimum.height * ratio).min(maximum.width);
            size.height = (size.width / ratio).max(minimum.height).min(maximum.height);
        }
        (true, false, false, true) => {
            size.width = minimum.width;
            size.height = maximum.height;
        }
        (false, true, true, false) => {
            size.width = maximum.width;
            size.height = minimum.height;
        }
        _ => {
            size.width = original.width.clamp(minimum.width, maximum.width);
            size.height = original.height.clamp(minimum.height, maximum.height);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use html_style_model::{ObjectPositionOrigin, UsedLengthPct, UsedObjectPositionAxis};

    fn position(x: f32, y: f32) -> UsedObjectPosition {
        UsedObjectPosition {
            x: UsedObjectPositionAxis {
                origin: ObjectPositionOrigin::Start,
                offset: UsedLengthPct::Pct(x),
            },
            y: UsedObjectPositionAxis {
                origin: ObjectPositionOrigin::Start,
                offset: UsedLengthPct::Pct(y),
            },
        }
    }

    #[test]
    fn object_fit_preserves_ratio_and_positions_in_free_space() {
        let contain = replaced_object_geometry(
            Size::new(100.0, 100.0),
            Size::new(200.0, 100.0),
            ObjectFit::Contain,
            position(0.5, 0.5),
        );
        assert_eq!(contain.rect, Rect::new(0.0, 25.0, 100.0, 75.0));
        assert_eq!(contain.clip, Rect::new(0.0, 0.0, 100.0, 100.0));

        let cover = replaced_object_geometry(
            Size::new(100.0, 100.0),
            Size::new(200.0, 100.0),
            ObjectFit::Cover,
            position(0.5, 0.5),
        );
        assert_eq!(cover.rect, Rect::new(-50.0, 0.0, 150.0, 100.0));
        assert_eq!(cover.clip, Rect::new(0.0, 0.0, 100.0, 100.0));
    }

    #[test]
    fn object_fit_none_and_scale_down_choose_the_correct_concrete_size() {
        let none = replaced_object_geometry(
            Size::new(100.0, 100.0),
            Size::new(40.0, 20.0),
            ObjectFit::None,
            position(1.0, 1.0),
        );
        assert_eq!(none.rect, Rect::new(60.0, 80.0, 100.0, 100.0));
        let smaller = replaced_object_geometry(
            Size::new(100.0, 100.0),
            Size::new(40.0, 20.0),
            ObjectFit::ScaleDown,
            position(0.5, 0.5),
        );
        assert_eq!(smaller.rect.size(), Size::new(40.0, 20.0));
        let reduced = replaced_object_geometry(
            Size::new(100.0, 100.0),
            Size::new(200.0, 100.0),
            ObjectFit::ScaleDown,
            position(0.5, 0.5),
        );
        assert_eq!(reduced.rect.size(), Size::new(100.0, 50.0));

        let from_end = UsedObjectPosition {
            x: UsedObjectPositionAxis {
                origin: ObjectPositionOrigin::End,
                offset: UsedLengthPct::Px(10.0),
            },
            y: UsedObjectPositionAxis {
                origin: ObjectPositionOrigin::End,
                offset: UsedLengthPct::Px(5.0),
            },
        };
        let positioned = replaced_object_geometry(
            Size::new(100.0, 100.0),
            Size::new(40.0, 20.0),
            ObjectFit::None,
            from_end,
        );
        assert_eq!(positioned.rect, Rect::new(50.0, 75.0, 90.0, 95.0));
    }

    fn base_input() -> ReplacedSizeInput {
        ReplacedSizeInput {
            horizontal: ReplacedAxis {
                size: PreferredSize::Auto,
                min: PreferredSize::Auto,
                max: PreferredSize::Auto,
                intrinsic: 100.0,
                basis: Some(200.0),
                margin: 10.0,
                padding_border: 10.0,
            },
            vertical: ReplacedAxis {
                size: PreferredSize::Auto,
                min: PreferredSize::Auto,
                max: PreferredSize::Auto,
                intrinsic: 100.0,
                basis: Some(100.0),
                margin: 10.0,
                padding_border: 10.0,
            },
            aspect_ratio: Some(1.0),
            box_sizing: BoxSizing::ContentBox,
        }
    }

    #[test]
    fn preferred_ratio_selects_intrinsic_and_authored_sources_once() {
        let explicit = AspectRatio::new(false, Some((4.0, 3.0))).expect("valid ratio");
        let auto_with_fallback = AspectRatio::new(true, Some((4.0, 3.0))).expect("valid ratio");

        assert_eq!(
            preferred_aspect_ratio(AspectRatio::AUTO, Some(2.0)),
            Some(2.0)
        );
        assert_eq!(
            preferred_aspect_ratio(explicit, Some(2.0)),
            explicit.preferred().map(f64::from)
        );
        assert_eq!(
            preferred_aspect_ratio(auto_with_fallback, None),
            auto_with_fallback.preferred().map(f64::from)
        );
    }

    #[test]
    fn stretch_fills_the_margin_box_and_transfers_through_ratio() {
        let mut input = base_input();
        input.horizontal.size = PreferredSize::Stretch;
        let size = resolve_replaced_content_size(input, [None; 3]);
        assert_eq!(size, Size::new(180.0, 180.0));

        let mut input = base_input();
        input.vertical.size = PreferredSize::Stretch;
        let size = resolve_replaced_content_size(input, [None; 3]);
        assert_eq!(size, Size::new(80.0, 80.0));
    }

    #[test]
    fn intrinsic_width_keyword_transfers_a_definite_height() {
        let mut input = base_input();
        input.horizontal.size = PreferredSize::MinContent;
        input.vertical.size = PreferredSize::Px(50.0);
        assert_eq!(
            resolve_replaced_content_size(input, [None; 3]),
            Size::new(50.0, 50.0)
        );
    }

    #[test]
    fn intrinsic_minimum_transfers_across_the_ratio() {
        let mut input = base_input();
        input.horizontal.size = PreferredSize::Auto;
        input.vertical.size = PreferredSize::Px(0.0);
        input.vertical.min = PreferredSize::MinContent;
        assert_eq!(
            resolve_replaced_content_size(input, [None; 3]),
            Size::new(100.0, 100.0)
        );
    }

    #[test]
    fn opposing_constraints_are_allowed_to_break_the_ratio() {
        let mut input = base_input();
        input.horizontal.min = PreferredSize::Px(130.0);
        input.vertical.max = PreferredSize::Px(10.0);
        assert_eq!(
            resolve_replaced_content_size(input, [None; 3]),
            Size::new(130.0, 10.0)
        );
    }

    #[test]
    fn inline_constraint_does_not_change_a_definite_block_size() {
        let mut input = base_input();
        input.horizontal.size = PreferredSize::Auto;
        input.vertical.size = PreferredSize::Px(0.0);
        input.horizontal.min = PreferredSize::Px(50.0);
        input.vertical.min = PreferredSize::Stretch;
        input.vertical.basis = None;
        assert_eq!(
            resolve_replaced_content_size(input, [None; 3]),
            Size::new(50.0, 0.0)
        );
    }

    #[test]
    fn simultaneous_intrinsic_minimums_choose_the_larger_ratio_preserving_size() {
        let mut input = base_input();
        input.horizontal.size = PreferredSize::Auto;
        input.vertical.size = PreferredSize::Px(0.0);
        input.horizontal.min = PreferredSize::Px(150.0);
        input.vertical.min = PreferredSize::MinContent;
        input.vertical.basis = None;
        assert_eq!(
            resolve_replaced_content_size(input, [None; 3]),
            Size::new(150.0, 150.0)
        );
    }

    fn flex_auto_min_input() -> ReplacedSizeInput {
        ReplacedSizeInput {
            horizontal: ReplacedAxis {
                intrinsic: 200.0,
                basis: Some(0.0),
                margin: 0.0,
                padding_border: 0.0,
                ..base_input().horizontal
            },
            vertical: ReplacedAxis {
                intrinsic: 200.0,
                basis: Some(120.0),
                margin: 20.0,
                padding_border: 0.0,
                ..base_input().vertical
            },
            ..base_input()
        }
    }

    #[test]
    fn flex_auto_minimum_transfers_the_stretched_cross_size() {
        assert_eq!(
            resolve_replaced_flex_auto_min_main_size(
                flex_auto_min_input(),
                ReplacedMainAxis::Horizontal,
                true,
            ),
            100.0
        );
    }

    #[test]
    fn flex_auto_minimum_applies_cross_constraints_before_transfer() {
        let mut input = flex_auto_min_input();
        input.vertical.max = PreferredSize::Px(5.0);
        assert_eq!(
            resolve_replaced_flex_auto_min_main_size(input, ReplacedMainAxis::Horizontal, true),
            5.0
        );

        input.vertical.min = PreferredSize::Px(10.0);
        assert_eq!(
            resolve_replaced_flex_auto_min_main_size(input, ReplacedMainAxis::Horizontal, true),
            10.0
        );
    }

    #[test]
    fn flex_auto_minimum_caps_transfer_by_the_specified_main_size() {
        let mut input = flex_auto_min_input();
        input.horizontal.intrinsic = 10.0;
        input.vertical.intrinsic = 10.0;
        input.horizontal.basis = Some(50.0);
        input.vertical.basis = Some(0.0);
        input.vertical.margin = 0.0;
        input.vertical.size = PreferredSize::Px(100.0);
        assert_eq!(
            resolve_replaced_flex_auto_min_main_size(input, ReplacedMainAxis::Vertical, true),
            50.0
        );
    }

    #[test]
    fn flex_auto_minimum_preserves_border_box_constraint_semantics() {
        let mut input = flex_auto_min_input();
        input.vertical.basis = Some(30.0);
        input.vertical.margin = 0.0;
        input.horizontal.padding_border = 10.0;
        input.vertical.padding_border = 10.0;
        input.box_sizing = BoxSizing::BorderBox;
        assert_eq!(
            resolve_replaced_flex_auto_min_main_size(input, ReplacedMainAxis::Horizontal, true),
            30.0
        );
    }

    #[test]
    fn flex_auto_minimum_uses_each_physical_axis_for_percentage_constraints() {
        let input = ReplacedSizeInput {
            horizontal: ReplacedAxis {
                size: PreferredSize::Percent(0.5),
                min: PreferredSize::Px(50.0),
                max: PreferredSize::Px(400.0),
                intrinsic: 360.0,
                basis: Some(500.0),
                margin: 11.0,
                padding_border: 20.0,
            },
            vertical: ReplacedAxis {
                size: PreferredSize::Percent(0.5),
                min: PreferredSize::Px(20.0),
                max: PreferredSize::Px(100.0),
                intrinsic: 120.0,
                basis: Some(100.0),
                margin: 7.0,
                padding_border: 10.0,
            },
            aspect_ratio: Some(3.0),
            box_sizing: BoxSizing::BorderBox,
        };
        // A 40px cross size transfers to a 120px main size, plus its 20px inset.
        assert_eq!(
            resolve_replaced_flex_auto_min_main_size(input, ReplacedMainAxis::Horizontal, false),
            140.0
        );
        // Width transfers 230 / 3, capped by the definite 40px main size.
        assert_eq!(
            resolve_replaced_flex_auto_min_main_size(input, ReplacedMainAxis::Vertical, false),
            50.0
        );
        assert_eq!(
            resolve_replaced_content_size(input, [None; 3]),
            Size::new(230.0, 40.0)
        );
    }

    #[test]
    fn flex_auto_minimum_keeps_an_indefinite_height_indefinite() {
        let mut input = flex_auto_min_input();
        input.horizontal.intrinsic = 300.0;
        input.vertical.intrinsic = 100.0;
        input.aspect_ratio = Some(3.0);
        input.horizontal.basis = Some(90.0);
        input.vertical.basis = None;
        input.vertical.margin = 0.0;
        input.vertical.size = PreferredSize::Percent(0.5);
        assert_eq!(
            resolve_replaced_flex_auto_min_main_size(input, ReplacedMainAxis::Horizontal, true),
            300.0
        );
        assert_eq!(
            resolve_replaced_flex_auto_min_main_size(input, ReplacedMainAxis::Vertical, true),
            30.0
        );
    }

    #[test]
    fn intrinsic_constraint_conversion_preserves_the_adapter_box_model() {
        assert_eq!(
            resolve_replaced_intrinsic_constraint(
                PreferredSize::MaxContent,
                50.0,
                10.0,
                BoxSizing::ContentBox
            ),
            Some(50.0)
        );
        assert_eq!(
            resolve_replaced_intrinsic_constraint(
                PreferredSize::MinContent,
                50.0,
                10.0,
                BoxSizing::BorderBox
            ),
            Some(60.0)
        );
        assert_eq!(
            resolve_replaced_intrinsic_constraint(
                PreferredSize::FitContent,
                50.0,
                10.0,
                BoxSizing::BorderBox
            ),
            Some(60.0)
        );
        assert_eq!(
            resolve_replaced_intrinsic_constraint(
                PreferredSize::Auto,
                50.0,
                10.0,
                BoxSizing::BorderBox
            ),
            None
        );
    }

    #[test]
    fn definite_intrinsic_contribution_is_clamped_before_adapter_transfer() {
        assert_eq!(
            clamp_replaced_definite_size_by_intrinsic_constraints(
                PreferredSize::Px(100.0),
                None,
                Some(50.0)
            ),
            Some(50.0)
        );
        assert_eq!(
            clamp_replaced_definite_size_by_intrinsic_constraints(
                PreferredSize::Px(0.0),
                Some(50.0),
                None
            ),
            Some(50.0)
        );
        assert_eq!(
            clamp_replaced_definite_size_by_intrinsic_constraints(
                PreferredSize::Auto,
                Some(50.0),
                Some(50.0)
            ),
            None
        );
    }
}
