//! Construction of inline tokens and measurement of atomic inline content.

use super::*;

fn create_image_token(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    image_idx: u32,
    box_idx: u32,
    container_box_idx: usize,
    max_width: f64,
    containing_block_height: Option<f64>,
    standalone_image: Option<u32>,
) -> (InlineToken, InlineTokenMetrics, ReplacedToken) {
    let style = engine.reader.style(box_idx as usize);
    let self_owned_replaced = box_idx as usize == container_box_idx;
    let vertical_align = if self_owned_replaced {
        VerticalAlignValue::Top
    } else {
        effective_vertical_align(engine, box_idx as usize)
    };
    let smart_standalone = standalone_image == Some(box_idx)
        && style.position() != html_style_model::PositionMode::Absolute;
    let max_width = max_width.max(0.0);
    let authored_margin_left = style.margin_left().resolve(max_width);
    let authored_margin_right = style.margin_right().resolve(max_width);
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
    let image_intrinsic = engine
        .reader
        .image_intrinsic(box_idx as usize)
        .expect("image token owner must retain image identity in the layout topology");
    let intrinsic = image_intrinsic.size;
    if self_owned_replaced {
        let content_height = containing_block_height.unwrap_or_else(|| {
            if intrinsic.width > 0.0 {
                max_width * intrinsic.height / intrinsic.width
            } else {
                intrinsic.height
            }
        });
        let content_size = Size::new(max_width, content_height.max(0.0));
        let font_size = style.font_size();
        let line_height = resolved_line_height(style);
        return (
            InlineToken::new(
                InlineTokenKind::Image {
                    payload_idx: u32::MAX,
                },
                content_size.width,
                BreakKind::None,
                TokenWrap::Normal,
                true,
            ),
            InlineTokenMetrics {
                owner_box_idx: box_idx,
                ascent: content_size.height as f32,
                descent: 0.0,
                line_height,
                tab_interval: -1.0,
                tab_min_advance: 0.0,
                font_size,
                vertical_align,
                white_space: style.white_space(),
                visible: style.visibility() == html_style_model::Visibility::Visible,
                placement_required: false,
            },
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
    let aspect_ratio = preferred_aspect_ratio(style.aspect_ratio(), image_intrinsic.aspect_ratio);
    let smart_width = engine.reader.smart_standalone_image_width(
        engine.config.image_sizing_policy(),
        box_idx as usize,
        intrinsic,
        max_width,
        engine.config.viewport_height(),
        horizontal_padding + horizontal_border,
        margin_top + margin_bottom + vertical_padding + vertical_border,
        smart_standalone,
    );
    let (margin_left, margin_right) = if smart_width.is_some() {
        (0.0, 0.0)
    } else {
        (authored_margin_left, authored_margin_right)
    };
    let preferred_width = smart_width.unwrap_or_else(|| style.width());
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
            preferred_width,
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
    );
    let border_size = Size::new(
        content_size.width + horizontal_padding + horizontal_border,
        content_size.height + vertical_padding + vertical_border,
    );
    let outer_width = (border_size.width + margin_left + margin_right).max(0.0);
    let outer_height = (border_size.height + margin_top + margin_bottom).max(0.0);
    let font_size = style.font_size();
    let line_height = resolved_line_height(style);
    let position_offset = if style.position() == html_style_model::PositionMode::Relative {
        crate::layout::box_positioning::relative_position_offset(
            &style,
            max_width,
            containing_block_height,
        )
    } else {
        Vec2::ZERO
    };
    (
        InlineToken::new(
            InlineTokenKind::Image {
                payload_idx: u32::MAX,
            },
            outer_width,
            BreakKind::None,
            TokenWrap::Normal,
            true,
        ),
        InlineTokenMetrics {
            owner_box_idx: box_idx,
            ascent: outer_height as f32,
            descent: 0.0,
            line_height,
            tab_interval: -1.0,
            tab_min_advance: 0.0,
            font_size,
            vertical_align,
            white_space: style.white_space(),
            visible: style.visibility() == html_style_model::Visibility::Visible,
            placement_required: false,
        },
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

fn create_break_token(
    box_idx: u32,
    clear: html_style_model::Clear,
    style: html_style_model::UsedStyleView,
    vertical_align: VerticalAlignValue,
) -> (InlineToken, InlineTokenMetrics) {
    let line_height = resolved_line_height(style);
    (
        InlineToken::new(
            InlineTokenKind::Break { clear },
            0.0,
            BreakKind::Hard,
            TokenWrap::Normal,
            true,
        ),
        InlineTokenMetrics {
            owner_box_idx: box_idx,
            ascent: 0.0,
            descent: 0.0,
            line_height,
            tab_interval: -1.0,
            tab_min_advance: 0.0,
            font_size: style.font_size(),
            vertical_align,
            white_space: style.white_space(),
            visible: style.visibility() == html_style_model::Visibility::Visible,
            placement_required: false,
        },
    )
}

fn create_float_anchor_token(
    box_idx: u32,
    style: html_style_model::UsedStyleView,
    vertical_align: VerticalAlignValue,
) -> (InlineToken, InlineTokenMetrics) {
    let line_height = resolved_line_height(style);
    (
        InlineToken::new(
            InlineTokenKind::FloatAnchor { box_idx },
            0.0,
            BreakKind::None,
            TokenWrap::Normal,
            true,
        ),
        InlineTokenMetrics {
            owner_box_idx: box_idx,
            ascent: 0.0,
            descent: 0.0,
            line_height,
            tab_interval: -1.0,
            tab_min_advance: 0.0,
            font_size: style.font_size(),
            vertical_align,
            white_space: style.white_space(),
            visible: style.visibility() == html_style_model::Visibility::Visible,
            placement_required: false,
        },
    )
}

fn create_absolute_anchor_token(
    box_idx: u32,
    style: html_style_model::UsedStyleView,
    vertical_align: VerticalAlignValue,
) -> (InlineToken, InlineTokenMetrics) {
    let line_height = resolved_line_height(style);
    (
        InlineToken::new(
            InlineTokenKind::AbsoluteAnchor { box_idx },
            0.0,
            BreakKind::None,
            TokenWrap::Normal,
            true,
        ),
        InlineTokenMetrics {
            owner_box_idx: box_idx,
            ascent: 0.0,
            descent: 0.0,
            line_height,
            tab_interval: -1.0,
            tab_min_advance: 0.0,
            font_size: style.font_size(),
            vertical_align,
            white_space: style.white_space(),
            visible: style.visibility() == html_style_model::Visibility::Visible,
            placement_required: false,
        },
    )
}

fn create_inline_boundary_token(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    box_idx: u32,
    inline_start: bool,
    inline_end: bool,
    style: html_style_model::UsedStyleView,
    vertical_align: VerticalAlignValue,
    max_width: f64,
) -> (InlineToken, InlineTokenMetrics, ReplacedToken) {
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
    let line_height = resolved_line_height(style);
    let font_size = style.font_size() as f64;
    // A boundary is the zero-content strut of its inline box. Its baseline
    // therefore uses the selected font's ascent/descent plus half-leading,
    // exactly like text owned by that box. Basing edge boundaries on a fixed
    // 80/20 split made empty inlines disagree with otherwise identical text.
    let (font_ascent, font_descent) = engine
        .reader
        .font_metrics(box_idx as usize)
        .line_box_ratios()
        .map_or((font_size * 0.8, font_size * 0.2), |(ascent, descent)| {
            (font_size * ascent as f64, font_size * descent as f64)
        });
    let half_leading = (line_height - font_ascent - font_descent) * 0.5;
    let (ascent, descent) = (font_ascent + half_leading, font_descent + half_leading);
    let token = InlineToken::new(
        InlineTokenKind::InlineBoundary {
            payload_idx: u32::MAX,
        },
        margin_left + left + right + margin_right,
        BreakKind::None,
        TokenWrap::Normal,
        true,
    );
    let metrics = InlineTokenMetrics {
        owner_box_idx: box_idx,
        ascent: ascent as f32,
        descent: descent as f32,
        line_height,
        tab_interval: -1.0,
        tab_min_advance: 0.0,
        font_size: font_size as f32,
        vertical_align,
        white_space: style.white_space(),
        visible: style.visibility() == html_style_model::Visibility::Visible,
        placement_required: false,
    };
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

/// Converts inline items into layout tokens.
///
/// Each token carries the geometry and break metadata needed by the line
/// breaker plus the placement inputs needed later by line measurement.
pub(in crate::layout::inline) fn build_inline_tokens(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    run_range: Range<u32>,
    container_box_idx: usize,
    max_width: f64,
    containing_block_height: Option<f64>,
) -> InlineTokens {
    let timing_started = engine.start_timing();
    let plan_key = PreparedInlinePlanKey {
        run_start: run_range.start,
        run_end: run_range.end,
        container_box_idx: u32::try_from(container_box_idx)
            .expect("inline-plan box index exhausted"),
        high_quality_hyphenation: engine.config.hyphenation_quality(),
    };
    let source_runs = &engine.text.inline_items()[run_range.start as usize..run_range.end as usize];
    let cacheable = !engine.config.sentence_per_line()
        && source_runs.iter().all(|run| {
            matches!(
                run.kind,
                InlineItemKind::Text { .. } | InlineItemKind::Marker { .. }
            )
        });
    let glyph_count = source_runs
        .iter()
        .map(|run| match &run.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => glyphs.len(),
            _ => 0,
        })
        .sum::<usize>();
    if cacheable && let Some(tokens) = engine.inline_plans.get(plan_key) {
        engine.record_timing(|t| t.build_inline_tokens += timing_started.elapsed());
        return tokens;
    }

    // Atomic descendants allocate their own run ranges in the shared arena.
    // Clone this small range so token measurement can mutably invoke their
    // independent layout algorithms without aliasing the arena borrow.
    let runs =
        engine.text.inline_items()[run_range.start as usize..run_range.end as usize].to_vec();
    let standalone_image = standalone_inline_image(engine, &runs, container_box_idx);
    let mut tokens = build_inline_tokens_from_runs(
        engine,
        &runs,
        container_box_idx,
        max_width,
        containing_block_height,
        standalone_image,
    );
    if engine.config.sentence_per_line() {
        tokens.insert_sentence_breaks(engine);
        tokens.rebuild_summary_blocks();
    }
    if cacheable {
        if glyph_count >= INLINE_KP_CACHE_MIN_GLYPHS
            && (engine.config.force_justify()
                || engine.reader.style(container_box_idx).text_align() == TextAlign::Justify
                || (engine.config.book_optimized_text()
                    && engine.reader.style(container_box_idx).text_align() == TextAlign::Left))
        {
            tokens.enable_kp_plan_cache();
        }
        tokens.share();
        tokens = engine.inline_plans.insert(plan_key, tokens);
    }
    // Nested timing is measured in build_inline_tokens_from_runs; this captures the outer wrapper.
    engine.record_timing(|t| t.build_inline_tokens += timing_started.elapsed());
    tokens
}

pub(super) fn build_inline_tokens_from_runs(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    runs: &[InlineItem],
    container_box_idx: usize,
    max_width: f64,
    containing_block_height: Option<f64>,
    standalone_image: Option<u32>,
) -> InlineTokens {
    let timing_started = engine.start_timing();
    let glyph_capacity = runs
        .iter()
        .map(|run| match &run.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => {
                glyphs.end.saturating_sub(glyphs.start) as usize
            }
            _ => 1,
        })
        .sum();
    let mut tokens = InlineTokens::with_capacity(glyph_capacity);
    for run in runs {
        let ownership_box = inline_item_ownership_box(engine, run);
        if !ownership_box.is_some_and(|box_idx| {
            run_belongs_to_inline_context(engine, box_idx, container_box_idx)
        }) {
            continue;
        }
        let style = engine.reader.style(run.box_idx as usize);
        let white_space = style.white_space();
        let vertical_align = effective_vertical_align(engine, run.box_idx as usize);
        match &run.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => {
                let vertical_align = text_vertical_align(engine, run.box_idx as usize);
                append_text_tokens(
                    engine,
                    &mut tokens,
                    glyphs.clone(),
                    run.box_idx as usize,
                    style,
                    white_space,
                    vertical_align,
                );
            }
            InlineItemKind::Image { image_idx } => {
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
                if white_space.allows_wrap() && run.box_idx as usize != container_box_idx {
                    tokens.push(
                        InlineToken::new(
                            InlineTokenKind::Opportunity,
                            0.0,
                            BreakKind::Discretionary,
                            TokenWrap::Normal,
                            true,
                        ),
                        metrics,
                    );
                }
                tokens.push_replaced(
                    payload,
                    |payload_idx| InlineTokenKind::Image { payload_idx },
                    token,
                    metrics,
                );
                if white_space.allows_wrap() && run.box_idx as usize != container_box_idx {
                    tokens.push(
                        InlineToken::new(
                            InlineTokenKind::Opportunity,
                            0.0,
                            BreakKind::Discretionary,
                            TokenWrap::Normal,
                            true,
                        ),
                        metrics,
                    );
                }
            }
            InlineItemKind::AtomicBox { box_idx } => {
                let (token, metrics, payload) = create_atomic_box_token(
                    engine,
                    *box_idx,
                    style,
                    vertical_align,
                    max_width,
                    containing_block_height,
                );
                if white_space.allows_wrap() {
                    tokens.push(
                        InlineToken::new(
                            InlineTokenKind::Opportunity,
                            0.0,
                            BreakKind::Discretionary,
                            TokenWrap::Normal,
                            true,
                        ),
                        metrics,
                    );
                }
                tokens.push_replaced(
                    payload,
                    |payload_idx| InlineTokenKind::AtomicBox { payload_idx },
                    token,
                    metrics,
                );
                if white_space.allows_wrap() {
                    tokens.push(
                        InlineToken::new(
                            InlineTokenKind::Opportunity,
                            0.0,
                            BreakKind::Discretionary,
                            TokenWrap::Normal,
                            true,
                        ),
                        metrics,
                    );
                }
            }
            InlineItemKind::Break { clear } => {
                let (token, metrics) =
                    create_break_token(run.box_idx, *clear, style, vertical_align);
                tokens.push(token, metrics);
            }
            InlineItemKind::FloatAnchor { box_idx } => {
                let (token, metrics) = create_float_anchor_token(*box_idx, style, vertical_align);
                tokens.push(token, metrics);
            }
            InlineItemKind::AbsoluteAnchor { box_idx } => {
                let (token, metrics) =
                    create_absolute_anchor_token(*box_idx, style, vertical_align);
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
                );
            }
        }
    }
    tokens.insert_preserved_space_break_opportunities(engine);
    tokens.collapse_edge_whitespace_through_boundaries();
    tokens.classify_space_glue(engine);
    tokens.rebuild_summary_blocks();
    engine.record_timing(|t| t.build_inline_tokens_from_runs += timing_started.elapsed());
    tokens
}

/// Identifies an image that is the sole meaningful inline content of its
/// formatting context. Whitespace and transparent inline wrappers are
/// harmless; text, another image, a break, a float, or an atomic inline
/// box makes the context ineligible.
pub(super) fn standalone_inline_image(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    runs: &[InlineItem],
    container_box_idx: usize,
) -> Option<u32> {
    let mut candidate = None;
    for run in runs {
        if !inline_item_ownership_box(engine, run).is_some_and(|box_idx| {
            run_belongs_to_inline_context(engine, box_idx, container_box_idx)
        }) {
            continue;
        }
        match &run.kind {
            InlineItemKind::Image { .. } if candidate.is_none() => candidate = Some(run.box_idx),
            InlineItemKind::Text { glyphs } => {
                let has_text = glyphs.clone().any(|glyph_idx| {
                    let glyph = engine.text.glyph_at(glyph_idx as usize).unwrap_or_default();
                    !engine.text.glyph_metric(glyph).ch().is_whitespace()
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

pub(in crate::layout) fn inline_item_ownership_box(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    item: &InlineItem,
) -> Option<usize> {
    match item.kind {
        // The atomic box is a formatting-context root. Its parent owns the
        // placeholder in the surrounding inline formatting context.
        InlineItemKind::AtomicBox { box_idx } => engine.reader.get_parent(box_idx as usize),
        _ => Some(item.box_idx as usize),
    }
}

/// Shared inline items belonging to a nested formatting context must not be
/// flattened into an ancestor line. Stop at the requested container; any
/// block/table/flex/grid root encountered before it owns the run instead.
pub(in crate::layout) fn run_belongs_to_inline_context(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    run_box_idx: usize,
    container_box_idx: usize,
) -> bool {
    let mut current = Some(run_box_idx);
    while let Some(box_idx) = current {
        if box_idx == container_box_idx {
            return true;
        }
        if matches!(
            engine.reader.box_layout_mode(box_idx),
            Some(
                crate::layout_model::LayoutMode::Block(_)
                    | crate::layout_model::LayoutMode::Table(_)
                    | crate::layout_model::LayoutMode::TableRow(_)
                    | crate::layout_model::LayoutMode::TableCell(_)
                    | crate::layout_model::LayoutMode::Flex(_)
                    | crate::layout_model::LayoutMode::Grid(_)
            )
        ) {
            return false;
        }
        current = engine.reader.get_parent(box_idx);
    }
    false
}

fn create_atomic_box_token(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    box_idx: u32,
    style: html_style_model::UsedStyleView,
    vertical_align: VerticalAlignValue,
    max_width: f64,
    containing_block_height: Option<f64>,
) -> (InlineToken, InlineTokenMetrics, ReplacedToken) {
    let max_width = max_width.max(0.0);
    let margin_left = style.margin_left().resolve(max_width);
    let margin_right = style.margin_right().resolve(max_width);
    let margin_top = style.margin_top().resolve(max_width);
    let margin_bottom = style.margin_bottom().resolve(max_width);
    let resolved_padding = style.get_horizontal_padding(max_width);
    let horizontal_border = style.border_left_width() as f64 + style.border_right_width() as f64;
    let vertical_padding = style.get_vertical_padding(max_width);
    let vertical_border = style.border_top_width() as f64 + style.border_bottom_width() as f64;
    // Anonymous inline tables carry an anonymous reset style whose computed
    // `display` is not `inline-table`; participation in this atomic inline
    // context is represented authoritatively by the layout mode instead.
    let is_inline_table = matches!(
        engine.reader.box_layout_mode(box_idx as usize),
        Some(crate::layout_model::LayoutMode::Table(_))
    );
    let (border_size, first_baseline, last_baseline) = if let Some(intrinsic) =
        engine.reader.image_intrinsic(box_idx as usize)
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
                margin_left + margin_right,
                margin_top + margin_bottom,
                resolved_padding + horizontal_border,
                vertical_padding + vertical_border,
            ),
        );
        (
            Size::new(
                content.width + resolved_padding + horizontal_border,
                content.height + vertical_padding + vertical_border,
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
        crate::layout::measure_box_and_baselines_isolated(
            engine,
            box_idx as usize,
            max_width,
            containing_block_height,
        )
    } else {
        // Atomic inline sizing needs the raw content contributions here. The
        // outer intrinsic helper already applies the box's preferred width;
        // using it for `fit-content` collapses min-content to max-content and
        // prevents the available inline size from clamping between them.
        let (intrinsic_min, intrinsic_max) =
            crate::layout::box_content_intrinsic_widths(engine, box_idx as usize);
        let min_border_width = (intrinsic_min + resolved_padding + horizontal_border).max(0.0);
        let max_border_width =
            (intrinsic_max + resolved_padding + horizontal_border).max(min_border_width);
        let border_inset = resolved_padding + horizontal_border;
        let available_border_width = (max_width - margin_left - margin_right).max(border_inset);
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
                let vertical_border_inset = vertical_padding + vertical_border;
                let vertical_box_sizing_inset = matches!(style.box_sizing(), BoxSizing::BorderBox)
                    .then_some(vertical_border_inset)
                    .unwrap_or(0.0);
                let content_height = crate::layout::resolve_vertical_size(
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
        let automatic_minimum = if ratio_transferred_width.is_some()
            && !matches!(
                style.overflow_x(),
                html_style_model::OverflowMode::Hidden
                    | html_style_model::OverflowMode::Scroll
                    | html_style_model::OverflowMode::Auto
            ) {
            min_border_width
        } else {
            border_inset
        };
        let minimum = resolve_border_width(style.min_width(), automatic_minimum);
        let maximum = resolve_border_width(style.max_width(), f64::INFINITY).max(minimum);
        let assigned_width = preferred.clamp(minimum, maximum);
        crate::layout::measure_box_width_and_baselines_isolated(
            engine,
            box_idx as usize,
            max_width,
            assigned_width,
            containing_block_height,
        )
    };
    let outer_width = (border_size.width + margin_left + margin_right).max(0.0);
    let outer_height = (border_size.height + margin_top + margin_bottom).max(0.0);
    let is_inline_block = matches!(style.display(), html_style_model::Display::InlineBlock);
    let is_inline_flex_or_grid = matches!(
        engine.reader.box_layout_mode(box_idx as usize),
        Some(crate::layout_model::LayoutMode::Flex(_) | crate::layout_model::LayoutMode::Grid(_))
    );
    let visible_overflow = !style.overflow_x().clips() && !style.overflow_y().clips();
    let propagated_baseline = if is_inline_table {
        // CSS exposes the first row's baseline for an inline-table. When
        // that row has no line boxes, its bottom edge is the fallback
        // baseline; treating the entire table as bottom-baseline incorrectly
        // adds the parent font strut below the table's margin box.
        first_baseline.or_else(|| inline_table_first_row_bottom(engine, box_idx as usize))
    } else if is_inline_block && visible_overflow {
        last_baseline
    } else if is_inline_flex_or_grid {
        first_baseline
    } else {
        None
    };
    let (ascent, descent) = if let Some(baseline) = propagated_baseline {
        let ascent = margin_top + baseline;
        (ascent, (outer_height - ascent).max(0.0))
    } else {
        (outer_height, 0.0)
    };
    let line_height = if is_inline_table {
        outer_height
    } else {
        resolved_line_height(style)
    };
    (
        InlineToken::new(
            InlineTokenKind::AtomicBox {
                payload_idx: u32::MAX,
            },
            outer_width,
            BreakKind::None,
            TokenWrap::Normal,
            true,
        ),
        InlineTokenMetrics {
            owner_box_idx: box_idx,
            ascent: ascent as f32,
            descent: descent as f32,
            line_height,
            tab_interval: -1.0,
            tab_min_advance: 0.0,
            font_size: style.font_size(),
            vertical_align,
            white_space: style.white_space(),
            visible: style.visibility() == html_style_model::Visibility::Visible,
            placement_required: false,
        },
        ReplacedToken::AtomicBox {
            box_idx,
            border_size,
            containing_width: max_width,
            containing_height: containing_block_height,
            margin_left,
            margin_top,
        },
    )
}

fn inline_table_first_row_bottom(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    table_idx: usize,
) -> Option<f64> {
    let crate::layout_model::LayoutMode::Table(table) = engine.reader.box_layout_mode(table_idx)?
    else {
        return None;
    };
    let row_idx = *table.rows.first()? as usize;
    let table_y = engine.geometry.point(table_idx).y;
    let row_point = engine.geometry.point(row_idx);
    let row_size = engine.geometry.size(row_idx);
    Some(row_point.y + row_size.height - table_y)
}
