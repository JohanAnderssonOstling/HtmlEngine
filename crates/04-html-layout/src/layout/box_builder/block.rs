use html_dom::{Document, DomNodeId, NodeRef};
use html_style_model::{ComputedStyles, Display};

use super::classification::{display_for_element, display_for_style, is_block_level_pseudo_display, is_table_internal_display, starts_new_block_child_for_element};
use super::inline::has_block_child;

pub(super) enum BlockChildAction {
    GeneratedPseudo { before: bool },
    InlineSegment { nodes: Vec<u32>, before: bool, after: bool },
    AnonymousTable(Vec<u32>),
    Principal(DomNodeId),
    SplitInline(DomNodeId),
    ContentsWithTableInternals(DomNodeId),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GeneratedEdge {
    None,
    Inline,
    Block,
}

pub(super) fn plan_block_children(document: &Document, styles: &ComputedStyles, element_id: DomNodeId) -> Vec<BlockChildAction> {
    let Some(element) = document.element_ref(element_id) else {
        return Vec::new();
    };

    // A real row-group box is already inside the table formatting context, so
    // its rows are proper children. A positioned row group has been
    // blockified and deliberately takes the anonymous-table path instead.
    let wrap_table_internals = !matches!(display_for_element(styles, element), Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup);
    let before = generated_edge(styles, element_id, true);
    let after = generated_edge(styles, element_id, false);
    let mut before_pending = before == GeneratedEdge::Inline;
    let mut inline_segment = Vec::new();
    let mut table_segment = Vec::new();
    let mut actions = Vec::new();

    if before == GeneratedEdge::Block {
        actions.push(BlockChildAction::GeneratedPseudo { before: true });
    }

    append_children(document, styles, element, wrap_table_internals, &mut before_pending, &mut inline_segment, &mut table_segment, &mut actions);

    flush_table(&mut table_segment, &mut actions);
    flush_inline(&mut inline_segment, &mut before_pending, after == GeneratedEdge::Inline, &mut actions);
    if after == GeneratedEdge::Block {
        actions.push(BlockChildAction::GeneratedPseudo { before: false });
    }
    actions
}

fn append_children(
    document: &Document, styles: &ComputedStyles, element: html_dom::ElementRef<'_>, wrap_table_internals: bool, before_pending: &mut bool, inline_segment: &mut Vec<u32>, table_segment: &mut Vec<u32>, actions: &mut Vec<BlockChildAction>,
) {
    for child_id in element.children() {
        match document.node_ref(child_id) {
            Some(NodeRef::Element(child)) => {
                let display = display_for_element(styles, child);
                if display == Display::Contents && has_table_internal_descendant(document, styles, child) {
                    // A box-suppressed ancestor must be transparent to table
                    // fixup. Keep its descendants in the current table
                    // segment so siblings exposed through adjacent contents
                    // ancestors participate in one anonymous table. Pseudos
                    // and counters require ordered construction events, so
                    // retain the existing recursive action for those cases.
                    let has_generated_edges = styles.before_style_for_node(child_id).is_some() || styles.after_style_for_node(child_id).is_some();
                    if !has_generated_edges && styles.counter_directives_for_node(child_id).is_none() {
                        append_children(document, styles, child, wrap_table_internals, before_pending, inline_segment, table_segment, actions);
                        continue;
                    }
                    flush_inline(inline_segment, before_pending, false, actions);
                    flush_table(table_segment, actions);
                    actions.push(BlockChildAction::ContentsWithTableInternals(child_id));
                    continue;
                }
                if wrap_table_internals && is_table_internal_display(display) {
                    flush_inline(inline_segment, before_pending, false, actions);
                    table_segment.push(child_id.raw());
                    continue;
                }

                flush_table(table_segment, actions);
                if starts_new_block_child_for_element(styles, child) {
                    flush_inline(inline_segment, before_pending, false, actions);
                    actions.push(BlockChildAction::Principal(child_id));
                } else if matches!(display, Display::Inline | Display::Contents) && has_block_child(document, styles, child) {
                    flush_inline(inline_segment, before_pending, false, actions);
                    actions.push(BlockChildAction::SplitInline(child_id));
                } else {
                    inline_segment.push(child_id.raw());
                }
            }
            Some(NodeRef::Text(_)) => {
                if table_segment.is_empty() {
                    inline_segment.push(child_id.raw());
                } else {
                    table_segment.push(child_id.raw());
                }
            }
            None => {}
        }
    }
}

fn has_table_internal_descendant(document: &Document, styles: &ComputedStyles, element: html_dom::ElementRef<'_>) -> bool {
    element.children().any(|child_id| {
        document.element_ref(child_id).is_some_and(|child| {
            let display = display_for_element(styles, child);
            is_table_internal_display(display) || (display == Display::Contents && has_table_internal_descendant(document, styles, child))
        })
    })
}

fn generated_edge(styles: &ComputedStyles, origin: DomNodeId, before: bool) -> GeneratedEdge {
    let pseudo = if before { styles.before_style_for_node(origin) } else { styles.after_style_for_node(origin) };
    match pseudo {
        None => GeneratedEdge::None,
        Some((style, _, _)) if is_block_level_pseudo_display(display_for_style(styles, style)) => GeneratedEdge::Block,
        Some(_) => GeneratedEdge::Inline,
    }
}

fn flush_inline(nodes: &mut Vec<u32>, before: &mut bool, after: bool, actions: &mut Vec<BlockChildAction>) {
    if nodes.is_empty() && !*before && !after {
        return;
    }
    actions.push(BlockChildAction::InlineSegment { nodes: std::mem::take(nodes), before: std::mem::take(before), after });
}

fn flush_table(nodes: &mut Vec<u32>, actions: &mut Vec<BlockChildAction>) {
    if !nodes.is_empty() {
        actions.push(BlockChildAction::AnonymousTable(std::mem::take(nodes)));
    }
}
