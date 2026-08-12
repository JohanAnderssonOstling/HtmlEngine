//! CSS whitespace normalization and text transformation.

use super::*;

pub(super) fn collapse_whitespace(
    styles: &ComputedStyles,
    layout_tree: &LayoutTree,
    inline_content: &mut InlineContent,
) {
    let mut new_glyphs = Vec::with_capacity(inline_content.glyphs().len());
    let mut new_source_offsets = Vec::with_capacity(inline_content.glyphs().len());
    let mut new_whitespace_wrap_before = Vec::with_capacity(inline_content.glyphs().len());
    let mut remap = vec![u32::MAX; inline_content.glyphs().len()];
    let mut at_line_start = true;
    let mut pending_space: Option<(usize, u32, bool)> = None;
    let mut current_context = None;

    // Non-text inline items participate in whitespace collapsing too. A
    // pending segment-break space before an image or atomic inline box must be
    // committed, while whitespace at the true end of a formatting context is
    // discarded. Preserve run order here instead of filtering down to text
    // runs and losing those boundaries.
    let inline_items = inline_content.inline_items().to_vec();
    for run in &inline_items {
        // Out-of-flow anchors and formatting-only boundaries are absent from
        // the inline text stream. They neither commit nor discard pending
        // whitespace and cannot start a new whitespace context.
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
            if current_context
                .is_some_and(|previous| layout_box_is_descendant_of(layout_tree, context, previous))
            {
                // Atomic descendants allocate their runs before the carrier
                // token in the parent's run arena. Entering that nested
                // formatting context therefore follows the parent's pending
                // whitespace logically; commit it before shaping descendants.
                if let Some((source, offset, wrap_before)) = pending_space.take() {
                    remap[source] = new_glyphs.len() as u32;
                    new_glyphs.push(' ' as GlyphId);
                    new_source_offsets.push(offset);
                    new_whitespace_wrap_before.push(WhitespaceWrapOverride::retained(wrap_before));
                }
            } else {
                // A trailing collapsible space is discarded between sibling
                // formatting contexts and must not leak into the next block.
                pending_space = None;
            }
            at_line_start = true;
            current_context = Some(context);
        }
        let (InlineItemKind::Text { glyphs: range } | InlineItemKind::Marker { glyphs: range }) =
            &run.kind
        else {
            match run.kind {
                InlineItemKind::Image { .. } | InlineItemKind::AtomicBox { .. } => {
                    if let Some((source, offset, wrap_before)) = pending_space.take() {
                        remap[source] = new_glyphs.len() as u32;
                        new_glyphs.push(' ' as GlyphId);
                        new_source_offsets.push(offset);
                        new_whitespace_wrap_before
                            .push(WhitespaceWrapOverride::retained(wrap_before));
                    }
                    at_line_start = false;
                }
                InlineItemKind::Break { .. } => {
                    pending_space = None;
                    at_line_start = true;
                }
                InlineItemKind::FloatAnchor { .. }
                | InlineItemKind::AbsoluteAnchor { .. }
                | InlineItemKind::InlineBoundary { .. } => unreachable!(
                    "non-textual boundaries are filtered before whitespace context selection"
                ),
                InlineItemKind::Text { .. } | InlineItemKind::Marker { .. } => unreachable!(),
            }
            continue;
        };
        let white_space = get_style(styles, layout_tree, run.box_idx as usize).white_space();
        let collapse = white_space.collapses_spaces();
        let preserve_newlines = white_space.preserves_newlines();

        for i in range.clone() {
            let i = i as usize;
            let char_code = inline_content.glyph_at(i).unwrap_or('?' as GlyphId);
            let mut character = char::from_u32(char_code).unwrap_or('?');
            // CSS Text treats CRLF as one segment break and a standalone CR as
            // an LF. Character references can introduce CR after HTML input
            // preprocessing, so this normalization belongs at the whitespace
            // boundary rather than only in the byte parser.
            if character == '\r' {
                let followed_by_lf = (i + 1) < range.end as usize
                    && inline_content.glyph_at(i + 1).and_then(char::from_u32) == Some('\n');
                if followed_by_lf {
                    continue;
                }
                character = '\n';
            }
            if character == '\n' {
                if preserve_newlines {
                    // Collapsible spaces immediately before a segment break
                    // are removed before that break is preserved.
                    pending_space = None;
                    remap[i] = new_glyphs.len() as u32;
                    new_glyphs.push('\n' as GlyphId);
                    new_source_offsets.push(inline_content.raw_glyph_source_offset(i));
                    new_whitespace_wrap_before.push(WhitespaceWrapOverride::Style);
                    at_line_start = true;
                } else if collapse && !at_line_start {
                    // The segment break becomes one collapsible space. Keep an
                    // already-pending space as the representative of the
                    // sequence so its originating inline style paints the
                    // collapsed glyph (CSS2 collapsing can cross element and
                    // display:none boundaries).
                    if let Some((_, _, wrap_before)) = pending_space.as_mut() {
                        *wrap_before |= white_space.allows_wrap();
                    } else {
                        pending_space = Some((
                            i,
                            inline_content.raw_glyph_source_offset(i),
                            white_space.allows_wrap(),
                        ));
                    }
                }
            } else if is_css_collapsible_space(character) {
                if collapse {
                    if !at_line_start {
                        if let Some((_, _, wrap_before)) = pending_space.as_mut() {
                            *wrap_before |= white_space.allows_wrap();
                        } else {
                            pending_space = Some((
                                i,
                                inline_content.raw_glyph_source_offset(i),
                                white_space.allows_wrap(),
                            ));
                        }
                    }
                } else {
                    if let Some((source, offset, _)) = pending_space.take() {
                        remap[source] = new_glyphs.len() as u32;
                        new_glyphs.push(' ' as GlyphId);
                        new_source_offsets.push(offset);
                        // A collapsible space immediately adjoining a
                        // preserved space cannot introduce a break into the
                        // unwrappable preserved sequence.
                        new_whitespace_wrap_before.push(WhitespaceWrapOverride::Suppress);
                    }
                    remap[i] = new_glyphs.len() as u32;
                    new_glyphs.push(char_code);
                    new_source_offsets.push(inline_content.raw_glyph_source_offset(i));
                    new_whitespace_wrap_before.push(WhitespaceWrapOverride::Style);
                    at_line_start = false;
                }
            } else {
                if let Some((source, offset, wrap_before)) = pending_space.take() {
                    remap[source] = new_glyphs.len() as u32;
                    new_glyphs.push(' ' as GlyphId);
                    new_source_offsets.push(offset);
                    new_whitespace_wrap_before.push(WhitespaceWrapOverride::retained(wrap_before));
                }
                remap[i] = new_glyphs.len() as u32;
                new_glyphs.push(char_code);
                new_source_offsets.push(inline_content.raw_glyph_source_offset(i));
                new_whitespace_wrap_before.push(WhitespaceWrapOverride::Style);
                at_line_start = false;
            }
        }
    }

    for run in inline_content.inline_items_mut() {
        let (InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs }) = &mut run.kind
        else {
            continue;
        };
        let old_start = glyphs.start as usize;
        let old_end = glyphs.end as usize;
        let new_start = (old_start..old_end)
            .find_map(|i| remap.get(i).copied().filter(|mapped| *mapped != u32::MAX));
        let new_end = (old_start..old_end)
            .filter_map(|i| remap.get(i).copied().filter(|mapped| *mapped != u32::MAX))
            .next_back()
            .map(|mapped| mapped + 1);
        *glyphs = match (new_start, new_end) {
            (Some(start), Some(end)) => start..end,
            _ => 0..0,
        };
    }
    inline_content.replace_glyphs(new_glyphs, new_source_offsets, new_whitespace_wrap_before);
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
    let old_len = inline_content.glyphs().len();
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
    let mut new_source_offsets = Vec::with_capacity(old_len);
    let mut new_whitespace_wrap_before = Vec::with_capacity(old_len);
    let mut new_first_letter_styles = Vec::with_capacity(old_len);
    let mut remap_start = vec![0u32; old_len];
    let mut remap_end = vec![0u32; old_len];

    for index in 0..old_len {
        remap_start[index] = new_glyphs.len() as u32;
        let character =
            char::from_u32(inline_content.glyph_at(index).unwrap_or('?' as GlyphId)).unwrap_or('?');
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
            new_first_letter_styles.push(first_letter_style);
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
    inline_content.replace_glyphs(new_glyphs, new_source_offsets, new_whitespace_wrap_before);
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
                            .glyph_at(glyph_idx as usize)
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
