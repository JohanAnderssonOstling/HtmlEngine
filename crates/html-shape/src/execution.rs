//! Document shaping execution and targeted pseudo-style reshaping.

use super::*;
use crate::GlyphRequest;
#[cfg(feature = "profiling")]
use crate::profiling::scope;
use html_engine_model::grapheme_boundaries;
use html_style_model::StyleView;
use html_text::suppresses_glyph;
use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::sync::Arc;

/// CSS requires otherwise unhandled C0/DEL controls to have visible output.
/// Substitute control pictures only for shaping; logical text and source
/// offsets retain the authored characters.
fn visible_control(character: char) -> char {
    match character {
        '\u{0000}'..='\u{0008}' | '\u{000b}' | '\u{000c}' | '\u{000e}'..='\u{001f}' => {
            char::from_u32(0x2400 + u32::from(character)).expect("control picture scalar")
        }
        '\u{007f}' => '\u{2421}',
        '\u{0085}' => '\u{2424}',
        _ => character,
    }
}

fn shaping_features(style: StyleView<'_>, has_tracking: bool) -> Vec<OpenTypeFeature> {
    let mut features = style.open_type_features();
    if has_tracking {
        // Suppress optional ligatures without splitting contextual shaping
        // at spacing-only span boundaries. Required script features remain.
        for tag in [*b"liga", *b"clig", *b"dlig", *b"hlig"] {
            if style.font.font_feature_settings.iter().any(|feature| feature.tag() == tag) {
                continue;
            }
            let disabled = OpenTypeFeature::new(tag, 0);
            if let Some(feature) = features.iter_mut().find(|feature| feature.tag() == tag) {
                *feature = disabled;
            } else {
                features.push(disabled);
            }
        }
    }
    features
}

/// Shapes prepared logical text without repeating whitespace or case conversion.
/// The renderer supplies native run geometry when available; per-character
/// metrics remain the renderer-independent fallback.
pub fn shape_document(
    styles: &ComputedStyles,
    layout_tree: &LayoutTree,
    inline_content: &InlineContent,
    glyphs: &mut [GlyphId],
    first_letter_styles: &FirstLetterStyles,
    glyph_shaper: &mut impl GlyphShaper,
    resources: &mut GlyphResourceStore,
) -> Result<
    (
        FxHashMap<u32, GlyphId>,
        FxHashMap<u32, GlyphId>,
        ShapedTextGeometry,
        ShapedFontMetrics,
    ),
    ShapeError,
> {
    profile_scope!("shape.document");
    #[cfg(feature = "profiling")]
    let font_setup_scope = scope("shape.font_setup");
    let mut boxes_with_conditional_hyphens = vec![false; layout_tree.box_count()];
    let mut font_content_extents_required = vec![false; layout_tree.box_count()];
    for run in inline_content.text_runs().filter(|run| !run.glyphs.is_empty()) {
        // Authored line-height needs the selected face's ascent and descent
        // even when no font-relative CSS length otherwise requests them.
        font_content_extents_required[run.box_idx as usize] = true;
        boxes_with_conditional_hyphens[run.box_idx as usize] |= run.glyphs.clone().any(|glyph_idx| {
            inline_content.character_at(glyph_idx as usize) == Some('\u{00ad}')
        });
    }
    let mut ellipsis_boxes = Vec::new();
    let mut hyphen_boxes = Vec::new();
    let empty_inline_metrics_required = inline_content.inline_items().iter().any(|item| matches!(item.kind, InlineItemKind::InlineBoundary { inline_start, inline_end } if inline_start == inline_end));
    let mut dense_box_metrics_required = empty_inline_metrics_required
        || inline_content.characters().contains(&'\t');
    for idx in 0..layout_tree.box_count() {
        let style = get_style(styles, layout_tree, idx);
        let paints_inline_content_box = matches!(
            layout_tree.get_box_layout_mode(idx),
            Some(LayoutMode::Inline(_))
        ) && (style.background_color() & 0xff != 0
            || style.background_image_present()
            || style.border_top_style().paints_stroke()
            || style.border_right_style().paints_stroke()
            || style.border_bottom_style().paints_stroke()
            || style.border_left_style().paints_stroke()
            || style.outline().style.paints_stroke());
        // A non-replaced inline's painted content box uses the selected
        // face's ascent and descent, independently of line-height. Request
        // those metrics even when no font-relative CSS length needs them.
        dense_box_metrics_required |= style.requires_font_metrics() || style.line_height_is_normal();
        font_content_extents_required[idx] |=
            !style.text_decoration_lines().is_empty() || paints_inline_content_box;
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
        for parent_idx in layout_tree.ancestors(box_idx) {
            font_content_extents_required[parent_idx] = true;
            if !matches!(
                layout_tree.get_box_layout_mode(parent_idx),
                Some(LayoutMode::Inline(_))
            ) {
                break;
            }
        }
    }

    #[cfg(feature = "profiling")]
    drop(font_setup_scope);
    #[cfg(feature = "profiling")]
    let font_metrics_scope = scope("shape.font_metrics");
    let mut font_metric_cache = FxHashMap::<
        (
            u32,
            u16,
            FontSlant,
            u32,
            u32,
            html_style_model::FontSynthesisStyle,
            Option<&str>,
            Option<LanguageId>,
        ),
        FontRelativeMetrics,
    >::default();
    let mut resolve_font_metrics = |style: StyleView<'_>| {
        let family = style.font_family().and_then(|id| styles.string(id));
        let key = (
            style.font_size().to_bits(),
            style.font_weight(),
            style.font_style().into(),
            style.font.font_stretch.to_bits(),
            style.font.font_oblique_angle.to_bits(),
            style.font.font_synthesis_style,
            family,
            style.language(),
        );
        if let Some(metrics) = font_metric_cache.get(&key).copied() {
            return Ok::<_, ShapeError>(metrics);
        }
        let metrics = if let Some(request) = FontMetricsRequest::new(
            style.font_size(),
            style.font_weight(),
            style.font_style().into(),
            family,
        ) {
            glyph_shaper.font_relative_metrics(
                request
                    .with_font_selection(
                        style.font.font_stretch,
                        style.font.font_oblique_angle,
                        style.font.font_palette.and_then(|id| styles.string(id)),
                    )
                    .with_style_synthesis(style.font.font_synthesis_style)
                    .with_language(style.language().and_then(|id| styles.language(id))),
            )?
        } else {
            FontRelativeMetrics::fallback()
        };
        font_metric_cache.insert(key, metrics);
        Ok(metrics)
    };
    let box_metrics = if dense_box_metrics_required {
        let mut metrics = Vec::with_capacity(layout_tree.box_count());
        for box_idx in 0..layout_tree.box_count() {
            let style = get_style(styles, layout_tree, box_idx);
            metrics.push(resolve_font_metrics(style)?);
        }
        BoxFontMetrics::Dense(metrics)
    } else if font_content_extents_required
        .iter()
        .any(|required| *required)
    {
        let mut metric_ids = vec![0; layout_tree.box_count()];
        let mut distinct_metrics = Vec::<FontRelativeMetrics>::new();
        for box_idx in (0..layout_tree.box_count())
            .filter(|&box_idx| font_content_extents_required[box_idx])
        {
            let style = get_style(styles, layout_tree, box_idx);
            let font_metrics = resolve_font_metrics(style)?;
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

    // Text pseudo styles and table columns do not necessarily have their own
    // formatting box. Resolve their metrics before layout consumes used values.
    let non_box_styles = || styles.text_pseudo_styles().chain(layout_tree.non_box_styles())
        .chain(styles.text_spacing_origins());
    // Pseudo text needs selected-face layout bounds just like ordinary text,
    // including styles with an authored line-height and no font-relative units.
    let non_box_metrics = if styles.text_pseudo_styles().next().is_some()
        || non_box_styles().any(|indices| {
            let style = styles.view(indices).expect("validated style handle");
            style.requires_font_metrics() || style.line_height_is_normal()
        })
    {
        let mut metrics = FxHashMap::default();
        for indices in non_box_styles() {
            if let Entry::Vacant(entry) = metrics.entry(indices) {
                let style = styles.view(indices).expect("validated style handle");
                entry.insert(resolve_font_metrics(style)?);
            }
        }
        RequiredFontMetrics::Required(metrics)
    } else {
        RequiredFontMetrics::NotRequired
    };
    let (root_ch_px, root_cap_height_px, root_line_height_px) = layout_tree
        .root_box()
        .map(|root_box| {
            let style = get_style(styles, layout_tree, root_box);
            let metrics = box_metrics.for_box(root_box);
            let font_size = style
                .resolved_font_size(
                    metrics.x_height_ratio(),
                    metrics.ch_advance_ratio(),
                    metrics.cap_height_ratio(),
                )
                .unwrap_or_else(|| style.font_size());
            let line_height = if style.line_height_is_normal() {
                metrics.normal_line_height(font_size)
            } else {
                None
            }.unwrap_or_else(|| {
                style.used_line_height(font_size, metrics.x_height_ratio())
                    .expect("shaping provides validated font metrics")
            }) as f32;
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

    #[cfg(feature = "profiling")]
    drop(font_metrics_scope);
    let mut text_geometry = profile!(
        "shape.allocate_geometry",
        ShapedTextGeometry::with_len(inline_content.characters().len())
    );
    let bidi = profile!(
        "shape.resolve_bidi",
        ResolvedBidi::new(styles, layout_tree, inline_content)
    );
    let mut scattered_characters = Vec::new();
    let mut scattered_glyphs = Vec::new();
    let mut normalized_text = String::new();
    let spans = profile!(
        "shape.plan_spans",
        plan_shaping_spans(
            styles,
            layout_tree,
            inline_content,
            first_letter_styles,
            &font_metrics,
            &bidi,
        )
    );
    for (span, bidi_level) in spans.into_iter().flat_map(|span| span.split_bidi(&bidi)) {
        #[cfg(feature = "profiling")]
        let span_buffers_scope = scope("shape.span_buffers_and_styles");
        let box_idx = span.shaping_box;
        // Conditional hyphens are source break opportunities, not input to
        // contextual shaping. Some native shapers give them advance or sever
        // Arabic joining even when their own advance is later suppressed.
        let has_soft_hyphen = span.character_indices().any(|index| {
            inline_content.character_at(index as usize) == Some('\u{00ad}')
        });
        let source_indices = span.character_indices().filter(|&index| {
            !has_soft_hyphen
                || inline_content.character_at(index as usize) != Some('\u{00ad}')
        });
        let contiguous_range = if has_soft_hyphen { None } else { span.contiguous_range() };
        let style = span.style(styles, layout_tree);
        let font_size = font_metrics.resolved_font_size(style, box_idx);
        let (paint_colors, placement_required) = span.paint_and_placement(styles, layout_tree, &font_metrics);
        let paint_colors = paint_colors.map(|colors| {
            if has_soft_hyphen {
                span.character_indices().zip(colors).filter_map(|(index, color)| {
                    (inline_content.character_at(index as usize) != Some('\u{00ad}'))
                        .then_some(color)
                }).collect()
            } else {
                colors
            }
        });
        let has_tracking = span.has_tracking(styles, layout_tree, &font_metrics);
        let font_features = shaping_features(style, has_tracking);

        if has_soft_hyphen {
            let control_style = shaping_style_span(
                styles, style, font_size, 1, &font_features, bidi_level,
            ).expect("validated shaping style");
            shape_soft_hyphen_slots(
                span.character_indices().filter(|&index| {
                    inline_content.character_at(index as usize) == Some('\u{00ad}')
                }),
                &control_style, glyphs, 0, &mut text_geometry, glyph_shaper, resources,
            )?;
            if source_indices.clone().next().is_none() {
                continue;
            }
        }

        // Out-of-flow descendants can interleave source indices. Gather those
        // spans in reusable scratch; ordinary spans borrow prepared characters.
        let characters: &[char] = if let Some(range) = &contiguous_range {
            &inline_content.characters()[range.start as usize..range.end as usize]
        } else {
            scattered_characters.clear();
            scattered_characters.extend(
                source_indices
                    .clone()
                    .map(|index| inline_content.character_at(index as usize).unwrap_or('?')),
            );
            &scattered_characters
        };
        normalized_text.clear();
        normalized_text.extend(characters.iter().copied().map(visible_control));

        let style_span = shaping_style_span(
            styles,
            style,
            font_size,
            normalized_text.len(),
            &font_features,
            bidi_level,
        ).map(|style| style.with_joining_context(span.join_before, span.join_after));
        if contiguous_range.is_none() {
            scattered_glyphs.clear();
            scattered_glyphs.extend(source_indices.clone().map(|index| {
                inline_content
                    .character_at(index as usize)
                    .unwrap_or_default() as GlyphId
            }));
        }
        #[cfg(feature = "profiling")]
        drop(span_buffers_scope);
        let native_geometry = profile!(
            "shape.backend_and_validation",
            if let Some(request) = style_span.clone()
                .and_then(|style| TextRunShapeRequest::new(&normalized_text, style))
                .and_then(|request| match &paint_colors {
                    Some(colors) => request.with_paint_colors(colors),
                    None => Some(request),
                })
            {
                let run_glyphs = if let Some(range) = &contiguous_range {
                    &mut glyphs[range.start as usize..range.end as usize]
                } else {
                    scattered_glyphs.as_mut_slice()
                };
                let geometry = if characters.iter().any(|&ch| ch == '\n' || html_text::is_mandatory_line_break(ch)) {
                    // Segment breaks terminate shaping runs. Asking a backend to
                    // shape text and a newline together can change the preceding
                    // line's ascent/baseline (and is invalid for several native
                    // shaping APIs), so register these glyphs independently.
                    resources.shape_characters(glyph_shaper, &request, run_glyphs)?;
                    None
                } else {
                    glyph_shaper.shape_glyph_run(request, run_glyphs, resources)?
                };
                profile!(
                    "shape.validate_glyph_ids",
                    for &glyph in run_glyphs.iter() {
                        if !resources.contains(glyph) {
                            return Err(ShapeError::unregistered_glyph_id(glyph, resources.len()));
                        }
                    }
                );
                geometry
            } else {
                None
            }
        );
        #[cfg(feature = "profiling")]
        let geometry_scope = scope("shape.graphemes_and_geometry");
        if contiguous_range.is_none() {
            for (index, &glyph) in source_indices.clone().zip(&scattered_glyphs) {
                glyphs[index as usize] = glyph;
            }
        }
        let geometry = resolve_run_geometry(
            native_geometry,
            &normalized_text,
            characters,
            source_indices.clone().map(|index| glyphs[index as usize]),
            resources,
        );
        if bidi_level % 2 == 1
            && let Some(style_span) = &style_span
        {
            shape_mirrored_glyphs(
                source_indices.clone().zip(characters.iter().copied()),
                style_span,
                &mut text_geometry,
                glyph_shaper,
                resources,
            )?;
        }
        #[cfg(feature = "profiling")]
        drop(geometry_scope);
        profile!("shape.retain_geometry", {
            text_geometry.push_run_iter(source_indices, geometry, placement_required);
        });
    }

    profile!("shape.finish_runs", text_geometry.finish_runs());
    text_geometry.bidi = bidi;

    // Text-overflow markers are shaped once per applicable container so width
    // changes can relayout without calling the renderer's font backend again.
    // They are not inserted into InlineContent: synthetic paint must not become
    // selectable or acquire a DOM source position.
    #[cfg(feature = "profiling")]
    let synthetic_scope = scope("shape.synthetic_glyphs");
    let mut ellipsis_glyphs = FxHashMap::default();
    // Automatic hyphens are synthetic paint artifacts, just like ellipses:
    // shape them once per applicable style without assigning DOM positions.
    let mut hyphen_glyphs = FxHashMap::default();
    for (character, boxes, output) in [
        ('\u{2026}', ellipsis_boxes, &mut ellipsis_glyphs),
        ('\u{2010}', hyphen_boxes, &mut hyphen_glyphs),
    ] {
        for box_idx in boxes {
            let style = get_style(styles, layout_tree, box_idx);
            let glyph = resources.shape_glyph(
                glyph_shaper,
                GlyphRequest {
                    font_stretch: style.font.font_stretch,
                    font_oblique_angle: style.font.font_oblique_angle,
                    font_palette: style.font.font_palette.and_then(|id| styles.string(id)),
                    font_synthesis_style: style.font.font_synthesis_style,
                    character,
                    font_size: font_metrics.resolved_font_size(style, box_idx),
                    font_weight: style.font_weight(),
                    font_slant: style.font_style().into(),
                    font_family: style.font_family().and_then(|id| styles.string(id)),
                },
            )?;
            output.insert(box_idx as u32, glyph);
        }
    }

    #[cfg(feature = "profiling")]
    drop(synthetic_scope);
    Ok((ellipsis_glyphs, hyphen_glyphs, text_geometry, font_metrics))
}

fn shaping_style_span<'a>(
    styles: &'a ComputedStyles,
    style: StyleView<'_>,
    font_size: f32,
    text_len: usize,
    font_features: &'a [OpenTypeFeature],
    bidi_level: u8,
) -> Option<TextStyleSpan<'a>> {
    TextStyleSpan::new(
        0..text_len,
        font_size,
        style.font_weight(),
        style.font_style().into(),
        style.color(),
        style.font_family().and_then(|id| styles.string(id)),
    )
    .map(|span| {
        let caps_synthesis =
            (style.font.font_synthesis_small_caps && style.font_variant_small_caps()).then(|| {
                style
                    .font
                    .font_variant_caps_features
                    .iter()
                    .any(|feature| feature.tag() == *b"c2sc" && feature.value() != 0)
            });
        span.with_font_selection(
            style.font.font_stretch,
            style.font.font_oblique_angle,
            style.font.font_palette.and_then(|id| styles.string(id)),
        )
        .with_style_synthesis(style.font.font_synthesis_style)
        .with_font_features(font_features)
        .with_caps_synthesis(caps_synthesis)
        .with_cjk_font_family(style.cjk_font_family().and_then(|id| styles.string(id)))
        .with_language(style.language().and_then(|id| styles.language(id)))
        .with_direction(if bidi_level % 2 == 1 {
            TextDirection::Rtl
        } else {
            TextDirection::Ltr
        })
    })
}

// Keep a registered zero-width glyph and source geometry for each omitted
// soft hyphen so layout can still select that source position as a break.
fn shape_soft_hyphen_slots(
    indices: impl Iterator<Item = u32>,
    style: &TextStyleSpan<'_>,
    glyphs: &mut [GlyphId],
    glyph_start: u32,
    geometry: &mut ShapedTextGeometry,
    glyph_shaper: &mut impl GlyphShaper,
    resources: &mut GlyphResourceStore,
) -> Result<(), ShapeError> {
    let glyph = resources.shape_glyph(glyph_shaper, style.glyph_request('\u{00ad}'))?;
    let zero = ShapedTextRun::new(Arc::from([0.0]), Arc::from([true, true]))
        .expect("zero-width source control");
    for index in indices {
        glyphs[(index - glyph_start) as usize] = glyph;
        geometry.push_run_iter(std::iter::once(index), zero.clone(), false);
    }
    Ok(())
}

fn shape_mirrored_glyphs(
    characters: impl Iterator<Item = (u32, char)>,
    style: &TextStyleSpan<'_>,
    geometry: &mut ShapedTextGeometry,
    glyph_shaper: &mut impl GlyphShaper,
    resources: &mut GlyphResourceStore,
) -> Result<(), ShapeError> {
    for (index, character) in characters {
        let Some(mirrored) = unicode_bidi_mirroring::get_mirrored(character) else {
            continue;
        };
        let glyph = resources.shape_glyph(glyph_shaper, style.glyph_request(mirrored))?;
        geometry.mirrored_glyphs.insert(index, glyph);
    }
    Ok(())
}

/// Retain valid native geometry or derive a fallback from registered metrics.
/// Both paths preserve Unicode graphemes and suppress conditional advances.
fn resolve_run_geometry(
    native: Option<ShapedTextRun>,
    text: &str,
    characters: &[char],
    glyphs: impl Iterator<Item = GlyphId>,
    resources: &GlyphResourceStore,
) -> ShapedTextRun {
    let mut geometry = native
        .filter(|geometry| geometry.advances().len() == characters.len())
        .map(|mut geometry| {
            geometry.preserve_graphemes(text);
            geometry
        })
        .unwrap_or_else(|| {
            let advances = glyphs
                .map(|glyph| {
                    resources
                        .get(glyph)
                        .expect("registered glyph metric")
                        .advance()
                })
                .collect::<Arc<[f32]>>();
            // Fallback boundaries are already canonical; only native geometry
            // needs its cluster boundaries intersected with Unicode graphemes.
            ShapedTextRun::new(advances, grapheme_boundaries(text, characters.len()))
                .expect("validated glyph metrics")
        });
    // Detach the shared advances once, only when a suppressed character occurs.
    let mut offsets = characters
        .iter()
        .enumerate()
        .filter_map(|(offset, &character)| suppresses_glyph(character).then_some(offset));
    if let Some(first) = offsets.next() {
        let advances = Arc::make_mut(&mut geometry.advances);
        advances[first] = 0.0;
        for offset in offsets {
            advances[offset] = 0.0;
        }
    }
    geometry
}

/// Re-shape one normalized source range with a dynamically selected pseudo
/// style. Ordinary document shaping remains independent of viewport width;
/// this path is entered only for `::first-line` after composition identifies
/// the affected range.
pub fn reshape_range_with_style(
    styles: &ComputedStyles,
    font_metrics: &ShapedFontMetrics,
    inline_content: &InlineContent,
    glyphs: &mut [GlyphId],
    glyph_start: u32,
    bidi: &ResolvedBidi,
    text_geometry: &mut ShapedTextGeometry,
    range: Range<u32>,
    style_indices: StyleIndices,
    base_style_indices: StyleIndices,
    glyph_shaper: &mut impl GlyphShaper,
    resources: &mut GlyphResourceStore,
) -> Result<(), ShapeError> {
    if range.is_empty() {
        return Ok(());
    }
    let style = styles
        .view(style_indices)
        .expect("validated pseudo style handle");
    let base_style = styles
        .view(base_style_indices)
        .expect("validated originating style handle");
    // Inline token construction still contributes the originating run's CSS
    // spacing. Put only the pseudo/base delta into authoritative geometry so
    // the final total is the computed `::first-line` spacing.
    let letter_spacing = font_metrics.resolve_text_spacing(styles, style.text.letter_spacing, style.font_size());
    let word_spacing = font_metrics.resolve_text_spacing(styles, style.text.word_spacing, style.font_size());
    let letter_spacing_delta = letter_spacing - font_metrics.resolve_text_spacing(styles, base_style.text.letter_spacing, base_style.font_size());
    let word_spacing_delta = word_spacing - font_metrics.resolve_text_spacing(styles, base_style.text.word_spacing, base_style.font_size());
    let font_features = shaping_features(style, letter_spacing != 0.0);
    let needs_placement = letter_spacing != 0.0
        || word_spacing != 0.0
        || letter_spacing_delta != 0.0
        || word_spacing_delta != 0.0;
    for (range, level) in bidi.level_runs(range) {
        let source_characters = &inline_content.characters()[range.start as usize..range.end as usize];
        let has_soft_hyphen = source_characters.contains(&'\u{00ad}');
        let source_indices = range.clone().filter(|&index| {
            !has_soft_hyphen
                || inline_content.character_at(index as usize) != Some('\u{00ad}')
        });
        let characters: Cow<'_, [char]> = if has_soft_hyphen {
            Cow::Owned(source_characters.iter().copied().filter(|&ch| ch != '\u{00ad}').collect())
        } else {
            Cow::Borrowed(source_characters)
        };
        let text = characters.iter().copied().map(visible_control).collect::<String>();
        let Some(span) = shaping_style_span(
            styles,
            style,
            style.font_size(),
            text.len().max(1),
            &font_features,
            level,
        ) else {
            continue;
        };
        let start = (range.start - glyph_start) as usize;
        let end = (range.end - glyph_start) as usize;
        let mut scattered_glyphs = if has_soft_hyphen {
            source_indices.clone().map(|index| glyphs[(index - glyph_start) as usize]).collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let run_glyphs = if has_soft_hyphen {
            scattered_glyphs.as_mut_slice()
        } else {
            &mut glyphs[start..end]
        };
        let native = if let Some(request) = TextRunShapeRequest::new(&text, span.clone()) {
            glyph_shaper.shape_glyph_run(request, run_glyphs, resources)?
        } else {
            None
        };
        for &glyph in run_glyphs.iter() {
            if !resources.contains(glyph) {
                return Err(ShapeError::unregistered_glyph_id(glyph, resources.len()));
            }
        }

        let mut geometry = resolve_run_geometry(
            native,
            &text,
            &characters,
            run_glyphs.iter().copied(),
            resources,
        );
        if has_soft_hyphen {
            for (index, &glyph) in source_indices.clone().zip(&scattered_glyphs) {
                glyphs[(index - glyph_start) as usize] = glyph;
            }
        }
        if level % 2 == 1 {
            shape_mirrored_glyphs(
                source_indices.clone().zip(characters.iter().copied()),
                &span,
                text_geometry,
                glyph_shaper,
                resources,
            )?;
        }
        if letter_spacing_delta != 0.0 || word_spacing_delta != 0.0 {
            let spacing_deltas = characters.iter().map(|character| {
                if suppresses_glyph(*character) {
                    return 0.0;
                }
                letter_spacing_delta
                    + if *character == ' ' {
                        word_spacing_delta
                    } else {
                        0.0
                    }
            });
            geometry.apply_spacing_iter(spacing_deltas, level % 2 == 1);
        }
        text_geometry.replace_indexed_range(range.clone(), source_indices, geometry, needs_placement);
        if has_soft_hyphen {
            shape_soft_hyphen_slots(
                range.filter(|&index| {
                    inline_content.character_at(index as usize) == Some('\u{00ad}')
                }),
                &span, glyphs, glyph_start, text_geometry, glyph_shaper, resources,
            )?;
            text_geometry.finish_runs();
        }
    }
    Ok(())
}
