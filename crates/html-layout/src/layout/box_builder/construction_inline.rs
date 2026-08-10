//! Materialization of inline runs, split inline boxes, and generated content.

use super::*;

impl<'a, 'out> LayoutTreeBuilder<'a, 'out> {
    pub(super) fn build_split_inline_child(&mut self, elem_id: DomNodeId, parent_box_idx: u32, direct_children: &mut Vec<u32>) {
        for part in plan_split_inline(self.document, self.styles, elem_id) {
            match part {
                SplitInlinePart::Segment(events) => {
                    if let Some(anonymous) = self.materialize_split_inline_segment(events, parent_box_idx) {
                        direct_children.push(anonymous);
                    }
                }
                SplitInlinePart::Block { node, position_ancestors } => {
                    if let Some(block_idx) = self.build_box_for_node(node, Some(parent_box_idx)) {
                        self.output.set_split_inline_position_ancestors(block_idx, position_ancestors);
                        direct_children.push(block_idx);
                    }
                }
                SplitInlinePart::GeneratedBlock { origin, before, position_ancestors } => {
                    if let Some(block_idx) = self.build_generated_block_pseudo(origin, parent_box_idx, before) {
                        self.output.set_split_inline_position_ancestors(block_idx, position_ancestors);
                        direct_children.push(block_idx);
                    }
                }
            }
        }
    }

    pub(super) fn materialize_split_inline_segment(&mut self, events: Vec<SplitInlineEvent>, parent_box_idx: u32) -> Option<u32> {
        if events.is_empty() {
            return None;
        }
        let (anonymous_idx, item_start) = self.output.push_anonymous_inline(parent_box_idx);

        let mut fragments = Vec::<InlineFragmentFrame>::new();
        for event in events {
            match event {
                SplitInlineEvent::Open { fragment, first_fragment } => {
                    let fragment_parent = fragments.last().map_or(anonymous_idx, |frame| frame.box_idx);
                    let frame = self.begin_inline_fragment(fragment.node, fragment_parent, fragment.style, first_fragment);
                    if first_fragment {
                        self.append_generated_inline_pseudo(fragment.node, frame.box_idx, true);
                        if fragment.start_edge {
                            self.output.push_item(InlineItemKind::InlineBoundary { inline_start: true, inline_end: false }, frame.box_idx, None);
                        }
                    }
                    fragments.push(frame);
                }
                SplitInlineEvent::Close { fragment, last_fragment } => {
                    let Some(frame) = fragments.pop() else { continue };
                    debug_assert_eq!(frame.node, fragment.node);
                    if last_fragment {
                        self.append_generated_inline_pseudo(fragment.node, frame.box_idx, false);
                        if fragment.end_edge {
                            self.output.push_item(InlineItemKind::InlineBoundary { inline_start: false, inline_end: true }, frame.box_idx, None);
                        }
                    }
                    self.output.set_inline_fragment_edges(frame.box_idx, frame.first_fragment, last_fragment);
                    self.finish_inline_fragment(frame, last_fragment);
                }
                SplitInlineEvent::Node(node) => {
                    let owner = fragments.last().map_or(anonymous_idx, |frame| frame.box_idx);
                    self.process_inline_node(node, owner);
                }
                SplitInlineEvent::EnterContents(node) => {
                    if let Some(directives) = self.styles.counter_directives_for_node(node) {
                        self.generated.apply(directives);
                    }
                }
                SplitInlineEvent::ExitContents => {}
                SplitInlineEvent::GeneratedPseudo { origin, before } => {
                    let owner = fragments.last().map_or(anonymous_idx, |frame| frame.box_idx);
                    self.append_generated_inline_pseudo(origin, owner, before);
                }
            }
        }
        debug_assert!(fragments.is_empty());
        self.output.finish_anonymous_box(anonymous_idx, item_start).then_some(anonymous_idx)
    }

    pub(super) fn build_inline_children(&mut self, elem_id: DomNodeId, box_idx: u32) {
        let dom = self.document;
        let Some(element) = dom.element_ref(elem_id) else {
            return;
        };

        let start = self.output.item_position();
        let image_idx = element.image_idx();
        let children: Vec<u32> = element.children().map(|idx| idx.raw()).collect();

        if let Some(image_idx) = image_idx {
            self.output.push_item(InlineItemKind::Image { image_idx }, box_idx, None);
        }

        self.append_generated_inline_pseudo(elem_id, box_idx, true);

        let owns_floated_first_letter = image_idx.is_none() && self.begin_floated_first_letter(elem_id, box_idx);

        // Collapsible whitespace at the edges of a block container does not
        // generate a line box. This matters in particular when all intervening
        // element children compute to display:none (for example the unusual
        // HTML elements whose display:contents used value is none).
        let children = if image_idx.is_none() && self.styles.before_style_for_node(elem_id).is_none() && self.styles.after_style_for_node(elem_id).is_none() {
            let white_space = self.output.style_for_box(box_idx as usize).white_space();
            trim_inline_segment_nodes(self.document, self.styles, &children, white_space)
        } else {
            &children
        };
        self.process_inline_nodes(children, box_idx);

        if owns_floated_first_letter {
            self.finish_floated_first_letter();
        }

        self.append_generated_inline_pseudo(elem_id, box_idx, false);

        let end = self.output.item_position();

        if start < end {
            self.output.set_box_children(box_idx, Children::InlineItems(start..end));
        }
    }

    pub(super) fn build_replaced_content(&mut self, box_idx: u32, image_idx: u32) {
        let start = self.output.item_position();
        self.output.push_item(InlineItemKind::Image { image_idx }, box_idx, None);
        self.output.set_box_children(box_idx, Children::InlineItems(start..self.output.item_position()));
    }

    pub(super) fn build_cell_children(&mut self, elem_id: DomNodeId, box_idx: u32) {
        let Some(element) = self.document.element_ref(elem_id) else {
            return;
        };

        if has_block_child(self.document, self.styles, element) {
            self.build_block_children(elem_id, box_idx);
        } else {
            self.build_inline_children(elem_id, box_idx);
        }
    }

    /// Each in-flow element child is a flex/grid item regardless of its outer
    /// display type. Consecutive text nodes become one anonymous item. This is
    /// deliberately distinct from block-flow construction, which wraps inline
    /// elements together with text in an anonymous block.
    pub(super) fn begin_inline_fragment(&mut self, node: DomNodeId, parent: u32, style: Option<StyleIndices>, first_fragment: bool) -> InlineFragmentFrame {
        if first_fragment {
            if let Some(directives) = self.styles.counter_directives_for_node(node) {
                self.generated.apply(directives);
            }
            self.generated.enter_sibling_scope();
        }
        let item_start = self.output.item_position();
        let box_idx = self.push_layout_box(node, Some(parent), style, LayoutMode::Inline(Range::default()));
        InlineFragmentFrame { node, box_idx, item_start, first_fragment }
    }

    pub(super) fn finish_inline_fragment(&mut self, frame: InlineFragmentFrame, last_fragment: bool) {
        self.output.finish_inline_box(frame.box_idx, frame.item_start);
        if last_fragment {
            self.generated.leave_sibling_scope();
        }
    }

    pub(super) fn process_inline_node(&mut self, node_id: DomNodeId, box_idx: u32) {
        match plan_inline_node(self.document, self.styles, node_id) {
            InlineNodePlan::Ignore => {}
            InlineNodePlan::Text(node) => self.process_text_node_with_contents_style(node, box_idx),
            InlineNodePlan::Break { clear } => self.output.push_item(InlineItemKind::Break { clear }, box_idx, None),
            InlineNodePlan::GeneratedBreak { node, style } => {
                let frame = self.begin_inline_fragment(node, box_idx, style, true);
                self.append_generated_inline_pseudo(node, frame.box_idx, true);
                self.append_generated_inline_pseudo(node, frame.box_idx, false);
                self.finish_inline_fragment(frame, true);
            }
            InlineNodePlan::OutOfFlow(node) => {
                if let Some(element_box) = self.build_box_for_element(node, Some(box_idx)) {
                    let kind = self
                        .styles
                        .style_for_node(node)
                        .and_then(|style| self.styles.layout_style(style))
                        .filter(|style| style.position == html_style_model::PositionMode::Absolute)
                        .map_or(InlineItemKind::FloatAnchor { box_idx: element_box }, |_| InlineItemKind::AbsoluteAnchor { box_idx: element_box });
                    self.output.push_item(kind, box_idx, None);
                }
            }
            InlineNodePlan::Atomic(node) => {
                if let Some(element_box) = self.build_box_for_element(node, Some(box_idx)) {
                    self.output.push_item(InlineItemKind::AtomicBox { box_idx: element_box }, element_box, None);
                }
            }
            InlineNodePlan::Contents { node, children } => {
                if let Some(directives) = self.styles.counter_directives_for_node(node) {
                    self.generated.apply(directives);
                }
                self.append_generated_inline_pseudo(node, box_idx, true);
                self.process_inline_nodes(&children, box_idx);
                self.append_generated_inline_pseudo(node, box_idx, false);
            }
            InlineNodePlan::Element(InlineElementPlan { fragment: InlineFragmentPlan { node, style, start_edge, end_edge }, image, children }) => {
                let frame = self.begin_inline_fragment(node, box_idx, style, true);
                if let Some(image_idx) = image {
                    self.output.push_item(InlineItemKind::Image { image_idx }, frame.box_idx, None);
                } else {
                    self.append_generated_inline_pseudo(node, frame.box_idx, true);
                    if children.is_empty() {
                        self.output.push_item(InlineItemKind::InlineBoundary { inline_start: start_edge, inline_end: end_edge }, frame.box_idx, None);
                    } else {
                        if start_edge {
                            self.output.push_item(InlineItemKind::InlineBoundary { inline_start: true, inline_end: false }, frame.box_idx, None);
                        }
                        self.process_inline_nodes(&children, frame.box_idx);
                        self.append_generated_inline_pseudo(node, frame.box_idx, false);
                        if end_edge {
                            self.output.push_item(InlineItemKind::InlineBoundary { inline_start: false, inline_end: true }, frame.box_idx, None);
                        }
                    }
                    if children.is_empty() {
                        self.append_generated_inline_pseudo(node, frame.box_idx, false);
                    }
                }
                self.finish_inline_fragment(frame, true);
            }
        }
    }

    pub(super) fn process_inline_nodes(&mut self, nodes: &[u32], box_idx: u32) {
        let mut table_segment = Vec::new();
        let mut pending_table_whitespace = Vec::new();
        for &raw in nodes {
            let Some(node_id) = self.document.node_id_from_raw(raw) else { continue };
            let table_internal = self.document.element_ref(node_id).is_some_and(|element| is_table_internal_display(display_for_element(self.styles, element)));
            if table_internal {
                table_segment.append(&mut pending_table_whitespace);
                table_segment.push(raw);
                continue;
            }
            if !table_segment.is_empty() && matches!(self.document.node_ref(node_id), Some(NodeRef::Text(text)) if text.text().chars().all(char::is_whitespace)) {
                pending_table_whitespace.push(raw);
                continue;
            }
            self.flush_inline_anonymous_table(&mut table_segment, box_idx);
            for whitespace in pending_table_whitespace.drain(..) {
                if let Some(node) = self.document.node_id_from_raw(whitespace) {
                    self.process_inline_node(node, box_idx);
                }
            }
            self.process_inline_node(node_id, box_idx);
        }
        self.flush_inline_anonymous_table(&mut table_segment, box_idx);
        for whitespace in pending_table_whitespace {
            if let Some(node) = self.document.node_id_from_raw(whitespace) {
                self.process_inline_node(node, box_idx);
            }
        }
    }

    pub(super) fn flush_inline_anonymous_table(&mut self, nodes: &mut Vec<u32>, parent_box_idx: u32) {
        let Some(table_idx) = self.flush_anonymous_table_segment(nodes, parent_box_idx) else { return };
        self.output.push_item(InlineItemKind::AtomicBox { box_idx: table_idx }, table_idx, None);
    }

    pub(super) fn process_text_node(&mut self, node_id: DomNodeId, box_idx: u32) {
        let Some(text_node) = self.document.text_ref(node_id) else {
            return;
        };
        let text = text_node.text();

        if text.is_empty() {
            return;
        }

        if self.floated_first_letter.is_none() {
            self.output.push_source_text(text, box_idx, node_id.raw());
            return;
        }

        let mut fragment_start = 0;
        let mut fragment_target = None;
        let mut source_utf16_offset = 0u32;
        for (byte_offset, character) in text.char_indices() {
            let target = self.first_letter_target_box(character, box_idx);
            if fragment_target.is_none() && target == box_idx && self.floated_first_letter.as_ref().is_some_and(|capture| capture.phase == FirstLetterCapturePhase::Done && capture.item_end.is_none()) {
                let end = self.output.item_position();
                self.floated_first_letter.as_mut().expect("active first-letter capture").item_end = Some(end);
            }
            if fragment_target.is_some() && fragment_target != Some(target) {
                let previous = fragment_target.expect("text fragment target");
                let fragment = &text[fragment_start..byte_offset];
                self.output.push_source_text_fragment(fragment, previous, node_id.raw(), source_utf16_offset);
                if self.floated_first_letter.as_ref().is_some_and(|capture| capture.box_idx == previous && capture.item_end.is_none()) {
                    let end = self.output.item_position();
                    self.floated_first_letter.as_mut().expect("active first-letter capture").item_end = Some(end);
                }
                source_utf16_offset += fragment.encode_utf16().count() as u32;
                fragment_start = byte_offset;
            }
            fragment_target = Some(target);
        }
        if let Some(target) = fragment_target {
            self.output.push_source_text_fragment(&text[fragment_start..], target, node_id.raw(), source_utf16_offset);
        }
    }

    pub(super) fn process_text_node_with_contents_style(&mut self, node_id: DomNodeId, box_idx: u32) {
        let Some(&style) = self.contents_text_styles.get(&node_id) else {
            self.process_text_node(node_id, box_idx);
            return;
        };
        let (carrier, item_start) = self.output.push_contents_style_box(box_idx, style);
        self.process_text_node(node_id, carrier);
        self.output.finish_inline_box(carrier, item_start);
    }

    pub(super) fn begin_floated_first_letter(&mut self, origin: DomNodeId, parent_box: u32) -> bool {
        if self.floated_first_letter.is_some() {
            return false;
        }
        let Some(style) = self.styles.first_letter_style_for_node(origin) else { return false };
        let Some(box_style) = self.styles.box_model_style(style) else { return false };
        if !matches!(box_style.float, Float::Left | Float::Right) {
            return false;
        }

        let pseudo_box = self.output.push_synthetic_box(parent_box, style, LayoutMode::Block(BlockBox { children: Children::Empty }));
        self.output.push_item(InlineItemKind::FloatAnchor { box_idx: pseudo_box }, parent_box, None);
        self.floated_first_letter = Some(FloatedFirstLetterCapture { box_idx: pseudo_box, item_start: self.output.item_position(), item_end: None, phase: FirstLetterCapturePhase::Leading });
        true
    }

    pub(super) fn first_letter_target_box(&mut self, character: char, ordinary_box: u32) -> u32 {
        let Some(capture) = self.floated_first_letter.as_mut() else { return ordinary_box };
        let selected = match capture.phase {
            FirstLetterCapturePhase::Leading if character.is_whitespace() => true,
            FirstLetterCapturePhase::Leading if is_first_letter_punctuation(character) => true,
            FirstLetterCapturePhase::Leading => {
                capture.phase = FirstLetterCapturePhase::Letter;
                true
            }
            FirstLetterCapturePhase::Letter if character.is_mark() => true,
            FirstLetterCapturePhase::Letter if is_first_letter_punctuation(character) => {
                capture.phase = FirstLetterCapturePhase::Trailing;
                true
            }
            FirstLetterCapturePhase::Letter => false,
            FirstLetterCapturePhase::Trailing if character.is_mark() || is_first_letter_punctuation(character) => true,
            FirstLetterCapturePhase::Trailing => false,
            FirstLetterCapturePhase::Done => false,
        };
        if selected {
            capture.box_idx
        } else {
            capture.phase = FirstLetterCapturePhase::Done;
            ordinary_box
        }
    }

    pub(super) fn finish_floated_first_letter(&mut self) {
        let Some(capture) = self.floated_first_letter.take() else { return };
        let item_end = capture.item_end.unwrap_or_else(|| self.output.item_position());
        if capture.item_start < item_end {
            self.output.set_box_children(capture.box_idx, Children::InlineItems(capture.item_start..item_end));
        }
    }

    pub(super) fn append_generated_inline_pseudo(&mut self, origin: DomNodeId, parent_box: u32, before: bool) {
        let Some(display) = self.generated.pseudo_display(origin, before) else { return };
        if matches!(display, Display::None | Display::TableColumn | Display::TableColumnGroup) {
            return;
        }
        let resolved = self.generated.resolve_pseudo(origin, before).expect("pseudo existence was checked above");
        self.output.emit_generated_inline(parent_box, plan_generated_inline(self.styles, resolved));
    }

    /// Build a generated principal box that participates in block flow.  A
    /// pseudo-element is an actual CSS box, not merely a text run: its own
    /// margins, borders, sizing, display, and list marker all belong to this
    /// synthetic box just inside the originating element.
    pub(super) fn build_generated_block_pseudo(&mut self, origin: DomNodeId, parent_box: u32, before: bool) -> Option<u32> {
        if !self.generated.pseudo_is_block_level(origin, before) {
            return None;
        }
        let resolved = self.generated.resolve_pseudo(origin, before)?;
        match resolved.display {
            Display::Block | Display::FlowRoot | Display::ListItem | Display::FlowRootListItem => {
                let style = resolved.style;
                let pseudo_box = self.output.push_synthetic_box(parent_box, style, LayoutMode::Block(BlockBox { children: Children::Empty }));
                self.output.set_generated_text_children(pseudo_box, style, &resolved.text, false);
                if matches!(resolved.display, Display::ListItem | Display::FlowRootListItem) {
                    self.build_list_marker_for_ordinal(pseudo_box, style, 1);
                }
                Some(pseudo_box)
            }
            _ => self.output.materialize_generated_table(parent_box, plan_generated_table_principal(self.styles, resolved)?),
        }
    }

    pub(super) fn flush_inline_segment(&mut self, nodes: &mut Vec<u32>, parent_box_idx: u32) -> Option<u32> {
        self.flush_inline_segment_with_generated_edges(nodes, parent_box_idx, None, None)
    }

    pub(super) fn flush_inline_segment_with_generated_edges(&mut self, nodes: &mut Vec<u32>, parent_box_idx: u32, before: Option<DomNodeId>, after: Option<DomNodeId>) -> Option<u32> {
        if nodes.is_empty() && before.is_none() && after.is_none() {
            return None;
        }

        let white_space = self.output.style_for_box(parent_box_idx as usize).white_space();
        let segment = trim_inline_segment_nodes(self.document, self.styles, nodes.as_slice(), white_space);
        if segment.is_empty() && before.is_none() && after.is_none() {
            nodes.clear();
            return None;
        }

        let (anonymous_idx, item_start) = self.output.push_anonymous_inline(parent_box_idx);
        if let Some(origin) = before {
            self.append_generated_inline_pseudo(origin, anonymous_idx, true);
        }
        for &node_idx in segment {
            let Some(node_id) = self.document.node_id_from_raw(node_idx) else {
                continue;
            };
            self.process_inline_node(node_id, anonymous_idx);
        }
        if let Some(origin) = after {
            self.append_generated_inline_pseudo(origin, anonymous_idx, false);
        }
        nodes.clear();
        self.output.finish_anonymous_box(anonymous_idx, item_start);
        Some(anonymous_idx)
    }

}
