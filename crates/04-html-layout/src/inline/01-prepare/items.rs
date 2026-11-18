//! Construction of inline tokens and measurement of atomic inline content.

use super::{
    BoxSizing, BreakKind, InlineItem, InlineItemKind, InlineToken, InlineTokenKind,
    InlineTokenMetrics, InlineTokens, InlineVerticalAlign, Point, PreferredSize, ReplacedSizeInput,
    ReplacedToken, Size, TextDirection, Vec2, VerticalAlignValue, break_kind,
    preferred_aspect_ratio, resolve_replaced_content_size, resolve_used_preferred_size,
};
use crate::measurement::measure_box_isolated;
use crate::{BoxLayoutRequest, LayoutEngine, ResolvedBoxModel};
use html_dom::ImageSource;
use html_style_model::{PositionMode, UsedStyleView, Visibility};

/// Append one non-text item and report whether it participates in atomic wrapping.
pub(super) fn append_non_text_item(
    engine: &mut LayoutEngine<'_, '_>,
    run: &InlineItem,
    tokens: &mut InlineTokens<'_>,
    container_box_idx: usize,
    max_width: f64,
    containing_block_height: Option<f64>,
    standalone_image: Option<u32>,
) -> bool {
    let style = engine.read.reader.style(run.box_idx as usize);
    let vertical_align = style.vertical_align();
    let mut has_atomic = false;
    match &run.kind {
        InlineItemKind::Image { image_idx } => {
            has_atomic = true;
            // A blockified replaced element is represented by a self-owned
            // image run so it can reuse the image fragment pipeline. It is
            // content of its own border box, not a baseline-aligned child.
            // Blockified images reuse an inline item only as an internal
            // fragment carrier. The image is content of its own border
            // box, so baseline alignment must never shift or clip it.
            // The absolute box's hypothetical static position is a
            // separate positioning decision made by the parent flow.
            let (token, metrics, payload) = create_image_token(
                engine,
                *image_idx,
                run.box_idx,
                container_box_idx,
                max_width,
                containing_block_height,
                standalone_image,
            );
            tokens.push_replaced(
                payload,
                |payload_idx| InlineTokenKind::Image { payload_idx },
                token,
                metrics,
                run.box_idx as usize != container_box_idx,
            );
        }
        InlineItemKind::AtomicBox { box_idx } => {
            has_atomic = true;
            let (token, metrics, payload) = create_atomic_box_token(
                engine,
                *box_idx,
                style,
                vertical_align,
                max_width,
                containing_block_height,
            );
            tokens.push_replaced(
                payload,
                |payload_idx| InlineTokenKind::AtomicBox { payload_idx },
                token,
                metrics,
                true,
            );
        }
        InlineItemKind::Break { clear } => {
            let (mut token, metrics) = create_control_token(
                run.box_idx,
                InlineTokenKind::Break { clear: *clear },
                style,
                vertical_align,
            );
            token.mark_authored_break();
            tokens.push(token, metrics);
        }
        InlineItemKind::FloatAnchor { box_idx } => {
            let (token, metrics) = create_control_token(
                *box_idx,
                InlineTokenKind::FloatAnchor { box_idx: *box_idx },
                style,
                vertical_align,
            );
            tokens.push(token, metrics);
        }
        InlineItemKind::AbsoluteAnchor { box_idx } => {
            let (token, metrics) = create_control_token(
                *box_idx,
                InlineTokenKind::AbsoluteAnchor { box_idx: *box_idx },
                style,
                vertical_align,
            );
            tokens.push(token, metrics);
        }
        InlineItemKind::InlineBoundary {
            inline_start,
            inline_end,
        } => {
            let (token, metrics, payload) = create_inline_boundary_token(
                engine,
                run.box_idx,
                *inline_start,
                *inline_end,
                style,
                vertical_align,
                max_width,
            );
            tokens.push_replaced(
                payload,
                |payload_idx| InlineTokenKind::InlineBoundary { payload_idx },
                token,
                metrics,
                false,
            );
        }

        InlineItemKind::Text { .. } | InlineItemKind::Marker { .. } => {
            unreachable!("text items are prepared by stage 01");
        }
    }
    has_atomic
}

/// Identifies an image that is the sole meaningful inline content of its
/// formatting context. Whitespace and transparent inline wrappers are
/// harmless; text, another image, a break, a float, or an atomic inline
/// box makes the context ineligible.
pub(super) fn standalone_inline_image(
    engine: &LayoutEngine<'_, '_>,
    runs: &[InlineItem],
    container_box_idx: usize,
) -> Option<u32> {
    let mut candidate = None;
    for run in runs {
        if !engine
            .read
            .inline_item_belongs_to_context(run, container_box_idx)
        {
            continue;
        }
        match &run.kind {
            InlineItemKind::Image { .. } if candidate.is_none() => candidate = Some(run.box_idx),
            InlineItemKind::Text { glyphs } => {
                let has_text = glyphs.clone().any(|glyph_idx| {
                    !html_text::is_document_white_space(engine.read.text.character(glyph_idx))
                });
                if has_text {
                    return None;
                }
            }
            InlineItemKind::Image { .. }
            | InlineItemKind::Marker { .. }
            | InlineItemKind::Break { .. }
            | InlineItemKind::FloatAnchor { .. }
            | InlineItemKind::AbsoluteAnchor { .. }
            | InlineItemKind::AtomicBox { .. }
            | InlineItemKind::InlineBoundary { .. } => {
                return None;
            }
        }
    }
    candidate
}

fn box_token_metrics(
    box_idx: u32,
    style: UsedStyleView,
    vertical_align: VerticalAlignValue,
    line_height: f64,
    (ascent, descent): (f64, f64),
) -> InlineTokenMetrics {
    let font_size = style.font_size();
    InlineTokenMetrics {
        owner_box_idx: box_idx,
        ascent: ascent as f32,
        descent: descent as f32,
        line_height: line_height as f32,
        font_size,
        vertical_align: InlineVerticalAlign::resolve(vertical_align, line_height, font_size),
        white_space: style.white_space(),
        contributes_line_metrics: true,
        visible: style.visibility() == Visibility::Visible,
        placement_required: false,
    }
}

fn create_image_token(
    engine: &LayoutEngine<'_, '_>,
    image_idx: u32,
    box_idx: u32,
    container_box_idx: usize,
    max_width: f64,
    containing_block_height: Option<f64>,
    standalone_image: Option<u32>,
) -> (InlineToken, InlineTokenMetrics, ReplacedToken) {
    let style = engine.read.reader.style(box_idx as usize);
    let self_owned_replaced = box_idx as usize == container_box_idx;
    let vertical_align = if self_owned_replaced {
        VerticalAlignValue::Top
    } else {
        style.vertical_align()
    };
    let max_width = max_width.max(0.0);
    let image_intrinsic = engine
        .read
        .reader
        .image_intrinsic(box_idx as usize)
        .expect("image token owner must retain image identity in the layout topology");
    let intrinsic = image_intrinsic.size;
    let make_token = |width, extents, replaced| {
        let line_height = style.resolved_line_height();
        (
            InlineToken::new(
                InlineTokenKind::Image {
                    payload_idx: InlineToken::MAX_DATA,
                },
                width,
                BreakKind::None,
                false,
                true,
            ),
            box_token_metrics(box_idx, style, vertical_align, line_height, extents),
            replaced,
        )
    };
    if self_owned_replaced {
        let content_height = containing_block_height.unwrap_or_else(|| {
            if intrinsic.width > 0.0 {
                max_width * intrinsic.height / intrinsic.width
            } else {
                intrinsic.height
            }
        });
        let content_size = Size::new(max_width, content_height.max(0.0));
        return make_token(
            content_size.width,
            (content_size.height, 0.0),
            ReplacedToken::Image {
                image_idx,
                box_idx,
                content_size,
                border_size: content_size,
                content_inset: Point::ZERO,
                margin_left: 0.0,
                margin_top: 0.0,
                position_offset: Vec2::ZERO,
                set_box_geometry: false,
            },
        );
    }
    let source = &engine
        .read
        .reader
        .document()
        .image(image_idx)
        .expect("registered image")
        .source;
    let smart_standalone = !matches!(source, ImageSource::MathMl { .. })
        && standalone_image == Some(box_idx)
        && style.position() != PositionMode::Absolute;
    let margin_top = style.margin_top().resolve(max_width);
    let margin_bottom = style.margin_bottom().resolve(max_width);
    let padding_left = style.padding_left().resolve(max_width);
    let padding_top = style.padding_top().resolve(max_width);
    let horizontal_padding = style.get_horizontal_padding(max_width);
    let vertical_padding = style.get_vertical_padding(max_width);
    let border_left = style.border_left_width() as f64;
    let border_top = style.border_top_width() as f64;
    let horizontal_border = border_left + style.border_right_width() as f64;
    let vertical_border = border_top + style.border_bottom_width() as f64;
    let aspect_ratio = preferred_aspect_ratio(style.aspect_ratio(), image_intrinsic.aspect_ratio);
    let smart_width = engine.smart_standalone_image_width(
        box_idx as usize,
        intrinsic,
        max_width,
        horizontal_padding + horizontal_border,
        margin_top + margin_bottom + vertical_padding + vertical_border,
        smart_standalone,
    );
    let (margin_left, margin_right) = if smart_width.is_some() {
        (0.0, 0.0)
    } else {
        (
            style.margin_left().resolve(max_width),
            style.margin_right().resolve(max_width),
        )
    };
    let content_size = resolve_replaced_content_size(
        ReplacedSizeInput::from_style(
            style,
            intrinsic,
            aspect_ratio,
            max_width,
            containing_block_height,
        )
        .with_box_model(
            margin_left + margin_right,
            margin_top + margin_bottom,
            horizontal_padding + horizontal_border,
            vertical_padding + vertical_border,
        )
        .with_width_constraints(
            style.width(),
            if smart_width.is_some() {
                PreferredSize::Auto
            } else {
                style.min_width()
            },
            if smart_width.is_some() {
                PreferredSize::Auto
            } else {
                style.max_width()
            },
        ),
        [smart_width, None, None],
    );
    let content_size = engine.fit_image(
        box_idx as usize,
        content_size,
        max_width,
        Size::new(
            margin_left + margin_right + horizontal_padding + horizontal_border,
            margin_top + margin_bottom + vertical_padding + vertical_border,
        ),
    );
    let border_size = Size::new(
        content_size.width + horizontal_padding + horizontal_border,
        content_size.height + vertical_padding + vertical_border,
    );
    let outer_width = (border_size.width + margin_left + margin_right).max(0.0);
    let outer_height = (border_size.height + margin_top + margin_bottom).max(0.0);
    let math_descent = match source {
        ImageSource::MathMl { baseline, .. } => {
            content_size.height * f64::from(1.0 - baseline) + margin_bottom + vertical_padding
                - padding_top
                + style.border_bottom_width() as f64
        }
        _ => 0.0,
    };
    let position_offset = if style.position() == PositionMode::Relative {
        crate::boxes::relative_position_offset(&style, max_width, containing_block_height)
    } else {
        Vec2::ZERO
    };
    make_token(
        outer_width,
        (outer_height - math_descent, math_descent),
        ReplacedToken::Image {
            image_idx,
            box_idx,
            content_size,
            border_size,
            content_inset: Point::new(padding_left + border_left, padding_top + border_top),
            margin_left,
            margin_top,
            position_offset,
            set_box_geometry: true,
        },
    )
}

fn create_control_token(
    box_idx: u32,
    kind: InlineTokenKind,
    style: UsedStyleView,
    vertical_align: VerticalAlignValue,
) -> (InlineToken, InlineTokenMetrics) {
    let line_height = style.resolved_line_height();
    let break_kind = if matches!(kind, InlineTokenKind::Break { .. }) {
        BreakKind::Hard
    } else {
        BreakKind::None
    };
    (
        InlineToken::new(kind, 0.0, break_kind, false, true),
        box_token_metrics(box_idx, style, vertical_align, line_height, (0.0, 0.0)),
    )
}

fn create_inline_boundary_token(
    engine: &LayoutEngine<'_, '_>,
    box_idx: u32,
    inline_start: bool,
    inline_end: bool,
    style: UsedStyleView,
    vertical_align: VerticalAlignValue,
    max_width: f64,
) -> (InlineToken, InlineTokenMetrics, ReplacedToken) {
    let parent_direction = engine
        .read
        .reader
        .get_parent(box_idx as usize)
        .map_or(style.direction(), |parent| {
            engine.read.reader.style(parent).direction()
        });
    let (inline_start, inline_end) = if parent_direction == TextDirection::Rtl {
        (inline_end, inline_start)
    } else {
        (inline_start, inline_end)
    };
    let margin_left = if inline_start {
        style.margin_left().resolve(max_width)
    } else {
        0.0
    };
    let margin_right = if inline_end {
        style.margin_right().resolve(max_width)
    } else {
        0.0
    };
    let left = if inline_start {
        style.padding_left().resolve(max_width) + style.border_left_width() as f64
    } else {
        0.0
    };
    let right = if inline_end {
        style.padding_right().resolve(max_width) + style.border_right_width() as f64
    } else {
        0.0
    };
    let line_height = style.resolved_line_height();
    let font_size = style.font_size() as f64;
    // A boundary is the zero-content strut of its inline box. Its baseline
    // therefore uses the selected font's ascent/descent plus half-leading,
    // exactly like text owned by that box. Basing edge boundaries on a fixed
    // 80/20 split made empty inlines disagree with otherwise identical text.
    let (font_ascent, font_descent) =
        (engine.read.reader.font_metrics(box_idx as usize)).line_box_extents(font_size);
    let half_leading = (line_height - font_ascent - font_descent) * 0.5;
    let (ascent, descent) = (font_ascent + half_leading, font_descent + half_leading);
    let token = InlineToken::new(
        InlineTokenKind::InlineBoundary {
            payload_idx: InlineToken::MAX_DATA,
        },
        margin_left + left + right + margin_right,
        BreakKind::None,
        false,
        true,
    );
    let metrics = box_token_metrics(
        box_idx,
        style,
        vertical_align,
        line_height,
        (ascent, descent),
    );
    (
        token,
        metrics,
        ReplacedToken::InlineBoundary {
            box_idx,
            margin_left,
            left_inset: left,
            inline_start,
            inline_end,
        },
    )
}

fn create_atomic_box_token(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: u32,
    style: UsedStyleView,
    vertical_align: VerticalAlignValue,
    max_width: f64,
    containing_block_height: Option<f64>,
) -> (InlineToken, InlineTokenMetrics, ReplacedToken) {
    let max_width = max_width.max(0.0);
    let model = ResolvedBoxModel::new(style, max_width).with_nonnegative_padding();
    // Anonymous inline tables carry an anonymous reset style whose computed
    // `display` is not `inline-table`; participation in this atomic inline
    // context is represented authoritatively by the layout mode instead.
    let is_inline_table = engine.read.reader.is_table_box(box_idx as usize);
    let (border_size, first_baseline, last_baseline) = if let Some(intrinsic) =
        engine.read.reader.image_intrinsic(box_idx as usize)
    {
        let content = resolve_replaced_content_size(
            ReplacedSizeInput::from_style(
                style,
                intrinsic.size,
                preferred_aspect_ratio(style.aspect_ratio(), intrinsic.aspect_ratio),
                max_width,
                containing_block_height,
            )
            .with_box_model(
                model.horizontal_margin(),
                model.vertical_margin(),
                model.horizontal_padding_border(),
                model.vertical_padding_border(),
            ),
            [None; 3],
        );
        let content = engine.fit_image(
            box_idx as usize,
            content,
            max_width,
            Size::new(
                model.horizontal_margin() + model.horizontal_padding_border(),
                model.vertical_margin() + model.vertical_padding_border(),
            ),
        );
        (
            Size::new(
                content.width + model.horizontal_padding_border(),
                content.height + model.vertical_padding_border(),
            ),
            None,
            None,
        )
    } else if is_inline_table
        && matches!(
            style.border_collapse(),
            html_style_model::BorderCollapseMode::Collapse
        )
    {
        // The collapsed edge grid, not the authored full borders, determines
        // the inline table's used border box. Let normal table layout resolve
        // that grid instead of duplicating box sizing here.
        let measured = measure_box_isolated(
            engine,
            BoxLayoutRequest::normal(
                box_idx as usize,
                max_width.max(0.0),
                containing_block_height,
            ),
        );
        (
            measured.size,
            measured.first_baseline,
            measured.last_baseline,
        )
    } else {
        // Atomic inline sizing needs the raw content contributions here. The
        // outer intrinsic helper already applies the box's preferred width;
        // using it for `fit-content` collapses min-content to max-content and
        // prevents the available inline size from clamping between them.
        let (intrinsic_min, intrinsic_max) =
            crate::measurement::box_content_intrinsic_widths(engine, box_idx as usize, None);
        let min_border_width = (intrinsic_min + model.horizontal_padding_border()).max(0.0);
        let max_border_width =
            (intrinsic_max + model.horizontal_padding_border()).max(min_border_width);
        let border_inset = model.horizontal_padding_border();
        let available_border_width = (max_width - model.horizontal_margin()).max(border_inset);
        let shrink_to_fit = max_border_width.min(available_border_width.max(min_border_width));
        let resolve_border_width = |size: PreferredSize, auto: f64| match size {
            PreferredSize::Auto => auto,
            PreferredSize::MinContent => min_border_width,
            PreferredSize::MaxContent => max_border_width,
            PreferredSize::FitContent => shrink_to_fit,
            PreferredSize::Stretch => available_border_width,
            _ => {
                let resolved = resolve_used_preferred_size(size, auto, max_width).max(0.0);
                match style.box_sizing() {
                    BoxSizing::ContentBox => resolved + border_inset,
                    BoxSizing::BorderBox => resolved.max(border_inset),
                }
            }
        };
        let ratio_transferred_width = if matches!(style.width(), PreferredSize::Auto) {
            (|| {
                let authored_ratio = style.aspect_ratio();
                let ratio = authored_ratio.preferred().map(f64::from)?;
                let vertical_border_inset = model.vertical_padding_border();
                let vertical_box_sizing_inset = matches!(style.box_sizing(), BoxSizing::BorderBox)
                    .then_some(vertical_border_inset)
                    .unwrap_or(0.0);
                let content_height = crate::resolve_vertical_size(
                    style.height(),
                    containing_block_height,
                    vertical_box_sizing_inset,
                )?;
                Some(match style.box_sizing() {
                    BoxSizing::ContentBox => content_height * ratio + border_inset,
                    BoxSizing::BorderBox => {
                        ((content_height + vertical_border_inset) * ratio).max(border_inset)
                    }
                })
            })()
        } else {
            None
        };
        let preferred = ratio_transferred_width
            .unwrap_or_else(|| resolve_border_width(style.width(), shrink_to_fit));
        let automatic_minimum =
            if ratio_transferred_width.is_some() && !style.overflow_x().is_scrollable() {
                min_border_width
            } else {
                border_inset
            };
        let minimum = resolve_border_width(style.min_width(), automatic_minimum);
        let maximum = resolve_border_width(style.max_width(), f64::INFINITY).max(minimum);
        let assigned_width = preferred.clamp(minimum, maximum);
        let measured = measure_box_isolated(
            engine,
            BoxLayoutRequest::width_assigned(
                box_idx as usize,
                max_width.max(0.0),
                assigned_width.max(0.0),
                containing_block_height,
            ),
        );
        (
            measured.size,
            measured.first_baseline,
            measured.last_baseline,
        )
    };
    let outer_width = (border_size.width + model.horizontal_margin()).max(0.0);
    let outer_height = (border_size.height + model.vertical_margin()).max(0.0);
    let is_inline_block = matches!(style.display(), html_style_model::Display::InlineBlock);
    let is_inline_flex_or_grid = engine.read.reader.is_flex_grid_box(box_idx as usize);
    let visible_overflow = !style.overflow_x().clips() && !style.overflow_y().clips();
    let propagated_baseline = if is_inline_table {
        // CSS exposes the first row's baseline for an inline-table. When
        // that row has no line boxes, its bottom edge is the fallback
        // baseline; treating the entire table as bottom-baseline incorrectly
        // adds the parent font strut below the table's margin box.
        first_baseline
    } else if is_inline_block && visible_overflow {
        last_baseline
    } else if is_inline_flex_or_grid {
        first_baseline
    } else {
        None
    };
    let (ascent, descent) = if let Some(baseline) = propagated_baseline {
        let ascent = f64::from(model.margin_top) + baseline;
        (ascent, (outer_height - ascent).max(0.0))
    } else {
        (outer_height, 0.0)
    };
    let line_height = if is_inline_table {
        outer_height
    } else {
        style.resolved_line_height()
    };
    (
        InlineToken::new(
            InlineTokenKind::AtomicBox {
                payload_idx: InlineToken::MAX_DATA,
            },
            outer_width,
            BreakKind::None,
            false,
            true,
        ),
        box_token_metrics(
            box_idx,
            style,
            vertical_align,
            line_height,
            (ascent, descent),
        ),
        ReplacedToken::AtomicBox {
            box_idx,
            border_size,
            containing_width: max_width,
            containing_height: containing_block_height,
            margin_left: f64::from(model.margin_left),
            margin_top: f64::from(model.margin_top),
        },
    )
}
