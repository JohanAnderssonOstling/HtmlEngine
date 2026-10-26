//! CSS whitespace normalization and text transformation.

use super::line_breaks::{CjkWritingSystem, is_unspaced_cjk};
use super::*;
use crate::layout_model::SourceOffsets;
use html_style_model::{LineBreak, WordBreak};

struct PendingSpace {
    source: usize,
    offset: u32,
    wrap_before: WhitespaceWrapOverride,
    segment_break: bool,
}

impl PendingSpace {
    fn new(source: usize, offset: u32, wrap_before: bool, segment_break: bool) -> Self {
        Self {
            source,
            offset,
            wrap_before: WhitespaceWrapOverride::retained(wrap_before),
            segment_break,
        }
    }
}

struct NormalizedText {
    glyphs: Vec<GlyphId>,
    source_offsets: SourceOffsets,
    wraps: Vec<WhitespaceWrapOverride>,
    remap: Vec<u32>,
    // Allocated only for controls following pending whitespace; reuse this
    // scratch buffer when later whitespace sequences contain controls too.
    pending_controls: Vec<usize>,
}

impl NormalizedText {
    fn new(capacity: usize) -> Self {
        Self {
            glyphs: Vec::with_capacity(capacity),
            source_offsets: SourceOffsets::with_capacity(capacity),
            wraps: Vec::with_capacity(capacity),
            remap: vec![u32::MAX; capacity],
            pending_controls: Vec::new(),
        }
    }

    fn push(
        &mut self,
        source: usize,
        character: GlyphId,
        offset: u32,
        wrap: WhitespaceWrapOverride,
    ) {
        self.remap[source] = self.glyphs.len() as u32;
        self.glyphs.push(character);
        self.source_offsets.push(offset);
        self.wraps.push(wrap);
    }

    fn finish_pending(&mut self, pending: PendingSpace, keep_space: bool, content: &InlineContent) {
        if keep_space {
            self.push(
                pending.source,
                ' ' as GlyphId,
                pending.offset,
                pending.wrap_before,
            );
        }
        // Retain source order whether the preceding space survives or not.
        for index in 0..self.pending_controls.len() {
            let source = self.pending_controls[index];
            self.push(
                source,
                content.character_at(source).expect("pending control glyph"),
                content.raw_glyph_source_offset(source),
                WhitespaceWrapOverride::Style,
            );
        }
        self.pending_controls.clear();
    }
}

pub(super) fn collapse_whitespace(
    styles: &ComputedStyles,
    layout_tree: &LayoutTree,
    inline_content: &mut InlineContent,
) -> FxHashSet<usize> {
    let mut unicode_roots = FxHashSet::default();
    let mut output = NormalizedText::new(inline_content.characters().len());
    let mut at_line_start = true;
    let mut pending_space: Option<PendingSpace> = None;
    let mut current_context = None;
    // Limit joining lookbehind to this flow, excluding atomic descendants and
    // previous blocks. Ordinary characters need no additional bookkeeping.
    let mut context_start = 0;

    let inline_items = inline_content.inline_items().to_vec();
    for run in &inline_items {
        // Formatting-only boundaries and out-of-flow anchors do not affect
        // the inline text stream or its whitespace context.
        if matches!(
            run.kind,
            InlineItemKind::FloatAnchor { .. }
                | InlineItemKind::AbsoluteAnchor { .. }
                | InlineItemKind::InlineBoundary { .. }
        ) {
            continue;
        }
        let context_box = match &run.kind {
            InlineItemKind::Image { .. } | InlineItemKind::AtomicBox { .. } => layout_tree
                .get_box_parent(run.box_idx as usize)
                .unwrap_or(run.box_idx as usize),
            _ => run.box_idx as usize,
        };
        let context = whitespace_context_root(layout_tree, context_box);
        if current_context != Some(context) {
            if let Some(pending) = pending_space.take() {
                // Atomic descendants precede their carrier in the run arena;
                // commit the parent's pending space before those descendants.
                // Sibling flows instead discard trailing collapsible space.
                let enters_descendant = current_context.is_some_and(|previous| {
                    layout_box_is_descendant_of(layout_tree, context, previous)
                });
                output.finish_pending(pending, enters_descendant, inline_content);
            }
            at_line_start = true;
            context_start = output.glyphs.len();
            current_context = Some(context);
        }
        let (InlineItemKind::Text { glyphs: range } | InlineItemKind::Marker { glyphs: range }) =
            &run.kind
        else {
            match run.kind {
                InlineItemKind::Image { .. } | InlineItemKind::AtomicBox { .. } => {
                    if let Some(pending) = pending_space.take() {
                        output.finish_pending(pending, true, inline_content);
                    }
                    at_line_start = false;
                    context_start = output.glyphs.len();
                }
                InlineItemKind::Break { .. } => {
                    if let Some(pending) = pending_space.take() {
                        output.finish_pending(pending, false, inline_content);
                    }
                    at_line_start = true;
                    context_start = output.glyphs.len();
                }
                _ => unreachable!("non-textual boundaries were filtered before context selection"),
            }
            continue;
        };
        let style = get_style(styles, layout_tree, run.box_idx as usize);
        let white_space = style.white_space();
        // A branch-free integer reduction vectorizes for Latin runs. Detailed
        // classification stays outside the per-character whitespace loop.
        let characters = &inline_content.characters()[range.start as usize..range.end as usize];
        let character_bits = characters
            .iter()
            .fold(0, |bits, &character| bits | character);
        let unicode_breaks = style.word_break() == WordBreak::BreakAll
            || style.line_break() == LineBreak::Anywhere
            || (character_bits >= 0x1100
                && characters
                    .iter()
                    .copied()
                    .filter_map(char::from_u32)
                    .any(super::line_breaks::is_cjk));
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
            let char_code = inline_content.character_at(i).unwrap_or('?' as GlyphId);
            let mut character = char::from_u32(char_code).unwrap_or('?');
            let offset = inline_content.raw_glyph_source_offset(i);
            // Bidi controls are retained for shaping and addressing, but are
            // transparent to whitespace collapsing, line starts and joining.
            if character_bits >= 0x061c && is_bidi_control(character) {
                if pending_space.is_some() {
                    output.pending_controls.push(i);
                } else {
                    output.push(i, char_code, offset, WhitespaceWrapOverride::Style);
                }
                continue;
            }
            // Character references can introduce CR after HTML preprocessing.
            if character == '\r' {
                let followed_by_lf = (i + 1) < range.end as usize
                    && inline_content.character_at(i + 1).and_then(char::from_u32) == Some('\n');
                if followed_by_lf {
                    continue;
                }
                character = '\n';
            }
            if character == '\n' {
                if preserve_newlines {
                    if let Some(pending) = pending_space.take() {
                        output.finish_pending(pending, false, inline_content);
                    }
                    output.push(i, '\n' as GlyphId, offset, WhitespaceWrapOverride::Style);
                    at_line_start = true;
                    context_start = output.glyphs.len();
                } else if collapse && !at_line_start {
                    if let Some(pending) = pending_space.as_mut() {
                        if white_space.allows_wrap() {
                            pending.wrap_before = WhitespaceWrapOverride::Allow;
                        }
                        pending.segment_break = true;
                    } else {
                        pending_space = Some(PendingSpace::new(
                            i,
                            offset,
                            white_space.allows_wrap(),
                            true,
                        ));
                    }
                }
            } else if is_css_collapsible_space(character) {
                if collapse {
                    if !at_line_start {
                        if let Some(pending) = pending_space.as_mut() {
                            if white_space.allows_wrap() {
                                pending.wrap_before = WhitespaceWrapOverride::Allow;
                            }
                        } else {
                            pending_space = Some(PendingSpace::new(
                                i,
                                offset,
                                white_space.allows_wrap(),
                                false,
                            ));
                        }
                    }
                } else {
                    if let Some(mut pending) = pending_space.take() {
                        // Collapsed space next to preserved space must not
                        // introduce a break into an unwrappable sequence.
                        pending.wrap_before = WhitespaceWrapOverride::Suppress;
                        output.finish_pending(pending, true, inline_content);
                    }
                    output.push(i, char_code, offset, WhitespaceWrapOverride::Style);
                    at_line_start = false;
                }
            } else {
                if let Some(pending) = pending_space.take() {
                    let join = pending.segment_break
                        && !korean
                        && is_unspaced_cjk(character)
                        && output.glyphs[context_start..].iter().rev().copied()
                            .filter_map(char::from_u32)
                            .find(|&character| !is_bidi_control(character)
                                && !matches!(character as u32, 0xfe00..=0xfe0f | 0xe0100..=0xe01ef))
                            .is_some_and(is_unspaced_cjk);
                    output.finish_pending(pending, !join, inline_content);
                }
                output.push(i, char_code, offset, WhitespaceWrapOverride::Style);
                at_line_start = false;
            }
        }
    }
    if let Some(pending) = pending_space {
        output.finish_pending(pending, false, inline_content);
    }

    for run in inline_content.inline_items_mut() {
        let (InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs }) = &mut run.kind
        else {
            continue;
        };
        let old_start = glyphs.start as usize;
        let old_end = glyphs.end as usize;
        let new_start = (old_start..old_end).find_map(|i| {
            output
                .remap
                .get(i)
                .copied()
                .filter(|mapped| *mapped != u32::MAX)
        });
        let new_end = (old_start..old_end)
            .filter_map(|i| {
                output
                    .remap
                    .get(i)
                    .copied()
                    .filter(|mapped| *mapped != u32::MAX)
            })
            .next_back()
            .map(|mapped| mapped + 1);
        *glyphs = match (new_start, new_end) {
            (Some(start), Some(end)) => start..end,
            _ => 0..0,
        };
    }
    inline_content.replace_characters(output.glyphs, output.source_offsets, output.wraps);
    unicode_roots
}

/// Applies CSS case conversion to the normalized character stream before any
/// shaping spans are selected. Case conversion is not necessarily one-to-one
/// (`ß` uppercases to `SS`), so this pass rebuilds the dense stream and keeps
/// every generated scalar attached to the originating DOM source offset.
pub(super) fn transform_text(
    styles: &ComputedStyles,
    layout_tree: &LayoutTree,
    inline_content: &mut InlineContent,
    first_letter_styles: &mut Vec<Option<StyleIndices>>,
) {
    let old_len = inline_content.characters().len();
    let mut transforms = vec![TextTransform::None; old_len];
    for item in inline_content.inline_items() {
        let (InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs }) = &item.kind
        else {
            continue;
        };
        let base_style = get_style(styles, layout_tree, item.box_idx as usize);
        for glyph_idx in glyphs.clone() {
            let index = glyph_idx as usize;
            transforms[index] = first_letter_styles
                .get(index)
                .copied()
                .flatten()
                .and_then(|indices| styles.view(indices))
                .unwrap_or(base_style)
                .text_transform();
        }
    }
    if transforms
        .iter()
        .all(|transform| *transform == TextTransform::None)
    {
        return;
    }

    let capitalize_starts = capitalization_starts(layout_tree, inline_content);
    let mut new_glyphs = Vec::with_capacity(old_len);
    let mut new_source_offsets = SourceOffsets::with_capacity(old_len);
    let mut new_whitespace_wrap_before = Vec::with_capacity(old_len);
    let has_first_letter_styles = !first_letter_styles.is_empty();
    let mut new_first_letter_styles =
        Vec::with_capacity(if has_first_letter_styles { old_len } else { 0 });
    let mut remap_start = vec![0u32; old_len];
    let mut remap_end = vec![0u32; old_len];

    for index in 0..old_len {
        remap_start[index] = new_glyphs.len() as u32;
        let character =
            char::from_u32(inline_content.character_at(index).unwrap_or('?' as GlyphId)).unwrap_or('?');
        let source_offset = inline_content.raw_glyph_source_offset(index);
        let wrap_override = inline_content.whitespace_wrap_before(index);
        let first_letter_style = first_letter_styles.get(index).copied().flatten();
        let mut first_output = true;
        let mut append = |transformed: char| {
            new_glyphs.push(transformed as GlyphId);
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

        match transforms[index] {
            TextTransform::Uppercase => append_css_uppercase(character, &mut append),
            TextTransform::Lowercase => character.to_lowercase().for_each(&mut append),
            TextTransform::Capitalize if capitalize_starts.contains(&index) => {
                append_css_uppercase(character, &mut append)
            }
            TextTransform::Capitalize | TextTransform::None => append(character),
        }
        remap_end[index] = new_glyphs.len() as u32;
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
        *glyphs = remap_start[old_start]..remap_end[old_end - 1];
    }
    inline_content.replace_characters(new_glyphs, new_source_offsets, new_whitespace_wrap_before);
    *first_letter_styles = new_first_letter_styles;
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

/// Returns glyph indices containing the first typographic letter of each
/// Unicode word. Formatting boundaries are deliberately absent from the word
/// string, while forced breaks and atomic inline content introduce separators.
fn capitalization_starts(
    layout_tree: &LayoutTree,
    inline_content: &InlineContent,
) -> FxHashSet<usize> {
    fn publish(
        text: &mut String,
        glyphs_by_byte: &mut Vec<(usize, usize)>,
        starts: &mut FxHashSet<usize>,
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
            }
        }
        text.clear();
        glyphs_by_byte.clear();
    }

    let mut starts = FxHashSet::default();
    let mut text = String::new();
    let mut glyphs_by_byte = Vec::new();
    let mut current_context = None;
    for item in inline_content.inline_items() {
        // Out-of-flow boxes and formatting boundaries have no textual content
        // and must not affect word boundaries on either side of them.
        if matches!(
            item.kind,
            InlineItemKind::FloatAnchor { .. }
                | InlineItemKind::AbsoluteAnchor { .. }
                | InlineItemKind::InlineBoundary { .. }
        ) {
            continue;
        }
        let context_box = match item.kind {
            InlineItemKind::Image { .. } | InlineItemKind::AtomicBox { .. } => layout_tree
                .get_box_parent(item.box_idx as usize)
                .unwrap_or(item.box_idx as usize),
            _ => item.box_idx as usize,
        };
        let context = whitespace_context_root(layout_tree, context_box);
        if current_context != Some(context) {
            publish(&mut text, &mut glyphs_by_byte, &mut starts);
            current_context = Some(context);
        }
        match &item.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => {
                for glyph_idx in glyphs.clone() {
                    let character = char::from_u32(
                        inline_content
                            .character_at(glyph_idx as usize)
                            .unwrap_or('?' as GlyphId),
                    )
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
    publish(&mut text, &mut glyphs_by_byte, &mut starts);
    starts
}

/// CSS white-space collapsing applies to document spaces and tabs, not every
/// Unicode character classified as whitespace. In particular, typographic
/// spaces such as EN QUAD (U+2000) and IDEOGRAPHIC SPACE (U+3000) remain
/// visible under `white-space: normal`.
pub(super) fn is_css_collapsible_space(character: char) -> bool {
    matches!(character, ' ' | '\t')
}

pub(crate) fn whitespace_context_root(layout_tree: &LayoutTree, mut box_idx: usize) -> usize {
    while matches!(
        layout_tree.box_at(box_idx).map(|box_| box_.layout_mode()),
        Some(LayoutMode::Inline(_))
    ) {
        let Some(parent) = layout_tree.get_box_parent(box_idx) else {
            break;
        };
        box_idx = parent;
    }
    box_idx
}

fn layout_box_is_descendant_of(
    layout_tree: &LayoutTree,
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

pub(super) fn get_style<'a>(
    styles: &'a ComputedStyles,
    layout_tree: &LayoutTree,
    box_idx: usize,
) -> StyleView<'a> {
    let indices = layout_tree
        .get_box_style_indices(box_idx)
        .unwrap_or_else(|| styles.default_indices());
    styles.view(indices).expect("validated style handle")
}
