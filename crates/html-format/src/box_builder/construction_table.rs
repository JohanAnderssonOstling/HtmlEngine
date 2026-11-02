//! Materialization of planned table, row-group, row, and cell topology.

use super::*;
use html_dom::ElementRef;

impl<'a, 'out> LayoutTreeBuilder<'a, 'out> {
    pub(super) fn build_table_children_from_nodes(
        &mut self,
        children: impl IntoIterator<Item = u32>,
        table_box_idx: u32,
    ) {
        let plan = plan_table_children(self.document, self.styles, children);
        let mut header_rows = Vec::new();
        let mut body_rows = Vec::new();
        let mut footer_rows = Vec::new();
        let mut header_groups = Vec::new();
        let mut body_groups = Vec::new();
        let mut footer_groups = Vec::new();
        let mut captions_top = Vec::new();
        let mut captions_bottom = Vec::new();
        for action in plan.actions {
            match action {
                TableChildAction::Anonymous(nodes) => {
                    self.materialize_anonymous_table_children(nodes, table_box_idx, &mut body_rows)
                }
                TableChildAction::Principal { node, role } => {
                    let group_display = self
                        .document
                        .element_ref(node)
                        .map(|element| display_for_element(self.styles, element));
                    if let Some(box_idx) = self.build_box_for_element(node, Some(table_box_idx)) {
                        match role {
                            TableRole::Caption(html_style_model::CaptionSide::Bottom) => {
                                captions_bottom.push(box_idx)
                            }
                            TableRole::Caption(html_style_model::CaptionSide::Top) => {
                                captions_top.push(box_idx)
                            }
                            TableRole::Row => body_rows.push(box_idx),
                            TableRole::RowGroup => {
                                let (rows, groups) = match group_display {
                                    Some(Display::TableHeaderGroup) if header_groups.is_empty() => {
                                        (&mut header_rows, &mut header_groups)
                                    }
                                    Some(Display::TableFooterGroup) if footer_groups.is_empty() => {
                                        (&mut footer_rows, &mut footer_groups)
                                    }
                                    _ => (&mut body_rows, &mut body_groups),
                                };
                                self.output.extend_with_group_rows(box_idx, rows);
                                groups.push(box_idx);
                            }
                            TableRole::Table | TableRole::Cell => unreachable!(
                                "direct table children cannot be table or cell principals"
                            ),
                        }
                    }
                }
            }
        }

        // CSS table layout has a visual group order independent of source
        // order: the first header precedes ordinary rows and the first footer
        // follows them. Later header/footer groups behave as ordinary groups.
        // Keep construction in source order, but expose the normalized grid
        // order to every sizing, border, and painting stage.
        let rows = header_rows
            .into_iter()
            .chain(body_rows)
            .chain(footer_rows)
            .collect();
        let row_groups = header_groups
            .into_iter()
            .chain(body_groups)
            .chain(footer_groups)
            .collect();

        self.output.finish_table(
            table_box_idx,
            TableBox {
                rows,
                row_groups,
                captions_top,
                captions_bottom,
                columns: plan.columns,
                column_groups: plan.column_groups,
                column_width_hints: plan.column_width_hints,
            },
        );
    }

    /// CSS table fixup wraps otherwise-improper table children in one
    /// anonymous row and cell. Keeping those boxes in the private layout model
    /// preserves the parsed DOM while ensuring ordinary block/float content
    /// contributes to table intrinsic sizing instead of being dropped.
    pub(super) fn materialize_anonymous_table_children(
        &mut self,
        nodes: Vec<u32>,
        table_box_idx: u32,
        rows: &mut Vec<u32>,
    ) {
        if nodes.iter().any(|&raw| {
            self.document
                .node_id_from_raw(raw)
                .and_then(|node| self.document.element_ref(node))
                .is_some_and(|element| {
                    display_for_element(self.styles, element) == Display::TableCell
                })
        }) {
            let row_idx = self.output.push_anonymous_row(table_box_idx);
            self.build_row_children(nodes, row_idx);
            rows.push(row_idx);
            return;
        }

        let white_space = self
            .output
            .style_for_box(table_box_idx as usize)
            .white_space();
        let plan = plan_anonymous_table_children(self.document, self.styles, &nodes, white_space);
        if matches!(plan, AnonymousTablePlan::Empty) {
            return;
        }
        let row_idx = self.output.push_anonymous_row(table_box_idx);
        let mut row = TableRowBox::default();
        self.materialize_anonymous_row_plan(plan, row_idx, &mut row.cells);
        self.output.finish_row(row_idx, row);
        rows.push(row_idx);
    }

    pub(super) fn build_row_children(
        &mut self,
        nodes: impl IntoIterator<Item = u32>,
        row_box_idx: u32,
    ) {
        let mut row = TableRowBox::default();
        for action in plan_table_row_children(self.document, self.styles, nodes) {
            match action {
                TableRowChildAction::Anonymous(nodes) => {
                    let white_space = self
                        .output
                        .style_for_box(row_box_idx as usize)
                        .white_space();
                    let plan = plan_anonymous_table_children(
                        self.document,
                        self.styles,
                        &nodes,
                        white_space,
                    );
                    self.materialize_anonymous_row_plan(plan, row_box_idx, &mut row.cells);
                }
                TableRowChildAction::Cell(node) => {
                    row.cells
                        .extend(self.build_box_for_element(node, Some(row_box_idx)));
                }
                TableRowChildAction::OutOfFlow(node) => {
                    row.out_of_flow
                        .extend(self.build_box_for_element(node, Some(row_box_idx)));
                }
            }
        }

        self.output.finish_row(row_box_idx, row);
    }

    pub(super) fn build_table_row_group_children(
        &mut self,
        element: ElementRef<'_>,
        group_box_idx: u32,
    ) {
        let mut rows = Vec::new();
        let mut anonymous = Vec::new();
        for node in element.children() {
            let display = self
                .document
                .element_ref(node)
                .map(|element| display_for_element(self.styles, element));
            match display {
                Some(Display::TableRow) => {
                    self.materialize_anonymous_group_row(
                        std::mem::take(&mut anonymous),
                        group_box_idx,
                        &mut rows,
                    );
                    rows.extend(self.build_box_for_element(node, Some(group_box_idx)));
                }
                Some(Display::None) => {}
                _ => anonymous.push(node.raw()),
            }
        }
        self.materialize_anonymous_group_row(anonymous, group_box_idx, &mut rows);
        self.output
            .set_box_children(group_box_idx, Children::Blocks(rows));
    }

    pub(super) fn materialize_anonymous_group_row(
        &mut self,
        nodes: Vec<u32>,
        group_box_idx: u32,
        rows: &mut Vec<u32>,
    ) {
        if nodes.is_empty()
            || nodes.iter().all(|&raw| {
                self.document
                    .node_id_from_raw(raw)
                    .and_then(|node| self.document.text_ref(node))
                    .is_some_and(|text| text.text().chars().all(char::is_whitespace))
            })
        {
            return;
        }
        let row_idx = self.output.push_anonymous_row(group_box_idx);
        self.build_row_children(nodes, row_idx);
        rows.push(row_idx);
    }

    fn materialize_anonymous_row_plan(
        &mut self,
        plan: AnonymousTablePlan,
        row_box_idx: u32,
        cells: &mut Vec<u32>,
    ) {
        let parts = match plan {
            AnonymousTablePlan::Empty => return,
            AnonymousTablePlan::Cells(cell_nodes) => {
                cells.extend(
                    cell_nodes
                        .into_iter()
                        .filter_map(|node| self.build_box_for_element(node, Some(row_box_idx))),
                );
                return;
            }
            AnonymousTablePlan::Cell(parts) => parts,
        };
        let cell_idx = self.output.push_anonymous_cell(row_box_idx);
        self.materialize_anonymous_cell_parts(parts, cell_idx);
        cells.push(cell_idx);
    }

    pub(super) fn materialize_anonymous_cell_parts(
        &mut self,
        parts: Vec<AnonymousCellPart>,
        cell_idx: u32,
    ) {
        let contains_block = parts
            .iter()
            .any(|part| matches!(part, AnonymousCellPart::Block(_)));
        if contains_block {
            let mut direct_children = Vec::new();
            let mut table_segment = Vec::new();
            for part in parts {
                match part {
                    AnonymousCellPart::Inline(mut nodes) => {
                        if table_segment.is_empty() {
                            direct_children.extend(self.flush_inline_segment(&mut nodes, cell_idx));
                        } else {
                            table_segment.append(&mut nodes);
                        }
                    }
                    AnonymousCellPart::Block(node)
                        if self.document.element_ref(node).is_some_and(|element| {
                            display_for_element(self.styles, element).is_table_internal()
                        }) =>
                    {
                        table_segment.push(node.raw());
                    }
                    AnonymousCellPart::Block(node) => {
                        direct_children.extend(
                            self.flush_anonymous_table_segment(&mut table_segment, cell_idx),
                        );
                        direct_children.extend(self.build_box_for_element(node, Some(cell_idx)));
                    }
                }
            }
            direct_children
                .extend(self.flush_anonymous_table_segment(&mut table_segment, cell_idx));
            self.output
                .set_box_children(cell_idx, Children::Blocks(direct_children));
        } else {
            let start = self.output.item_position();
            for part in parts {
                if let AnonymousCellPart::Inline(nodes) = part {
                    self.process_inline_nodes(nodes, cell_idx);
                }
            }
            let end = self.output.item_position();
            if start < end {
                self.output
                    .set_box_children(cell_idx, Children::InlineItems(start..end));
            }
        }
    }
}
