use super::*;

#[derive(Clone, Copy)]
pub(super) struct InlineOverflow {
    pub(super) clips: bool,
    pub(super) ellipsis: Option<InlineToken>,
}

pub(super) fn inline_overflow(engine: &crate::layout::LayoutEngine<'_, '_>, container_box_idx: usize, tokens: &mut InlineTokens) -> InlineOverflow {
    let style = engine.reader.style(container_box_idx);
    let clips = style.overflow_x().clips();
    let ellipsis = (clips && style.text_overflow() == TextOverflow::Ellipsis).then(|| engine.text.ellipsis_glyph(container_box_idx)).flatten().map(|glyph| {
        let (token, metrics) = create_ellipsis_token(engine, glyph, style);
        tokens.bind_metrics(token, metrics)
    });
    InlineOverflow { clips, ellipsis }
}

fn automatic_hyphen_breaks(engine: &crate::layout::LayoutEngine<'_, '_>, glyphs: Range<u32>, language: &str) -> rustc_data_structures::fx::FxHashSet<u32> {
    let Some(primary) = language.split(['-', '_']).next().filter(|value| value.len() == 2) else { return Default::default() };
    let bytes = primary.as_bytes();
    let code = [bytes[0].to_ascii_lowercase(), bytes[1].to_ascii_lowercase()];
    let Some(language) = hypher::Lang::from_iso(code) else { return Default::default() };
    let mut breaks = rustc_data_structures::fx::FxHashSet::default();
    let mut word = String::new();
    let mut positions = Vec::<(u32, usize)>::new();
    let mut has_conditional_hyphen = false;
    let book_hyphen_quality = engine.config.hyphenation_quality();
    let flush = |word: &mut String, positions: &mut Vec<(u32, usize)>, has_conditional_hyphen: &mut bool, breaks: &mut rustc_data_structures::fx::FxHashSet<u32>| {
        let minimum_word_chars = if book_hyphen_quality { 5 } else { 4 };
        if !*has_conditional_hyphen && positions.len() >= minimum_word_chars {
            let mut byte_offset = 0usize;
            let mut syllables = hypher::hyphenate(word, language).peekable();
            while let Some(syllable) = syllables.next() {
                byte_offset += syllable.len();
                if syllables.peek().is_some()
                    && let Some((position, &(glyph_idx, _))) = positions.iter().enumerate().find(|(_, (_, end))| *end == byte_offset)
                    && (!book_hyphen_quality || automatic_hyphen_fragments_are_long_enough(position + 1, positions.len()))
                    && engine.text.is_cluster_boundary(glyph_idx as usize + 1)
                {
                    breaks.insert(glyph_idx);
                }
            }
        }
        word.clear();
        positions.clear();
        *has_conditional_hyphen = false;
    };
    for glyph_idx in glyphs {
        let glyph = engine.text.glyph_at(glyph_idx as usize).unwrap_or_default();
        let character = engine.text.glyph_metric(glyph).ch();
        if character.is_alphabetic() {
            word.push(character);
            positions.push((glyph_idx, word.len()));
        } else if character == '\u{00ad}' {
            has_conditional_hyphen = true;
        } else {
            flush(&mut word, &mut positions, &mut has_conditional_hyphen, &mut breaks);
        }
    }
    flush(&mut word, &mut positions, &mut has_conditional_hyphen, &mut breaks);
    breaks
}

pub(super) fn automatic_hyphen_fragments_are_long_enough(prefix_chars: usize, word_chars: usize) -> bool {
    prefix_chars >= 2 && word_chars.saturating_sub(prefix_chars) >= 2
}

pub(super) fn resolved_line_height(style: html_style_model::UsedStyleView<'_>) -> f64 {
    if style.line_height_is_normal() { style.font_size() as f64 * 1.2 } else { style.line_height().max(0.0) as f64 }
}

pub(super) fn append_text_tokens(
    engine: &crate::layout::LayoutEngine<'_, '_>, tokens: &mut InlineTokens, glyphs: Range<u32>, box_idx: usize, style: html_style_model::UsedStyleView, white_space: WhiteSpace, vertical_align: VerticalAlignValue,
) {
    let letter_spacing = style.letter_spacing() as f64;
    let word_spacing = style.word_spacing() as f64;
    let font_size = style.font_size();
    let authored_line_height = style.line_height();
    let tab_reference_style = tab_reference_style(engine, box_idx);
    let uses_ahem = engine.reader.box_uses_ahem(box_idx);
    let wrap = token_wrap(style, white_space);
    let hyphens = style.hyphens();
    let hyphen_glyph = (hyphens != html_style_model::Hyphens::None).then(|| engine.text.hyphen_glyph(box_idx)).flatten();
    let language = engine.reader.style(box_idx).language().and_then(|language| engine.reader.styles().string(language));
    let hyphen_breaks = (hyphens == html_style_model::Hyphens::Auto).then(|| hyphen_glyph.and_then(|_| language).map(|language| automatic_hyphen_breaks(engine, glyphs.clone(), language))).flatten().unwrap_or_default();
    let mut preserved_tab = None;
    let (InlineStorage::Owned(dense), InlineStorage::Owned(runs)) = (&mut tokens.dense, &mut tokens.runs) else { unreachable!("new inline token plans must own their construction buffers") };
    let mut previous_run_idx = dense.last().map(|previous| previous.run_idx);

    for glyph_idx in glyphs {
        let glyph = engine.text.glyph_at(glyph_idx as usize).unwrap_or_default();
        let metric = engine.text.glyph_metric(glyph);
        let line_height = if !style.line_height_is_normal() {
            authored_line_height as f64
        } else if uses_ahem {
            f64::from(metric.ascent() + metric.descent())
        } else {
            font_size as f64 * 1.2
        };
        let unit = canonical_text_unit(engine, glyph_idx, style, white_space);
        let character = unit.character;
        let width = unit.natural_advance;
        let tab_advance = if character == '\t' && white_space.preserves_spaces() { Some(*preserved_tab.get_or_insert_with(|| preserved_tab_metrics(style, tab_reference_style, uses_ahem))) } else { None };
        // ZERO WIDTH SPACE is a source-authored wrap opportunity, not a
        // paintable glyph. Retaining it as an Opportunity also prevents a
        // fallback font's nonzero missing-glyph advance from leaking into
        // line and intrinsic widths.
        let token_kind = if character == '\u{200b}' && white_space.allows_wrap() { InlineTokenKind::Opportunity } else { InlineTokenKind::Glyph { glyph_idx } };
        let mut token = InlineToken::new(token_kind, width, unit.break_kind, wrap, engine.text.is_cluster_boundary(glyph_idx as usize));
        let metrics = InlineTokenMetrics {
            owner_box_idx: box_idx as u32,
            ascent: metric.ascent(),
            descent: metric.descent(),
            line_height,
            tab_interval: tab_advance.map_or(-1.0, |tab| tab.0),
            tab_min_advance: tab_advance.map_or(0.0, |tab| tab.1),
            font_size,
            vertical_align,
            white_space,
            placement_required: letter_spacing != 0.0 || word_spacing != 0.0 || has_aligned_inline_ancestor(engine, box_idx),
        };
        previous_run_idx = Some(InlineTokens::bind_metrics_in(runs, previous_run_idx, &mut token, metrics));
        dense.push(token);
        if white_space.allows_wrap()
            && (hyphen_breaks.contains(&glyph_idx) || character == '\u{00ad}')
            && let Some(hyphen_glyph) = hyphen_glyph
        {
            let hyphen_width = engine.text.glyph_metric(hyphen_glyph).advance() as f64;
            let mut discretionary = InlineToken::new(InlineTokenKind::Discretionary { glyph: hyphen_glyph }, hyphen_width, BreakKind::Discretionary, TokenWrap::Normal, true);
            previous_run_idx = Some(InlineTokens::bind_metrics_in(runs, previous_run_idx, &mut discretionary, metrics));
            dense.push(discretionary);
        }
    }
}

fn create_ellipsis_token(engine: &crate::layout::LayoutEngine<'_, '_>, glyph: crate::GlyphId, style: html_style_model::UsedStyleView) -> (InlineToken, InlineTokenMetrics) {
    let metric = engine.text.glyph_metric(glyph);
    let line_height = resolved_line_height(style);
    (
        InlineToken::new(InlineTokenKind::Ellipsis { glyph }, metric.advance() as f64 + style.letter_spacing() as f64, BreakKind::None, TokenWrap::Normal, true),
        InlineTokenMetrics {
            owner_box_idx: u32::MAX,
            ascent: metric.ascent(),
            descent: metric.descent(),
            line_height,
            tab_interval: -1.0,
            tab_min_advance: 0.0,
            font_size: style.font_size(),
            vertical_align: VerticalAlignValue::Baseline,
            white_space: style.white_space(),
            placement_required: style.letter_spacing() != 0.0,
        },
    )
}

fn token_wrap(style: html_style_model::UsedStyleView, white_space: WhiteSpace) -> TokenWrap {
    if !white_space.allows_wrap() {
        return TokenWrap::Normal;
    }
    if style.word_break() == WordBreak::BreakAll {
        return TokenWrap::Anywhere;
    }
    match style.overflow_wrap() {
        OverflowWrap::BreakWord | OverflowWrap::Anywhere => TokenWrap::BreakWord,
        OverflowWrap::Normal => TokenWrap::Normal,
    }
}
