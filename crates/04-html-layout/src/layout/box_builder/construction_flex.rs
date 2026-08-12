//! Materialization of flex and grid item boxes from planned DOM sources.

use super::*;

impl<'a, 'out> LayoutTreeBuilder<'a, 'out> {
    pub(super) fn build_flex_grid_children(&mut self, elem_id: DomNodeId, box_idx: u32) {
        let Some(element) = self.document.element_ref(elem_id) else {
            return;
        };
        let children: Vec<u32> = element.children().map(DomNodeId::raw).collect();
        let mut items = Vec::new();
        let mut inline_segment = Vec::new();

        self.append_flex_grid_pseudo(elem_id, box_idx, true, &mut items, &mut inline_segment);
        self.build_flex_grid_items_from_nodes(&children, box_idx, &mut items, &mut inline_segment);
        self.append_flex_grid_pseudo(elem_id, box_idx, false, &mut items, &mut inline_segment);
        if let Some(anonymous) = self.flush_flex_inline_sources(&mut inline_segment, box_idx) {
            items.push(anonymous);
        }

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
        if self.generated.pseudo_display(origin, before) == Some(Display::Contents) {
            inline_segment.push(FlexInlineSource::GeneratedPseudo { origin, before });
            return;
        }
        let Some(pseudo) = self.build_generated_flex_grid_pseudo(origin, box_idx, before) else {
            return;
        };
        if let Some(anonymous) = self.flush_flex_inline_sources(inline_segment, box_idx) {
            items.push(anonymous);
        }
        items.push(pseudo);
    }

    pub(super) fn build_flex_grid_items_from_nodes(
        &mut self,
        children: &[u32],
        box_idx: u32,
        items: &mut Vec<u32>,
        inline_segment: &mut Vec<FlexInlineSource>,
    ) {
        for &raw in children {
            let Some(child_id) = self.document.node_id_from_raw(raw) else {
                continue;
            };
            match self.document.node_ref(child_id) {
                Some(NodeRef::Text(_)) => inline_segment.push(FlexInlineSource::Node(raw)),
                Some(NodeRef::Element(child)) => {
                    let display = display_for_element(self.styles, child);
                    if display == Display::Contents {
                        if let Some(directives) = self.styles.counter_directives_for_node(child_id)
                        {
                            self.generated.apply(directives);
                        }
                        if self.generated.pseudo_display(child_id, true) == Some(Display::Contents)
                        {
                            inline_segment.push(FlexInlineSource::GeneratedPseudo {
                                origin: child_id,
                                before: true,
                            });
                        } else if let Some(pseudo) =
                            self.build_generated_flex_grid_pseudo(child_id, box_idx, true)
                        {
                            if let Some(anonymous) =
                                self.flush_flex_inline_sources(inline_segment, box_idx)
                            {
                                items.push(anonymous);
                            }
                            items.push(pseudo);
                        }
                        let grandchildren: Vec<u32> =
                            child.children().map(DomNodeId::raw).collect();
                        self.build_flex_grid_items_from_nodes(
                            &grandchildren,
                            box_idx,
                            items,
                            inline_segment,
                        );
                        if self.generated.pseudo_display(child_id, false) == Some(Display::Contents)
                        {
                            inline_segment.push(FlexInlineSource::GeneratedPseudo {
                                origin: child_id,
                                before: false,
                            });
                        } else if let Some(pseudo) =
                            self.build_generated_flex_grid_pseudo(child_id, box_idx, false)
                        {
                            if let Some(anonymous) =
                                self.flush_flex_inline_sources(inline_segment, box_idx)
                            {
                                items.push(anonymous);
                            }
                            items.push(pseudo);
                        }
                        continue;
                    }
                    if let Some(anonymous) = self.flush_flex_inline_sources(inline_segment, box_idx)
                    {
                        items.push(anonymous);
                    }
                    if display != Display::None
                        && let Some(item) = self.build_box_for_node(child_id, Some(box_idx))
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
            FlexInlineSource::Node(raw) => self
                .document
                .node_id_from_raw(*raw)
                .is_none_or(|node| !self.contents_text_styles.contains_key(&node)),
            FlexInlineSource::GeneratedPseudo { .. } => false,
        }) {
            let mut nodes = sources
                .drain(..)
                .map(|source| match source {
                    FlexInlineSource::Node(raw) => raw,
                    FlexInlineSource::GeneratedPseudo { .. } => unreachable!(),
                })
                .collect();
            return self.flush_inline_segment(&mut nodes, parent_box);
        }

        let whitespace_only = sources.iter().all(|source| match *source {
            FlexInlineSource::Node(raw) => {
                let collapsible = self
                    .document
                    .node_id_from_raw(raw)
                    .and_then(|node| self.contents_text_styles.get(&node).copied())
                    .and_then(|style| self.styles.view(style))
                    .map_or(true, |style| !style.white_space().preserves_spaces());
                collapsible
                    && self
                        .document
                        .node_id_from_raw(raw)
                        .and_then(|node| self.document.text_ref(node))
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
                FlexInlineSource::Node(raw) => {
                    if let Some(node) = self.document.node_id_from_raw(raw) {
                        self.process_inline_node(node, anonymous);
                    }
                }
                FlexInlineSource::GeneratedPseudo { origin, before } => {
                    self.append_generated_inline_pseudo(origin, anonymous, before)
                }
            }
        }
        self.output
            .finish_anonymous_box(anonymous, item_start)
            .then_some(anonymous)
    }

    pub(super) fn build_generated_flex_grid_pseudo(
        &mut self,
        origin: DomNodeId,
        parent_box: u32,
        before: bool,
    ) -> Option<u32> {
        let display = self.generated.pseudo_display(origin, before)?;
        if matches!(
            display,
            Display::None | Display::TableColumn | Display::TableColumnGroup
        ) {
            return None;
        }
        if is_block_level_pseudo_display(display) {
            return self.build_generated_block_pseudo(origin, parent_box, before);
        }
        let resolved = self.generated.resolve_pseudo(origin, before)?;
        let style = resolved.style;
        let pseudo_box = self.output.push_synthetic_box(
            parent_box,
            style,
            LayoutMode::Block(BlockBox {
                children: Children::Empty,
            }),
        );
        self.output
            .set_generated_text_children(pseudo_box, style, &resolved.text, false);
        Some(pseudo_box)
    }
}
