use crate::layout_model::{Children, GlyphId, InlineContent, InlineItem, InlineItemKind, LayoutBox, LayoutMode, LayoutTree, ListItemMarker, TableCellBox, TableColumnGroupSpan, TableColumnTrack, TableColumnWidthHint, TableRowBox};
use html_style_model::{ComputedStyles, Display, ListStylePosition, StyleIndices, StyleView};
use std::ops::Range;

use super::inline::GeneratedInlinePlan;
use super::table::{GeneratedTablePlan, TableRole, nearest_table};

/// Owns the invariant-sensitive construction of the two box-building outputs.
/// DOM traversal and CSS generated-content state deliberately stay outside.
pub(super) struct BoxTreeWriter<'styles, 'out> {
    styles: &'styles ComputedStyles,
    tree: &'out mut LayoutTree,
    inline: &'out mut InlineContent,
}

impl<'styles, 'out> BoxTreeWriter<'styles, 'out> {
    pub(super) fn new(styles: &'styles ComputedStyles, tree: &'out mut LayoutTree, inline: &'out mut InlineContent) -> Self {
        Self { styles, tree, inline }
    }

    pub(super) fn set_root_box(&mut self, root: Option<u32>) {
        self.tree.set_root_box(root);
    }

    pub(super) fn item_position(&self) -> u32 {
        self.inline.inline_items().len() as u32
    }

    pub(super) fn style_for_box(&self, box_idx: usize) -> StyleView<'_> {
        let indices = self.tree.box_at(box_idx).and_then(LayoutBox::style).unwrap_or_else(|| self.styles.default_indices());
        self.styles.view(indices).expect("validated style handle")
    }

    pub(super) fn anonymous_style_for_box(&self, box_idx: u32) -> Option<StyleIndices> {
        self.tree.box_at(box_idx as usize).and_then(LayoutBox::style).and_then(|style| self.styles.anonymous_box_indices(style))
    }

    pub(super) fn push_dom_box(&mut self, dom_element: u32, image_idx: Option<u32>, parent: Option<u32>, style: Option<StyleIndices>, layout_mode: LayoutMode, is_body: bool) -> u32 {
        let box_idx = self.tree.push_box(LayoutBox { layout_mode, dom_element: Some(dom_element), parent, style });
        if let Some(image_idx) = image_idx {
            self.tree.set_box_image_idx(box_idx, image_idx);
        }
        if is_body {
            self.tree.set_body_box(box_idx);
        }
        box_idx
    }

    pub(super) fn push_anonymous_table(&mut self, parent: u32, layout_mode: LayoutMode) -> u32 {
        let style = self.anonymous_style_for_box(parent);
        self.tree.push_box(LayoutBox { layout_mode, dom_element: None, parent: Some(parent), style })
    }

    pub(super) fn push_anonymous_row(&mut self, parent: u32) -> u32 {
        let style = self.anonymous_style_for_box(parent);
        self.tree.push_box(LayoutBox { layout_mode: LayoutMode::TableRow(TableRowBox { cells: Vec::new(), out_of_flow: Vec::new() }), dom_element: None, parent: Some(parent), style })
    }

    pub(super) fn push_anonymous_cell(&mut self, parent: u32, inherit_style: bool) -> u32 {
        let style = inherit_style.then(|| self.anonymous_style_for_box(parent)).flatten();
        self.tree.push_box(LayoutBox { layout_mode: LayoutMode::TableCell(TableCellBox::new(Children::Empty)), dom_element: None, parent: Some(parent), style })
    }

    pub(super) fn push_anonymous_inline(&mut self, parent: u32) -> (u32, u32) {
        let start = self.item_position();
        let style = self.anonymous_style_for_box(parent);
        let mut box_ = LayoutBox::new_anonymous_box(start..start, Some(parent));
        box_.set_style(style);
        let box_idx = self.tree.push_box(box_);
        (box_idx, start)
    }

    /// Anonymous inline style carrier for text promoted through a
    /// `display: contents` element. Only inherited values are copied; reset
    /// box/paint properties on the suppressed principal box must not leak.
    pub(super) fn push_contents_style_box(&mut self, parent: u32, inherited_from: StyleIndices) -> (u32, u32) {
        let start = self.item_position();
        let style = self.styles.anonymous_box_indices(inherited_from);
        let box_ = LayoutBox { layout_mode: LayoutMode::Inline(start..start), dom_element: None, parent: Some(parent), style };
        let box_idx = self.tree.push_box(box_);
        (box_idx, start)
    }

    pub(super) fn push_item(&mut self, kind: InlineItemKind, box_idx: u32, dom_text_node: Option<u32>) {
        self.inline.push_inline_item(InlineItem { kind, box_idx, dom_text_node });
    }

    pub(super) fn push_source_text(&mut self, text: &str, box_idx: u32, dom_text_node: u32) {
        self.push_source_text_fragment(text, box_idx, dom_text_node, 0);
    }

    pub(super) fn push_source_text_fragment(&mut self, text: &str, box_idx: u32, dom_text_node: u32, source_utf16_offset: u32) {
        if text.is_empty() {
            return;
        }
        let glyph_start = self.inline.glyphs().len() as u32;
        let mut utf16_offset = source_utf16_offset;
        for character in text.chars() {
            self.inline.push_source_glyph(character as GlyphId, utf16_offset);
            utf16_offset = utf16_offset.saturating_add(character.len_utf16() as u32);
        }
        let glyph_end = self.inline.glyphs().len() as u32;
        self.push_item(InlineItemKind::Text { glyphs: glyph_start..glyph_end }, box_idx, Some(dom_text_node));
    }

    fn push_generated_text(&mut self, text: &str, box_idx: u32, kind: impl FnOnce(Range<u32>) -> InlineItemKind) {
        let glyph_start = self.inline.glyphs().len() as u32;
        for character in text.chars() {
            self.inline.push_glyph(character as GlyphId);
        }
        let glyph_end = self.inline.glyphs().len() as u32;
        self.push_item(kind(glyph_start..glyph_end), box_idx, None);
    }

    pub(super) fn finish_inline_box(&mut self, box_idx: u32, start: u32) {
        let end = self.item_position();
        if let Some(box_) = self.tree.box_at_mut(box_idx as usize) {
            *box_.layout_mode_mut() = LayoutMode::Inline(start..end);
        }
    }

    pub(super) fn finish_anonymous_box(&mut self, box_idx: u32, start: u32) -> bool {
        let end = self.item_position();
        if let Some(box_) = self.tree.box_at_mut(box_idx as usize) {
            *box_.layout_mode_mut() = LayoutMode::Anonymous(start..end);
        }
        start < end
    }

    pub(super) fn set_inline_fragment_edges(&mut self, box_idx: u32, first: bool, last: bool) {
        self.tree.set_inline_fragment_edges(box_idx, first, last);
    }

    pub(super) fn set_split_inline_position_ancestors(&mut self, box_idx: u32, ancestors: Vec<StyleIndices>) {
        self.tree.set_split_inline_position_ancestors(box_idx, ancestors);
    }

    pub(super) fn set_box_children(&mut self, box_idx: u32, children: Children) {
        if let Some(layout_mode) = self.tree.box_at_mut(box_idx as usize).map(LayoutBox::layout_mode_mut) {
            match layout_mode {
                LayoutMode::Block(block) => block.children = children,
                LayoutMode::TableCell(cell) => cell.children = children,
                _ => {}
            }
        }
    }

    pub(super) fn set_flex_grid_children(&mut self, box_idx: u32, children: Vec<u32>) {
        match self.tree.box_at_mut(box_idx as usize).map(LayoutBox::layout_mode_mut) {
            Some(LayoutMode::Flex(container)) => container.children = children,
            Some(LayoutMode::Grid(container)) => container.children = children,
            _ => unreachable!("flex/grid child construction requires a flex/grid layout mode"),
        }
    }

    pub(super) fn extend_with_group_rows(&self, group_idx: u32, rows: &mut Vec<u32>) {
        if let Some(LayoutMode::Block(group)) = self.tree.box_at(group_idx as usize).map(LayoutBox::layout_mode)
            && let Children::Blocks(group_rows) = &group.children
        {
            rows.extend(group_rows.iter().copied().filter(|&row| matches!(self.tree.box_at(row as usize).map(LayoutBox::layout_mode), Some(LayoutMode::TableRow(_)))));
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_table(
        &mut self, table_idx: u32, rows: Vec<u32>, row_groups: Vec<u32>, captions_top: Vec<u32>, captions_bottom: Vec<u32>, columns: Vec<TableColumnTrack>, column_groups: Vec<TableColumnGroupSpan>,
        column_width_hints: Vec<TableColumnWidthHint>,
    ) {
        if let Some(LayoutMode::Table(table)) = self.tree.box_at_mut(table_idx as usize).map(LayoutBox::layout_mode_mut) {
            table.rows = rows;
            table.row_groups = row_groups;
            table.captions_top = captions_top;
            table.captions_bottom = captions_bottom;
            table.columns = columns;
            table.column_groups = column_groups;
            table.column_width_hints = column_width_hints;
        }
    }

    pub(super) fn set_row_children(&mut self, row_idx: u32, cells: Vec<u32>, out_of_flow: Vec<u32>) {
        if let Some(LayoutMode::TableRow(row)) = self.tree.box_at_mut(row_idx as usize).map(LayoutBox::layout_mode_mut) {
            row.cells = cells;
            row.out_of_flow = out_of_flow;
        }
    }

    pub(super) fn set_row_cells(&mut self, row_idx: u32, cells: Vec<u32>) {
        if let Some(LayoutMode::TableRow(row)) = self.tree.box_at_mut(row_idx as usize).map(LayoutBox::layout_mode_mut) {
            row.cells = cells;
        }
    }

    pub(super) fn append_row_cell(&mut self, row_idx: u32, cell_idx: u32) {
        if let Some(LayoutMode::TableRow(row)) = self.tree.box_at_mut(row_idx as usize).map(LayoutBox::layout_mode_mut) {
            row.cells.push(cell_idx);
        }
    }

    pub(super) fn emit_marker(&mut self, list_item_box: u32, style: StyleIndices, position: ListStylePosition, text: &str) {
        let marker_style = self.styles.anonymous_box_indices(style).unwrap_or(style);
        let marker_box = self.tree.push_box(LayoutBox { layout_mode: LayoutMode::Inline(Range::default()), dom_element: None, parent: Some(list_item_box), style: Some(marker_style) });
        let start = self.item_position();
        self.push_generated_text(text, marker_box, |glyphs| InlineItemKind::Marker { glyphs });
        self.finish_inline_box(marker_box, start);
        self.tree.set_list_marker(list_item_box, ListItemMarker { marker_box, position });
    }

    pub(super) fn emit_generated_inline(&mut self, parent: u32, plan: GeneratedInlinePlan) {
        let GeneratedInlinePlan { content, start_edge, end_edge } = plan;
        if content.display == Display::InlineBlock {
            // Generated inline-blocks are atomic inline-level boxes. Keeping
            // them as text-only inline fragments would discard their authored
            // width, height, and baseline contribution when `content` is empty.
            let pseudo_box = self.tree.push_box(LayoutBox {
                layout_mode: LayoutMode::Block(crate::layout_model::BlockBox { children: Children::Empty }),
                dom_element: None,
                parent: Some(parent),
                style: Some(content.style),
            });
            self.set_generated_text_children(pseudo_box, content.style, &content.text, false);
            self.push_item(InlineItemKind::AtomicBox { box_idx: pseudo_box }, pseudo_box, None);
            return;
        }
        let start = self.item_position();
        let style = if content.display == html_style_model::Display::Contents { self.styles.anonymous_box_indices(content.style) } else { Some(content.style) };
        let pseudo_box = self.tree.push_box(LayoutBox { layout_mode: LayoutMode::Inline(Range::default()), dom_element: None, parent: Some(parent), style });
        if content.text.is_empty() {
            self.push_item(InlineItemKind::InlineBoundary { inline_start: start_edge, inline_end: end_edge }, pseudo_box, None);
        } else {
            self.push_generated_text(&content.text, pseudo_box, |glyphs| InlineItemKind::Text { glyphs });
        }
        self.finish_inline_box(pseudo_box, start);
    }

    pub(super) fn push_synthetic_box(&mut self, parent: u32, style: StyleIndices, layout_mode: LayoutMode) -> u32 {
        self.tree.push_box(LayoutBox { layout_mode, dom_element: None, parent: Some(parent), style: Some(style) })
    }

    fn push_anonymous_box(&mut self, parent: u32, layout_mode: LayoutMode, inherited_from: StyleIndices) -> u32 {
        let style = self.styles.anonymous_box_indices(inherited_from);
        self.tree.push_box(LayoutBox { layout_mode, dom_element: None, parent: Some(parent), style })
    }

    pub(super) fn set_generated_text_children(&mut self, container: u32, style: StyleIndices, text: &str, anonymous_text_box: bool) {
        let start = self.item_position();
        let run_box = if anonymous_text_box { self.push_anonymous_box(container, LayoutMode::Inline(Range::default()), style) } else { container };
        if !text.is_empty() {
            self.push_generated_text(text, run_box, |glyphs| InlineItemKind::Text { glyphs });
        }
        if anonymous_text_box {
            self.finish_inline_box(run_box, start);
        }
        let end = self.item_position();
        if start < end {
            self.set_box_children(container, Children::InlineItems(start..end));
        }
    }

    pub(super) fn materialize_generated_table(&mut self, parent_box: u32, plan: GeneratedTablePlan) -> Option<u32> {
        let GeneratedTablePlan { source: resolved, boxes, principal_box, content_target, anonymous_text_box } = plan;
        let style = resolved.style;
        let mut box_indices = Vec::with_capacity(boxes.len());
        for (index, planned) in boxes.iter().enumerate() {
            let parent = planned.parent.map(|parent| box_indices[parent]).unwrap_or(parent_box);
            let box_idx = if index == principal_box { self.push_synthetic_box(parent, style, planned.role.layout_mode()) } else { self.push_anonymous_box(parent, planned.role.layout_mode(), style) };
            box_indices.push(box_idx);
            match planned.role {
                TableRole::Table => {}
                TableRole::RowGroup => {
                    if let Some(table) = nearest_table(&boxes, planned.parent)
                        && let Some(LayoutMode::Table(table)) = self.tree.box_at_mut(box_indices[table] as usize).map(LayoutBox::layout_mode_mut)
                    {
                        table.row_groups.push(box_idx);
                    }
                }
                TableRole::Row => {
                    if let Some(table) = nearest_table(&boxes, planned.parent)
                        && let Some(LayoutMode::Table(table)) = self.tree.box_at_mut(box_indices[table] as usize).map(LayoutBox::layout_mode_mut)
                    {
                        table.rows.push(box_idx);
                    }
                    if let Some(group) = planned.parent.filter(|parent| matches!(boxes[*parent].role, TableRole::RowGroup))
                        && let Some(LayoutMode::Block(group)) = self.tree.box_at_mut(box_indices[group] as usize).map(LayoutBox::layout_mode_mut)
                    {
                        match &mut group.children {
                            children @ Children::Empty => *children = Children::Blocks(vec![box_idx]),
                            Children::Blocks(rows) => rows.push(box_idx),
                            Children::InlineItems(_) => unreachable!("planned table row groups cannot contain inline items"),
                        }
                    }
                }
                TableRole::Cell => {
                    if let Some(row) = planned.parent.filter(|parent| matches!(boxes[*parent].role, TableRole::Row))
                        && let Some(LayoutMode::TableRow(row)) = self.tree.box_at_mut(box_indices[row] as usize).map(LayoutBox::layout_mode_mut)
                    {
                        row.cells.push(box_idx);
                    }
                }
                TableRole::Caption(side) => {
                    if let Some(table) = nearest_table(&boxes, planned.parent)
                        && let Some(LayoutMode::Table(table)) = self.tree.box_at_mut(box_indices[table] as usize).map(LayoutBox::layout_mode_mut)
                    {
                        match side {
                            html_style_model::CaptionSide::Top => table.captions_top.push(box_idx),
                            html_style_model::CaptionSide::Bottom => table.captions_bottom.push(box_idx),
                        }
                    }
                }
            }
        }
        self.set_generated_text_children(box_indices[content_target], style, &resolved.text, anonymous_text_box);
        box_indices.first().copied()
    }
}
