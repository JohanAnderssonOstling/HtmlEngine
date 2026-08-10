//! DOM-to-layout semantic indexing for links, notes, and anchors.

use super::*;
use html_dom::element_is_note_reference;

#[derive(Clone, Copy)]
pub(super) struct LinkGlyphTarget {
    pub(super) href: u16,
    pub(super) note_reference: bool,
}

fn box_element<'a>(document: &'a Document, layout_tree: &LayoutTree, box_idx: usize) -> Option<html_dom::ElementRef<'a>> {
    layout_tree.box_at(box_idx)?.get_element(document)
}

pub(super) fn box_id(document: &Document, layout_tree: &LayoutTree, box_idx: usize) -> Option<u16> {
    box_element(document, layout_tree, box_idx).and_then(|element| element.id_idx())
}

fn box_href(document: &Document, layout_tree: &LayoutTree, box_idx: usize) -> Option<u16> {
    let href_name = document.lookup_string("href")?;
    box_element(document, layout_tree, box_idx).and_then(|element| element.attr_value_idx_no_namespace(href_name))
}

fn parent(layout_tree: &LayoutTree, box_idx: usize) -> Option<usize> {
    layout_tree.box_at(box_idx)?.parent().map(|parent| parent as usize)
}

fn is_block_container(layout_tree: &LayoutTree, box_idx: usize) -> bool {
    matches!(layout_tree.box_at(box_idx).map(|layout_box| layout_box.layout_mode()), Some(LayoutMode::Block(_) | LayoutMode::Table(_) | LayoutMode::TableCell(_)))
}

fn block_ancestor(layout_tree: &LayoutTree, box_idx: usize) -> Option<usize> {
    let mut current = Some(box_idx);
    while let Some(idx) = current {
        if is_block_container(layout_tree, idx) {
            return Some(idx);
        }
        current = parent(layout_tree, idx);
    }
    None
}

pub(super) fn collect_link_glyph_targets(document: &Document, layout_tree: &LayoutTree, inline_content: &InlineContent) -> Vec<(u32, LinkGlyphTarget)> {
    let mut links = Vec::new();
    for run in inline_content.inline_items() {
        let InlineItemKind::Text { glyphs } = &run.kind else { continue };
        if glyphs.is_empty() {
            continue;
        }
        let mut current = Some(run.box_idx as usize);
        let mut target = None;
        while let Some(box_idx) = current {
            if let Some(found) = box_href(document, layout_tree, box_idx) {
                target = Some(LinkGlyphTarget { href: found, note_reference: box_element(document, layout_tree, box_idx).is_some_and(element_is_note_reference) });
                break;
            }
            current = parent(layout_tree, box_idx);
        }
        if let Some(target) = target {
            links.extend(glyphs.clone().map(|glyph| (glyph, target)));
        }
    }
    links
}

pub(super) fn collect_anchor_glyphs(document: &Document, layout_tree: &LayoutTree, inline_content: &InlineContent) -> Vec<(u16, u32)> {
    let mut min_glyphs = vec![None; layout_tree.box_count()];
    for run in inline_content.inline_items() {
        let InlineItemKind::Text { glyphs } = &run.kind else { continue };
        if glyphs.is_empty() {
            continue;
        }
        let mut current = Some(run.box_idx as usize);
        while let Some(idx) = current {
            let entry = &mut min_glyphs[idx];
            if entry.is_none_or(|existing| glyphs.start < existing) {
                *entry = Some(glyphs.start);
            }
            current = parent(layout_tree, idx);
        }
    }

    for box_idx in 0..layout_tree.box_count() {
        if box_id(document, layout_tree, box_idx).is_none() || min_glyphs[box_idx].is_some() {
            continue;
        }
        let Some(LayoutMode::Inline(range)) = layout_tree.box_at(box_idx).map(|layout_box| layout_box.layout_mode()) else {
            continue;
        };
        let run_idx = range.start as usize;
        let anchor_block = block_ancestor(layout_tree, box_idx);
        let following = inline_content.inline_items().get(run_idx..).into_iter().flatten().find_map(|run| {
            let InlineItemKind::Text { glyphs } = &run.kind else { return None };
            (!glyphs.is_empty() && block_ancestor(layout_tree, run.box_idx as usize) == anchor_block).then_some(glyphs.start)
        });
        let preceding = || {
            inline_content.inline_items().get(..run_idx).into_iter().flatten().rev().find_map(|run| {
                let InlineItemKind::Text { glyphs } = &run.kind else { return None };
                (!glyphs.is_empty() && block_ancestor(layout_tree, run.box_idx as usize) == anchor_block).then(|| glyphs.end - 1)
            })
        };
        min_glyphs[box_idx] = following.or_else(preceding);
    }

    (0..layout_tree.box_count()).filter_map(|idx| Some((box_id(document, layout_tree, idx)?, min_glyphs[idx]?))).collect()
}
