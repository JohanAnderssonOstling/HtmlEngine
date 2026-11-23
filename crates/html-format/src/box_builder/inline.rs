use html_dom::{Document, DomNodeId, ElementRef, NodeRef};
use html_style_model::{Clear, ComputedStyles, Display, StyleIndices, StyleView, WhiteSpace};
use html_text::is_document_white_space;
use std::ops::Range;

use super::classification::BoxClassification;
use super::{BoxSource, LayoutTreeBuilder};

pub(super) enum InlineNodePlan<'a> {
    Ignore,
    Text(DomNodeId),
    Break {
        clear: Clear,
    },
    WordBreak {
        element: ElementRef<'a>,
        style: Option<StyleIndices>,
    },
    GeneratedBreak {
        element: ElementRef<'a>,
        style: Option<StyleIndices>,
    },
    OutOfFlow {
        node: DomNodeId,
        classification: BoxClassification,
    },
    Atomic {
        node: DomNodeId,
        classification: BoxClassification,
    },
    Contents(ElementRef<'a>),
    Element {
        fragment: InlineFragmentPlan,
        element: ElementRef<'a>,
    },
}

#[derive(Clone, Copy)]
pub(super) struct InlineFragmentPlan {
    pub(super) node: DomNodeId,
    pub(super) style: Option<StyleIndices>,
    pub(super) start_edge: bool,
    pub(super) end_edge: bool,
}

pub(super) fn plan_inline_fragment(
    styles: &ComputedStyles,
    node: DomNodeId,
    style: Option<StyleIndices>,
) -> InlineFragmentPlan {
    let (start_edge, end_edge) = inline_edges(styles, style);
    InlineFragmentPlan {
        node,
        style,
        start_edge,
        end_edge,
    }
}

fn inline_edges(styles: &ComputedStyles, style: Option<StyleIndices>) -> (bool, bool) {
    style
        .and_then(|indices| styles.view(indices))
        .map_or((false, false), |view| view.inline_edges())
}

#[derive(Clone, Copy)]
pub(super) enum SplitInlineEvent {
    Open {
        fragment: InlineFragmentPlan,
        first_fragment: bool,
    },
    Close {
        fragment: InlineFragmentPlan,
        last_fragment: bool,
    },
    EnterContents(DomNodeId),
    ExitContents,
    Source(BoxSource),
}

pub(super) fn trimmed_node_range(
    nodes: &[DomNodeId],
    mut ignorable: impl FnMut(DomNodeId) -> bool,
) -> Range<usize> {
    let start = nodes
        .iter()
        .position(|&node| !ignorable(node))
        .unwrap_or(nodes.len());
    let end = nodes[start..]
        .iter()
        .rposition(|&node| !ignorable(node))
        .map_or(start, |index| start + index + 1);
    start..end
}

pub(super) fn text_is_ignorable_at_block_edge(text: &str, white_space: WhiteSpace) -> bool {
    if white_space.preserves_spaces() {
        false
    } else if white_space.preserves_newlines() {
        !text.is_empty()
            && text
                .chars()
                .all(|character| matches!(character, ' ' | '\t' | '\r'))
    } else {
        !text.is_empty()
            && text
                .chars()
                .all(|character| is_document_white_space(character))
    }
}

fn text_has_renderable_content(text: &str, white_space: WhiteSpace) -> bool {
    if white_space.preserves_spaces() {
        !text.is_empty()
    } else if white_space.preserves_newlines() {
        text.chars()
            .any(|character| character == '\n' || !is_document_white_space(character))
    } else {
        text.chars()
            .any(|character| !is_document_white_space(character))
    }
}

pub(super) fn style_for_dom_node<'a>(
    document: &Document,
    styles: &'a ComputedStyles,
    node: DomNodeId,
) -> StyleView<'a> {
    let owner = match document.node_ref(node) {
        NodeRef::Element(element) => Some(element.node_id()),
        NodeRef::Text(text) => text.parent(),
    };
    let indices = owner
        .and_then(|owner| styles.style_for_node(owner))
        .unwrap_or_else(|| styles.default_indices());
    styles.view(indices).expect("validated style handle")
}

impl<'a, 'out> LayoutTreeBuilder<'a, 'out> {
    pub(super) fn plan_inline_node(&self, node: DomNodeId) -> InlineNodePlan<'a> {
        let node_ref = self.document.node_ref(node);
        let NodeRef::Element(element) = node_ref else {
            return InlineNodePlan::Text(node);
        };
        let classification = self.classifications.get(element);
        let display = classification.display;
        if display == Display::None {
            return InlineNodePlan::Ignore;
        }

        if display == Display::Contents {
            return InlineNodePlan::Contents(element);
        }

        let style = classification.style;
        if element.tag().eq_ignore_ascii_case("br") {
            if self.styles.before_style_for_node(node).is_some()
                || self.styles.after_style_for_node(node).is_some()
            {
                return InlineNodePlan::GeneratedBreak { element, style };
            }
            let clear = style
                .and_then(|indices| self.styles.box_model_style(indices))
                .map_or(Clear::None, |box_style| box_style.clear);
            return InlineNodePlan::Break { clear };
        }

        if element.tag().eq_ignore_ascii_case("wbr") {
            return InlineNodePlan::WordBreak { element, style };
        }

        let absolute = classification.absolute;
        if absolute || classification.floated {
            return InlineNodePlan::OutOfFlow {
                node,
                classification,
            };
        }
        if display.is_atomic_inline() {
            return InlineNodePlan::Atomic {
                node,
                classification,
            };
        }

        let fragment = plan_inline_fragment(self.styles, node, style);
        InlineNodePlan::Element { fragment, element }
    }

    pub(super) fn trim_inline_segment_range(
        &self,
        nodes: &[DomNodeId],
        white_space: WhiteSpace,
    ) -> Range<usize> {
        let range = trimmed_node_range(nodes, |node| {
            self.node_is_ignorable_at_block_edge(node, white_space)
        });
        if nodes[range.clone()]
            .iter()
            .any(|&node| self.node_has_renderable_inline_content(node))
        {
            range
        } else {
            range.start..range.start
        }
    }

    pub(super) fn node_is_ignorable_at_block_edge(
        &self,
        node: DomNodeId,
        white_space: WhiteSpace,
    ) -> bool {
        match self.document.node_ref(node) {
            NodeRef::Text(text) => text_is_ignorable_at_block_edge(text.text(), white_space),
            NodeRef::Element(_) => !self.node_has_renderable_inline_content(node),
        }
    }

    pub(super) fn node_has_renderable_inline_content(&self, node: DomNodeId) -> bool {
        match self.document.node_ref(node) {
            NodeRef::Text(text) => {
                let Some(parent) = text.parent() else {
                    return false;
                };
                text_has_renderable_content(
                    text.text(),
                    style_for_dom_node(self.document, self.styles, parent).white_space(),
                )
            }
            NodeRef::Element(element) => {
                let classification = self.classifications.get(element);
                if classification.style.is_none() {
                    return false;
                }
                let display = classification.display;
                if display == Display::None {
                    return false;
                }
                // Empty inline elements can still affect following generated
                // content through counters. Edge-whitespace trimming must retain
                // those nodes even though they emit no glyph or box edge.
                if self.styles.counter_directives_for_node(node).is_some() {
                    return true;
                }
                if self.styles.before_style_for_node(node).is_some()
                    || self.styles.after_style_for_node(node).is_some()
                {
                    return true;
                }
                if display.is_atomic_inline() || classification.floated || classification.absolute {
                    return true;
                }
                if element.tag().eq_ignore_ascii_case("br") || element.image_idx().is_some() {
                    return true;
                }
                // CSS 2 line-height calculation includes empty inline boxes even
                // when they have no painted edge or text. Retaining the element
                // lets its zero-width boundary contribute the selected font strut.
                if display == Display::Inline {
                    return true;
                }
                element
                    .children()
                    .any(|child| self.node_has_renderable_inline_content(child))
            }
        }
    }
}
