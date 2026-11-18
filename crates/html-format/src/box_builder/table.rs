use crate::layout_model::{
    Children, LayoutMode, TableCellBox, TableColumnGroupSpan, TableColumnTrack, TableRowBox,
    parse_positive_span,
};
use html_dom::{Document, DomNodeId, NodeRef};
use html_style_model::{CaptionSide, ComputedStyles, Display, StyleIndices, WhiteSpace};

use super::classification::{BoxClassification, BoxClassifications, starts_new_block_child};
use super::generated::ResolvedGeneratedContent;
use super::inline::{text_is_ignorable_at_block_edge, trimmed_node_range};

pub(super) struct TableChildrenPlan {
    pub(super) actions: Vec<TableChildAction>,
    pub(super) columns: Vec<TableColumnTrack>,
    pub(super) column_groups: Vec<TableColumnGroupSpan>,
}

pub(super) enum TableChildAction {
    Anonymous(Vec<DomNodeId>),
    Principal {
        node: DomNodeId,
        role: TableRole,
        classification: BoxClassification,
    },
}

pub(super) enum AnonymousTablePlan {
    Empty,
    Cells(Vec<DomNodeId>),
    Cell(Vec<AnonymousCellPart>),
}

pub(super) enum AnonymousCellPart {
    Inline(Vec<DomNodeId>),
    Block(DomNodeId),
}

pub(super) enum TableRowChildAction {
    Anonymous(Vec<DomNodeId>),
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
            Self::Table => LayoutMode::Table(Default::default()),
            Self::RowGroup | Self::Caption(_) => LayoutMode::Block(Children::default()),
            Self::Row => LayoutMode::TableRow(TableRowBox::default()),
            Self::Cell => LayoutMode::TableCell(TableCellBox::new(Children::Empty)),
        }
    }
}

pub(super) struct PlannedTableBox {
    pub(super) role: TableRole,
    pub(super) parent: Option<usize>,
}

pub(super) struct GeneratedTablePlan {
    pub(super) source: ResolvedGeneratedContent,
    pub(super) boxes: Vec<PlannedTableBox>,
    pub(super) principal_box: usize,
    pub(super) content_target: usize,
    pub(super) anonymous_text_box: bool,
}

pub(super) fn plan_generated_table_principal(
    styles: &ComputedStyles,
    content: ResolvedGeneratedContent,
) -> Option<GeneratedTablePlan> {
    let display = content.display;
    let caption_side = || {
        styles
            .box_model_style(content.style)
            .expect("validated generated caption style")
            .caption_side
    };
    let (boxes, principal_box, content_target, anonymous_text_box) = match display {
        Display::Table | Display::TableRow | Display::TableCell => (
            vec![
                planned(TableRole::Table, None),
                planned(TableRole::Row, Some(0)),
                planned(TableRole::Cell, Some(1)),
            ],
            match display {
                Display::Table => 0,
                Display::TableRow => 1,
                _ => 2,
            },
            2,
            display != Display::TableCell,
        ),
        Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup => (
            vec![
                planned(TableRole::Table, None),
                planned(TableRole::RowGroup, Some(0)),
                planned(TableRole::Row, Some(1)),
                planned(TableRole::Cell, Some(2)),
            ],
            1,
            3,
            true,
        ),
        Display::TableCaption => (
            vec![
                planned(TableRole::Table, None),
                planned(TableRole::Caption(caption_side()), Some(0)),
            ],
            1,
            1,
            false,
        ),
        _ => return None,
    };
    Some(GeneratedTablePlan {
        source: content,
        boxes,
        principal_box,
        content_target,
        anonymous_text_box,
    })
}

fn planned(role: TableRole, parent: Option<usize>) -> PlannedTableBox {
    PlannedTableBox { role, parent }
}

pub(super) fn nearest_table(
    boxes: &[PlannedTableBox],
    mut current: Option<usize>,
) -> Option<usize> {
    while let Some(index) = current {
        if matches!(boxes[index].role, TableRole::Table) {
            return Some(index);
        }
        current = boxes[index].parent;
    }
    None
}

pub(super) fn plan_table_children(
    document: &Document,
    styles: &ComputedStyles,
    classifications: &BoxClassifications,
    children: impl IntoIterator<Item = DomNodeId>,
) -> TableChildrenPlan {
    let children = flattened_contents_children(document, classifications, children);
    let mut actions = Vec::new();
    let mut columns = Vec::new();
    let mut column_groups = Vec::new();
    let mut anonymous = Vec::new();

    for raw in children {
        let node = raw;
        let Some(element) = document.try_element_ref(node) else {
            anonymous.push(raw);
            continue;
        };
        let classification = classifications.get(element);
        let style = classification.style;
        let display = classification.display;
        match display {
            Display::TableCaption => {
                flush_anonymous_action(&mut anonymous, &mut actions);
                let side = style
                    .map(|indices| {
                        styles
                            .box_model_style(indices)
                            .expect("validated style handle")
                            .caption_side
                    })
                    .unwrap_or(CaptionSide::Top);
                actions.push(TableChildAction::Principal {
                    node,
                    classification,
                    role: TableRole::Caption(side),
                });
            }
            Display::TableColumn => {
                flush_anonymous_action(&mut anonymous, &mut actions);
                let style = style.unwrap_or_else(|| styles.default_indices());
                append_columns(
                    &mut columns,
                    style,
                    parse_positive_span(element.attr("span")),
                    true,
                    None,
                );
            }
            Display::TableColumnGroup => {
                flush_anonymous_action(&mut anonymous, &mut actions);
                let group_style = style.unwrap_or_else(|| styles.default_indices());
                let start = columns.len();
                let mut group_span = 0;
                for child in element.child_elements() {
                    if classifications.get(child).display != Display::TableColumn {
                        continue;
                    }
                    let style = styles
                        .style_for_node(child.node_id())
                        .unwrap_or_else(|| styles.default_indices());
                    let span = parse_positive_span(child.attr("span"));
                    append_columns(&mut columns, style, span, true, Some(group_style));
                    group_span += span;
                }
                if group_span == 0 {
                    group_span = parse_positive_span(element.attr("span"));
                    append_columns(&mut columns, group_style, group_span, false, None);
                }
                if group_span > 0 {
                    column_groups.push(TableColumnGroupSpan {
                        style: group_style,
                        start,
                        span: group_span,
                    });
                }
            }
            Display::TableRow => {
                flush_anonymous_action(&mut anonymous, &mut actions);
                actions.push(TableChildAction::Principal {
                    node,
                    classification,
                    role: TableRole::Row,
                });
            }
            Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup => {
                flush_anonymous_action(&mut anonymous, &mut actions);
                actions.push(TableChildAction::Principal {
                    node,
                    classification,
                    role: TableRole::RowGroup,
                });
            }
            Display::None => {}
            _ => anonymous.push(raw),
        }
    }
    flush_anonymous_action(&mut anonymous, &mut actions);
    TableChildrenPlan {
        actions,
        columns,
        column_groups,
    }
}

pub(super) fn plan_anonymous_table_children(
    document: &Document,
    classifications: &BoxClassifications,
    nodes: &[DomNodeId],
    white_space: WhiteSpace,
) -> AnonymousTablePlan {
    let flattened;
    let nodes = if nodes.iter().any(|&raw| {
        document
            .try_element_ref(raw)
            .is_some_and(|element| classifications.get(element).display == Display::Contents)
    }) {
        flattened = flattened_contents_children(document, classifications, nodes.iter().copied());
        flattened.as_slice()
    } else {
        nodes
    };
    let whitespace_only = !nodes.is_empty()
        && nodes.iter().all(|&raw| {
            document
                .try_text_ref(raw)
                .is_some_and(|text| text.text().chars().all(char::is_whitespace))
        });
    if whitespace_only {
        return AnonymousTablePlan::Empty;
    }

    let nodes = &nodes[trimmed_node_range(nodes, |raw| {
        document
            .try_text_ref(raw)
            .is_some_and(|text| text_is_ignorable_at_block_edge(text.text(), white_space))
    })];
    if nodes.is_empty() {
        return AnonymousTablePlan::Empty;
    }

    let only_cells_and_ignorable_text = nodes.iter().all(|&raw| {
        let node = raw;
        match document.node_ref(node) {
            NodeRef::Element(element) => classifications.get(element).display == Display::TableCell,
            NodeRef::Text(text) => text.text().chars().all(char::is_whitespace),
        }
    });
    if only_cells_and_ignorable_text {
        return AnonymousTablePlan::Cells(
            nodes
                .iter()
                .copied()
                .filter(|&node| document.try_element_ref(node).is_some())
                .collect(),
        );
    }

    let mut parts = Vec::new();
    let mut inline = Vec::new();
    for &raw in nodes {
        let node = raw;
        let is_block = document
            .try_element_ref(node)
            .is_some_and(|element| starts_new_block_child(classifications.get(element).display));
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

pub(super) fn plan_table_row_children(
    document: &Document,
    classifications: &BoxClassifications,
    children: impl IntoIterator<Item = DomNodeId>,
) -> Vec<TableRowChildAction> {
    let children = flattened_contents_children(document, classifications, children);
    let mut actions = Vec::new();
    let mut anonymous = Vec::new();
    for raw in children {
        let node = raw;
        let Some(element) = document.try_element_ref(node) else {
            anonymous.push(raw);
            continue;
        };
        let classification = classifications.get(element);
        let display = classification.display;
        let out_of_flow = classification.absolute;
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

fn flattened_contents_children(
    document: &Document,
    classifications: &BoxClassifications,
    children: impl IntoIterator<Item = DomNodeId>,
) -> Vec<DomNodeId> {
    fn append(
        document: &Document,
        classifications: &BoxClassifications,
        raw: DomNodeId,
        output: &mut Vec<DomNodeId>,
    ) {
        let node = raw;
        let Some(element) = document.try_element_ref(node) else {
            output.push(raw);
            return;
        };
        match classifications.get(element).display {
            Display::None => {}
            Display::Contents => {
                for child in element.children() {
                    append(document, classifications, child, output);
                }
            }
            _ => output.push(raw),
        }
    }

    let children = children.into_iter();
    let mut flattened = Vec::with_capacity(children.size_hint().0);
    for raw in children {
        append(document, classifications, raw, &mut flattened);
    }
    flattened
}

fn append_columns(
    columns: &mut Vec<TableColumnTrack>,
    width_style: StyleIndices,
    span: usize,
    paints_background: bool,
    fallback_style: Option<StyleIndices>,
) {
    for _ in 0..span {
        columns.push(TableColumnTrack {
            width_style,
            fallback_style,
            paints_background,
        });
    }
}

fn flush_anonymous_action(nodes: &mut Vec<DomNodeId>, actions: &mut Vec<TableChildAction>) {
    if !nodes.is_empty() {
        actions.push(TableChildAction::Anonymous(std::mem::take(nodes)));
    }
}

fn flush_anonymous_row_action(nodes: &mut Vec<DomNodeId>, actions: &mut Vec<TableRowChildAction>) {
    if !nodes.is_empty() {
        actions.push(TableRowChildAction::Anonymous(std::mem::take(nodes)));
    }
}
