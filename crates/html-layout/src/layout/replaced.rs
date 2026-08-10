use html_style_model::{AspectRatio, BoxSizing, UsedPreferredSize as PreferredSize, UsedStyleView};
use kurbo::Size;
use taffy::geometry::Size as TaffySize;
use taffy::prelude::AvailableSpace;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ReplacedSizeInput {
    pub intrinsic: Size,
    pub aspect_ratio: Option<f64>,
    pub width: PreferredSize,
    pub height: PreferredSize,
    pub min_width: PreferredSize,
    pub min_height: PreferredSize,
    pub max_width: PreferredSize,
    pub max_height: PreferredSize,
    pub available_width: f64,
    pub available_height: Option<f64>,
    pub horizontal_margin: f64,
    pub vertical_margin: f64,
    pub horizontal_padding_border: f64,
    pub vertical_padding_border: f64,
    pub box_sizing: BoxSizing,
}

impl ReplacedSizeInput {
    pub(crate) fn from_style(style: UsedStyleView<'_>, intrinsic: Size, aspect_ratio: Option<f64>, available_width: f64, available_height: Option<f64>) -> Self {
        Self {
            intrinsic,
            aspect_ratio,
            width: style.width(),
            height: style.height(),
            min_width: style.min_width(),
            min_height: style.min_height(),
            max_width: style.max_width(),
            max_height: style.max_height(),
            available_width,
            available_height,
            horizontal_margin: 0.0,
            vertical_margin: 0.0,
            horizontal_padding_border: 0.0,
            vertical_padding_border: 0.0,
            box_sizing: style.box_sizing(),
        }
    }

    pub(crate) fn with_box_model(mut self, horizontal_margin: f64, vertical_margin: f64, horizontal_padding_border: f64, vertical_padding_border: f64) -> Self {
        self.horizontal_margin = horizontal_margin;
        self.vertical_margin = vertical_margin;
        self.horizontal_padding_border = horizontal_padding_border;
        self.vertical_padding_border = vertical_padding_border;
        self
    }

    pub(crate) fn with_width_constraints(mut self, width: PreferredSize, min_width: PreferredSize, max_width: PreferredSize) -> Self {
        self.width = width;
        self.min_width = min_width;
        self.max_width = max_width;
        self
    }
}

pub(crate) fn preferred_aspect_ratio(authored: AspectRatio, intrinsic: Option<f64>) -> Option<f64> {
    if authored.uses_intrinsic() { intrinsic.or_else(|| authored.preferred().map(f64::from)) } else { authored.preferred().map(f64::from) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReplacedMainAxis {
    Horizontal,
    Vertical,
}

/// Inputs for the automatic minimum main size of a replaced flex item.
///
/// Flexbox first obtains a content-size suggestion in the main axis. A
/// definite or stretched cross size is transferred through the preferred
/// aspect ratio, after applying the cross-axis constraints. The resulting
/// suggestion is then capped by a definite preferred main size and by the
/// main-axis constraints. Keeping this calculation outside the Taffy adapter
/// prevents its style and measure paths from independently guessing which
/// axis owns ratio transfer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReplacedFlexAutoMinInput {
    pub intrinsic: Size,
    pub aspect_ratio: Option<f64>,
    pub main_axis: ReplacedMainAxis,
    pub main_size: PreferredSize,
    pub cross_size: PreferredSize,
    pub min_main_size: PreferredSize,
    pub min_cross_size: PreferredSize,
    pub max_main_size: PreferredSize,
    pub max_cross_size: PreferredSize,
    pub main_basis: Option<f64>,
    pub cross_basis: Option<f64>,
    pub main_margin: f64,
    pub cross_margin: f64,
    pub main_padding_border: f64,
    pub cross_padding_border: f64,
    pub box_sizing: BoxSizing,
    pub stretch_cross_size: bool,
}

/// Resolve the constraint value Taffy should receive for `min-size: auto` in
/// the main axis of a replaced flex item.
pub(crate) fn resolve_replaced_flex_auto_min_main_size(input: ReplacedFlexAutoMinInput) -> f64 {
    let (intrinsic_main, intrinsic_cross) = match input.main_axis {
        ReplacedMainAxis::Horizontal => (input.intrinsic.width, input.intrinsic.height),
        ReplacedMainAxis::Vertical => (input.intrinsic.height, input.intrinsic.width),
    };
    let ratio = input.aspect_ratio.filter(|ratio| ratio.is_finite() && *ratio > 0.0);
    let effective_cross_size = if input.stretch_cross_size && matches!(input.cross_size, PreferredSize::Auto | PreferredSize::Stretch) { PreferredSize::Stretch } else { input.cross_size };
    let tentative_cross = resolve_definite(effective_cross_size, input.cross_basis, input.cross_margin, input.cross_padding_border, input.box_sizing).unwrap_or(intrinsic_cross.max(0.0));
    let cross_min = resolve_bound(input.min_cross_size, 0.0, intrinsic_cross.max(0.0), input.cross_basis, input.cross_margin, input.cross_padding_border, input.box_sizing);
    let cross_max = resolve_bound(input.max_cross_size, f64::INFINITY, intrinsic_cross.max(0.0), input.cross_basis, input.cross_margin, input.cross_padding_border, input.box_sizing).max(cross_min);
    let cross = tentative_cross.clamp(cross_min, cross_max);
    let transferred_main = ratio.map_or(intrinsic_main.max(0.0), |ratio| match input.main_axis {
        ReplacedMainAxis::Horizontal => cross * ratio,
        ReplacedMainAxis::Vertical => cross / ratio,
    });

    let main_min = resolve_bound(input.min_main_size, 0.0, intrinsic_main.max(0.0), input.main_basis, input.main_margin, input.main_padding_border, input.box_sizing);
    let main_max = resolve_bound(input.max_main_size, f64::INFINITY, intrinsic_main.max(0.0), input.main_basis, input.main_margin, input.main_padding_border, input.box_sizing).max(main_min);
    let mut automatic_minimum = transferred_main.clamp(main_min, main_max);
    if let Some(specified) = resolve_definite(input.main_size, input.main_basis, input.main_margin, input.main_padding_border, input.box_sizing) {
        automatic_minimum = automatic_minimum.min(specified.clamp(main_min, main_max));
    }

    match input.box_sizing {
        BoxSizing::ContentBox => automatic_minimum,
        BoxSizing::BorderBox => automatic_minimum + input.main_padding_border,
    }
}

/// Convert an intrinsic sizing keyword used as a replaced-element constraint
/// into the numeric value expected by a formatting-context adapter. The
/// intrinsic dimensions stored by the renderer are content-box dimensions;
/// an adapter whose sizing model consumes border-box constraints must include
/// the corresponding padding and border exactly once.
pub(crate) fn resolve_replaced_intrinsic_constraint(value: PreferredSize, intrinsic_content_size: f64, padding_border: f64, box_sizing: BoxSizing) -> Option<f64> {
    if !matches!(value, PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent) {
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
pub(crate) fn clamp_replaced_definite_size_by_intrinsic_constraints(preferred: PreferredSize, intrinsic_minimum: Option<f64>, intrinsic_maximum: Option<f64>) -> Option<f64> {
    let PreferredSize::Px(preferred) = preferred else { return None };
    let minimum = intrinsic_minimum.unwrap_or(0.0).max(0.0);
    let maximum = intrinsic_maximum.unwrap_or(f64::INFINITY).max(minimum);
    Some((preferred.max(0.0) as f64).clamp(minimum, maximum))
}

/// Resolve the content-box size of a replaced element before its formatting
/// context lays out descendants. Intrinsic sizing keywords on replaced
/// elements behave like `auto`, but can transfer a definite size through the
/// preferred aspect ratio. `stretch` is different: it fills the available
/// margin box, regardless of `box-sizing`.
pub(crate) fn resolve_replaced_content_size(input: ReplacedSizeInput) -> Size {
    let ratio = input.aspect_ratio.filter(|ratio| ratio.is_finite() && *ratio > 0.0);
    let definite_width = resolve_definite(input.width, Some(input.available_width), input.horizontal_margin, input.horizontal_padding_border, input.box_sizing);
    let definite_height = resolve_definite(input.height, input.available_height, input.vertical_margin, input.vertical_padding_border, input.box_sizing);

    let (mut width, mut height) = match (definite_width, definite_height, ratio) {
        (Some(width), Some(height), _) => (width, height),
        (Some(width), None, Some(ratio)) => (width, width / ratio),
        (None, Some(height), Some(ratio)) => (height * ratio, height),
        (Some(width), None, None) => (width, input.intrinsic.height.max(0.0)),
        (None, Some(height), None) => (input.intrinsic.width.max(0.0), height),
        (None, None, _) => {
            let intrinsic_width = input.intrinsic.width.max(0.0);
            let available_content = (input.available_width - input.horizontal_margin - input.horizontal_padding_border).max(0.0);
            let width = if matches!(input.width, PreferredSize::Auto | PreferredSize::FitContent) { intrinsic_width.min(available_content) } else { intrinsic_width };
            let height = ratio.map_or_else(|| input.intrinsic.height.max(0.0), |ratio| width / ratio);
            (width, height)
        }
    };

    let transferred_width = definite_height.and_then(|height| ratio.map(|ratio| height * ratio)).unwrap_or(input.intrinsic.width.max(0.0));
    let transferred_height = definite_width.and_then(|width| ratio.map(|ratio| width / ratio)).unwrap_or(input.intrinsic.height.max(0.0));
    let min_width = resolve_bound(input.min_width, 0.0, transferred_width, Some(input.available_width), input.horizontal_margin, input.horizontal_padding_border, input.box_sizing);
    let min_height = resolve_bound(input.min_height, 0.0, transferred_height, input.available_height, input.vertical_margin, input.vertical_padding_border, input.box_sizing);
    let max_width = resolve_bound(input.max_width, f64::INFINITY, transferred_width, Some(input.available_width), input.horizontal_margin, input.horizontal_padding_border, input.box_sizing).max(min_width);
    let max_height = resolve_bound(input.max_height, f64::INFINITY, transferred_height, input.available_height, input.vertical_margin, input.vertical_padding_border, input.box_sizing).max(min_height);

    constrain_replaced_size(&mut width, &mut height, min_width, min_height, max_width, max_height, ratio, definite_width.is_some(), definite_height.is_some());
    Size::new(width.max(0.0), height.max(0.0))
}

fn resolve_definite(value: PreferredSize, basis: Option<f64>, margin: f64, padding_border: f64, box_sizing: BoxSizing) -> Option<f64> {
    let border_box_to_content = |value: f64| match box_sizing {
        BoxSizing::ContentBox => value.max(0.0),
        BoxSizing::BorderBox => (value - padding_border).max(0.0),
    };
    match value {
        PreferredSize::Px(value) => Some(border_box_to_content(value as f64)),
        PreferredSize::Percent(value) => basis.map(|basis| border_box_to_content(basis * value as f64)),
        PreferredSize::Calc { absolute_px, percentage, percentage_dependent } => {
            if percentage_dependent {
                basis.map(|basis| border_box_to_content(absolute_px as f64 + basis * percentage as f64))
            } else {
                Some(border_box_to_content(absolute_px as f64))
            }
        }
        PreferredSize::Comparison { .. } => {
            if value.percentage_dependent() {
                basis.map(|basis| border_box_to_content(html_style_model::resolve_used_preferred_size(value, 0.0, basis)))
            } else {
                Some(border_box_to_content(html_style_model::resolve_used_preferred_size(value, 0.0, 0.0)))
            }
        }
        PreferredSize::Stretch => basis.map(|basis| (basis - margin - padding_border).max(0.0)),
        PreferredSize::Auto | PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent => None,
    }
}

fn resolve_bound(value: PreferredSize, auto: f64, intrinsic: f64, basis: Option<f64>, margin: f64, padding_border: f64, box_sizing: BoxSizing) -> f64 {
    match value {
        PreferredSize::Auto => auto,
        PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent => intrinsic,
        _ => resolve_definite(value, basis, margin, padding_border, box_sizing).unwrap_or(auto),
    }
}

fn constrain_replaced_size(width: &mut f64, height: &mut f64, min_width: f64, min_height: f64, max_width: f64, max_height: f64, fallback_ratio: Option<f64>, width_was_definite: bool, height_was_definite: bool) {
    let original_width = *width;
    let original_height = *height;
    // Constraint transfer on a replaced element follows its preferred aspect
    // ratio. Using the tentative ratio would make a clamped `width: 1px`
    // distort a definite stretched height instead of restoring the intrinsic
    // ratio.
    let ratio = fallback_ratio.unwrap_or_else(|| if original_width > 0.0 && original_height > 0.0 { original_width / original_height } else { 1.0 });
    let width_low = original_width < min_width;
    let width_high = original_width > max_width;
    let height_low = original_height < min_height;
    let height_high = original_height > max_height;

    match (width_low, width_high, height_low, height_high) {
        (false, false, false, false) => {}
        (false, true, false, false) => {
            *width = max_width;
            if !height_was_definite {
                *height = (max_width / ratio).max(min_height);
            }
        }
        (true, false, false, false) => {
            *width = min_width;
            if !height_was_definite {
                *height = (min_width / ratio).min(max_height);
            }
        }
        (false, false, false, true) => {
            if !width_was_definite {
                *width = (max_height * ratio).max(min_width);
            }
            *height = max_height;
        }
        (false, false, true, false) => {
            if !width_was_definite {
                *width = (min_height * ratio).min(max_width);
            }
            *height = min_height;
        }
        (false, true, false, true) => {
            if max_width / original_width <= max_height / original_height {
                *width = max_width;
                *height = (max_width / ratio).max(min_height);
            } else {
                *width = (max_height * ratio).max(min_width);
                *height = max_height;
            }
        }
        (true, false, true, false) => {
            *width = min_width.max(min_height * ratio).min(max_width);
            *height = (*width / ratio).max(min_height).min(max_height);
        }
        (true, false, false, true) => {
            *width = min_width;
            *height = max_height;
        }
        (false, true, true, false) => {
            *width = max_width;
            *height = min_height;
        }
        _ => {
            *width = original_width.clamp(min_width, max_width);
            *height = original_height.clamp(min_height, max_height);
        }
    }
}

/// Taffy's known dimensions are border-box dimensions while its measure
/// callback returns content-box dimensions. When a known axis is present,
/// Taffy has already subtracted padding and borders from the corresponding
/// definite available space; use that content size for ratio transfer.
pub(crate) fn measure_replaced_content(intrinsic: Size, aspect_ratio: Option<f64>, known: TaffySize<Option<f32>>, available: TaffySize<AvailableSpace>) -> TaffySize<f32> {
    let known_content_width = known.width.map(|width| match available.width {
        AvailableSpace::Definite(content) => content.max(0.0),
        AvailableSpace::MinContent | AvailableSpace::MaxContent => width.max(0.0),
    });
    let known_content_height = known.height.map(|height| match available.height {
        AvailableSpace::Definite(content) => content.max(0.0),
        AvailableSpace::MinContent | AvailableSpace::MaxContent => height.max(0.0),
    });
    let ratio = aspect_ratio.filter(|ratio| ratio.is_finite() && *ratio > 0.0);
    match (known_content_width, known_content_height, ratio) {
        (Some(width), Some(height), _) => TaffySize { width, height },
        (Some(width), None, Some(ratio)) => TaffySize { width, height: finite_f32(f64::from(width) / ratio) },
        (None, Some(height), Some(ratio)) => TaffySize { width: finite_f32(f64::from(height) * ratio), height },
        (width, height, _) => TaffySize { width: width.unwrap_or_else(|| finite_f32(intrinsic.width)), height: height.unwrap_or_else(|| finite_f32(intrinsic.height)) },
    }
}

fn finite_f32(value: f64) -> f32 {
    value.clamp(0.0, f32::MAX as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input() -> ReplacedSizeInput {
        ReplacedSizeInput {
            intrinsic: Size::new(100.0, 100.0),
            aspect_ratio: Some(1.0),
            width: PreferredSize::Auto,
            height: PreferredSize::Auto,
            min_width: PreferredSize::Auto,
            min_height: PreferredSize::Auto,
            max_width: PreferredSize::Auto,
            max_height: PreferredSize::Auto,
            available_width: 200.0,
            available_height: Some(100.0),
            horizontal_margin: 10.0,
            vertical_margin: 10.0,
            horizontal_padding_border: 10.0,
            vertical_padding_border: 10.0,
            box_sizing: BoxSizing::ContentBox,
        }
    }

    #[test]
    fn preferred_ratio_selects_intrinsic_and_authored_sources_once() {
        let explicit = AspectRatio::new(false, Some((4.0, 3.0))).expect("valid ratio");
        let auto_with_fallback = AspectRatio::new(true, Some((4.0, 3.0))).expect("valid ratio");

        assert_eq!(preferred_aspect_ratio(AspectRatio::AUTO, Some(2.0)), Some(2.0));
        assert_eq!(preferred_aspect_ratio(explicit, Some(2.0)), explicit.preferred().map(f64::from));
        assert_eq!(preferred_aspect_ratio(auto_with_fallback, None), auto_with_fallback.preferred().map(f64::from));
    }

    #[test]
    fn stretch_fills_the_margin_box_and_transfers_through_ratio() {
        let mut input = base_input();
        input.width = PreferredSize::Stretch;
        let size = resolve_replaced_content_size(input);
        assert_eq!(size, Size::new(180.0, 180.0));

        let mut input = base_input();
        input.height = PreferredSize::Stretch;
        let size = resolve_replaced_content_size(input);
        assert_eq!(size, Size::new(80.0, 80.0));
    }

    #[test]
    fn intrinsic_width_keyword_transfers_a_definite_height() {
        let mut input = base_input();
        input.width = PreferredSize::MinContent;
        input.height = PreferredSize::Px(50.0);
        assert_eq!(resolve_replaced_content_size(input), Size::new(50.0, 50.0));
    }

    #[test]
    fn intrinsic_minimum_transfers_across_the_ratio() {
        let mut input = base_input();
        input.width = PreferredSize::Auto;
        input.height = PreferredSize::Px(0.0);
        input.min_height = PreferredSize::MinContent;
        assert_eq!(resolve_replaced_content_size(input), Size::new(100.0, 100.0));
    }

    #[test]
    fn opposing_constraints_are_allowed_to_break_the_ratio() {
        let mut input = base_input();
        input.min_width = PreferredSize::Px(130.0);
        input.max_height = PreferredSize::Px(10.0);
        assert_eq!(resolve_replaced_content_size(input), Size::new(130.0, 10.0));
    }

    #[test]
    fn inline_constraint_does_not_change_a_definite_block_size() {
        let mut input = base_input();
        input.width = PreferredSize::Auto;
        input.height = PreferredSize::Px(0.0);
        input.min_width = PreferredSize::Px(50.0);
        input.min_height = PreferredSize::Stretch;
        input.available_height = None;
        assert_eq!(resolve_replaced_content_size(input), Size::new(50.0, 0.0));
    }

    #[test]
    fn simultaneous_intrinsic_minimums_choose_the_larger_ratio_preserving_size() {
        let mut input = base_input();
        input.width = PreferredSize::Auto;
        input.height = PreferredSize::Px(0.0);
        input.min_width = PreferredSize::Px(150.0);
        input.min_height = PreferredSize::MinContent;
        input.available_height = None;
        assert_eq!(resolve_replaced_content_size(input), Size::new(150.0, 150.0));
    }

    #[test]
    fn taffy_measurement_uses_definite_content_box_space() {
        let measured = measure_replaced_content(Size::new(16.0, 16.0), Some(1.0), TaffySize { width: Some(30.0), height: None }, TaffySize { width: AvailableSpace::Definite(24.0), height: AvailableSpace::MaxContent });
        assert_eq!(measured, TaffySize { width: 24.0, height: 24.0 });
    }

    fn flex_auto_min_input(main_axis: ReplacedMainAxis) -> ReplacedFlexAutoMinInput {
        ReplacedFlexAutoMinInput {
            intrinsic: Size::new(200.0, 200.0),
            aspect_ratio: Some(1.0),
            main_axis,
            main_size: PreferredSize::Auto,
            cross_size: PreferredSize::Auto,
            min_main_size: PreferredSize::Auto,
            min_cross_size: PreferredSize::Auto,
            max_main_size: PreferredSize::Auto,
            max_cross_size: PreferredSize::Auto,
            main_basis: Some(0.0),
            cross_basis: Some(120.0),
            main_margin: 0.0,
            cross_margin: 20.0,
            main_padding_border: 0.0,
            cross_padding_border: 0.0,
            box_sizing: BoxSizing::ContentBox,
            stretch_cross_size: true,
        }
    }

    #[test]
    fn flex_auto_minimum_transfers_the_stretched_cross_size() {
        assert_eq!(resolve_replaced_flex_auto_min_main_size(flex_auto_min_input(ReplacedMainAxis::Horizontal)), 100.0);
    }

    #[test]
    fn flex_auto_minimum_applies_cross_constraints_before_transfer() {
        let mut input = flex_auto_min_input(ReplacedMainAxis::Horizontal);
        input.max_cross_size = PreferredSize::Px(5.0);
        assert_eq!(resolve_replaced_flex_auto_min_main_size(input), 5.0);

        input.max_cross_size = PreferredSize::Px(5.0);
        input.min_cross_size = PreferredSize::Px(10.0);
        assert_eq!(resolve_replaced_flex_auto_min_main_size(input), 10.0);
    }

    #[test]
    fn flex_auto_minimum_caps_transfer_by_the_specified_main_size() {
        let mut input = flex_auto_min_input(ReplacedMainAxis::Vertical);
        input.intrinsic = Size::new(10.0, 10.0);
        input.cross_basis = Some(50.0);
        input.cross_margin = 0.0;
        input.main_size = PreferredSize::Px(100.0);
        assert_eq!(resolve_replaced_flex_auto_min_main_size(input), 50.0);
    }

    #[test]
    fn flex_auto_minimum_preserves_border_box_constraint_semantics() {
        let mut input = flex_auto_min_input(ReplacedMainAxis::Horizontal);
        input.cross_basis = Some(30.0);
        input.cross_margin = 0.0;
        input.main_padding_border = 10.0;
        input.cross_padding_border = 10.0;
        input.box_sizing = BoxSizing::BorderBox;
        assert_eq!(resolve_replaced_flex_auto_min_main_size(input), 30.0);
    }

    #[test]
    fn intrinsic_constraint_conversion_preserves_the_adapter_box_model() {
        assert_eq!(resolve_replaced_intrinsic_constraint(PreferredSize::MaxContent, 50.0, 10.0, BoxSizing::ContentBox), Some(50.0));
        assert_eq!(resolve_replaced_intrinsic_constraint(PreferredSize::MinContent, 50.0, 10.0, BoxSizing::BorderBox), Some(60.0));
        assert_eq!(resolve_replaced_intrinsic_constraint(PreferredSize::FitContent, 50.0, 10.0, BoxSizing::BorderBox), Some(60.0));
        assert_eq!(resolve_replaced_intrinsic_constraint(PreferredSize::Auto, 50.0, 10.0, BoxSizing::BorderBox), None);
    }

    #[test]
    fn definite_intrinsic_contribution_is_clamped_before_adapter_transfer() {
        assert_eq!(clamp_replaced_definite_size_by_intrinsic_constraints(PreferredSize::Px(100.0), None, Some(50.0)), Some(50.0));
        assert_eq!(clamp_replaced_definite_size_by_intrinsic_constraints(PreferredSize::Px(0.0), Some(50.0), None), Some(50.0));
        assert_eq!(clamp_replaced_definite_size_by_intrinsic_constraints(PreferredSize::Auto, Some(50.0), Some(50.0)), None);
    }
}
