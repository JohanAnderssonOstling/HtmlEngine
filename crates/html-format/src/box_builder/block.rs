//! Build block children in source order, buffering only the current segment.

use super::*;
use html_dom::ElementRef;

struct BlockSegment {
    start: usize,
    table: bool,
    before: bool,
    origin: DomNodeId,
}

impl LayoutTreeBuilder<'_, '_> {
    pub(super) fn append_block_children(
        &mut self,
        node: DomNodeId,
        parent: u32,
        children: &mut Vec<u32>,
    ) {
        let Some(element) = self.document.try_element_ref(node) else {
            return;
        };
        let wrap_tables = !matches!(
            self.classifications.get(element).display,
            Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup
        );
        let before = self.classify_source(BoxSource::GeneratedPseudo {
            origin: node,
            before: true,
        });
        let after = self.classify_source(BoxSource::GeneratedPseudo {
            origin: node,
            before: false,
        });
        let mut segment = BlockSegment {
            start: self.node_scratch.len(),
            table: false,
            before: before.is_some_and(|value| !value.splits_inline()),
            origin: node,
        };
        if let Some(classification) = before.filter(|value| value.splits_inline()) {
            children.extend(self.build_source(
                BoxSource::GeneratedPseudo {
                    origin: node,
                    before: true,
                },
                parent,
                classification,
            ));
        }
        self.append_block_nodes(element, parent, wrap_tables, &mut segment, children);
        if segment.table {
            self.flush_block_segment(&mut segment, parent, false, children);
        }
        self.flush_block_segment(
            &mut segment,
            parent,
            after.is_some_and(|value| !value.splits_inline()),
            children,
        );
        if let Some(classification) = after.filter(|value| value.splits_inline()) {
            children.extend(self.build_source(
                BoxSource::GeneratedPseudo {
                    origin: node,
                    before: false,
                },
                parent,
                classification,
            ));
        }
        debug_assert_eq!(self.node_scratch.len(), segment.start);
    }

    fn append_block_nodes(
        &mut self,
        element: ElementRef<'_>,
        parent: u32,
        wrap_tables: bool,
        segment: &mut BlockSegment,
        children: &mut Vec<u32>,
    ) {
        for node in element.children() {
            let Some(child) = self.document.try_element_ref(node) else {
                self.node_scratch.push(node);
                continue;
            };
            let classification = self.classifications.get(child);
            let display = classification.display;
            if display == Display::Contents && classification.table_internal_descendant {
                let has_pseudos = self.styles.before_style_for_node(node).is_some()
                    || self.styles.after_style_for_node(node).is_some();
                if !has_pseudos && self.styles.counter_directives_for_node(node).is_none() {
                    self.append_block_nodes(child, parent, wrap_tables, segment, children);
                } else {
                    self.flush_block_segment(segment, parent, false, children);
                    self.generated.apply_for_node(node);
                    self.append_block_children(node, parent, children);
                }
                continue;
            }
            if wrap_tables && display.is_table_internal() {
                if !segment.table {
                    self.flush_block_segment(segment, parent, false, children);
                }
                segment.table = true;
                self.node_scratch.push(node);
                continue;
            }
            if segment.table {
                self.flush_block_segment(segment, parent, false, children);
            }
            if classification.starts_block() {
                self.flush_block_segment(segment, parent, false, children);
                children.extend(self.build_classified_element(node, Some(parent), classification));
            } else if matches!(display, Display::Inline | Display::Contents)
                && classification.has_block_child
            {
                self.flush_block_segment(segment, parent, false, children);
                self.build_split_inline_child(node, parent, children);
            } else {
                self.node_scratch.push(node);
            }
        }
    }

    fn flush_block_segment(
        &mut self,
        segment: &mut BlockSegment,
        parent: u32,
        after: bool,
        children: &mut Vec<u32>,
    ) {
        if segment.table {
            let plan = plan_table_children(
                self.document,
                self.styles,
                &self.classifications,
                self.node_scratch[segment.start..].iter().copied(),
            );
            self.node_scratch.truncate(segment.start);
            let table = self
                .output
                .push_anonymous_child(parent, LayoutMode::Table(Default::default()));
            self.materialize_table_plan(plan, table);
            children.push(table);
            segment.table = false;
            return;
        }
        let white_space = self.output.style_for_box(parent as usize).white_space();
        let nodes = &self.node_scratch[segment.start..];
        let range = self.trim_inline_segment_range(nodes, white_space);
        let start = segment.start + range.start;
        let end = segment.start + range.end;
        let before = std::mem::take(&mut segment.before);
        if start < end || before || after {
            let (anonymous, item_start) = self.output.push_anonymous_inline(parent);
            if before {
                self.append_generated_inline_pseudo(segment.origin, anonymous, true);
            }
            for index in start..end {
                let node = self.node_scratch[index];
                self.process_inline_node(node, anonymous);
            }
            if after {
                self.append_generated_inline_pseudo(segment.origin, anonymous, false);
            }
            self.output.finish_inline_box(anonymous, item_start);
            children.push(anonymous);
        }
        self.node_scratch.truncate(segment.start);
    }
}
