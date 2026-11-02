use html_text::is_document_white_space;
use html_dom::{Document, DomNodeId, ElementRef, NodeRef};
use html_style_model::{
    Clear, ComputedStyles, Display, PositionMode, StyleIndices, StyleView, WhiteSpace,
};

use super::classification::{
    display_for_element, display_for_style, has_table_internal_descendant,
    is_absolutely_positioned, is_floated, pseudo_display,
    pseudo_is_block_level, starts_inline_split_child, starts_new_block_child_for_element,
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
        absolute: bool,
    },
    Atomic(DomNodeId),
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
) -> InlineNodePlan<'a> {
    let Some(node_ref) = document.node_ref(node) else {
        return InlineNodePlan::Ignore;
    };
    let NodeRef::Element(element) = node_ref else {
        return InlineNodePlan::Text(node);
    };
    let display = display_for_element(styles, element);
    if display == Display::None {
        return InlineNodePlan::Ignore;
    }

    if display == Display::Contents {
        return InlineNodePlan::Contents(element);
    }

    let style = styles.style_for_node(node);
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

    let absolute = is_absolutely_positioned(styles, element);
    if absolute || is_floated(styles, element) {
        return InlineNodePlan::OutOfFlow { node, absolute };
    }
    if display.is_atomic_inline() {
        return InlineNodePlan::Atomic(node);
    }

    let fragment = plan_inline_fragment(styles, node, style);
    InlineNodePlan::Element { fragment, element }
}

fn plan_inline_fragment(
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

pub(super) enum SplitInlinePart {
    Segment(Vec<SplitInlineEvent>),
    Block {
        node: DomNodeId,
        position_ancestors: Vec<StyleIndices>,
    },
    GeneratedBlock {
        origin: DomNodeId,
        before: bool,
        position_ancestors: Vec<StyleIndices>,
    },
}

enum CollectedSplitInlineEvent {
    Inline(SplitInlineEvent),
    Block(DomNodeId),
    GeneratedBlock { origin: DomNodeId, before: bool },
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
            let display = display_for_element(styles, child);
            let starts_block =
                if is_absolutely_positioned(styles, child) || is_floated(styles, child) {
                    false
                } else if inline_container {
                    starts_inline_split_child(styles, child)
                } else {
                    starts_new_block_child_for_element(styles, child)
                };
            starts_block
                || (display == Display::Contents && has_table_internal_descendant(styles, child))
                || (matches!(display, Display::Inline | Display::Contents)
                    && has_inline_splitting_block_descendant(styles, child))
        })
        || pseudo_is_block_level(styles, element.node_id(), false)
}

pub(super) fn plan_split_inline(
    document: &Document,
    styles: &ComputedStyles,
    element: DomNodeId,
) -> Vec<SplitInlinePart> {
    let mut events = Vec::new();
    collect_split_inline_events(document, styles, element, &mut events);

    let mut parts = Vec::new();
    let mut active = Vec::<InlineFragmentPlan>::new();
    let mut segment = Vec::new();
    for event in events {
        match event {
            CollectedSplitInlineEvent::Inline(event @ SplitInlineEvent::Open { fragment, .. }) => {
                active.push(fragment);
                segment.push(event);
            }
            CollectedSplitInlineEvent::Inline(event @ SplitInlineEvent::Close { fragment, .. }) => {
                segment.push(event);
                debug_assert_eq!(active.pop().map(|active| active.node), Some(fragment.node));
            }
            CollectedSplitInlineEvent::Inline(event) => segment.push(event),
            block @ (CollectedSplitInlineEvent::Block(node)
            | CollectedSplitInlineEvent::GeneratedBlock { origin: node, .. }) => {
                segment.extend(active.iter().rev().copied().map(|fragment| {
                    SplitInlineEvent::Close {
                        fragment,
                        last_fragment: false,
                    }
                }));
                finish_split_inline_segment(document, styles, &mut segment, &mut parts);
                let position_ancestors = active
                    .iter()
                    .filter_map(|fragment| fragment.style)
                    .filter(|&style| {
                        styles
                            .layout_style(style)
                            .is_some_and(|layout| layout.position == PositionMode::Relative)
                    })
                    .collect();
                parts.push(
                    if let CollectedSplitInlineEvent::GeneratedBlock { before, .. } = block {
                        SplitInlinePart::GeneratedBlock {
                            origin: node,
                            before,
                            position_ancestors,
                        }
                    } else {
                        SplitInlinePart::Block {
                            node,
                            position_ancestors,
                        }
                    },
                );
                segment.extend(
                    active
                        .iter()
                        .copied()
                        .map(|fragment| SplitInlineEvent::Open {
                            fragment,
                            first_fragment: false,
                        }),
                );
            }
        }
    }
    debug_assert!(active.is_empty());
    finish_split_inline_segment(document, styles, &mut segment, &mut parts);
    parts
}

fn collect_split_inline_events(
    document: &Document,
    styles: &ComputedStyles,
    element_id: DomNodeId,
    events: &mut Vec<CollectedSplitInlineEvent>,
) {
    let Some(element) = document.element_ref(element_id) else {
        return;
    };
    if display_for_element(styles, element) == Display::Contents {
        events.push(CollectedSplitInlineEvent::Inline(
            SplitInlineEvent::EnterContents(element_id),
        ));
        collect_split_pseudo(styles, element_id, true, events);
        collect_split_children(document, styles, element, events);
        collect_split_pseudo(styles, element_id, false, events);
        events.push(CollectedSplitInlineEvent::Inline(
            SplitInlineEvent::ExitContents,
        ));
        return;
    }
    let fragment = plan_inline_fragment(styles, element_id, styles.style_for_node(element_id));
    events.push(CollectedSplitInlineEvent::Inline(SplitInlineEvent::Open {
        fragment,
        first_fragment: true,
    }));
    collect_split_children(document, styles, element, events);
    events.push(CollectedSplitInlineEvent::Inline(SplitInlineEvent::Close {
        fragment,
        last_fragment: true,
    }));
}

fn collect_split_children(
    document: &Document,
    styles: &ComputedStyles,
    element: ElementRef<'_>,
    events: &mut Vec<CollectedSplitInlineEvent>,
) {
    for child_id in element.children() {
        let Some(child) = document.element_ref(child_id) else {
            events.push(CollectedSplitInlineEvent::Inline(SplitInlineEvent::Node(
                child_id,
            )));
            continue;
        };
        let display = display_for_element(styles, child);
        if starts_inline_split_child(styles, child) {
            events.push(CollectedSplitInlineEvent::Block(child_id));
        } else if matches!(display, Display::Inline | Display::Contents)
            && has_inline_splitting_block_descendant(styles, child)
        {
            collect_split_inline_events(document, styles, child_id, events);
        } else {
            events.push(CollectedSplitInlineEvent::Inline(SplitInlineEvent::Node(
                child_id,
            )));
        }
    }
}

fn collect_split_pseudo(
    styles: &ComputedStyles,
    origin: DomNodeId,
    before: bool,
    events: &mut Vec<CollectedSplitInlineEvent>,
) {
    let Some(display) = pseudo_display(styles, origin, before) else {
        return;
    };
    if display == Display::None {
        return;
    }
    if pseudo_is_block_level(styles, origin, before) {
        events.push(CollectedSplitInlineEvent::GeneratedBlock { origin, before });
    } else {
        events.push(CollectedSplitInlineEvent::Inline(
            SplitInlineEvent::GeneratedPseudo { origin, before },
        ));
    }
}

fn finish_split_inline_segment(
    document: &Document,
    styles: &ComputedStyles,
    events: &mut Vec<SplitInlineEvent>,
    parts: &mut Vec<SplitInlinePart>,
) {
    if events.is_empty() {
        return;
    }
    let ignorable = |node| {
        node_is_ignorable_at_block_edge(
            document,
            styles,
            node,
            style_for_dom_node(document, styles, node).white_space(),
        )
    };
    let mut content = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            SplitInlineEvent::Node(node) if !ignorable(*node) => Some(index),
            _ => None,
        });
    let first_content = content.next();
    let last_content = content.next_back().or(first_content);
    let events = std::mem::take(events)
        .into_iter()
        .enumerate()
        .filter_map(|(event_idx, event)| match event {
            SplitInlineEvent::Node(node)
                if first_content
                    .zip(last_content)
                    .is_none_or(|(first, last)| event_idx < first || event_idx > last)
                    && ignorable(node) =>
            {
                None
            }
            event => Some(event),
        })
        .collect();
    parts.push(SplitInlinePart::Segment(events));
}

fn has_inline_splitting_block_descendant(styles: &ComputedStyles, element: ElementRef<'_>) -> bool {
    if element.image_idx().is_some() {
        return false;
    }
    pseudo_is_block_level(styles, element.node_id(), true)
        || element.child_elements().any(|child| {
            let display = display_for_element(styles, child);
            starts_inline_split_child(styles, child)
                || (matches!(display, Display::Inline | Display::Contents)
                    && has_inline_splitting_block_descendant(styles, child))
        })
        || pseudo_is_block_level(styles, element.node_id(), false)
}

pub(super) fn trim_inline_segment_nodes<'a>(
    document: &Document,
    styles: &ComputedStyles,
    nodes: &'a [u32],
    white_space: WhiteSpace,
) -> &'a [u32] {
    let segment = trim_node_edges(nodes, |raw| {
        document.node_id_from_raw(raw).is_some_and(|node| {
            node_is_ignorable_at_block_edge(document, styles, node, white_space)
        })
    });
    if segment
        .iter()
        .copied()
        .filter_map(|raw| document.node_id_from_raw(raw))
        .any(|node| node_has_renderable_inline_content(document, styles, node))
    {
        segment
    } else {
        &nodes[0..0]
    }
}

pub(super) fn trim_node_edges(nodes: &[u32], mut ignorable: impl FnMut(u32) -> bool) -> &[u32] {
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

fn node_is_ignorable_at_block_edge(
    document: &Document,
    styles: &ComputedStyles,
    node: DomNodeId,
    white_space: WhiteSpace,
) -> bool {
    match document.node_ref(node) {
        Some(NodeRef::Text(text)) => text_is_ignorable_at_block_edge(text.text(), white_space),
        Some(NodeRef::Element(_)) => !node_has_renderable_inline_content(document, styles, node),
        None => false,
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

fn node_has_renderable_inline_content(
    document: &Document,
    styles: &ComputedStyles,
    node: DomNodeId,
) -> bool {
    match document.node_ref(node) {
        Some(NodeRef::Text(text)) => {
            let Some(parent) = text.parent() else {
                return false;
            };
            text_has_renderable_content(
                text.text(),
                style_for_dom_node(document, styles, parent).white_space(),
            )
        }
        Some(NodeRef::Element(element)) => {
            let Some(style_indices) = styles.style_for_node(element.node_id()) else {
                return false;
            };
            let display = display_for_style(styles, style_indices);
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
            if display.is_atomic_inline()
                || is_floated(styles, element)
                || is_absolutely_positioned(styles, element)
            {
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
        None => false,
    }
}

fn text_has_renderable_content(text: &str, white_space: WhiteSpace) -> bool {
    if white_space.preserves_spaces() {
        !text.is_empty()
    } else if white_space.preserves_newlines() {
        text.chars().any(|character| {
            character == '\n' || !is_document_white_space(character)
        })
    } else {
        text.chars()
            .any(|character| !is_document_white_space(character))
    }
}

fn style_for_dom_node<'a>(
    document: &Document,
    styles: &'a ComputedStyles,
    node: DomNodeId,
) -> StyleView<'a> {
    let owner = match document.node_ref(node) {
        Some(NodeRef::Element(element)) => Some(element.node_id()),
        Some(NodeRef::Text(text)) => text.parent(),
        None => None,
    };
    let indices = owner
        .and_then(|owner| styles.style_for_node(owner))
        .unwrap_or_else(|| styles.default_indices());
    styles.view(indices).expect("validated style handle")
}
