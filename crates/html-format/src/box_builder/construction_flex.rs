//! Materialization of flex and grid item boxes from planned DOM sources.

use super::*;
use html_dom::ElementRef;

impl<'a, 'out> LayoutTreeBuilder<'a, 'out> {
    pub(super) fn build_flex_grid_children(&mut self, element: ElementRef<'_>, box_idx: u32) {
        let elem_id = element.node_id();
        let mut items = Vec::new();
        let mut inline_segment = Vec::new();

        self.append_flex_grid_pseudo(elem_id, box_idx, true, &mut items, &mut inline_segment);
        self.build_flex_grid_items(element, box_idx, &mut items, &mut inline_segment);
        self.append_flex_grid_pseudo(elem_id, box_idx, false, &mut items, &mut inline_segment);
        items.extend(self.flush_flex_inline_sources(&mut inline_segment, box_idx));

        self.output.set_flex_grid_children(box_idx, items);
    }

    pub(super) fn append_flex_grid_pseudo(
        &mut self,
        origin: DomNodeId,
        box_idx: u32,
        before: bool,
        items: &mut Vec<u32>,
        inline_segment: &mut Vec<FlexInlineSource>,
    ) {
        if pseudo_display(self.styles, origin, before) == Some(Display::Contents) {
            inline_segment.push(FlexInlineSource::GeneratedPseudo { origin, before });
            return;
        }
        let Some(pseudo) = self.build_generated_flex_grid_pseudo(origin, box_idx, before) else {
            return;
        };
        items.extend(self.flush_flex_inline_sources(inline_segment, box_idx));
        items.push(pseudo);
    }

    pub(super) fn build_flex_grid_items(
        &mut self,
        element: ElementRef<'_>,
        box_idx: u32,
        items: &mut Vec<u32>,
        inline_segment: &mut Vec<FlexInlineSource>,
    ) {
        for child_id in element.children() {
            match self.document.node_ref(child_id) {
                Some(NodeRef::Text(_)) => inline_segment.push(FlexInlineSource::Node(child_id)),
                Some(NodeRef::Element(child)) => {
                    let display = display_for_element(self.styles, child);
                    if display == Display::Contents {
                        self.generated.apply_for_node(child_id);
                        self.append_flex_grid_pseudo(
                            child_id, box_idx, true, items, inline_segment,
                        );
                        self.build_flex_grid_items(child, box_idx, items, inline_segment);
                        self.append_flex_grid_pseudo(
                            child_id, box_idx, false, items, inline_segment,
                        );
                        continue;
                    }
                    items.extend(self.flush_flex_inline_sources(inline_segment, box_idx));
                    if display != Display::None
                        && let Some(item) = self.build_box_for_element(child_id, Some(box_idx))
                    {
                        items.push(item);
                    }
                }
                None => {}
            }
        }
    }

    pub(super) fn flush_flex_inline_sources(
        &mut self,
        sources: &mut Vec<FlexInlineSource>,
        parent_box: u32,
    ) -> Option<u32> {
        if sources.is_empty() {
            return None;
        }
        if sources.iter().all(|source| match source {
            FlexInlineSource::Node(node) => !self.contents_text_styles.contains_key(node),
            FlexInlineSource::GeneratedPseudo { .. } => false,
        }) {
            let mut nodes = sources
                .drain(..)
                .map(|source| match source {
                    FlexInlineSource::Node(node) => node.raw(),
                    FlexInlineSource::GeneratedPseudo { .. } => unreachable!(),
                })
                .collect();
            return self.flush_inline_segment(&mut nodes, parent_box);
        }

        let whitespace_only = sources.iter().all(|source| match *source {
            FlexInlineSource::Node(node) => {
                let collapsible = self
                    .contents_text_styles
                    .get(&node)
                    .copied()
                    .and_then(|style| self.styles.view(style))
                    .is_none_or(|style| !style.white_space().preserves_spaces());
                collapsible
                    && self
                        .document
                        .text_ref(node)
                        .is_some_and(|text| text.text().chars().all(char::is_whitespace))
            }
            FlexInlineSource::GeneratedPseudo { .. } => false,
        });
        if whitespace_only {
            sources.clear();
            return None;
        }

        let (anonymous, item_start) = self.output.push_anonymous_inline(parent_box);
        for source in sources.drain(..) {
            match source {
                FlexInlineSource::Node(node) => self.process_inline_node(node, anonymous),
                FlexInlineSource::GeneratedPseudo { origin, before } => {
                    self.append_generated_inline_pseudo(origin, anonymous, before)
                }
            }
        }
        self.output
            .finish_inline_box(anonymous, item_start)
            .then_some(anonymous)
    }

    pub(super) fn build_generated_flex_grid_pseudo(
        &mut self,
        origin: DomNodeId,
        parent_box: u32,
        before: bool,
    ) -> Option<u32> {
        let display = pseudo_display(self.styles, origin, before)?;
        if matches!(
            display,
            Display::None | Display::TableColumn | Display::TableColumnGroup
        ) {
            return None;
        }
        if pseudo_is_block_level(self.styles, origin, before) {
            return self.build_generated_block_pseudo(origin, parent_box, before);
        }
        let resolved = self.generated.resolve_pseudo(origin, before)?;
        let pseudo_box = self.output.push_generated_text_box(parent_box, &resolved);
        Some(pseudo_box)
    }
}
