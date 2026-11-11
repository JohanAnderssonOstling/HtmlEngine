//! Build block children in source order, buffering only the current segment.

use super::classification::has_table_internal_descendant;
use super::*;
use html_dom::ElementRef;

#[derive(Clone, Copy, PartialEq, Eq)]
enum GeneratedEdge {
    None,
    Inline,
    Block,
}

fn generated_edge(styles: &ComputedStyles, origin: DomNodeId, before: bool) -> GeneratedEdge {
    match pseudo_display(styles, origin, before) {
        None => GeneratedEdge::None,
        Some(_) if pseudo_is_block_level(styles, origin, before) => GeneratedEdge::Block,
        Some(_) => GeneratedEdge::Inline,
    }
}

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
            display_for_element(self.styles, element),
            Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup
        );
        let before = generated_edge(self.styles, node, true);
        let after = generated_edge(self.styles, node, false);
        let mut segment = BlockSegment {
            start: self.node_scratch.len(),
            table: false,
            before: before == GeneratedEdge::Inline,
            origin: node,
        };
        if before == GeneratedEdge::Block {
            children.extend(self.build_generated_block_pseudo(node, parent, true));
        }
        self.append_block_nodes(element, parent, wrap_tables, &mut segment, children);
        if segment.table {
            self.flush_block_segment(&mut segment, parent, false, children);
        }
        self.flush_block_segment(
            &mut segment,
            parent,
            after == GeneratedEdge::Inline,
            children,
        );
        if after == GeneratedEdge::Block {
            children.extend(self.build_generated_block_pseudo(node, parent, false));
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
            let classification = ElementClassification::new(self.styles, child);
            let display = classification.display;
            if display == Display::Contents && has_table_internal_descendant(self.styles, child) {
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
                && has_block_child(self.styles, child)
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
        let range = super::inline::trim_inline_segment_range(
            self.document,
            self.styles,
            nodes,
            white_space,
        );
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
