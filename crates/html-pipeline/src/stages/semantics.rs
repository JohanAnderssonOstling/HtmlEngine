//! DOM-to-layout semantic indexing for links, notes, and anchors.

use super::*;
use html_dom::{element_is_note_reference, storage};
use html_text::{InlineItem, InlineTextKind};
use std::iter::once;

#[derive(Clone, Copy)]
pub(super) struct LinkGlyphTarget {
    pub(super) href: u16,
    pub(super) note_reference: bool,
}

pub(super) fn box_id(document: &Document, layout_tree: &LayoutTree, box_idx: usize) -> Option<u16> {
    layout_tree
        .element_for_box(box_idx, document)
        .and_then(storage::id_idx)
}

pub(super) fn is_block_container(layout_tree: &LayoutTree, box_idx: usize) -> bool {
    matches!(
        layout_tree.get_box_layout_mode(box_idx),
        Some(LayoutMode::Block(_) | LayoutMode::Table(_) | LayoutMode::TableCell(_))
    )
}

fn block_ancestor(layout_tree: &LayoutTree, box_idx: usize) -> Option<usize> {
    once(box_idx)
        .chain(layout_tree.ancestors(box_idx))
        .find(|&idx| is_block_container(layout_tree, idx))
}

pub(super) fn collect_glyph_targets(
    document: &Document,
    layout_tree: &LayoutTree,
    inline_content: &InlineContent,
) -> (FxHashMap<u32, LinkGlyphTarget>, FxHashMap<u16, u32>) {
    let href_name = document.lookup_string("href");
    let mut links = FxHashMap::default();
    let mut min_glyphs = vec![None; layout_tree.box_count()];
    for run in inline_content
        .text_runs()
        .filter(|run| run.kind == InlineTextKind::Text && !run.glyphs.is_empty())
    {
        let glyphs = run.glyphs;
        let owner = run.box_idx as usize;
        let mut target = None;
        for box_idx in once(owner).chain(layout_tree.ancestors(owner)) {
            let entry = &mut min_glyphs[box_idx];
            if entry.is_none_or(|existing| glyphs.start < existing) {
                *entry = Some(glyphs.start);
            }
            if target.is_none()
                && let Some(href_name) = href_name
                && let Some(element) = layout_tree.element_for_box(box_idx, document)
                && let Some(href) = storage::attr_value_idx_no_namespace(element, href_name)
            {
                target = Some(LinkGlyphTarget {
                    href,
                    note_reference: element_is_note_reference(element),
                });
            }
        }
        if let Some(target) = target {
            links.extend(glyphs.clone().map(|glyph| (glyph, target)));
        }
    }

    let anchors = (0..layout_tree.box_count())
        .filter_map(|box_idx| {
            let id = box_id(document, layout_tree, box_idx)?;
            let glyph = min_glyphs[box_idx].or_else(|| {
                let LayoutMode::Inline(range) = layout_tree.get_box_layout_mode(box_idx)? else {
                    return None;
                };
                let run_idx = range.start as usize;
                let anchor_block = block_ancestor(layout_tree, box_idx);
                let matching_glyphs = |run: &InlineItem| {
                    let InlineItemKind::Text { glyphs } = &run.kind else {
                        return None;
                    };
                    (!glyphs.is_empty()
                        && block_ancestor(layout_tree, run.box_idx as usize) == anchor_block)
                        .then(|| glyphs.clone())
                };
                let runs = inline_content.inline_items();
                runs.get(run_idx..)
                    .into_iter()
                    .flatten()
                    .find_map(matching_glyphs)
                    .map(|glyphs| glyphs.start)
                    .or_else(|| {
                        runs.get(..run_idx)
                            .into_iter()
                            .flatten()
                            .rev()
                            .find_map(matching_glyphs)
                            .map(|glyphs| glyphs.end - 1)
                    })
            })?;
            Some((id, glyph))
        })
        .collect();
    (links, anchors)
}
