//! Materialization of flex and grid item boxes from planned DOM sources.

use super::*;
use html_dom::ElementRef;
use std::iter::once;

impl<'a, 'out> LayoutTreeBuilder<'a, 'out> {
    pub(super) fn build_flex_grid_children(&mut self, element: ElementRef<'_>, box_idx: u32) {
        let mut items = Vec::new();
        let mut inline_segment = Vec::new();

        self.build_flex_grid_items(element, box_idx, &mut items, &mut inline_segment);
        items.extend(self.flush_flex_inline_sources(&mut inline_segment, box_idx));

        self.output.set_child_boxes(box_idx, items);
    }

    pub(super) fn build_flex_grid_items(
        &mut self,
        element: ElementRef<'_>,
        box_idx: u32,
        items: &mut Vec<u32>,
        inline_segment: &mut Vec<BoxSource>,
    ) {
        let sources = once(BoxSource::GeneratedPseudo {
            origin: element.node_id(),
            before: true,
        })
        .chain(element.children().map(BoxSource::Node))
        .chain(once(BoxSource::GeneratedPseudo {
            origin: element.node_id(),
            before: false,
        }));
        for source in sources {
            let Some(classification) = self.classify_source(source) else {
                if matches!(source, BoxSource::Node(_)) {
                    inline_segment.push(source);
                }
                continue;
            };
            if classification.display == Display::Contents {
                match source {
                    BoxSource::Node(node) => {
                        self.generated.apply_for_node(node);
                        self.build_flex_grid_items(
                            self.document.element_ref(node),
                            box_idx,
                            items,
                            inline_segment,
                        );
                    }
                    BoxSource::GeneratedPseudo { .. } => inline_segment.push(source),
                }
                continue;
            }
            // Preserve counter/quote evaluation order: DOM children flush
            // pending text first; generated principals resolve before it.
            if matches!(source, BoxSource::Node(_)) {
                items.extend(self.flush_flex_inline_sources(inline_segment, box_idx));
            }
            if let Some(item) = self.build_source(source, box_idx, classification) {
                items.extend(self.flush_flex_inline_sources(inline_segment, box_idx));
                items.push(item);
            }
        }
    }

    pub(super) fn flush_flex_inline_sources(
        &mut self,
        sources: &mut Vec<BoxSource>,
        parent_box: u32,
    ) -> Option<u32> {
        if sources.is_empty() {
            return None;
        }
        if sources.iter().all(|source| match source {
            BoxSource::Node(node) => self.contents_text_style(*node).is_none(),
            BoxSource::GeneratedPseudo { .. } => false,
        }) {
            let mut nodes = sources
                .drain(..)
                .map(|source| match source {
                    BoxSource::Node(node) => node,
                    BoxSource::GeneratedPseudo { .. } => unreachable!(),
                })
                .collect();
            return self.flush_inline_segment(&mut nodes, parent_box);
        }

        let whitespace_only = sources.iter().all(|source| match *source {
            BoxSource::Node(node) => {
                let collapsible = self
                    .contents_text_style(node)
                    .and_then(|style| self.styles.view(style))
                    .is_none_or(|style| !style.white_space().preserves_spaces());
                collapsible
                    && self
                        .document
                        .text_ref(node)
                        .text()
                        .chars()
                        .all(char::is_whitespace)
            }
            BoxSource::GeneratedPseudo { .. } => false,
        });
        if whitespace_only {
            sources.clear();
            return None;
        }

        let (anonymous, item_start) = self.output.push_anonymous_inline(parent_box);
        for source in sources.drain(..) {
            self.process_inline_source(source, anonymous);
        }
        self.output
            .finish_inline_box(anonymous, item_start)
            .then_some(anonymous)
    }
}
