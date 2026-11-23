use super::replaced::{ReplacedSizeInput, preferred_aspect_ratio, resolve_replaced_content_size};
use crate::{BoxLayoutRequest, LayoutEngine};
use html_style_model::{
    BoxSizing, Display, Float, ItemAlignment, TextDirection, UsedPreferredSize as PreferredSize,
    UsedStyleView,
};
use kurbo::Size;

use super::constraints::{
    AutoMarginOverflow, resolve_auto_start_margin, constrain_size, resolve_preferred_content_width,
    resolve_definite_content_size_with_insets, resolve_margin_left_from_edges,
    resolve_vertical_min_size_with_stretch_inset,
};
use super::model::{ResolvedBoxModel, SizingPolicy};
use super::replaced::ImageIntrinsic;
use crate::engine::BoxSizeSnapshot;
use crate::measurement::box_content_intrinsic_widths;

#[derive(Clone, Copy)]
pub(crate) struct ResolvedHorizontalSizing {
    pub(crate) auto_width: bool,
    pub(crate) min: f64,
    pub(crate) max: f64,
    pub(crate) content: f64,
    pub(crate) margin_padding: f64,
    suppresses_margins: bool,
    pub(crate) shrink_to_fit: bool,
    ratio_transferred_width: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct ResolvedVerticalSizing {
    authored_explicit: Option<f64>,
    pub(crate) explicit: Option<f64>,
    pub(crate) descendant_basis: Option<f64>,
    pub(crate) min: HeightConstraint,
    pub(crate) max: HeightConstraint,
    pub(crate) ratio_auto_min_uses_content: bool,
}

/// A height constraint is either ready now or waits for natural content height.
#[derive(Clone, Copy)]
pub(crate) enum HeightConstraint {
    Unconstrained,
    Definite(f64),
    Content,
}

impl HeightConstraint {
    pub(crate) fn is_intrinsic(self) -> bool {
        matches!(self, Self::Content)
    }

    pub(crate) fn resolve(self, content_height: Option<f64>) -> Option<f64> {
        match self {
            Self::Unconstrained => None,
            Self::Definite(value) => Some(value),
            Self::Content => content_height,
        }
    }
}

#[derive(Clone, Copy)]
struct ResolvedWidthConstraints {
    content_widths: [Option<f64>; 3],
    content_limits: (f64, f64),
    preferred: PreferredSize,
    min: PreferredSize,
    max: PreferredSize,
    horizontal_margin: f64,
    margin_padding: f64,
    border_box_inset: f64,
    available_content: f64,
    smart_width: bool,
    intrinsic: Option<(f64, f64)>,
    source: WidthSource,
}

#[derive(Clone, Copy)]
pub struct ResolvedHorizontalMargins {
    pub left: f64,
    pub total: f64,
}

/// Geometry-independent used sizing for one box.
///
/// A parent formatting context may inspect this value to place the box, then
/// pass the same value to `layout_resolved_box`; sizing is never guessed or
/// repeated by the parent.
#[derive(Clone, Copy)]
pub struct ResolvedBoxSizing {
    pub(crate) request: BoxLayoutRequest,
    pub(crate) box_model: ResolvedBoxModel,
    pub(crate) box_sizing: BoxSizing,
    pub(crate) policy: SizingPolicy,
    pub(crate) horizontal: ResolvedHorizontalSizing,
    pub(crate) vertical: ResolvedVerticalSizing,
    pub(crate) used_aspect_ratio: Option<f64>,
    pub(crate) replaced_size: Option<Size>,
    pub(crate) is_float: bool,
    pub(crate) isolates_floats: bool,
    pub(crate) fills_available_width: bool,
    pub(crate) inline_alignment: ItemAlignment,
    horizontal_margins: ResolvedHorizontalMargins,
    pub(crate) resolved_bottom_margin: f64,
    pub(crate) text_indent: f64,
}

impl ResolvedBoxSizing {
    pub fn box_idx(&self) -> usize {
        self.request.box_idx
    }

    pub fn available_width(&self) -> f64 {
        self.request.available_width
    }

    pub fn horizontal_margins(&self) -> ResolvedHorizontalMargins {
        self.horizontal_margins
    }

    pub fn with_first_line_indent(mut self, first_line_indent: f64) -> Self {
        self.request.first_line_indent = first_line_indent;
        self
    }
}

impl<'a, 'out> LayoutEngine<'a, 'out> {
    pub fn resolve_box_sizing(&mut self, request: BoxLayoutRequest) -> ResolvedBoxSizing {
        if let Some(cached) = self.scratch.box_sizing[request.box_idx]
            && cached.request == request
        {
            return cached;
        }
        let isolates_floats = self.read.reader.box_uses_float_context(request.box_idx);
        if isolates_floats {
            self.scratch.floats.push_context();
        }
        let resolved = resolve_box_sizing(self, request, isolates_floats);
        if isolates_floats {
            self.scratch.floats.pop_context();
        }
        self.scratch.box_sizing[request.box_idx] = Some(resolved);
        resolved
    }
}

fn resolve_box_sizing(
    engine: &mut LayoutEngine<'_, '_>,
    request: BoxLayoutRequest,
    isolates_floats: bool,
) -> ResolvedBoxSizing {
    let style = engine.read.reader.style_ref(request.box_idx);
    let raw_model = engine
        .read
        .reader
        .box_model(request.box_idx, request.available_width);
    let policy = engine.read.reader.sizing_policy(request.box_idx);
    let (box_model, resolved_bottom_margin) = resolve_box_model(engine, request, policy, raw_model);
    let image_intrinsic = engine.read.reader.image_intrinsic(request.box_idx);
    let input = SizingInput {
        request,
        style,
        sizes: engine.read.reader.sizes(request.box_idx),
        policy,
        box_model,
        raw_horizontal_inset: raw_model.horizontal_margin() + raw_model.horizontal_padding_border(),
        image_intrinsic,
        aspect_ratio: preferred_aspect_ratio(
            style.aspect_ratio(),
            image_intrinsic.and_then(|image| image.aspect_ratio),
        ),
        inline_alignment: effective_inline_alignment(engine, request.box_idx),
        is_float: engine.read.reader.box_facts(request.box_idx).float != Float::None,
    };
    let vertical = input.resolve_vertical_sizing(engine);
    let (horizontal, replaced_size) = input.resolve_horizontal_sizing(engine, vertical);
    let vertical = input.resolve_descendant_basis(vertical, horizontal);
    let fills_available_width = input.image_intrinsic.is_none()
        && !input.is_float
        && !horizontal.shrink_to_fit
        && (engine.read.reader.is_block_box(request.box_idx)
            || engine.read.reader.is_anonymous_box(request.box_idx)
            || matches!(
                engine.read.reader.box_facts(request.box_idx).display,
                Display::Flex | Display::Grid
            ));
    let horizontal_margins =
        input.resolve_horizontal_margins(engine, horizontal, replaced_size, fills_available_width);
    ResolvedBoxSizing {
        request,
        box_model,
        box_sizing: style.box_sizing(),
        policy: input.policy,
        horizontal,
        vertical,
        used_aspect_ratio: input.aspect_ratio,
        replaced_size,
        is_float: input.is_float,
        isolates_floats,
        fills_available_width,
        inline_alignment: input.inline_alignment,
        horizontal_margins,
        resolved_bottom_margin,
        text_indent: style.text_indent().resolve(horizontal.content),
    }
}

/// Immutable inputs shared by the sizing steps; the engine is borrowed only
/// by steps that read topology or measure content.
struct SizingInput<'a> {
    request: BoxLayoutRequest,
    style: &'a UsedStyleView<'a>,
    sizes: &'a BoxSizeSnapshot,
    policy: SizingPolicy,
    box_model: ResolvedBoxModel,
    raw_horizontal_inset: f64,
    image_intrinsic: Option<ImageIntrinsic>,
    aspect_ratio: Option<f64>,
    inline_alignment: ItemAlignment,
    is_float: bool,
}

#[derive(Clone, Copy)]
enum WidthSource {
    Assigned(f64),
    Replaced,
    Style,
    ShrinkToFit,
}

impl SizingInput<'_> {
    fn resolve_horizontal_sizing(
        &self,
        engine: &mut LayoutEngine<'_, '_>,
        vertical: ResolvedVerticalSizing,
    ) -> (ResolvedHorizontalSizing, Option<Size>) {
        let constraints = self.resolve_width_constraints(engine, vertical);
        let replaced_size = self.resolve_replaced_size(engine, constraints);
        let (content, ratio_transferred_width) =
            self.resolve_width(engine, vertical, constraints, replaced_size);
        (
            ResolvedHorizontalSizing {
                auto_width: constraints.content_widths[0].is_none()
                    && matches!(constraints.preferred, PreferredSize::Auto),
                min: constraints.content_limits.0,
                max: constraints.content_limits.1,
                content,
                margin_padding: constraints.margin_padding,
                suppresses_margins: constraints.smart_width,
                shrink_to_fit: matches!(constraints.source, WidthSource::ShrinkToFit),
                ratio_transferred_width,
            },
            replaced_size,
        )
    }

    fn resolve_width(
        &self,
        engine: &mut LayoutEngine<'_, '_>,
        vertical: ResolvedVerticalSizing,
        constraints: ResolvedWidthConstraints,
        replaced_size: Option<Size>,
    ) -> (f64, bool) {
        let ratio = match (constraints.source, self.sizes.width, self.aspect_ratio) {
            (WidthSource::Assigned(_) | WidthSource::Replaced, _, _) => None,
            (_, PreferredSize::Auto, ratio) => ratio,
            _ => None,
        };
        let Some(ratio) = ratio else {
            return (
                self.resolve_used_content_width(vertical, constraints, replaced_size),
                false,
            );
        };
        let transfer_height = |height: f64| match self.style.box_sizing() {
            BoxSizing::ContentBox => height * ratio,
            BoxSizing::BorderBox => ((height + self.box_model.vertical_padding_border()) * ratio
                - self.box_model.horizontal_padding_border())
            .max(0.0),
        };
        if let Some(height) = vertical.explicit {
            let mut width = transfer_height(constrain_size(
                height,
                vertical.min.resolve(None),
                vertical.max.resolve(None),
            ));
            if matches!(self.sizes.min_width, PreferredSize::Auto)
                && !self.style.overflow_x().is_scrollable()
            {
                let intrinsic_min = constraints
                    .intrinsic
                    .unwrap_or_else(|| {
                        box_content_intrinsic_widths(engine, self.request.box_idx, None)
                    })
                    .0;
                width = width.max(intrinsic_min);
            }
            return (self.constrain_width(width, constraints), true);
        }
        let width = self.resolve_used_content_width(vertical, constraints, replaced_size);
        let transferred = constrain_size(
            width,
            vertical.min.resolve(None).map(transfer_height),
            vertical.max.resolve(None).map(transfer_height),
        );
        let content = self.constrain_width(transferred, constraints);
        (content, content != width)
    }

    fn constrain_width(&self, width: f64, constraints: ResolvedWidthConstraints) -> f64 {
        width
            .clamp(constraints.content_limits.0, constraints.content_limits.1)
            .max(0.0)
    }

    fn resolve_descendant_basis(
        &self,
        mut vertical: ResolvedVerticalSizing,
        horizontal: ResolvedHorizontalSizing,
    ) -> ResolvedVerticalSizing {
        // Probe requests and content-dependent widths cannot supply this basis.
        if self.request.is_intrinsic_block_measurement()
            || self.request.suppresses_authored_height_basis()
            || horizontal.shrink_to_fit
            || self.sizes.width.is_intrinsic_keyword()
        {
            return vertical;
        }
        if let (None, None, PreferredSize::Auto, Some(ratio)) = (
            self.image_intrinsic,
            vertical.explicit,
            self.sizes.height,
            self.aspect_ratio,
        ) {
            vertical.descendant_basis = Some(constrain_size(
                self.box_model
                    .ratio_height(horizontal.content, ratio, self.style.box_sizing()),
                vertical.min.resolve(None),
                vertical.max.resolve(None),
            ));
        }
        vertical
    }

    fn width_source(&self, engine: &LayoutEngine<'_, '_>, preferred: PreferredSize) -> WidthSource {
        if let Some(width) = self
            .request
            .assigned_border_size
            .and_then(|assigned| assigned.width)
        {
            return WidthSource::Assigned(width);
        }
        if self.image_intrinsic.is_some() {
            return WidthSource::Replaced;
        }
        if !matches!(preferred, PreferredSize::Auto) {
            return WidthSource::Style;
        }
        let shrink_to_fit = if self.is_float {
            true
        } else if engine.read.reader.box_facts(self.request.box_idx).absolute {
            !(self.style.inset_left().is_some() && self.style.inset_right().is_some())
        } else {
            engine.read.reader.is_block_box(self.request.box_idx)
                && !matches!(
                    self.inline_alignment,
                    ItemAlignment::Auto | ItemAlignment::Normal | ItemAlignment::Stretch
                )
        };
        if shrink_to_fit {
            WidthSource::ShrinkToFit
        } else {
            WidthSource::Style
        }
    }

    fn resolve_vertical_sizing(&self, engine: &LayoutEngine<'_, '_>) -> ResolvedVerticalSizing {
        let padding_border = self.box_model.vertical_padding_border();
        let border_box_inset = match self.style.box_sizing() {
            BoxSizing::ContentBox => 0.0,
            BoxSizing::BorderBox => padding_border,
        };
        let stretch_inset = crate::block::stretch_margin_inset(
            &engine.read.reader,
            self.request.box_idx,
            self.request.available_width,
        ) + padding_border;
        let resolve = |size: PreferredSize, minimum| {
            if self.request.is_intrinsic_block_measurement() {
                return HeightConstraint::Unconstrained;
            }
            if size.is_intrinsic_keyword() {
                return HeightConstraint::Content;
            }
            let resolve_value = if minimum {
                resolve_vertical_min_size_with_stretch_inset
            } else {
                resolve_definite_content_size_with_insets
            };
            resolve_value(
                size,
                self.request.parent_content_height,
                border_box_inset,
                stretch_inset,
            )
            .map_or(HeightConstraint::Unconstrained, HeightConstraint::Definite)
        };
        let authored_explicit = resolve(self.sizes.height, false).resolve(None);
        let min = resolve(self.sizes.min_height, true);
        let max = match resolve(self.sizes.max_height, false) {
            HeightConstraint::Definite(maximum) => {
                HeightConstraint::Definite(maximum.max(min.resolve(None).unwrap_or(0.0)))
            }
            constraint => constraint,
        };
        let assigned_content = self
            .request
            .assigned_border_size
            .and_then(|size| size.height.map(|height| (height - padding_border).max(0.0)));
        let explicit = assigned_content.or(authored_explicit);
        // A preferred aspect ratio can turn an automatic block size into a
        // transferred size. Its automatic minimum remains content-based unless
        // this axis is a scroll container; `clip` deliberately is not scrollable.
        let ratio_auto_min_uses_content = matches!(self.sizes.height, PreferredSize::Auto)
            && matches!(self.sizes.min_height, PreferredSize::Auto)
            && !self.style.overflow_y().is_scrollable();
        let descendant_basis = match (self.request.assigned_border_size, assigned_content) {
            (Some(assigned), Some(height)) if assigned.height_is_definite => Some(height),
            _ if self.request.suppresses_authored_height_basis() => None,
            _ => authored_explicit
                .map(|height| constrain_size(height, min.resolve(None), max.resolve(None))),
        };
        ResolvedVerticalSizing {
            authored_explicit,
            explicit,
            descendant_basis,
            min,
            max,
            ratio_auto_min_uses_content,
        }
    }

    fn resolve_horizontal_margins(
        &self,
        engine: &LayoutEngine<'_, '_>,
        horizontal: ResolvedHorizontalSizing,
        replaced_size: Option<Size>,
        fills_available_width: bool,
    ) -> ResolvedHorizontalMargins {
        if horizontal.suppresses_margins {
            return ResolvedHorizontalMargins {
                left: 0.0,
                total: 0.0,
            };
        }

        let authored_left = f64::from(self.box_model.margin_left);
        let border_width = resolved_border_width(
            self.box_model,
            horizontal,
            replaced_size,
            fills_available_width,
        );
        let left = if self.is_float {
            authored_left
        } else if let Some(border_width) = border_width {
            if !horizontal.ratio_transferred_width
                && matches!(self.sizes.width, PreferredSize::Auto)
            {
                if self.style.margin_left_auto() {
                    0.0
                } else {
                    authored_left
                }
            } else {
                resolve_margin_left_from_edges(
                    authored_left,
                    f64::from(self.box_model.margin_right),
                    self.style.margin_left_auto(),
                    self.style.margin_right_auto(),
                    engine
                        .read
                        .reader
                        .parent_direction(self.request.box_idx)
                        .unwrap_or(TextDirection::Ltr),
                    self.request.available_width,
                    border_width,
                )
            }
        } else {
            authored_left
        };
        ResolvedHorizontalMargins {
            left,
            total: self.box_model.horizontal_margin(),
        }
    }

    fn resolve_width_constraints(
        &self,
        engine: &mut LayoutEngine<'_, '_>,
        vertical: ResolvedVerticalSizing,
    ) -> ResolvedWidthConstraints {
        let [preferred, min, max] = if self.policy.ignores_inline_sizes {
            [PreferredSize::Auto; 3]
        } else {
            [self.sizes.width, self.sizes.min_width, self.sizes.max_width]
        };
        let source = self.width_source(engine, preferred);
        let intrinsic_widths =
            self.resolve_intrinsic_widths(engine, vertical, source, [preferred, min, max]);
        // Table-cell margins compute normally but do not participate in its used
        // table geometry.
        let horizontal_margin = if self.policy.ignores_inline_sizes {
            0.0
        } else {
            self.box_model.horizontal_margin()
        };
        let margin_padding = horizontal_margin + self.box_model.horizontal_padding();
        let horizontal_padding_border = self.box_model.horizontal_padding_border();
        let smart_width = self.image_intrinsic.and_then(|intrinsic| {
            engine.smart_standalone_image_width(
                self.request.box_idx,
                intrinsic.size,
                self.request.available_width,
                horizontal_padding_border,
                self.box_model.vertical_margin()
                    + self.box_model.vertical_padding()
                    + self.box_model.vertical_border(),
                !matches!(source, WidthSource::Assigned(_))
                    && engine.read.reader.is_only_block_child(self.request.box_idx),
            )
        });
        let border_box_inset = match self.style.box_sizing() {
            BoxSizing::ContentBox => 0.0,
            BoxSizing::BorderBox => horizontal_padding_border,
        };
        let auto_width_limit = self
            .request
            .auto_width_limit
            .unwrap_or(self.request.available_width);
        let available_content =
            (auto_width_limit - margin_padding - self.box_model.horizontal_border()).max(0.0);
        let parent_owns_item_sizing = engine
            .read
            .reader
            .box_facts(self.request.box_idx)
            .flex_grid_item;
        let keyword_content = |size| match size {
            PreferredSize::Stretch if !parent_owns_item_sizing => Some(available_content),
            PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent
                if self.image_intrinsic.is_none() =>
            {
                let (minimum, maximum) = intrinsic_widths
                    .expect("intrinsic width keywords require measured intrinsic widths");
                Some(match size {
                    PreferredSize::MinContent => minimum,
                    PreferredSize::MaxContent => maximum,
                    _ => maximum.min(available_content.max(minimum)),
                })
            }
            _ => None,
        };
        let bound = |size, auto| {
            keyword_content(size).unwrap_or_else(|| {
                resolve_preferred_content_width(
                    size,
                    auto,
                    self.request.available_width,
                    border_box_inset,
                )
            })
        };
        let minimum = bound(min, 0.0);
        ResolvedWidthConstraints {
            content_widths: [
                smart_width.or_else(|| keyword_content(preferred)),
                keyword_content(min),
                keyword_content(max),
            ],
            content_limits: (minimum, bound(max, f64::INFINITY).max(minimum)),
            preferred,
            min,
            max,
            horizontal_margin,
            margin_padding,
            border_box_inset,
            available_content,
            smart_width: smart_width.is_some(),
            intrinsic: intrinsic_widths,
            source,
        }
    }

    fn resolve_intrinsic_widths(
        &self,
        engine: &mut LayoutEngine<'_, '_>,
        vertical: ResolvedVerticalSizing,
        source: WidthSource,
        [preferred, min, max]: [PreferredSize; 3],
    ) -> Option<(f64, f64)> {
        let uses_intrinsic_keyword = self.image_intrinsic.is_none()
            && [preferred, min, max]
                .into_iter()
                .any(PreferredSize::is_intrinsic_keyword);
        let shrink_to_fit = matches!(source, WidthSource::ShrinkToFit);
        if !shrink_to_fit && !uses_intrinsic_keyword {
            return None;
        }
        let available_width = if shrink_to_fit {
            Some(
                (self
                    .request
                    .auto_width_limit
                    .unwrap_or(self.request.available_width)
                    - self.raw_horizontal_inset)
                    .max(0.0),
            )
        } else if self.sizes.max_width.percentage_dependent()
            || (self.style.box_sizing() == BoxSizing::BorderBox
                && (self.style.padding_left().has_percentage()
                    || self.style.padding_right().has_percentage()))
        {
            Some(self.request.available_width)
        } else {
            None
        };
        let mut measured = Some(box_content_intrinsic_widths(
            engine,
            self.request.box_idx,
            available_width,
        ));
        // A definite float height makes a percentage-height replaced child
        // definite while the float's shrink-to-fit width is being measured. Its
        // intrinsic aspect ratio therefore contributes the transferred width,
        // rather than the child's unscaled intrinsic width. The helper disables
        // this contribution in book mode.
        if shrink_to_fit
            && let Some(transferred_width) = vertical.authored_explicit.and_then(|height| {
                crate::measurement::percentage_height_image_width(
                    engine,
                    self.request.box_idx,
                    height,
                )
            })
        {
            measured = Some((transferred_width, transferred_width));
        }
        measured
    }

    fn resolve_replaced_size(
        &self,
        engine: &LayoutEngine<'_, '_>,
        constraints: ResolvedWidthConstraints,
    ) -> Option<Size> {
        let intrinsic = self.image_intrinsic?;
        if self.request.assigned_border_size.is_some() {
            return None;
        }
        let mut input = ReplacedSizeInput::from_style(
            *self.style,
            intrinsic.size,
            self.aspect_ratio,
            self.request.available_width,
            self.request.parent_content_height,
        )
        .with_box_model(
            constraints.horizontal_margin,
            self.box_model.vertical_margin(),
            self.box_model.horizontal_padding_border(),
            self.box_model.vertical_padding_border(),
        )
        .with_width_constraints(constraints.preferred, constraints.min, constraints.max);
        // Image fitting uses the raw outer margin; replaced sizing uses the
        // table-adjusted margin. Smart standalone sizing suppresses both.
        let mut content_widths = constraints.content_widths;
        let fitting_margin = if constraints.smart_width {
            content_widths[1..].fill(None);
            input.horizontal.margin = 0.0;
            input.horizontal.min = PreferredSize::Auto;
            input.horizontal.max = PreferredSize::Auto;
            0.0
        } else {
            self.box_model.horizontal_margin()
        };
        Some(engine.fit_image(
            self.request.box_idx,
            resolve_replaced_content_size(input, content_widths),
            self.request.available_width,
            Size::new(
                fitting_margin + self.box_model.horizontal_padding_border(),
                self.box_model.vertical_margin() + self.box_model.vertical_padding_border(),
            ),
        ))
    }

    fn resolve_used_content_width(
        &self,
        vertical: ResolvedVerticalSizing,
        constraints: ResolvedWidthConstraints,
        replaced_size: Option<Size>,
    ) -> f64 {
        match constraints.source {
            WidthSource::ShrinkToFit => {
                let (min, max) = constraints
                    .intrinsic
                    .expect("shrink-to-fit width requires measured intrinsic widths");
                return self
                    .constrain_width(max.min(constraints.available_content.max(min)), constraints);
            }
            WidthSource::Assigned(width) => {
                return (width - self.box_model.horizontal_padding_border()).max(0.0);
            }
            _ => {}
        }
        if let Some(size) = replaced_size {
            return size.width;
        }
        let replaced_auto_width = self
            .image_intrinsic
            .map(|image| image.size)
            .map(|intrinsic| {
                vertical
                    .authored_explicit
                    .and_then(|height| {
                        (intrinsic.height > 0.0)
                            .then_some(height * intrinsic.width / intrinsic.height)
                    })
                    .unwrap_or(intrinsic.width)
                    .min(constraints.available_content)
            });
        self.constrain_width(
            constraints.content_widths[0].unwrap_or_else(|| {
                resolve_preferred_content_width(
                    constraints.preferred,
                    replaced_auto_width.unwrap_or(constraints.available_content),
                    self.request.available_width,
                    constraints.border_box_inset,
                )
            }),
            constraints,
        )
    }
}

fn effective_inline_alignment(engine: &LayoutEngine<'_, '_>, box_idx: usize) -> ItemAlignment {
    let alignment = engine.read.reader.layout_style(box_idx).justify_self;
    let alignment = if alignment == ItemAlignment::Auto {
        engine
            .read
            .reader
            .get_parent(box_idx)
            .map(|parent| engine.read.reader.layout_style(parent).justify_items)
            .unwrap_or(ItemAlignment::Normal)
    } else {
        alignment
    };
    if alignment == ItemAlignment::Auto {
        ItemAlignment::Normal
    } else {
        alignment
    }
}

fn resolved_border_width(
    box_model: ResolvedBoxModel,
    horizontal: ResolvedHorizontalSizing,
    replaced_size: Option<Size>,
    fills_available_width: bool,
) -> Option<f64> {
    let content_width = match (replaced_size, horizontal.auto_width) {
        (Some(size), _) => size.width,
        (None, true) if fills_available_width => horizontal.content,
        (None, true) => return None,
        (None, _) => horizontal.content,
    };
    Some(content_width + box_model.horizontal_padding() + box_model.horizontal_border())
}

fn resolve_box_model(
    engine: &LayoutEngine<'_, '_>,
    request: BoxLayoutRequest,
    policy: SizingPolicy,
    mut box_model: ResolvedBoxModel,
) -> (ResolvedBoxModel, f64) {
    let (top, bottom) = engine.read.reader.adjust_vertical_margins(
        request.box_idx,
        f64::from(box_model.margin_top),
        f64::from(box_model.margin_bottom),
    );
    box_model.margin_top = top as f32;
    box_model.margin_bottom = bottom as f32;
    box_model = policy.adjust_box_model(box_model);
    if let Some(borders) = request.used_borders() {
        box_model = box_model.with_used_borders(borders);
    }
    (box_model, bottom)
}

/// Resolve normal-flow block alignment after width calculation. Alignment is
/// applied to the margin box, and automatic margins continue to take
/// precedence over `justify-self`.
pub(crate) fn block_inline_alignment_offset(
    engine: &LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
) -> f64 {
    let request = resolved.request;
    let style = engine.read.reader.style(request.box_idx);
    if !engine.read.reader.is_block_box(request.box_idx)
        || resolved.is_float
        || engine.read.reader.box_facts(request.box_idx).absolute
    {
        return resolved.horizontal_margins.left;
    }

    let border_width = resolved
        .replaced_size
        .map_or(resolved.horizontal.content, |size| size.width)
        + resolved.box_model.horizontal_padding()
        + resolved.box_model.horizontal_border();
    let left = style.margin_left().resolve(request.available_width);
    let right = style.margin_right().resolve(request.available_width);
    let left_auto = style.margin_left_auto();
    let right_auto = style.margin_right_auto();
    if left_auto || right_auto {
        return resolve_auto_start_margin(
            (left, right),
            (left_auto, right_auto),
            request.available_width - border_width,
            AutoMarginOverflow::Preserve,
        );
    }

    let free_space = request.available_width - border_width - left - right;
    let containing_direction = engine
        .read
        .reader
        .parent_direction(request.box_idx)
        .unwrap_or(TextDirection::Ltr);
    let subject_direction = style.direction();
    let start_offset = |direction| {
        if direction == TextDirection::Rtl {
            free_space
        } else {
            0.0
        }
    };
    let end_offset = |direction| {
        if direction == TextDirection::Rtl {
            0.0
        } else {
            free_space
        }
    };
    let alignment_offset = match resolved.inline_alignment {
        ItemAlignment::Start | ItemAlignment::FlexStart => start_offset(containing_direction),
        ItemAlignment::End | ItemAlignment::FlexEnd => end_offset(containing_direction),
        ItemAlignment::SelfStart | ItemAlignment::Baseline => start_offset(subject_direction),
        ItemAlignment::SelfEnd => end_offset(subject_direction),
        ItemAlignment::Left | ItemAlignment::LegacyLeft => 0.0,
        ItemAlignment::Right | ItemAlignment::LegacyRight => free_space,
        ItemAlignment::Center | ItemAlignment::LegacyCenter => free_space / 2.0,
        ItemAlignment::Auto | ItemAlignment::Normal | ItemAlignment::Stretch => {
            return resolved.horizontal_margins.left;
        }
    };
    left + alignment_offset
}
