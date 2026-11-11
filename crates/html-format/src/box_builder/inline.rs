use html_dom::{Document, DomNodeId, ElementRef, NodeRef};
use html_style_model::{Clear, ComputedStyles, Display, StyleIndices, StyleView, WhiteSpace};
use html_text::is_document_white_space;

use super::classification::{
    ElementClassification, display_for_element, has_table_internal_descendant, pseudo_display,
    pseudo_is_block_level,
};
use super::generated::ResolvedGeneratedContent;

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
        classification: ElementClassification,
    },
    Atomic {
        node: DomNodeId,
        classification: ElementClassification,
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

pub(super) struct GeneratedInlinePlan {
    pub(super) content: ResolvedGeneratedContent,
    pub(super) start_edge: bool,
    pub(super) end_edge: bool,
}

pub(super) fn plan_inline_node<'a>(
    document: &'a Document,
    styles: &ComputedStyles,
    node: DomNodeId,
    classification: Option<ElementClassification>,
) -> InlineNodePlan<'a> {
    let node_ref = document.node_ref(node);
    let NodeRef::Element(element) = node_ref else {
        return InlineNodePlan::Text(node);
    };
    let classification =
        classification.unwrap_or_else(|| ElementClassification::new(styles, element));
    let display = classification.display;
    if display == Display::None {
        return InlineNodePlan::Ignore;
    }

    if display == Display::Contents {
        return InlineNodePlan::Contents(element);
    }

    let style = classification.style;
    if element.tag().eq_ignore_ascii_case("br") {
        if styles.before_style_for_node(node).is_some()
            || styles.after_style_for_node(node).is_some()
        {
            return InlineNodePlan::GeneratedBreak { element, style };
        }
        let clear = style
            .and_then(|indices| styles.box_model_style(indices))
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

    let fragment = plan_inline_fragment(styles, node, style);
    InlineNodePlan::Element { fragment, element }
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

pub(super) fn plan_generated_inline(
    styles: &ComputedStyles,
    content: ResolvedGeneratedContent,
) -> GeneratedInlinePlan {
    let (start_edge, end_edge) = if content.display == Display::Contents {
        (false, false)
    } else {
        inline_edges(styles, Some(content.style))
    };
    GeneratedInlinePlan {
        content,
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
    GeneratedPseudo {
        origin: DomNodeId,
        before: bool,
    },
    Node(DomNodeId),
}

pub(super) fn has_block_child(styles: &ComputedStyles, element: ElementRef<'_>) -> bool {
    // Replaced elements are atomic CSS boxes. Their source DOM can describe
    // the resource (for example an inline SVG tree), but those descendants
    // never participate in block-in-inline splitting of the surrounding HTML.
    if element.image_idx().is_some() {
        return false;
    }
    let inline_container = matches!(
        display_for_element(styles, element),
        Display::Inline | Display::Contents
    );
    pseudo_is_block_level(styles, element.node_id(), true)
        || element.child_elements().any(|child| {
            let classification = ElementClassification::new(styles, child);
            let display = classification.display;
            let starts_block = if classification.absolute || classification.floated {
                false
            } else if inline_container {
                classification.splits_inline()
            } else {
                classification.starts_block()
            };
            starts_block
                || (display == Display::Contents && has_table_internal_descendant(styles, child))
                || (matches!(display, Display::Inline | Display::Contents)
                    && has_inline_splitting_block_descendant(styles, child))
        })
        || pseudo_is_block_level(styles, element.node_id(), false)
}

pub(super) fn has_inline_splitting_block_descendant(
    styles: &ComputedStyles,
    element: ElementRef<'_>,
) -> bool {
    if element.image_idx().is_some() {
        return false;
    }
    pseudo_is_block_level(styles, element.node_id(), true)
        || element.child_elements().any(|child| {
            let classification = ElementClassification::new(styles, child);
            let display = classification.display;
            classification.splits_inline()
                || (matches!(display, Display::Inline | Display::Contents)
                    && has_inline_splitting_block_descendant(styles, child))
        })
        || pseudo_is_block_level(styles, element.node_id(), false)
}

pub(super) fn trim_inline_segment_nodes<'a>(
    document: &Document,
    styles: &ComputedStyles,
    nodes: &'a [DomNodeId],
    white_space: WhiteSpace,
) -> &'a [DomNodeId] {
    &nodes[trim_inline_segment_range(document, styles, nodes, white_space)]
}

pub(super) fn trim_inline_segment_range(
    document: &Document,
    styles: &ComputedStyles,
    nodes: &[DomNodeId],
    white_space: WhiteSpace,
) -> std::ops::Range<usize> {
    let ignorable = |node| node_is_ignorable_at_block_edge(document, styles, node, white_space);
    let start = nodes
        .iter()
        .position(|&node| !ignorable(node))
        .unwrap_or(nodes.len());
    let end = nodes[start..]
        .iter()
        .rposition(|&node| !ignorable(node))
        .map_or(start, |index| start + index + 1);
    if nodes[start..end]
        .iter()
        .any(|&node| node_has_renderable_inline_content(document, styles, node))
    {
        start..end
    } else {
        start..start
    }
}

pub(super) fn trim_node_edges(
    nodes: &[DomNodeId],
    mut ignorable: impl FnMut(DomNodeId) -> bool,
) -> &[DomNodeId] {
    let start = nodes
        .iter()
        .position(|&node| !ignorable(node))
        .unwrap_or(nodes.len());
    let nodes = &nodes[start..];
    let end = nodes
        .iter()
        .rposition(|&node| !ignorable(node))
        .map_or(0, |index| index + 1);
    &nodes[..end]
}

pub(super) fn node_is_ignorable_at_block_edge(
    document: &Document,
    styles: &ComputedStyles,
    node: DomNodeId,
    white_space: WhiteSpace,
) -> bool {
    match document.node_ref(node) {
        NodeRef::Text(text) => text_is_ignorable_at_block_edge(text.text(), white_space),
        NodeRef::Element(_) => !node_has_renderable_inline_content(document, styles, node),
    }
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

pub(super) fn node_has_renderable_inline_content(
    document: &Document,
    styles: &ComputedStyles,
    node: DomNodeId,
) -> bool {
    match document.node_ref(node) {
        NodeRef::Text(text) => {
            let Some(parent) = text.parent() else {
                return false;
            };
            text_has_renderable_content(
                text.text(),
                style_for_dom_node(document, styles, parent).white_space(),
            )
        }
        NodeRef::Element(element) => {
            let classification = ElementClassification::new(styles, element);
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
            if styles.counter_directives_for_node(node).is_some() {
                return true;
            }
            if styles.before_style_for_node(node).is_some()
                || styles.after_style_for_node(node).is_some()
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
                .any(|child| node_has_renderable_inline_content(document, styles, child))
        }
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
