use super::*;
use html_style_model::{UsedStyleView, Visibility};
use rustc_data_structures::fx::FxHashSet;
#[cfg(feature = "profiling")]
use crate::profiling::scope;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct InlineOverflow {
    pub(super) clips: bool,
    pub(super) ellipsis: Option<InlineToken>,
}

pub(super) fn inline_overflow(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    container_box_idx: usize,
    tokens: &mut InlineTokens,
) -> InlineOverflow {
    let style = engine.reader.style(container_box_idx);
    let clips = style.overflow_x().clips();
    let ellipsis = (clips && style.text_overflow() == TextOverflow::Ellipsis)
        .then(|| engine.text.ellipsis_glyph(container_box_idx))
        .flatten()
        .map(|glyph| {
            let (mut token, metrics) = create_ellipsis_token(engine, glyph, style);
            token.set_bidi_level(u8::from(style.direction() == TextDirection::Rtl));
            tokens.bind_metrics(token, metrics)
        });
    InlineOverflow { clips, ellipsis }
}

pub(super) fn resolved_line_height(style: UsedStyleView<'_>) -> f64 {
    if style.line_height_is_normal() {
        style.font_size() as f64 * 1.2
    } else {
        style.line_height().max(0.0) as f64
    }
}

pub(super) fn append_text_tokens(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    tokens: &mut InlineTokens,
    glyphs: Range<u32>,
    box_idx: usize,
    style: UsedStyleView,
    white_space: WhiteSpace,
    vertical_align: VerticalAlignValue,
    hyphen_breaks: &FxHashSet<u32>,
) {
    profile_scope!("tokens.text_glyphs");
    #[cfg(feature = "profiling")]
    let setup_scope = scope("tokens.text_run_setup");
    let letter_spacing = style.letter_spacing() as f64;
    let word_spacing = style.word_spacing() as f64;
    let word_spacing_factor = style.word_spacing_factor() as f64;
    let font_size = style.font_size();
    let authored_line_height = style.line_height();
    let uses_ahem = engine.reader.box_uses_ahem(box_idx);
    let fixed_line_height = if !style.line_height_is_normal() {
        Some(authored_line_height as f64)
    } else if !uses_ahem {
        Some(font_size as f64 * 1.2)
    } else {
        None
    };
    let visible = style.visibility() == Visibility::Visible;
    let placement_required = letter_spacing != 0.0
        || word_spacing != 0.0
        || has_aligned_inline_ancestor(engine, box_idx);
    let allows_wrap = white_space.allows_wrap();
    let preserves_spaces = white_space.preserves_spaces();
    let wrap = token_wrap(style, white_space);
    let hyphens = if style.word_break() == WordBreak::BreakAll
        || style.line_break() == LineBreak::Anywhere
    {
        Hyphens::None
    } else {
        style.hyphens()
    };
    let hyphen_glyph = (hyphens != Hyphens::None)
        .then(|| engine.text.hyphen_glyph(box_idx))
        .flatten();
    let discretionary_hyphen = hyphen_glyph.map(|glyph| {
        (glyph, engine.text.glyph_metric(glyph).advance() as f64)
    });
    let (dense, runs) = (tokens.dense.make_mut(), &mut tokens.runs);
    let mut previous_run_idx = dense.last().map(|previous| previous.run_idx());
    let mut previous_dimensions = None;

    let mut text_cursor = engine.text.cursor();
    #[cfg(feature = "profiling")]
    drop(setup_scope);
    profile_scope!("tokens.text_emit");
    for glyph_idx in glyphs {
        let glyph = engine.text.glyph_at(glyph_idx as usize).unwrap_or_default();
        let metric = text_cursor.glyph_metric(glyph).expect("registered text metric");
        let (advance, cluster_boundary) = text_cursor.character_geometry(
            glyph_idx as usize, metric.advance(),
        );
        let unit = canonical_text_unit_from_facts(
            engine,
            glyph_idx,
            metric.ch(),
            advance,
            font_size,
            letter_spacing,
            word_spacing,
            word_spacing_factor,
            white_space,
        );
        let character = unit.character;
        let width = unit.natural_advance;
        // ZERO WIDTH SPACE is a source-authored wrap opportunity, not a
        // paintable glyph. Retaining it as an Opportunity also prevents a
        // fallback font's nonzero missing-glyph advance from leaking into
        // line and intrinsic widths.
        let token_kind = if character == '\u{200b}' && allows_wrap {
            InlineTokenKind::Opportunity
        } else {
            InlineTokenKind::Glyph { glyph_idx }
        };
        let mut token = InlineToken::new(
            token_kind,
            width,
            unit.break_kind,
            // Pre-wrap space sequences hang together rather than acquiring
            // emergency breaks. Break-spaces still permits those breaks.
            if white_space == WhiteSpace::PreWrap && matches!(character, ' ' | '\t') {
                TokenWrap::Normal
            } else {
                wrap
            },
            cluster_boundary,
        );
        if character == ' ' {
            token.set_is_space();
        }
        if character == '\t' && preserves_spaces {
            token.set_is_tab();
        }
        // All other metrics are invariant for this source run. Reuse the
        // interned ID while glyph extents and preserved-tab metrics match.
        let placement_required = placement_required
            || (word_spacing_factor != 1.0 && matches!(character, ' ' | '\u{00a0}'));
        let dimensions = (metric.ascent(), metric.descent(), placement_required);
        if previous_dimensions == Some(dimensions) {
            token.set_run_idx(previous_run_idx.expect("previous text metrics"));
        } else {
            let line_height = fixed_line_height
                .unwrap_or_else(|| f64::from(metric.ascent() + metric.descent()));
            let metrics = InlineTokenMetrics {
                owner_box_idx: box_idx as u32,
                ascent: metric.ascent(),
                descent: metric.descent(),
                line_height: line_height as f32,
                font_size,
                vertical_align: InlineVerticalAlign::resolve(vertical_align, line_height, font_size),
                white_space,
                visible,
                placement_required,
            };
            previous_run_idx = Some(InlineTokens::bind_metrics_in(
                runs,
                previous_run_idx,
                &mut token,
                metrics,
            ));
            previous_dimensions = Some(dimensions);
        }
        dense.push(token);
        if let Some((hyphen_glyph, hyphen_width)) = discretionary_hyphen
            && allows_wrap
            && (hyphen_breaks.contains(&glyph_idx) || character == '\u{00ad}')
        {
            let mut discretionary = InlineToken::new(
                InlineTokenKind::Discretionary {
                    glyph: hyphen_glyph,
                },
                hyphen_width,
                BreakKind::Discretionary,
                TokenWrap::Normal,
                true,
            );
            discretionary.set_run_idx(token.run_idx());
            dense.push(discretionary);
        }
    }
}

fn create_ellipsis_token(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    glyph: crate::GlyphId,
    style: UsedStyleView,
) -> (InlineToken, InlineTokenMetrics) {
    let metric = engine.text.glyph_metric(glyph);
    let line_height = resolved_line_height(style);
    (
        InlineToken::new(
            InlineTokenKind::Ellipsis { glyph },
            metric.advance() as f64 + style.letter_spacing() as f64,
            BreakKind::None,
            TokenWrap::Normal,
            true,
        ),
        InlineTokenMetrics {
            owner_box_idx: u32::MAX,
            ascent: metric.ascent(),
            descent: metric.descent(),
            line_height: line_height as f32,
            font_size: style.font_size(),
            vertical_align: InlineVerticalAlign::Baseline,
            white_space: style.white_space(),
            visible: style.visibility() == Visibility::Visible,
            placement_required: style.letter_spacing() != 0.0,
        },
    )
}

fn token_wrap(style: UsedStyleView, white_space: WhiteSpace) -> TokenWrap {
    if !white_space.allows_wrap() {
        return TokenWrap::Normal;
    }
    if style.word_break() == WordBreak::BreakWord {
        return TokenWrap::BreakWord;
    }
    // `break-all` is resolved into Unicode opportunities during preparation;
    // using emergency character wrapping here would bypass punctuation rules.
    match style.overflow_wrap() {
        OverflowWrap::BreakWord | OverflowWrap::Anywhere => TokenWrap::BreakWord,
        OverflowWrap::Normal => TokenWrap::Normal,
    }
}
