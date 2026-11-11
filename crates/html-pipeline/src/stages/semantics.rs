//! DOM-to-layout semantic indexing for links, notes, and anchors.

use super::*;
use html_dom::{element_is_note_reference};
use html_text::InlineTextKind;
use std::cell::OnceCell;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct LinkGlyphTarget {
    pub(super) href: u16,
    pub(super) note_reference: bool,
}

/// Source-ordered disjoint link ranges, shared by every glyph in a text run.
#[derive(Default)]
pub(super) struct LinkGlyphTargets {
    ranges: Vec<Range<u32>>,
    targets: Vec<LinkGlyphTarget>,
}

impl LinkGlyphTargets {
    pub(super) fn get(&self, glyph: u32) -> Option<&LinkGlyphTarget> {
        let index = self.ranges.partition_point(|range| range.end <= glyph);
        self.ranges
            .get(index)
            .filter(|range| range.contains(&glyph))
            .map(|_| &self.targets[index])
    }

    fn push(&mut self, range: Range<u32>, target: LinkGlyphTarget) {
        if self.targets.last() == Some(&target)
            && let Some(previous) = self.ranges.last_mut()
            && previous.end == range.start
        {
            previous.end = range.end;
            return;
        }
        self.ranges.push(range);
        self.targets.push(target);
    }

    pub(super) fn memory_usage_bytes(&self) -> usize {
        self.ranges.capacity() * size_of::<Range<u32>>()
            + self.targets.capacity() * size_of::<LinkGlyphTarget>()
    }

    pub(super) fn len(&self) -> usize {
        self.ranges.len()
    }
}

pub(super) fn box_id(document: &Document, layout_tree: &LayoutTree, box_idx: usize) -> Option<u16> {
    layout_tree
        .element_for_box(box_idx, document)
        .and_then(html_dom::ElementRef::id_idx)
}

pub(super) fn is_block_container(layout_tree: &LayoutTree, box_idx: usize) -> bool {
    matches!(
        layout_tree.get_box_layout_mode(box_idx),
        Some(LayoutMode::Block(_) | LayoutMode::Table(_) | LayoutMode::TableCell(_))
    )
}

pub(super) fn collect_glyph_targets(
    document: &Document,
    layout_tree: &LayoutTree,
    inline_content: &InlineContent,
) -> (LinkGlyphTargets, FxHashMap<u16, u32>) {
    let href_name = document.lookup_string("href");
    let count = layout_tree.box_count();
    let mut links = LinkGlyphTargets::default();
    let mut min_glyphs = vec![None; count];
    let mut targets = vec![None; count];
    let mut blocks = vec![None; count];
    let mut resolved = vec![false; count];
    let mut order = Vec::with_capacity(count);
    let mut pending = Vec::new();
    // Resolve parents before children without relying on box allocation order.
    for box_idx in 0..count {
        let mut cursor = Some(box_idx);
        while let Some(owner) = cursor {
            if resolved[owner] {
                break;
            }
            pending.push(owner);
            cursor = layout_tree.get_box_parent(owner);
        }
        while let Some(owner) = pending.pop() {
            let parent = layout_tree.get_box_parent(owner);
            targets[owner] = href_name
                .and_then(|name| {
                    let element = layout_tree.element_for_box(owner, document)?;
                    Some(LinkGlyphTarget {
                        href: element.attr_value_idx_no_namespace(name)?,
                        note_reference: element_is_note_reference(element),
                    })
                })
                .or_else(|| parent.and_then(|parent| targets[parent]));
            blocks[owner] = if is_block_container(layout_tree, owner) {
                Some(owner)
            } else {
                parent.and_then(|parent| blocks[parent])
            };
            resolved[owner] = true;
            order.push(owner);
        }
    }
    for run in inline_content
        .text_runs()
        .filter(|run| run.kind == InlineTextKind::Text && !run.glyphs.is_empty())
    {
        let owner = run.box_idx as usize;
        let entry = &mut min_glyphs[owner];
        *entry = Some(entry.map_or(run.glyphs.start, |value: u32| value.min(run.glyphs.start)));
        if let Some(target) = targets[owner] {
            links.push(run.glyphs.clone(), target);
        }
    }
    for owner in order.into_iter().rev() {
        if let Some(glyph) = min_glyphs[owner]
            && let Some(parent) = layout_tree.get_box_parent(owner)
        {
            let entry = &mut min_glyphs[parent];
            *entry = Some(entry.map_or(glyph, |value| value.min(glyph)));
        }
    }

    // Build this only if an empty inline anchor needs a nearby text position.
    // Runs stay ordered within each block, so every anchor uses a binary search
    // instead of rescanning the document's remaining inline content.
    let anchor_runs = OnceCell::new();
    let anchors = (0..layout_tree.box_count())
        .filter_map(|box_idx| {
            let id = box_id(document, layout_tree, box_idx)?;
            let glyph = min_glyphs[box_idx].or_else(|| {
                let LayoutMode::Inline(range) = layout_tree.get_box_layout_mode(box_idx)? else {
                    return None;
                };
                let run_idx = range.start as usize;
                if run_idx > inline_content.inline_items().len() {
                    return None;
                }
                let runs = anchor_runs.get_or_init(|| {
                    // The last slot represents boxes without a block ancestor.
                    let mut runs = vec![Vec::new(); count + 1];
                    for (index, run) in inline_content.inline_items().iter().enumerate() {
                        if let InlineItemKind::Text { glyphs } = &run.kind
                            && !glyphs.is_empty()
                        {
                            let block = blocks[run.box_idx as usize].unwrap_or(count);
                            runs[block].push((index, glyphs.clone()));
                        }
                    }
                    runs
                });
                let runs = &runs[blocks[box_idx].unwrap_or(count)];
                let next = runs.partition_point(|(index, _)| *index < run_idx);
                runs.get(next)
                    .map(|(_, glyphs)| glyphs.start)
                    .or_else(|| next.checked_sub(1).map(|previous| runs[previous].1.end - 1))
            })?;
            Some((id, glyph))
        })
        .collect();
    (links, anchors)
}
