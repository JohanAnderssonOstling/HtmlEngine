//! Document shaping execution and targeted pseudo-style reshaping.

use super::*;

/// Shapes all styled text in a document. Whitespace normalization and CSS text
/// transforms are framework-neutral. The renderer supplies native run geometry
/// when available and the established per-character metrics remain the
/// renderer-independent fallback.
pub(crate) fn shape_document(
    document: &Document,
    styles: &ComputedStyles,
    layout_tree: &LayoutTree,
    inline_content: &mut InlineContent,
    glyph_shaper: &mut impl GlyphShaper,
    glyph_metrics: &mut GlyphMetrics,
) -> Result<
    (
        FxHashMap<u32, GlyphId>,
        FxHashMap<u32, GlyphId>,
        ShapedTextGeometry,
        ShapedFontMetrics,
    ),
    ShapeError,
> {
    if !inline_content.glyphs().is_empty() {
        collapse_whitespace(styles, layout_tree, inline_content);
    }
    let mut first_letter_styles =
        first_letter_style_overrides(document, styles, layout_tree, inline_content);
    if !inline_content.glyphs().is_empty() {
        transform_text(
            styles,
            layout_tree,
            inline_content,
            &mut first_letter_styles,
        );
    }
    let mut glyph_metrics = GlyphRegistry::new(glyph_metrics);

    let mut box_family: Vec<Option<String>> = vec![None; layout_tree.box_count()];
    let mut boxes_with_conditional_hyphens = vec![false; layout_tree.box_count()];
    for (box_idx, range) in text_runs(inline_content) {
        boxes_with_conditional_hyphens[box_idx] = range.clone().any(|glyph_idx| {
            inline_content.glyph_at(glyph_idx as usize) == Some('\u{00ad}' as GlyphId)
        });
    }
    let mut ellipsis_boxes = Vec::new();
    let mut hyphen_boxes = Vec::new();
    let empty_inline_metrics_required = inline_content.inline_items().iter().any(|item| matches!(item.kind, InlineItemKind::InlineBoundary { inline_start, inline_end } if inline_start == inline_end));
    let mut dense_box_metrics_required = empty_inline_metrics_required;
    let mut font_content_extents_required = vec![false; layout_tree.box_count()];
    for (idx, family) in box_family.iter_mut().enumerate() {
        let style = get_style(styles, layout_tree, idx);
        let paints_inline_content_box = matches!(
            layout_tree
                .box_at(idx)
                .map(|layout_box| layout_box.layout_mode()),
            Some(LayoutMode::Inline(_))
        ) && (style.background_color() & 0xff != 0
            || style.background_image_present()
            || !matches!(
                style.border_top_style(),
                BorderStyle::None | BorderStyle::Hidden
            )
            || !matches!(
                style.border_right_style(),
                BorderStyle::None | BorderStyle::Hidden
            )
            || !matches!(
                style.border_bottom_style(),
                BorderStyle::None | BorderStyle::Hidden
            )
            || !matches!(
                style.border_left_style(),
                BorderStyle::None | BorderStyle::Hidden
            )
            || !matches!(
                style.outline().style,
                BorderStyle::None | BorderStyle::Hidden
            ));
        // A non-replaced inline's painted content box uses the selected
        // face's ascent and descent, independently of line-height. Request
        // those metrics even when no font-relative CSS length needs them.
        dense_box_metrics_required |= style.requires_font_metrics();
        font_content_extents_required[idx] =
            !style.text_decoration_lines().is_empty() || paints_inline_content_box;
        if let Some(family_idx) = style.font_family() {
            *family = styles.string(family_idx).map(str::to_owned);
        }
        if style.text_overflow() == TextOverflow::Ellipsis && style.overflow_x().clips() {
            ellipsis_boxes.push(idx);
        }
        if style.hyphens() == html_style_model::Hyphens::Auto
            || (style.hyphens() == html_style_model::Hyphens::Manual
                && boxes_with_conditional_hyphens[idx])
        {
            hyphen_boxes.push(idx);
        }
    }

    // The inline content box is positioned within a line whose root strut is
    // owned by the first non-inline ancestor. Keep that container and every
    // intervening inline ancestor on the same selected-font metric path.
    let extent_boxes = font_content_extents_required
        .iter()
        .enumerate()
        .filter_map(|(box_idx, required)| required.then_some(box_idx))
        .collect::<Vec<_>>();
    for box_idx in extent_boxes {
        let mut current = layout_tree.get_box_parent(box_idx);
        while let Some(parent_idx) = current {
            font_content_extents_required[parent_idx] = true;
            if !matches!(
                layout_tree.get_box_layout_mode(parent_idx),
                Some(LayoutMode::Inline(_))
            ) {
                break;
            }
            current = layout_tree.get_box_parent(parent_idx);
        }
    }

    let mut font_metric_cache =
        FxHashMap::<(u32, u16, FontSlant, Option<String>), FontRelativeMetrics>::default();
    let box_metrics = if dense_box_metrics_required {
        let mut metrics = Vec::with_capacity(layout_tree.box_count());
        for (box_idx, family) in box_family.iter().enumerate() {
            let style = get_style(styles, layout_tree, box_idx);
            let key = (
                style.font_size().to_bits(),
                style.font_weight(),
                style.font_style().into(),
                family.clone(),
            );
            let font_metrics = if let Some(metrics) = font_metric_cache.get(&key).copied() {
                metrics
            } else {
                let metrics = if let Some(request) = FontMetricsRequest::new(
                    style.font_size(),
                    style.font_weight(),
                    style.font_style().into(),
                    family.as_deref(),
                ) {
                    glyph_shaper.font_relative_metrics(request)?
                } else {
                    FontRelativeMetrics::fallback()
                };
                font_metric_cache.insert(key, metrics);
                metrics
            };
            metrics.push(font_metrics);
        }
        BoxFontMetrics::Dense(metrics)
    } else if font_content_extents_required
        .iter()
        .any(|required| *required)
    {
        let mut metric_ids = vec![0; layout_tree.box_count()];
        let mut distinct_metrics = Vec::<FontRelativeMetrics>::new();
        for (box_idx, family) in box_family
            .iter()
            .enumerate()
            .filter(|(box_idx, _)| font_content_extents_required[*box_idx])
        {
            let style = get_style(styles, layout_tree, box_idx);
            let key = (
                style.font_size().to_bits(),
                style.font_weight(),
                style.font_style().into(),
                family.clone(),
            );
            let font_metrics = if let Some(metrics) = font_metric_cache.get(&key).copied() {
                metrics
            } else {
                let metrics = if let Some(request) = FontMetricsRequest::new(
                    style.font_size(),
                    style.font_weight(),
                    style.font_style().into(),
                    family.as_deref(),
                ) {
                    glyph_shaper.font_relative_metrics(request)?
                } else {
                    FontRelativeMetrics::fallback()
                };
                font_metric_cache.insert(key, metrics);
                metrics
            };
            let metric_idx = distinct_metrics
                .iter()
                .position(|metrics| *metrics == font_metrics)
                .unwrap_or_else(|| {
                    distinct_metrics.push(font_metrics);
                    distinct_metrics.len() - 1
                });
            metric_ids[box_idx] =
                u32::try_from(metric_idx + 1).expect("too many distinct shaped font metrics");
        }
        BoxFontMetrics::Indexed {
            metric_ids,
            metrics: distinct_metrics,
        }
    } else {
        BoxFontMetrics::NotRequired
    };

    // Table columns are style-bearing CSS boxes but do not generate layout
    // boxes. Resolve metrics for their opaque style handles here so layout
    // never needs to inspect an unresolved computed width.
    let mut non_box_styles = Vec::new();
    for box_idx in 0..layout_tree.box_count() {
        let Some(LayoutMode::Table(table)) = layout_tree
            .box_at(box_idx)
            .map(|layout_box| layout_box.layout_mode())
        else {
            continue;
        };
        for hint in &table.column_width_hints {
            if !non_box_styles.contains(&hint.style) {
                non_box_styles.push(hint.style);
            }
        }
    }
    let non_box_metrics_required = non_box_styles.iter().copied().any(|indices| {
        styles
            .view(indices)
            .expect("validated style handle")
            .requires_font_metrics()
    });
    let non_box_metrics = if non_box_metrics_required {
        let mut metrics = FxHashMap::default();
        for indices in non_box_styles {
            let style = styles.view(indices).expect("validated style handle");
            let family = style
                .font_family()
                .and_then(|family_idx| styles.string(family_idx));
            let key = (
                style.font_size().to_bits(),
                style.font_weight(),
                style.font_style().into(),
                family.map(str::to_owned),
            );
            let font_metrics = if let Some(metrics) = font_metric_cache.get(&key).copied() {
                metrics
            } else {
                let metrics = if let Some(request) = FontMetricsRequest::new(
                    style.font_size(),
                    style.font_weight(),
                    style.font_style().into(),
                    family,
                ) {
                    glyph_shaper.font_relative_metrics(request)?
                } else {
                    FontRelativeMetrics::fallback()
                };
                font_metric_cache.insert(key, metrics);
                metrics
            };
            metrics.insert(indices, font_metrics);
        }
        RequiredFontMetrics::Required(metrics)
    } else {
        RequiredFontMetrics::NotRequired
    };
    let (root_ch_px, root_cap_height_px, root_line_height_px) = layout_tree
        .root_box()
        .map(|root_box| {
            let style = get_style(styles, layout_tree, root_box);
            let metrics = match &box_metrics {
                BoxFontMetrics::NotRequired => FontRelativeMetrics::fallback(),
                BoxFontMetrics::Dense(values) => values[root_box],
                BoxFontMetrics::Indexed {
                    metric_ids,
                    metrics,
                } => metric_ids[root_box]
                    .checked_sub(1)
                    .map_or_else(FontRelativeMetrics::fallback, |id| metrics[id as usize]),
            };
            let font_size = style
                .resolved_font_size(
                    metrics.x_height_ratio(),
                    metrics.ch_advance_ratio(),
                    metrics.cap_height_ratio(),
                )
                .unwrap_or_else(|| style.font_size());
            let line_height = if style.line_height_is_normal() {
                font_size * 1.2
            } else {
                style
                    .resolved_line_height(metrics.x_height_ratio())
                    .unwrap_or_else(|| style.line_height())
                    .max(0.0)
            };
            (
                font_size * metrics.ch_advance_ratio(),
                font_size * metrics.cap_height_ratio(),
                line_height,
            )
        })
        .unwrap_or((0.0, 0.0, 0.0));
    let font_metrics = ShapedFontMetrics::new(
        box_metrics,
        non_box_metrics,
        layout_tree.box_count(),
        root_ch_px,
        root_cap_height_px,
        root_line_height_px,
    );

    let mut text_geometry = ShapedTextGeometry::with_len(inline_content.glyphs().len());
    let mut source_indices = Vec::<u32>::new();
    for span in plan_shaping_spans(
        styles,
        layout_tree,
        inline_content,
        &first_letter_styles,
        &font_metrics,
    ) {
        let box_idx = span.shaping_box;
        source_indices.clear();
        source_indices.extend(span.character_indices());
        debug_assert_eq!(source_indices.len(), span.character_count());
        let contiguous_range = span.contiguous_range();
        let style_override = span.style_override;
        let style = style_override
            .and_then(|indices| styles.view(indices))
            .unwrap_or_else(|| get_style(styles, layout_tree, box_idx));
        let font_size = font_metrics.resolved_font_size(style, box_idx);
        let font_weight = style.font_weight();
        let font_style = style.font_style();
        let color = style.color();
        let family = style
            .font_family()
            .and_then(|family_idx| styles.string(family_idx));
        let font_features = style.open_type_features();

        let mut normalized_text = String::new();
        let mut normalized_characters = Vec::with_capacity(source_indices.len());
        for &i in &source_indices {
            let character = char::from_u32(
                inline_content
                    .glyph_at(i as usize)
                    .unwrap_or('?' as GlyphId),
            )
            .unwrap_or('?');

            normalized_text.push(character);
            normalized_characters.push(character);
        }

        let style_span = TextStyleSpan::new(
            0..normalized_text.len(),
            font_size,
            font_weight,
            font_style.into(),
            color,
            family,
        )
        .map(|style| style.with_font_features(font_features));
        let paint_colors = span.paint_colors(styles, layout_tree);
        let mut scattered_glyphs = contiguous_range.is_none().then(|| {
            source_indices
                .iter()
                .map(|&index| inline_content.glyph_at(index as usize).unwrap_or_default())
                .collect::<Vec<_>>()
        });
        let native_geometry = if let Some(request) = style_span
            .and_then(|style| TextRunShapeRequest::new(&normalized_text, style))
            .and_then(|request| request.with_paint_colors(&paint_colors))
        {
            let run_glyphs = if let Some(range) = &contiguous_range {
                &mut inline_content.glyphs_mut()[range.start as usize..range.end as usize]
            } else {
                scattered_glyphs
                    .as_mut()
                    .expect("discontiguous shaping buffer")
                    .as_mut_slice()
            };
            let geometry = if normalized_characters.contains(&'\n') {
                // Segment breaks terminate shaping runs. Asking a backend to
                // shape text and a newline together can change the preceding
                // line's ascent/baseline (and is invalid for several native
                // shaping APIs), so register these glyphs independently.
                for (glyph, &character) in run_glyphs.iter_mut().zip(&normalized_characters) {
                    *glyph = glyph_shaper.shape_glyph(
                        &mut glyph_metrics,
                        character,
                        font_size,
                        font_weight,
                        font_style.into(),
                        color,
                        family,
                    )?;
                }
                None
            } else {
                glyph_shaper.shape_glyph_run(&mut glyph_metrics, request, run_glyphs)?
            };
            for &glyph in run_glyphs.iter() {
                if !glyph_metrics.contains(glyph) {
                    return Err(ShapeError::unregistered_glyph_id(
                        glyph,
                        glyph_metrics.len(),
                    ));
                }
            }
            geometry
        } else {
            None
        };
        if let Some(glyphs) = scattered_glyphs {
            for (&index, glyph) in source_indices.iter().zip(glyphs) {
                inline_content.glyphs_mut()[index as usize] = glyph;
            }
        }
        if let Some(native_geometry) = native_geometry
            .filter(|geometry| geometry.advances().len() == normalized_characters.len())
        {
            for (offset, &index) in source_indices.iter().enumerate() {
                text_geometry.advances[index as usize] = native_geometry.advances()[offset];
                text_geometry.cluster_boundaries[index as usize] &=
                    native_geometry.cluster_boundaries()[offset];
            }
            if let Some(&last) = source_indices.last() {
                text_geometry.cluster_boundaries[last as usize + 1] &=
                    native_geometry.cluster_boundaries()[source_indices.len()];
            }
            if let Some(range) = contiguous_range
                && let (Some(backend_run), Some(caret_stops)) =
                    (native_geometry.backend_run(), native_geometry.caret_stops())
            {
                text_geometry
                    .authoritative_runs
                    .push(AuthoritativeShapedRun {
                        source_range: range.clone(),
                        backend_run,
                        caret_stops: Arc::from(caret_stops),
                        ascent: native_geometry.ascent(),
                        placement_required: span.placement_required(styles, layout_tree),
                    });
            }
        } else {
            for (offset, &index) in source_indices.iter().enumerate() {
                let glyph = inline_content.glyph_at(index as usize).unwrap_or_default();
                let metric = glyph_metrics.get(glyph).expect("registered glyph metric");
                text_geometry.advances[index as usize] = metric.advance();
                debug_assert_eq!(metric.ch(), normalized_characters[offset]);
            }
        }
        // U+00AD is a conditional break marker, not visible inline content.
        // Some shaping backends already report a zero advance, but enforcing
        // it here keeps layout independent of backend/font behavior.
        for (offset, character) in normalized_characters.iter().copied().enumerate() {
            if character == '\u{00ad}' {
                text_geometry.advances[source_indices[offset] as usize] = 0.0;
            }
        }
    }

    // Text-overflow markers are shaped once per applicable container so width
    // changes can relayout without calling the renderer's font backend again.
    // They are not inserted into InlineContent: synthetic paint must not become
    // selectable or acquire a DOM source position.
    let mut ellipsis_glyphs = FxHashMap::default();
    for box_idx in ellipsis_boxes {
        let style = get_style(styles, layout_tree, box_idx);
        let family = box_family[box_idx].as_deref();
        let font_size = font_metrics.resolved_font_size(style, box_idx);
        let glyph = glyph_shaper.shape_glyph(
            &mut glyph_metrics,
            '\u{2026}',
            font_size,
            style.font_weight(),
            style.font_style().into(),
            style.color(),
            family,
        )?;
        if !glyph_metrics.contains(glyph) {
            return Err(ShapeError::unregistered_glyph_id(
                glyph,
                glyph_metrics.len(),
            ));
        }
        ellipsis_glyphs.insert(box_idx as u32, glyph);
    }
    // Automatic hyphens are synthetic paint artifacts, just like ellipses:
    // shape them once per applicable style without assigning DOM positions.
    let mut hyphen_glyphs = FxHashMap::default();
    for box_idx in hyphen_boxes {
        let style = get_style(styles, layout_tree, box_idx);
        let family = box_family[box_idx].as_deref();
        let font_size = font_metrics.resolved_font_size(style, box_idx);
        let glyph = glyph_shaper.shape_glyph(
            &mut glyph_metrics,
            '\u{2010}',
            font_size,
            style.font_weight(),
            style.font_style().into(),
            style.color(),
            family,
        )?;
        if !glyph_metrics.contains(glyph) {
            return Err(ShapeError::unregistered_glyph_id(
                glyph,
                glyph_metrics.len(),
            ));
        }
        hyphen_glyphs.insert(box_idx as u32, glyph);
    }
    Ok((ellipsis_glyphs, hyphen_glyphs, text_geometry, font_metrics))
}

/// Re-shape one normalized source range with a dynamically selected pseudo
/// style. Ordinary document shaping remains independent of viewport width;
/// this path is entered only for `::first-line` after composition identifies
/// the affected range.
pub(crate) fn reshape_range_with_style(
    styles: &ComputedStyles,
    inline_content: &mut InlineContent,
    glyph_metrics: &mut GlyphMetrics,
    text_geometry: &mut ShapedTextGeometry,
    range: Range<u32>,
    style_indices: StyleIndices,
    base_style_indices: StyleIndices,
    glyph_shaper: &mut impl GlyphShaper,
) -> Result<(), ShapeError> {
    if range.start >= range.end {
        return Ok(());
    }
    let style = styles
        .view(style_indices)
        .expect("validated pseudo style handle");
    let base_style = styles
        .view(base_style_indices)
        .expect("validated originating style handle");
    let characters = range
        .clone()
        .map(|index| {
            glyph_metrics
                .get(inline_content.glyph_at(index as usize).unwrap_or_default())
                .ch()
        })
        .collect::<Vec<_>>();
    let text = characters.iter().collect::<String>();
    // Inline token construction still contributes the originating run's CSS
    // spacing. Put only the pseudo/base delta into authoritative geometry so
    // the final total is the computed `::first-line` spacing.
    let letter_spacing_delta = style.letter_spacing() - base_style.letter_spacing();
    let word_spacing_delta = style.word_spacing() - base_style.word_spacing();
    let spacing_deltas = characters
        .iter()
        .map(|character| {
            letter_spacing_delta
                + if *character == ' ' {
                    word_spacing_delta
                } else {
                    0.0
                }
        })
        .collect::<Vec<_>>();
    let family = style
        .font_family()
        .and_then(|family_idx| styles.string(family_idx));
    let Some(span) = TextStyleSpan::new(
        0..text.len(),
        style.font_size(),
        style.font_weight(),
        style.font_style().into(),
        style.color(),
        family,
    )
    .map(|span| span.with_font_features(style.open_type_features())) else {
        return Ok(());
    };
    let Some(request) = TextRunShapeRequest::new(&text, span) else {
        return Ok(());
    };
    let start = range.start as usize;
    let end = range.end as usize;
    let mut registry = GlyphRegistry::new(glyph_metrics);
    let native = glyph_shaper.shape_glyph_run(
        &mut registry,
        request,
        &mut inline_content.glyphs_mut()[start..end],
    )?;
    for &glyph in &inline_content.glyphs()[start..end] {
        if !registry.contains(glyph) {
            return Err(ShapeError::unregistered_glyph_id(glyph, registry.len()));
        }
    }

    text_geometry
        .authoritative_runs
        .retain(|run| run.source_range.end <= range.start || run.source_range.start >= range.end);
    if let Some(native) = native.filter(|geometry| geometry.advances().len() == characters.len()) {
        for ((target, advance), spacing) in text_geometry.advances[start..end]
            .iter_mut()
            .zip(native.advances())
            .zip(&spacing_deltas)
        {
            *target = *advance + *spacing;
        }
        text_geometry.cluster_boundaries[start..=end].copy_from_slice(native.cluster_boundaries());
        if let (Some(backend_run), Some(caret_stops)) = (native.backend_run(), native.caret_stops())
        {
            let mut adjusted_stops = Vec::with_capacity(caret_stops.len());
            let mut cumulative_spacing = 0.0;
            for (offset, stop) in caret_stops.iter().copied().enumerate() {
                adjusted_stops.push(stop + cumulative_spacing);
                if let Some(delta) = spacing_deltas.get(offset) {
                    cumulative_spacing += *delta;
                }
            }
            text_geometry
                .authoritative_runs
                .push(AuthoritativeShapedRun {
                    source_range: range,
                    backend_run,
                    caret_stops: Arc::from(adjusted_stops),
                    ascent: native.ascent(),
                    placement_required: style.letter_spacing() != 0.0
                        || style.word_spacing() != 0.0
                        || letter_spacing_delta != 0.0
                        || word_spacing_delta != 0.0,
                });
        }
    } else {
        for index in start..end {
            let glyph = inline_content.glyph_at(index).unwrap_or_default();
            text_geometry.advances[index] = registry
                .get(glyph)
                .expect("registered pseudo glyph")
                .advance()
                + spacing_deltas[index - start];
        }
        text_geometry.cluster_boundaries[start..=end].fill(true);
    }
    Ok(())
}
