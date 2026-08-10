use crate::layout_model::{BlockBox, Children, LayoutMode, TableCellBox, TableColumnGroupSpan, TableColumnTrack, TableColumnWidthHint, TableRowBox};
use html_dom::{Document, DomNodeId, NodeRef};
use html_style_model::{CaptionSide, ComputedStyles, Display, PositionMode, WhiteSpace};

use super::classification::{display_for_element, empty_table_box, starts_new_block_child};
use super::generated::ResolvedGeneratedContent;
use super::inline::text_is_ignorable_at_block_edge;

pub(super) struct TableChildrenPlan {
    pub(super) actions: Vec<TableChildAction>,
    pub(super) columns: Vec<TableColumnTrack>,
    pub(super) column_groups: Vec<TableColumnGroupSpan>,
    pub(super) column_width_hints: Vec<TableColumnWidthHint>,
}

pub(super) enum TableChildAction {
    Anonymous(Vec<u32>),
    Principal(TablePrincipal<DomNodeId>),
}

pub(super) struct TablePrincipal<S> {
    pub(super) source: S,
    pub(super) role: TableRole,
}

pub(super) enum AnonymousTablePlan {
    Empty,
    Cells(Vec<DomNodeId>),
    Cell(Vec<AnonymousCellPart>),
}

pub(super) enum AnonymousCellPart {
    Inline(Vec<u32>),
    Block(DomNodeId),
}

pub(super) enum TableRowChildAction {
    Anonymous(Vec<u32>),
    Cell(DomNodeId),
    OutOfFlow(DomNodeId),
}

#[derive(Clone, Copy)]
pub(super) enum TableRole {
    Table,
    RowGroup,
    Row,
    Cell,
    Caption(CaptionSide),
}

impl TableRole {
    pub(super) fn layout_mode(self) -> LayoutMode {
        match self {
            Self::Table => LayoutMode::Table(empty_table_box()),
            Self::RowGroup | Self::Caption(_) => LayoutMode::Block(BlockBox { children: Children::Empty }),
            Self::Row => LayoutMode::TableRow(TableRowBox { cells: Vec::new(), out_of_flow: Vec::new() }),
            Self::Cell => LayoutMode::TableCell(TableCellBox::new(Children::Empty)),
        }
    }
}

pub(super) struct PlannedTableBox {
    pub(super) role: TableRole,
    pub(super) parent: Option<usize>,
}

pub(super) struct TableTopologyPlan<S> {
    pub(super) source: S,
    pub(super) boxes: Vec<PlannedTableBox>,
    pub(super) principal_box: usize,
    pub(super) content_target: usize,
    pub(super) anonymous_text_box: bool,
}

pub(super) type GeneratedTablePlan = TableTopologyPlan<ResolvedGeneratedContent>;

pub(super) fn plan_generated_table_principal(styles: &ComputedStyles, content: ResolvedGeneratedContent) -> Option<GeneratedTablePlan> {
    let display = content.display;
    let caption_side = || styles.box_model_style(content.style).expect("validated generated caption style").caption_side;
    let (boxes, principal_box, content_target, anonymous_text_box) = match display {
        Display::Table => (vec![planned(TableRole::Table, None), planned(TableRole::Row, Some(0)), planned(TableRole::Cell, Some(1))], 0, 2, true),
        Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup => {
            (vec![planned(TableRole::Table, None), planned(TableRole::RowGroup, Some(0)), planned(TableRole::Row, Some(1)), planned(TableRole::Cell, Some(2))], 1, 3, true)
        }
        Display::TableRow => (vec![planned(TableRole::Table, None), planned(TableRole::Row, Some(0)), planned(TableRole::Cell, Some(1))], 1, 2, true),
        Display::TableCell => (vec![planned(TableRole::Table, None), planned(TableRole::Row, Some(0)), planned(TableRole::Cell, Some(1))], 2, 2, false),
        Display::TableCaption => (vec![planned(TableRole::Table, None), planned(TableRole::Caption(caption_side()), Some(0))], 1, 1, false),
        _ => return None,
    };
    Some(TableTopologyPlan { source: content, boxes, principal_box, content_target, anonymous_text_box })
}

fn planned(role: TableRole, parent: Option<usize>) -> PlannedTableBox {
    PlannedTableBox { role, parent }
}

pub(super) fn nearest_table(boxes: &[PlannedTableBox], mut current: Option<usize>) -> Option<usize> {
    while let Some(index) = current {
        if matches!(boxes[index].role, TableRole::Table) {
            return Some(index);
        }
        current = boxes[index].parent;
    }
    None
}

pub(super) fn plan_table_children(document: &Document, styles: &ComputedStyles, children: Vec<u32>) -> TableChildrenPlan {
    let children = flattened_contents_children(document, styles, children);
    let mut actions = Vec::new();
    let mut columns = Vec::new();
    let mut column_groups = Vec::new();
    let mut column_width_hints = Vec::new();
    let mut anonymous = Vec::new();

    for raw in children {
        let Some(node) = document.node_id_from_raw(raw) else { continue };
        let Some(element) = document.element_ref(node) else {
            anonymous.push(raw);
            continue;
        };
        let style = styles.style_for_node(element.node_id());
        let display = display_for_element(styles, element);
        match display {
            Display::TableCaption => {
                flush_anonymous_action(&mut anonymous, &mut actions);
                let side = style.map(|indices| styles.box_model_style(indices).expect("validated style handle").caption_side).unwrap_or(CaptionSide::Top);
                actions.push(TableChildAction::Principal(TablePrincipal { source: node, role: TableRole::Caption(side) }));
            }
            Display::TableColumn => {
                flush_anonymous_action(&mut anonymous, &mut actions);
                let style = style.unwrap_or_else(|| styles.default_indices());
                append_columns(&mut columns, &mut column_width_hints, style, parse_span(document, node), Some(style), None);
            }
            Display::TableColumnGroup => {
                flush_anonymous_action(&mut anonymous, &mut actions);
                let group_style = style.unwrap_or_else(|| styles.default_indices());
                let start = columns.len();
                let mut group_span = 0;
                for child in element.children().filter_map(|child| document.element_ref(child)) {
                    if display_for_element(styles, child) != Display::TableColumn {
                        continue;
                    }
                    let style = styles.style_for_node(child.node_id()).unwrap_or_else(|| styles.default_indices());
                    let span = parse_span(document, child.node_id());
                    append_columns(&mut columns, &mut column_width_hints, style, span, Some(style), Some(group_style));
                    group_span += span;
                }
                if group_span == 0 {
                    group_span = parse_span(document, node);
                    append_columns(&mut columns, &mut column_width_hints, group_style, group_span, None, None);
                }
                if group_span > 0 {
                    column_groups.push(TableColumnGroupSpan { style: group_style, start, span: group_span });
                }
            }
            Display::TableRow => {
                flush_anonymous_action(&mut anonymous, &mut actions);
                actions.push(TableChildAction::Principal(TablePrincipal { source: node, role: TableRole::Row }));
            }
            Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup => {
                flush_anonymous_action(&mut anonymous, &mut actions);
                actions.push(TableChildAction::Principal(TablePrincipal { source: node, role: TableRole::RowGroup }));
            }
            Display::None => {}
            _ => anonymous.push(raw),
        }
    }
    flush_anonymous_action(&mut anonymous, &mut actions);
    TableChildrenPlan { actions, columns, column_groups, column_width_hints }
}

pub(super) fn plan_anonymous_table_children(document: &Document, styles: &ComputedStyles, nodes: &[u32], white_space: WhiteSpace) -> AnonymousTablePlan {
    let flattened;
    let nodes = if nodes.iter().any(|&raw| document.node_id_from_raw(raw).and_then(|node| document.element_ref(node)).is_some_and(|element| display_for_element(styles, element) == Display::Contents)) {
        flattened = flattened_contents_children(document, styles, nodes.to_vec());
        flattened.as_slice()
    } else {
        nodes
    };
    let whitespace_only = !nodes.is_empty() && nodes.iter().all(|&raw| document.node_id_from_raw(raw).and_then(|node| document.text_ref(node)).is_some_and(|text| text.text().chars().all(char::is_whitespace)));
    if whitespace_only {
        return AnonymousTablePlan::Empty;
    }

    let mut start = 0;
    let mut end = nodes.len();
    let collapsible_text = |raw| {
        document
            .node_id_from_raw(raw)
            .and_then(|node| match document.node_ref(node) {
                Some(NodeRef::Text(text)) => Some(text_is_ignorable_at_block_edge(text.text(), white_space)),
                Some(NodeRef::Element(_)) | None => None,
            })
            .unwrap_or(false)
    };
    while start < end && collapsible_text(nodes[start]) {
        start += 1;
    }
    while start < end && collapsible_text(nodes[end - 1]) {
        end -= 1;
    }
    let nodes = &nodes[start..end];
    if nodes.is_empty() {
        return AnonymousTablePlan::Empty;
    }

    let only_cells_and_ignorable_text = nodes.iter().all(|&raw| {
        let Some(node) = document.node_id_from_raw(raw) else { return true };
        match document.node_ref(node) {
            Some(NodeRef::Element(element)) => display_for_element(styles, element) == Display::TableCell,
            Some(NodeRef::Text(text)) => text.text().chars().all(char::is_whitespace),
            None => true,
        }
    });
    if only_cells_and_ignorable_text {
        return AnonymousTablePlan::Cells(nodes.iter().filter_map(|&raw| document.node_id_from_raw(raw)).filter(|&node| document.element_ref(node).is_some()).collect());
    }

    let mut parts = Vec::new();
    let mut inline = Vec::new();
    for &raw in nodes {
        let Some(node) = document.node_id_from_raw(raw) else { continue };
        let is_block = document.element_ref(node).is_some_and(|element| starts_new_block_child(display_for_element(styles, element)));
        if is_block {
            if !inline.is_empty() {
                parts.push(AnonymousCellPart::Inline(std::mem::take(&mut inline)));
            }
            parts.push(AnonymousCellPart::Block(node));
        } else {
            inline.push(raw);
        }
    }
    if !inline.is_empty() {
        parts.push(AnonymousCellPart::Inline(inline));
    }
    AnonymousTablePlan::Cell(parts)
}

pub(super) fn plan_table_row_children(document: &Document, styles: &ComputedStyles, children: Vec<u32>) -> Vec<TableRowChildAction> {
    let children = flattened_contents_children(document, styles, children);
    let mut actions = Vec::new();
    let mut anonymous = Vec::new();
    for raw in children {
        let Some(node) = document.node_id_from_raw(raw) else { continue };
        let Some(element) = document.element_ref(node) else {
            anonymous.push(raw);
            continue;
        };
        let display = display_for_element(styles, element);
        let out_of_flow = styles.style_for_node(element.node_id()).and_then(|style| styles.layout_style(style)).is_some_and(|style| style.position == PositionMode::Absolute);
        if display == Display::TableCell {
            flush_anonymous_row_action(&mut anonymous, &mut actions);
            actions.push(TableRowChildAction::Cell(node));
        } else if out_of_flow {
            flush_anonymous_row_action(&mut anonymous, &mut actions);
            actions.push(TableRowChildAction::OutOfFlow(node));
        } else if display != Display::None {
            anonymous.push(raw);
        }
    }
    flush_anonymous_row_action(&mut anonymous, &mut actions);
    actions
}

fn flattened_contents_children(document: &Document, styles: &ComputedStyles, children: Vec<u32>) -> Vec<u32> {
    fn append(document: &Document, styles: &ComputedStyles, raw: u32, output: &mut Vec<u32>) {
        let Some(node) = document.node_id_from_raw(raw) else { return };
        let Some(element) = document.element_ref(node) else {
            output.push(raw);
            return;
        };
        match display_for_element(styles, element) {
            Display::None => {}
            Display::Contents => {
                for child in element.children() {
                    append(document, styles, child.raw(), output);
                }
            }
            _ => output.push(raw),
        }
    }

    let mut flattened = Vec::with_capacity(children.len());
    for raw in children {
        append(document, styles, raw, &mut flattened);
    }
    flattened
}

fn append_columns(
    columns: &mut Vec<TableColumnTrack>, hints: &mut Vec<TableColumnWidthHint>, width_style: html_style_model::StyleIndices, span: usize, background_style: Option<html_style_model::StyleIndices>, fallback_style: Option<html_style_model::StyleIndices>,
) {
    for _ in 0..span {
        columns.push(TableColumnTrack { width_style, background_style });
        hints.push(TableColumnWidthHint { style: width_style, fallback_style });
    }
}

fn flush_anonymous_action(nodes: &mut Vec<u32>, actions: &mut Vec<TableChildAction>) {
    if !nodes.is_empty() {
        actions.push(TableChildAction::Anonymous(std::mem::take(nodes)));
    }
}

fn flush_anonymous_row_action(nodes: &mut Vec<u32>, actions: &mut Vec<TableRowChildAction>) {
    if !nodes.is_empty() {
        actions.push(TableRowChildAction::Anonymous(std::mem::take(nodes)));
    }
}

fn parse_span(document: &Document, node: DomNodeId) -> usize {
    crate::table::parse_positive_span(document.get_dom_attr(node, "span"))
}
