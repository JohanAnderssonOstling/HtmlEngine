//! Classify source items using their inline content, styles, and box ancestry.
use crate::engine::LayoutReadContext;
use html_engine_model::{InlineItem, InlineItemKind, LayoutMode};
use html_style_model::Float;
use std::ops::Range;

impl LayoutReadContext<'_> {
    /// Whether an inline-run child range creates an in-flow line box. Floats
    /// and absolutely positioned descendants leave anchors in the run stream,
    /// but those anchors do not give their containing block a line box or
    /// prevent its vertical margins from collapsing through.
    pub fn inline_items_establish_line_box(
        &self,
        container_box_idx: usize,
        runs: &Range<u32>,
    ) -> bool {
        self.text.content().inline_items()[runs.start as usize..runs.end as usize]
            .iter()
            .any(|run| {
                if !self.inline_item_belongs_to_context(run, container_box_idx) {
                    return false;
                }
                match &run.kind {
                    InlineItemKind::FloatAnchor { .. } | InlineItemKind::AbsoluteAnchor { .. } => {
                        false
                    }
                    InlineItemKind::Text { glyphs } => {
                        let preserves_space = self
                            .reader
                            .style(run.box_idx as usize)
                            .white_space()
                            .preserves_spaces();
                        glyphs.clone().any(|glyph_idx| {
                            let character = self
                                .text
                                .content()
                                .character_at(glyph_idx as usize)
                                .unwrap_or_default();
                            preserves_space || !html_text::is_document_white_space(character)
                        })
                    }
                    InlineItemKind::Image { .. }
                    | InlineItemKind::Break { .. }
                    | InlineItemKind::InlineBoundary { .. }
                    | InlineItemKind::AtomicBox { .. }
                    | InlineItemKind::Marker { .. } => true,
                }
            })
    }

    /// Float sides represented only by anchors in this block's inline-run
    /// stream. Anchor-only streams do not establish a line box, but their
    /// float sources still participate in adjoining-margin/clearance rules.
    pub fn inline_item_float_sides(
        &self,
        container_box_idx: usize,
        runs: &Range<u32>,
    ) -> (bool, bool) {
        let mut left = false;
        let mut right = false;
        for run in &self.text.content().inline_items()[runs.start as usize..runs.end as usize] {
            let InlineItemKind::FloatAnchor { box_idx } = run.kind else {
                continue;
            };
            if !self.inline_item_belongs_to_context(run, container_box_idx) {
                continue;
            }
            match self.reader.box_facts(box_idx as usize).float {
                Float::Left => left = true,
                Float::Right => right = true,
                Float::None => {}
            }
        }
        (left, right)
    }

    /// Inline items stop at their formatting-context root rather than leaking
    /// into ancestor lines. An atomic box's parent owns its placeholder.
    pub fn inline_item_belongs_to_context(
        &self,
        item: &InlineItem,
        container_box_idx: usize,
    ) -> bool {
        let mut current = match item.kind {
            InlineItemKind::AtomicBox { box_idx } => self.reader.get_parent(box_idx as usize),
            _ => Some(item.box_idx as usize),
        };
        while let Some(box_idx) = current {
            if box_idx == container_box_idx {
                return true;
            }
            if matches!(
                self.reader.box_layout_mode(box_idx),
                Some(
                    LayoutMode::Block(_)
                        | LayoutMode::Table(_)
                        | LayoutMode::TableRow(_)
                        | LayoutMode::TableCell(_)
                        | LayoutMode::Flex(_)
                        | LayoutMode::Grid(_)
                )
            ) {
                return false;
            }
            current = self.reader.get_parent(box_idx);
        }
        false
    }
}
