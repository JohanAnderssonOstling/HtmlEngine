use html_dom::{Document, DomNodeId, ElementRef, NodeRef};
use html_style_model::{BorderStyle, Clear, ComputedStyles, Display, Float, PositionMode, StyleIndices, StyleView, WhiteSpace};

use super::classification::{display_for_element, display_for_style, is_absolutely_positioned, is_block_level_pseudo_display, is_floated, is_table_internal_display, starts_inline_split_child, starts_new_block_child_for_element};
use super::generated::ResolvedGeneratedContent;

pub(super) enum InlineNodePlan {
    Ignore,
    Text(DomNodeId),
    Break { clear: Clear },
    GeneratedBreak { node: DomNodeId, style: Option<StyleIndices> },
    OutOfFlow(DomNodeId),
    Atomic(DomNodeId),
    Contents { node: DomNodeId, children: Vec<u32> },
    Element(InlineElementPlan),
}

pub(super) struct InlineElementPlan {
    pub(super) fragment: InlineFragmentPlan,
    pub(super) image: Option<u32>,
    pub(super) children: Vec<u32>,
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

pub(super) fn plan_inline_node(document: &Document, styles: &ComputedStyles, node: DomNodeId) -> InlineNodePlan {
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
        return InlineNodePlan::Contents { node, children: element.children().map(DomNodeId::raw).collect() };
    }

    let style = styles.style_for_node(node);
    if element.tag().eq_ignore_ascii_case("br") {
        if styles.before_style_for_node(node).is_some() || styles.after_style_for_node(node).is_some() {
            return InlineNodePlan::GeneratedBreak { node, style };
        }
        let clear = style.and_then(|indices| styles.box_model_style(indices)).map_or(Clear::None, |box_style| box_style.clear);
        return InlineNodePlan::Break { clear };
    }

    let float = style.and_then(|indices| styles.box_model_style(indices)).map_or(Float::None, |box_style| box_style.float);
    let absolute = style.and_then(|indices| styles.layout_style(indices)).is_some_and(|layout_style| layout_style.position == PositionMode::Absolute);
    if absolute || matches!(float, Float::Left | Float::Right) {
        return InlineNodePlan::OutOfFlow(node);
    }
    if matches!(display, Display::InlineBlock | Display::InlineFlex | Display::InlineGrid | Display::InlineTable) {
        return InlineNodePlan::Atomic(node);
    }

    let fragment = plan_inline_fragment(styles, node, style);
    InlineNodePlan::Element(InlineElementPlan { fragment, image: element.image_idx(), children: element.children().map(DomNodeId::raw).collect() })
}

fn plan_inline_fragment(styles: &ComputedStyles, node: DomNodeId, style: Option<StyleIndices>) -> InlineFragmentPlan {
    let (start_edge, end_edge) = inline_edges(styles, style);
    InlineFragmentPlan { node, style, start_edge, end_edge }
}

pub(super) fn plan_generated_inline(styles: &ComputedStyles, content: ResolvedGeneratedContent) -> GeneratedInlinePlan {
    let (start_edge, end_edge) = if content.display == Display::Contents { (false, false) } else { inline_edges(styles, Some(content.style)) };
    GeneratedInlinePlan { content, start_edge, end_edge }
}

fn inline_edges(styles: &ComputedStyles, style: Option<StyleIndices>) -> (bool, bool) {
    style.and_then(|indices| styles.view(indices)).map(|view| inline_edges_for_view(&view)).unwrap_or((false, false))
}

fn inline_edges_for_view(view: &StyleView<'_>) -> (bool, bool) {
    (
        !view.margin_left().is_zero() || !view.padding_left().is_zero() || (view.border_left_width() != html_style_model::FontRelativeLength::ZERO && !matches!(view.border_left_style(), BorderStyle::None | BorderStyle::Hidden)),
        !view.margin_right().is_zero() || !view.padding_right().is_zero() || (view.border_right_width() != html_style_model::FontRelativeLength::ZERO && !matches!(view.border_right_style(), BorderStyle::None | BorderStyle::Hidden)),
    )
}

#[derive(Clone, Copy)]
pub(super) enum SplitInlineEvent {
    Open { fragment: InlineFragmentPlan, first_fragment: bool },
    Close { fragment: InlineFragmentPlan, last_fragment: bool },
    EnterContents(DomNodeId),
    ExitContents,
    GeneratedPseudo { origin: DomNodeId, before: bool },
    Node(DomNodeId),
}

pub(super) enum SplitInlinePart {
    Segment(Vec<SplitInlineEvent>),
    Block { node: DomNodeId, position_ancestors: Vec<StyleIndices> },
    GeneratedBlock { origin: DomNodeId, before: bool, position_ancestors: Vec<StyleIndices> },
}

enum CollectedSplitInlineEvent {
    Inline(SplitInlineEvent),
    Block(DomNodeId),
    GeneratedBlock { origin: DomNodeId, before: bool },
}

pub(super) fn has_block_child(document: &Document, styles: &ComputedStyles, element: ElementRef<'_>) -> bool {
    // Replaced elements are atomic CSS boxes. Their source DOM can describe
    // the resource (for example an inline SVG tree), but those descendants
    // never participate in block-in-inline splitting of the surrounding HTML.
    if element.image_idx().is_some() {
        return false;
    }
    let inline_container = matches!(display_for_element(styles, element), Display::Inline | Display::Contents);
    pseudo_is_block_level(styles, element.node_id(), true)
        || element.children().any(|child_idx| {
            document.element_ref(child_idx).is_some_and(|child| {
                let display = display_for_element(styles, child);
                let starts_block = if is_absolutely_positioned(styles, child) || is_floated(styles, child) {
                    false
                } else if inline_container {
                    starts_inline_split_child(styles, child)
                } else {
                    starts_new_block_child_for_element(styles, child)
                };
                starts_block
                    || (display == Display::Contents && has_table_internal_descendant(document, styles, child))
                    || (matches!(display, Display::Inline | Display::Contents) && has_inline_splitting_block_descendant(document, styles, child))
            })
        })
        || pseudo_is_block_level(styles, element.node_id(), false)
}

fn has_table_internal_descendant(document: &Document, styles: &ComputedStyles, element: ElementRef<'_>) -> bool {
    element.children().any(|child_id| {
        document.element_ref(child_id).is_some_and(|child| {
            let display = display_for_element(styles, child);
            is_table_internal_display(display) || (display == Display::Contents && has_table_internal_descendant(document, styles, child))
        })
    })
}

pub(super) fn plan_split_inline(document: &Document, styles: &ComputedStyles, element: DomNodeId) -> Vec<SplitInlinePart> {
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
            CollectedSplitInlineEvent::Block(block) => {
                segment.extend(active.iter().rev().copied().map(|fragment| SplitInlineEvent::Close { fragment, last_fragment: false }));
                finish_split_inline_segment(document, styles, &mut segment, &mut parts);
                let position_ancestors = active.iter().filter_map(|fragment| fragment.style).filter(|&style| styles.layout_style(style).is_some_and(|layout| layout.position == PositionMode::Relative)).collect();
                parts.push(SplitInlinePart::Block { node: block, position_ancestors });
                segment.extend(active.iter().copied().map(|fragment| SplitInlineEvent::Open { fragment, first_fragment: false }));
            }
            CollectedSplitInlineEvent::GeneratedBlock { origin, before } => {
                segment.extend(active.iter().rev().copied().map(|fragment| SplitInlineEvent::Close { fragment, last_fragment: false }));
                finish_split_inline_segment(document, styles, &mut segment, &mut parts);
                let position_ancestors = active.iter().filter_map(|fragment| fragment.style).filter(|&style| styles.layout_style(style).is_some_and(|layout| layout.position == PositionMode::Relative)).collect();
                parts.push(SplitInlinePart::GeneratedBlock { origin, before, position_ancestors });
                segment.extend(active.iter().copied().map(|fragment| SplitInlineEvent::Open { fragment, first_fragment: false }));
            }
        }
    }
    debug_assert!(active.is_empty());
    finish_split_inline_segment(document, styles, &mut segment, &mut parts);
    parts
}

fn collect_split_inline_events(document: &Document, styles: &ComputedStyles, element_id: DomNodeId, events: &mut Vec<CollectedSplitInlineEvent>) {
    let Some(element) = document.element_ref(element_id) else { return };
    if display_for_element(styles, element) == Display::Contents {
        events.push(CollectedSplitInlineEvent::Inline(SplitInlineEvent::EnterContents(element_id)));
        collect_split_pseudo(styles, element_id, true, events);
        collect_split_children(document, styles, element, events);
        collect_split_pseudo(styles, element_id, false, events);
        events.push(CollectedSplitInlineEvent::Inline(SplitInlineEvent::ExitContents));
        return;
    }
    let fragment = plan_inline_fragment(styles, element_id, styles.style_for_node(element_id));
    events.push(CollectedSplitInlineEvent::Inline(SplitInlineEvent::Open { fragment, first_fragment: true }));
    collect_split_children(document, styles, element, events);
    events.push(CollectedSplitInlineEvent::Inline(SplitInlineEvent::Close { fragment, last_fragment: true }));
}

fn collect_split_children(document: &Document, styles: &ComputedStyles, element: ElementRef<'_>, events: &mut Vec<CollectedSplitInlineEvent>) {
    for child_id in element.children() {
        let Some(child) = document.element_ref(child_id) else {
            events.push(CollectedSplitInlineEvent::Inline(SplitInlineEvent::Node(child_id)));
            continue;
        };
        let display = display_for_element(styles, child);
        if starts_inline_split_child(styles, child) {
            events.push(CollectedSplitInlineEvent::Block(child_id));
        } else if matches!(display, Display::Inline | Display::Contents) && has_inline_splitting_block_descendant(document, styles, child) {
            collect_split_inline_events(document, styles, child_id, events);
        } else {
            events.push(CollectedSplitInlineEvent::Inline(SplitInlineEvent::Node(child_id)));
        }
    }
}

fn collect_split_pseudo(styles: &ComputedStyles, origin: DomNodeId, before: bool, events: &mut Vec<CollectedSplitInlineEvent>) {
    let Some((style, _, _)) = (if before { styles.before_style_for_node(origin) } else { styles.after_style_for_node(origin) }) else { return };
    let display = display_for_style(styles, style);
    if display == Display::None {
        return;
    }
    if is_block_level_pseudo_display(display) {
        events.push(CollectedSplitInlineEvent::GeneratedBlock { origin, before });
    } else {
        events.push(CollectedSplitInlineEvent::Inline(SplitInlineEvent::GeneratedPseudo { origin, before }));
    }
}

fn finish_split_inline_segment(document: &Document, styles: &ComputedStyles, events: &mut Vec<SplitInlineEvent>, parts: &mut Vec<SplitInlinePart>) {
    if events.is_empty() {
        return;
    }
    let first_content = events.iter().position(|event| match event {
        SplitInlineEvent::Node(node) => !node_is_ignorable_at_block_edge(document, styles, *node, white_space_for_node(document, styles, *node)),
        _ => false,
    });
    let last_content = events.iter().rposition(|event| match event {
        SplitInlineEvent::Node(node) => !node_is_ignorable_at_block_edge(document, styles, *node, white_space_for_node(document, styles, *node)),
        _ => false,
    });
    let events = std::mem::take(events)
        .into_iter()
        .enumerate()
        .filter_map(|(event_idx, event)| match event {
            SplitInlineEvent::Node(node)
                if first_content.zip(last_content).is_none_or(|(first, last)| event_idx < first || event_idx > last) && node_is_ignorable_at_block_edge(document, styles, node, white_space_for_node(document, styles, node)) =>
            {
                None
            }
            event => Some(event),
        })
        .collect();
    parts.push(SplitInlinePart::Segment(events));
}

fn has_inline_splitting_block_descendant(document: &Document, styles: &ComputedStyles, element: ElementRef<'_>) -> bool {
    if element.image_idx().is_some() {
        return false;
    }
    pseudo_is_block_level(styles, element.node_id(), true)
        || element.children().any(|child_idx| {
            document.element_ref(child_idx).is_some_and(|child| {
                let display = display_for_element(styles, child);
                starts_inline_split_child(styles, child) || (matches!(display, Display::Inline | Display::Contents) && has_inline_splitting_block_descendant(document, styles, child))
            })
        })
        || pseudo_is_block_level(styles, element.node_id(), false)
}

fn pseudo_is_block_level(styles: &ComputedStyles, origin: DomNodeId, before: bool) -> bool {
    let pseudo = if before { styles.before_style_for_node(origin) } else { styles.after_style_for_node(origin) };
    pseudo.is_some_and(|(style, _, _)| is_block_level_pseudo_display(display_for_style(styles, style)))
}

pub(super) fn trim_inline_segment_nodes<'a>(document: &Document, styles: &ComputedStyles, nodes: &'a [u32], white_space: WhiteSpace) -> &'a [u32] {
    let mut start = 0usize;
    let mut end = nodes.len();

    while start < end && document.node_id_from_raw(nodes[start]).is_some_and(|node| node_is_ignorable_at_block_edge(document, styles, node, white_space)) {
        start += 1;
    }
    while start < end && document.node_id_from_raw(nodes[end - 1]).is_some_and(|node| node_is_ignorable_at_block_edge(document, styles, node, white_space)) {
        end -= 1;
    }

    if start >= end || !nodes[start..end].iter().copied().filter_map(|raw| document.node_id_from_raw(raw)).any(|node| node_has_renderable_inline_content(document, styles, node)) { &nodes[0..0] } else { &nodes[start..end] }
}

fn node_is_ignorable_at_block_edge(document: &Document, styles: &ComputedStyles, node: DomNodeId, white_space: WhiteSpace) -> bool {
    match document.node_ref(node) {
        Some(NodeRef::Text(text)) => text_is_ignorable_at_block_edge(text.text(), white_space),
        Some(NodeRef::Element(_)) => !node_has_renderable_inline_content(document, styles, node),
        None => false,
    }
}

fn white_space_for_node(document: &Document, styles: &ComputedStyles, node: DomNodeId) -> WhiteSpace {
    style_for_dom_node(document, styles, node).white_space()
}

pub(super) fn text_is_ignorable_at_block_edge(text: &str, white_space: WhiteSpace) -> bool {
    if white_space.preserves_spaces() {
        false
    } else if white_space.preserves_newlines() {
        !text.is_empty() && text.chars().all(|character| matches!(character, ' ' | '\t' | '\r' | '\u{000C}'))
    } else {
        !text.is_empty() && text.chars().all(|character| character.is_whitespace() && character != '\u{00A0}')
    }
}

fn node_has_renderable_inline_content(document: &Document, styles: &ComputedStyles, node: DomNodeId) -> bool {
    match document.node_ref(node) {
        Some(NodeRef::Text(text)) => {
            let Some(parent) = text.parent() else { return false };
            text_has_renderable_content(text.text(), style_for_dom_node(document, styles, parent).white_space())
        }
        Some(NodeRef::Element(element)) => {
            let Some(style_indices) = styles.style_for_node(element.node_id()) else { return false };
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
            if styles.before_style_for_node(node).is_some() || styles.after_style_for_node(node).is_some() {
                return true;
            }
            let style = styles.view(style_indices).expect("validated style handle");
            if matches!(display, Display::InlineBlock | Display::InlineFlex | Display::InlineGrid | Display::InlineTable)
                || matches!(style.float(), Float::Left | Float::Right)
                || styles.layout_style(style_indices).is_some_and(|layout| layout.position == PositionMode::Absolute)
            {
                return true;
            }
            if element.tag().eq_ignore_ascii_case("br") || element.image_idx().is_some() {
                return true;
            }
            let (start_edge, end_edge) = inline_edges_for_view(&style);
            if display == Display::Inline && (start_edge || end_edge) {
                return true;
            }
            element.children().any(|child| node_has_renderable_inline_content(document, styles, child))
        }
        None => false,
    }
}

fn text_has_renderable_content(text: &str, white_space: WhiteSpace) -> bool {
    if white_space.preserves_spaces() {
        !text.is_empty()
    } else if white_space.preserves_newlines() {
        text.chars().any(|character| character == '\n' || !character.is_whitespace() || character == '\u{00A0}')
    } else {
        text.chars().any(|character| !character.is_whitespace() || character == '\u{00A0}')
    }
}

fn style_for_dom_node<'a>(document: &Document, styles: &'a ComputedStyles, node: DomNodeId) -> StyleView<'a> {
    let indices = match document.node_ref(node) {
        Some(NodeRef::Element(element)) => styles.style_for_node(element.node_id()).unwrap_or_else(|| styles.default_indices()),
        Some(NodeRef::Text(text)) => text.parent().and_then(|parent| styles.style_for_node(parent)).unwrap_or_else(|| styles.default_indices()),
        None => styles.default_indices(),
    };
    styles.view(indices).expect("validated style handle")
}
