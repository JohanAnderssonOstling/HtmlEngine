//! CSS whitespace normalization and text transformation.

use super::line_breaks::{CjkWritingSystem, is_unspaced_cjk};
use crate::source_offsets::SourceOffsets;
use crate::{
    InlineContent, InlineItemKind, TextStructure, WhitespaceWrapOverride, is_bidi_control,
};
use html_style_model::{ComputedStyles, LanguageId, StyleIndices, StyleView, TextTransform};
use html_style_model::{LineBreak, WordBreak};
use icu_casemap::{
    CaseMapper,
    options::{LeadingAdjustment, TitlecaseOptions, TrailingCase},
};
use icu_properties::{
    CodePointMapData, CodePointSetData,
    props::{Cased, CaseIgnorable, EastAsianWidth, Emoji},
};
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use std::mem::size_of;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_categories::UnicodeCategories;

/// Conservative unbreaking around punctuation in unspaced East Asian
/// writing systems. Full/half-width punctuation already encodes that context;
/// ambiguous punctuation needs an unambiguous wide neighbour.
fn joins_east_asian_punctuation(left: char, right: char) -> bool {
    if left == '\u{3000}' || right == '\u{3000}' { return true; }
    let widths = CodePointMapData::<EastAsianWidth>::new();
    let emoji = CodePointSetData::new::<Emoji>();
    let wide = |ch| !emoji.contains(ch) && is_unspaced_cjk(ch)
        && matches!(widths.get(ch), EastAsianWidth::Wide | EastAsianWidth::Fullwidth | EastAsianWidth::Halfwidth);
    let punctuation = |ch: char, other| (ch.is_punctuation() || ch.is_symbol())
        && !ch.is_symbol_currency()
        && !emoji.contains(ch)
        && (matches!(widths.get(ch), EastAsianWidth::Wide | EastAsianWidth::Fullwidth | EastAsianWidth::Halfwidth)
            || (widths.get(ch) == EastAsianWidth::Ambiguous && wide(other)));
    punctuation(left, right) || punctuation(right, left)
}

struct PendingSpace {
    run: usize,
    source: usize,
    offset: u32,
    wrap_before: WhitespaceWrapOverride,
    segment_break: bool,
}

struct PendingControl {
    run: u32,
    source: u32,
}

enum NormalizationMap {
    Characters {
        remap: Vec<u32>,
        pending_controls: Vec<usize>,
    },
    Runs {
        ranges: Vec<Range<u32>>,
        pending_controls: Vec<PendingControl>,
    },
}

impl NormalizationMap {
    fn new(characters: usize, runs: usize) -> Self {
        // Only run endpoints survive normalization. Keep the dense form for
        // unusually fragmented input. Compact control entries must also fit
        // the original source-only queue's element size on this target.
        if runs <= characters / 2
            && u32::try_from(runs).is_ok()
            && size_of::<PendingControl>() <= size_of::<usize>()
        {
            Self::Runs {
                ranges: vec![0..0; runs],
                pending_controls: Vec::new(),
            }
        } else {
            Self::Characters {
                remap: vec![u32::MAX; characters],
                pending_controls: Vec::new(),
            }
        }
    }

    fn record(&mut self, run: usize, source: usize, destination: u32) {
        match self {
            Self::Characters { remap, .. } => remap[source] = destination,
            Self::Runs { ranges, .. } => {
                let range = &mut ranges[run];
                if range.is_empty() {
                    range.start = destination;
                }
                range.end = destination + 1;
            }
        }
    }

    fn range(&self, run: usize, source: Range<u32>) -> Range<u32> {
        match self {
            Self::Runs { ranges, .. } => ranges[run].clone(),
            Self::Characters { remap, .. } => {
                let mut mapped = source.filter_map(|index| {
                    remap
                        .get(index as usize)
                        .copied()
                        .filter(|mapped| *mapped != u32::MAX)
                });
                match mapped.next() {
                    Some(start) => start..mapped.next_back().unwrap_or(start) + 1,
                    None => 0..0,
                }
            }
        }
    }

    fn defer_control(&mut self, run: usize, source: usize) {
        match self {
            Self::Characters { pending_controls, .. } => pending_controls.push(source),
            Self::Runs { pending_controls, .. } => pending_controls.push(PendingControl {
                run: u32::try_from(run).expect("compact mapping checked the run count"),
                source: u32::try_from(source).expect("source index comes from a u32 glyph range"),
            }),
        }
    }

    fn pending_control(&self, index: usize) -> Option<(usize, usize)> {
        match self {
            // Dense mapping uses only the source position; record ignores run.
            Self::Characters { pending_controls, .. } => {
                pending_controls.get(index).map(|&source| (0, source))
            }
            Self::Runs { pending_controls, .. } => pending_controls
                .get(index)
                .map(|control| (control.run as usize, control.source as usize)),
        }
    }

    fn clear_pending_controls(&mut self) {
        match self {
            Self::Characters { pending_controls, .. } => pending_controls.clear(),
            Self::Runs { pending_controls, .. } => pending_controls.clear(),
        }
    }
}

struct NormalizedText {
    glyphs: Vec<char>,
    source_offsets: SourceOffsets,
    wraps: Vec<WhitespaceWrapOverride>,
    remap: NormalizationMap,
}

impl NormalizedText {
    fn new(capacity: usize, runs: usize) -> Self {
        Self {
            glyphs: Vec::with_capacity(capacity),
            source_offsets: SourceOffsets::with_capacity(capacity),
            wraps: Vec::with_capacity(capacity),
            remap: NormalizationMap::new(capacity, runs),
        }
    }

    fn push(
        &mut self,
        run: usize,
        source: usize,
        character: char,
        offset: u32,
        wrap: WhitespaceWrapOverride,
    ) {
        self.remap.record(run, source, self.glyphs.len() as u32);
        self.glyphs.push(character);
        self.source_offsets.push(offset);
        self.wraps.push(wrap);
    }

    fn finish_pending(&mut self, pending: PendingSpace, keep_space: bool, content: &InlineContent) {
        if keep_space {
            self.push(
                pending.run,
                pending.source,
                ' ',
                pending.offset,
                pending.wrap_before,
            );
        }
        // Retain source order whether the preceding space survives or not.
        let mut index = 0;
        while let Some((run, source)) = self.remap.pending_control(index) {
            self.push(
                run,
                source,
                content.character_at(source).expect("pending control glyph"),
                content.raw_glyph_source_offset(source),
                WhitespaceWrapOverride::Style,
            );
            index += 1;
        }
        // Retain capacity for later pending-whitespace sequences.
        self.remap.clear_pending_controls();
    }
}

pub fn collapse_whitespace(
    styles: &ComputedStyles,
    layout_tree: &(impl TextStructure + ?Sized),
    inline_content: &mut InlineContent,
) -> FxHashSet<usize> {
    let mut unicode_roots = FxHashSet::default();
    let mut output = NormalizedText::new(
        inline_content.characters().len(),
        inline_content.text_runs().count(),
    );
    let mut at_line_start = true;
    let mut pending_space: Option<PendingSpace> = None;
    let mut current_context = None;
    let mut suspended_contexts = Vec::new();
    // Limit joining lookbehind to this flow, excluding atomic descendants and
    // previous blocks. Ordinary characters need no additional bookkeeping.
    let mut context_start = 0;
    let mut next_run = 0;

    for run in inline_content.logical_flow_items() {
        let context = run.flow_root(layout_tree);
        if current_context != Some(context) {
            let enters_descendant = current_context.is_some_and(|previous| {
                layout_box_is_descendant_of(layout_tree, context, previous)
            });
            // A collapsed space committed before a nested flow must also
            // collapse with leading whitespace when the parent resumes.
            let resumes_after_space = enters_descendant && pending_space.is_some();
            if let Some(pending) = pending_space.take() {
                // Atomic descendants precede their carrier in the run arena;
                // commit the parent's pending space before those descendants.
                // Sibling flows instead discard trailing collapsible space.
                output.finish_pending(pending, enters_descendant, inline_content);
            }
            // Out-of-flow text has its own whitespace state. Returning to the
            // parent flow must not erase the space after an absolute inline.
            at_line_start = if enters_descendant {
                suspended_contexts.push((current_context.expect("parent context"), at_line_start || resumes_after_space));
                true
            } else if let Some(index) = suspended_contexts.iter().rposition(|&(root, _)| root == context) {
                let (_, was_line_start) = suspended_contexts[index];
                suspended_contexts.truncate(index);
                was_line_start
            } else {
                suspended_contexts.clear();
                true
            };
            context_start = output.glyphs.len();
            current_context = Some(context);
        }
        let (InlineItemKind::Text { glyphs: range } | InlineItemKind::Marker { glyphs: range }) =
            &run.kind
        else {
            let is_break = match run.kind {
                InlineItemKind::Image { .. } | InlineItemKind::AtomicBox { .. } => false,
                InlineItemKind::Break { .. } => true,
                _ => unreachable!("non-textual boundaries were filtered before context selection"),
            };
            if let Some(pending) = pending_space.take() {
                output.finish_pending(pending, !is_break, inline_content);
            }
            at_line_start = is_break;
            context_start = output.glyphs.len();
            continue;
        };
        let run_index = next_run;
        next_run += 1;
        let style = get_style(styles, layout_tree, run.box_idx as usize);
        let white_space = style.white_space();
        // A branch-free integer reduction vectorizes for Latin runs. Detailed
        // classification stays outside the per-character whitespace loop.
        let characters = &inline_content.characters()[range.start as usize..range.end as usize];
        let character_bits = characters
            .iter()
            .fold(0, |bits, &character| bits | character as u32);
        let unicode_breaks = style.word_break() == WordBreak::BreakAll
            || style.line_break() == LineBreak::Anywhere
            || characters.iter().copied().any(super::line_breaks::needs_unicode_breaks);
        let break_root = if matches!(run.kind, InlineItemKind::Marker { .. }) {
            run.box_idx as usize
        } else {
            context
        };
        if unicode_breaks {
            unicode_roots.insert(break_root);
        }
        let korean = unicode_breaks
            && CjkWritingSystem::for_language(style.language().and_then(|id| styles.language(id)))
                == CjkWritingSystem::Korean;
        let collapse = white_space.collapses_spaces();
        let preserve_newlines = white_space.preserves_newlines();

        for i in range.clone() {
            let i = i as usize;
            let mut character = inline_content.character_at(i).unwrap_or('?');
            let offset = inline_content.raw_glyph_source_offset(i);
            // Bidi controls are retained for shaping and addressing, but are
            // transparent to whitespace collapsing, line starts and joining.
            if character_bits >= 0x00ad && (is_bidi_control(character)
                || matches!(character, '\u{00ad}' | '\u{200c}' | '\u{200d}')
                || matches!(character as u32, 0xfe00..=0xfe0f | 0xe0100..=0xe01ef)) {
                if pending_space.is_some() {
                    output.remap.defer_control(run_index, i);
                } else {
                    output.push(run_index, i, character, offset, WhitespaceWrapOverride::Style);
                }
                continue;
            }
            // A CR that survives markup preprocessing (for example from a
            // character reference) is a CSS space, not a segment break.
            if character == '\r' {
                character = ' ';
            }
            if super::is_mandatory_line_break(character) || (character == '\n' && preserve_newlines) {
                if let Some(pending) = pending_space.take() {
                    output.finish_pending(pending, false, inline_content);
                }
                output.push(run_index, i, character, offset, WhitespaceWrapOverride::Style);
                at_line_start = true;
                context_start = output.glyphs.len();
            } else if character == '\n' || is_css_collapsible_space(character) {
                if collapse && !at_line_start {
                    let pending = pending_space.get_or_insert(PendingSpace {
                        run: run_index,
                        source: i,
                        offset,
                        wrap_before: WhitespaceWrapOverride::Style,
                        segment_break: false,
                    });
                    if white_space.allows_wrap() {
                        pending.wrap_before = WhitespaceWrapOverride::Allow;
                    }
                    pending.segment_break |= character == '\n';
                } else if !collapse && character != '\n' {
                    if let Some(mut pending) = pending_space.take() {
                        // Collapsed space next to preserved space must not
                        // introduce a break into an unwrappable sequence.
                        pending.wrap_before = WhitespaceWrapOverride::Suppress;
                        output.finish_pending(pending, true, inline_content);
                    }
                    output.push(run_index, i, character, offset, WhitespaceWrapOverride::Style);
                    at_line_start = false;
                }
            } else {
                if let Some(pending) = pending_space.take() {
                    let previous = output.glyphs[context_start..].iter().rev().copied()
                        .find(|&character| !is_bidi_control(character)
                            && !matches!(character, '\u{00ad}' | '\u{200c}' | '\u{200d}')
                            && !matches!(character as u32, 0xfe00..=0xfe0f | 0xe0100..=0xe01ef));
                    let join = pending.segment_break
                        && (character == '\u{200b}'
                            || previous == Some('\u{200b}')
                            || (!korean && is_unspaced_cjk(character)
                                && previous.is_some_and(is_unspaced_cjk))
                            || (CjkWritingSystem::for_language(style.language().and_then(|id| styles.language(id)))
                                == CjkWritingSystem::ChineseJapanese
                                && previous.is_some_and(|previous| joins_east_asian_punctuation(previous, character))));
                    output.finish_pending(pending, !join, inline_content);
                }
                output.push(run_index, i, character, offset, WhitespaceWrapOverride::Style);
                at_line_start = false;
            }
        }
    }
    if let Some(pending) = pending_space {
        output.finish_pending(pending, false, inline_content);
    }

    let mut run_index = 0;
    for run in inline_content.inline_items_mut() {
        let (InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs }) = &mut run.kind
        else {
            continue;
        };
        *glyphs = output.remap.range(run_index, glyphs.clone());
        run_index += 1;
    }
    inline_content.replace_characters(output.glyphs, output.source_offsets, output.wraps);
    unicode_roots
}

/// Applies CSS case conversion to the normalized character stream before any
/// shaping spans are selected. Case conversion is not necessarily one-to-one
/// (`ß` uppercases to `SS`), so this pass rebuilds the dense stream and keeps
/// every generated scalar attached to the originating DOM source offset.
pub fn transform_text(
    styles: &ComputedStyles,
    layout_tree: &(impl TextStructure + ?Sized),
    inline_content: &mut InlineContent,
    first_letter_styles: &mut Vec<Option<StyleIndices>>,
) {
    let has_transform = inline_content.text_runs().any(|run| {
        !run.glyphs.is_empty()
            && get_style(styles, layout_tree, run.box_idx as usize).text_transform()
                != TextTransform::None
    }) || first_letter_styles.iter().flatten().any(|&indices| {
        styles
            .view(indices)
            .is_some_and(|style| style.text_transform() != TextTransform::None)
    });
    if !has_transform {
        return;
    }
    let old_len = inline_content.characters().len();
    let mut transforms = vec![TextTransform::None; old_len];
    let mut languages = vec![None; old_len];
    for run in inline_content.text_runs() {
        let base_style = get_style(styles, layout_tree, run.box_idx as usize);
        for glyph_idx in run.glyphs.clone() {
            let index = glyph_idx as usize;
            let style = first_letter_styles
                .get(index)
                .copied()
                .flatten()
                .and_then(|indices| styles.view(indices))
                .unwrap_or(base_style);
            transforms[index] = style.text_transform();
            languages[index] = style.language();
        }
    }
    if transforms
        .iter()
        .all(|transform| *transform == TextTransform::None)
    {
        return;
    }

    let (capitalize_starts, final_sigmas) = (transforms.contains(&TextTransform::Capitalize)
        || inline_content.characters().contains(&'Σ'))
        .then(|| casing_context(styles, layout_tree, inline_content, &languages))
        .unwrap_or_default();
    let mut new_glyphs = Vec::with_capacity(old_len);
    let mut new_source_offsets = SourceOffsets::with_capacity(old_len);
    let mut new_whitespace_wrap_before = Vec::with_capacity(old_len);
    let has_first_letter_styles = !first_letter_styles.is_empty();
    let mut new_first_letter_styles =
        Vec::with_capacity(if has_first_letter_styles { old_len } else { 0 });
    let mut glyph_boundaries = Vec::with_capacity(old_len + 1);
    glyph_boundaries.push(0);

    let contextual_uppercase = greek_uppercase_mappings(
        styles, layout_tree, inline_content, &transforms, &languages,
    );
    for (index, transform) in transforms.into_iter().enumerate() {
        let character = inline_content.character_at(index).unwrap_or('?');
        let source_offset = inline_content.raw_glyph_source_offset(index);
        let wrap_override = inline_content.whitespace_wrap_before(index);
        let first_letter_style = first_letter_styles.get(index).copied().flatten();
        let mut first_output = true;
        let mut append = |transformed: char| {
            new_glyphs.push(transformed);
            new_source_offsets.push(source_offset);
            new_whitespace_wrap_before.push(if first_output {
                wrap_override
            } else {
                WhitespaceWrapOverride::Style
            });
            if has_first_letter_styles {
                new_first_letter_styles.push(first_letter_style);
            }
            first_output = false;
        };

        match transform {
            TextTransform::Uppercase => {
                if let Some(mapped) = contextual_uppercase.get(&index) {
                    mapped.iter().copied().for_each(&mut append);
                } else {
                    append_css_uppercase(character, &mut append);
                }
            }
            TextTransform::Lowercase if final_sigmas.contains(&index) => append('ς'),
            TextTransform::Lowercase => character.to_lowercase().for_each(&mut append),
            TextTransform::Capitalize if capitalize_starts.contains(&index) => {
                append_css_titlecase(character, &mut append)
            }
            TextTransform::Capitalize | TextTransform::None => append(character),
        }
        glyph_boundaries.push(new_glyphs.len() as u32);
    }

    for item in inline_content.inline_items_mut() {
        let (InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs }) = &mut item.kind
        else {
            continue;
        };
        if glyphs.start >= glyphs.end {
            *glyphs = 0..0;
            continue;
        }
        let old_start = glyphs.start as usize;
        let old_end = glyphs.end as usize;
        *glyphs = glyph_boundaries[old_start]..glyph_boundaries[old_end];
    }
    inline_content.replace_characters(new_glyphs, new_source_offsets, new_whitespace_wrap_before);
    *first_letter_styles = new_first_letter_styles;
}

/// Greek uppercase tailoring can remove accents and add a diaeresis based on
/// neighboring letters. Shape the whole language/transform span, then retain
/// source ownership at extended grapheme boundaries rather than guessing a
/// one-scalar-to-one-scalar mapping.
fn greek_uppercase_mappings(
    styles: &ComputedStyles,
    tree: &(impl TextStructure + ?Sized),
    content: &InlineContent,
    transforms: &[TextTransform],
    languages: &[Option<LanguageId>],
) -> FxHashMap<usize, Vec<char>> {
    let greek = |index: usize| {
        languages[index]
            .and_then(|id| styles.language(id))
            .and_then(|language| language.tag())
            .is_some_and(|tag| tag.primary_language() == "el")
    };
    if !(0..transforms.len()).any(|index| transforms[index] == TextTransform::Uppercase && greek(index)) {
        return FxHashMap::default();
    }
    fn publish(text: &mut String, sources: &mut Vec<usize>, mappings: &mut FxHashMap<usize, Vec<char>>) {
        if text.is_empty() { return; }
        let upper = CaseMapper::new().uppercase_to_string(text, &"el".parse().expect("valid Greek language"));
        let mut output = upper.graphemes(true);
        let mut pending = Vec::new();
        let mut source = 0;
        let mut valid = true;
        for grapheme in text.graphemes(true) {
            // Tailoring changes diacritics within a grapheme. The number of
            // base-letter graphemes, including full-uppercase expansions of
            // ypogegrammeni, follows the root full uppercase mapping.
            let root: String = grapheme.chars().flat_map(char::to_uppercase).collect();
            let mut mapped = Vec::new();
            for _ in root.graphemes(true) {
                if let Some(grapheme) = output.next() {
                    mapped.extend(grapheme.chars());
                } else {
                    valid = false;
                    break;
                }
            }
            let count = grapheme.chars().count();
            pending.push((sources[source], mapped));
            for &index in &sources[source + 1..source + count] {
                pending.push((index, Vec::new()));
            }
            source += count;
        }
        // Keep the scalar mapping as a safe fallback if a future Unicode
        // version adds a tailoring with different grapheme expansion rules.
        if valid && output.next().is_none() {
            mappings.extend(pending);
        }
        text.clear();
        sources.clear();
    }
    let mut mappings = FxHashMap::default();
    let mut text = String::new();
    let mut sources = Vec::new();
    let mut context = None;
    for item in content.logical_flow_items() {
        let root = item.flow_root(tree);
        if context != Some(root) {
            publish(&mut text, &mut sources, &mut mappings);
            context = Some(root);
        }
        match &item.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => {
                for index in glyphs.clone().map(|index| index as usize) {
                    if transforms[index] == TextTransform::Uppercase && greek(index) {
                        sources.push(index);
                        text.push(content.character_at(index).expect("source glyph"));
                    } else {
                        publish(&mut text, &mut sources, &mut mappings);
                    }
                }
            }
            InlineItemKind::InlineBoundary { .. } | InlineItemKind::AbsoluteAnchor { .. }
                | InlineItemKind::FloatAnchor { .. } => {},
            _ => publish(&mut text, &mut sources, &mut mappings),
        }
    }
    publish(&mut text, &mut sources, &mut mappings);
    mappings
}

fn append_css_uppercase(character: char, append: &mut impl FnMut(char)) {
    // The pinned CSS2 oracle treats Georgian Mkhedruli as a unicase script.
    // Keep that compatibility mapping explicit instead of inheriting changes
    // in the Rust toolchain's Unicode data version.
    if ('\u{10d0}'..='\u{10ff}').contains(&character) {
        append(character);
    } else {
        character.to_uppercase().for_each(append);
    }
}

fn append_css_titlecase(character: char, append: &mut impl FnMut(char)) {
    if ('\u{10d0}'..='\u{10ff}').contains(&character) {
        append(character);
        return;
    }
    let mut utf8 = [0; 4];
    let mut options = TitlecaseOptions::default();
    options.leading_adjustment = Some(LeadingAdjustment::None);
    options.trailing_case = Some(TrailingCase::Unchanged);
    CaseMapper::new()
        .titlecase_segment_with_only_case_data_to_string(
            character.encode_utf8(&mut utf8),
            &"und".parse().expect("valid root language"),
            options,
        )
        .chars()
        .for_each(append);
}

/// Returns glyph indices containing the first typographic letter of each
/// Unicode word. Formatting boundaries are deliberately absent from the word
/// string, while forced breaks and atomic inline content introduce separators.
fn casing_context(
    styles: &ComputedStyles,
    layout_tree: &(impl TextStructure + ?Sized),
    inline_content: &InlineContent,
    languages: &[Option<LanguageId>],
) -> (FxHashSet<usize>, FxHashSet<usize>) {
    fn publish(
        text: &mut String,
        glyphs_by_byte: &mut Vec<(usize, usize)>,
        starts: &mut FxHashSet<usize>,
        final_sigmas: &mut FxHashSet<usize>,
        styles: &ComputedStyles,
        languages: &[Option<LanguageId>],
    ) {
        for (word_start, word) in text.unicode_word_indices() {
            let Some((relative, _)) = word
                .char_indices()
                .find(|(_, character)| character.is_alphabetic())
            else {
                continue;
            };
            let letter_byte = word_start + relative;
            if let Ok(position) =
                glyphs_by_byte.binary_search_by_key(&letter_byte, |(byte, _)| *byte)
            {
                starts.insert(glyphs_by_byte[position].1);
                let source = glyphs_by_byte[position].1;
                let dutch = languages[source]
                    .and_then(|id| styles.language(id))
                    .and_then(|language| language.tag())
                    .is_some_and(|tag| tag.primary_language() == "nl");
                let mut letters = word[relative..].chars();
                if dutch
                    && matches!(letters.next(), Some('i' | 'I'))
                    && matches!(letters.next(), Some('j' | 'J'))
                    && let Some(&(_, next)) = glyphs_by_byte.get(position + 1)
                {
                    starts.insert(next);
                }
            }
        }
        let cased = CodePointSetData::new::<Cased>();
        let ignorable = CodePointSetData::new::<CaseIgnorable>();
        for (byte, character) in text.char_indices() {
            if character != 'Σ' {
                continue;
            }
            let before = text[..byte]
                .chars()
                .rev()
                .find(|&ch| !ignorable.contains(ch));
            let after = text[byte + character.len_utf8()..]
                .chars()
                .find(|&ch| !ignorable.contains(ch));
            if before.is_some_and(|ch| cased.contains(ch))
                && !after.is_some_and(|ch| cased.contains(ch))
                && let Ok(position) = glyphs_by_byte.binary_search_by_key(&byte, |(byte, _)| *byte)
            {
                final_sigmas.insert(glyphs_by_byte[position].1);
            }
        }
        text.clear();
        glyphs_by_byte.clear();
    }

    let mut starts = FxHashSet::default();
    let mut final_sigmas = FxHashSet::default();
    let mut text = String::new();
    let mut glyphs_by_byte = Vec::new();
    let mut current_context = None;
    for item in inline_content.logical_flow_items() {
        let context = item.flow_root(layout_tree);
        if current_context != Some(context) {
            publish(
                &mut text,
                &mut glyphs_by_byte,
                &mut starts,
                &mut final_sigmas,
                styles,
                languages,
            );
            current_context = Some(context);
        }
        match &item.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => {
                for glyph_idx in glyphs.clone() {
                    let character = inline_content
                        .character_at(glyph_idx as usize)
                        .unwrap_or('?');
                    glyphs_by_byte.push((text.len(), glyph_idx as usize));
                    text.push(character);
                }
            }
            InlineItemKind::Break { .. }
            | InlineItemKind::Image { .. }
            | InlineItemKind::AtomicBox { .. } => text.push('\n'),
            InlineItemKind::FloatAnchor { .. }
            | InlineItemKind::AbsoluteAnchor { .. }
            | InlineItemKind::InlineBoundary { .. } => {
                unreachable!("non-textual boundaries are filtered before context selection")
            }
        }
    }
    publish(
        &mut text,
        &mut glyphs_by_byte,
        &mut starts,
        &mut final_sigmas,
        styles,
        languages,
    );
    (starts, final_sigmas)
}

/// CSS white-space collapsing applies to document spaces and tabs, not every
/// Unicode character classified as whitespace. In particular, typographic
/// spaces such as EN QUAD (U+2000) and IDEOGRAPHIC SPACE (U+3000) remain
/// visible under `white-space: normal`.
pub fn is_css_collapsible_space(character: char) -> bool {
    matches!(character, ' ' | '\t')
}

pub fn whitespace_context_root(
    layout_tree: &(impl TextStructure + ?Sized),
    mut box_idx: usize,
) -> usize {
    while layout_tree.is_inline(box_idx) {
        let Some(parent) = layout_tree.get_box_parent(box_idx) else {
            break;
        };
        box_idx = parent;
    }
    box_idx
}

fn layout_box_is_descendant_of(
    layout_tree: &(impl TextStructure + ?Sized),
    mut box_idx: usize,
    ancestor: usize,
) -> bool {
    while let Some(parent) = layout_tree.get_box_parent(box_idx) {
        if parent == ancestor {
            return true;
        }
        box_idx = parent;
    }
    false
}

pub fn get_style<'a>(
    styles: &'a ComputedStyles,
    layout_tree: &(impl TextStructure + ?Sized),
    box_idx: usize,
) -> StyleView<'a> {
    let indices = layout_tree
        .get_box_style_indices(box_idx)
        .unwrap_or_else(|| styles.default_indices());
    styles.view(indices).expect("validated style handle")
}
