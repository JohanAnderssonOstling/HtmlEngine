use html_dom::storage;
use crate::layout_model::{
    BlockBox, Children, InlineContent, InlineItem, InlineItemKind, LayoutBox, LayoutMode,
    LayoutTree, ListItemMarker, TableBox, TableCellBox, TableRowBox,
};
use html_dom::ElementRef;
use html_style_model::{CaptionSide, ComputedStyles, Display, Float, ListStylePosition, PositionMode, StyleIndices, StyleView};
use html_text::get_style;
use std::ops::Range;

use super::generated::ResolvedGeneratedContent;
use super::inline::GeneratedInlinePlan;
use super::table::{GeneratedTablePlan, TableRole, nearest_table, plan_generated_table_principal};

/// Owns the invariant-sensitive construction of the two box-building outputs.
/// DOM traversal and CSS generated-content state deliberately stay outside.
pub(super) struct BoxTreeWriter<'styles, 'out> {
    styles: &'styles ComputedStyles,
    tree: &'out mut LayoutTree,
    inline: &'out mut InlineContent,
}

impl<'styles, 'out> BoxTreeWriter<'styles, 'out> {
    pub(super) fn new(
        styles: &'styles ComputedStyles,
        tree: &'out mut LayoutTree,
        inline: &'out mut InlineContent,
    ) -> Self {
        Self {
            styles,
            tree,
            inline,
        }
    }

    pub(super) fn set_root_box(&mut self, root: Option<u32>) {
        self.tree.set_root_box(root);
    }

    pub(super) fn item_position(&self) -> u32 {
        self.inline.inline_items().len() as u32
    }

    fn layout_mode_mut(&mut self, box_idx: u32) -> Option<&mut LayoutMode> {
        self.tree
            .box_at_mut(box_idx as usize)
            .map(LayoutBox::layout_mode_mut)
    }

    pub(super) fn style_for_box(&self, box_idx: usize) -> StyleView<'_> {
        get_style(self.styles, self.tree, box_idx)
    }

    pub(super) fn push_dom_box(
        &mut self,
        element: ElementRef<'_>,
        parent: Option<u32>,
        style: Option<StyleIndices>,
        layout_mode: LayoutMode,
    ) -> u32 {
        let box_idx = self.tree.push_box(LayoutBox {
            layout_mode,
            dom_element: Some(element.node_id().raw()),
            parent,
            style,
        });
        if let Some(image_idx) = storage::image_idx(element) {
            self.tree.set_box_image_idx(box_idx, image_idx);
        }
        if element.tag().eq_ignore_ascii_case("body") {
            self.tree.set_body_box(box_idx);
        }
        box_idx
    }

    pub(super) fn push_anonymous_child(&mut self, parent: u32, layout_mode: LayoutMode) -> u32 {
        let style = self
            .tree
            .get_box_style_indices(parent as usize)
            .and_then(|style| self.styles.anonymous_box_indices(style));
        self.push_synthetic_box(parent, style, layout_mode)
    }

    pub(super) fn push_anonymous_row(&mut self, parent: u32) -> u32 {
        self.push_anonymous_child(parent, LayoutMode::TableRow(TableRowBox::default()))
    }

    pub(super) fn push_anonymous_cell(&mut self, parent: u32) -> u32 {
        self.push_anonymous_child(
            parent,
            LayoutMode::TableCell(TableCellBox::new(Children::Empty)),
        )
    }

    pub(super) fn push_anonymous_inline(&mut self, parent: u32) -> (u32, u32) {
        let start = self.item_position();
        let box_idx = self.push_anonymous_child(parent, LayoutMode::Anonymous(start..start));
        (box_idx, start)
    }

    /// Anonymous inline style carrier for text promoted through a
    /// `display: contents` element. Only inherited values are copied; reset
    /// box/paint properties on the suppressed principal box must not leak.
    pub(super) fn push_contents_style_box(
        &mut self,
        parent: u32,
        inherited_from: StyleIndices,
    ) -> (u32, u32) {
        let start = self.item_position();
        let style = self.styles.anonymous_box_indices(inherited_from);
        let box_idx = self.push_synthetic_box(parent, style, LayoutMode::Inline(start..start));
        (box_idx, start)
    }

    pub(super) fn push_item(
        &mut self,
        kind: InlineItemKind,
        box_idx: u32,
        dom_text_node: Option<u32>,
    ) {
        self.inline.push_inline_item(InlineItem {
            kind,
            box_idx,
            dom_text_node,
        });
    }

    pub(super) fn push_inline_boundary(
        &mut self,
        box_idx: u32,
        inline_start: bool,
        inline_end: bool,
    ) {
        self.push_item(
            InlineItemKind::InlineBoundary {
                inline_start,
                inline_end,
            },
            box_idx,
            None,
        );
    }

    pub(super) fn push_source_text(&mut self, text: &str, box_idx: u32, dom_text_node: u32) {
        self.push_source_text_fragment(text, box_idx, dom_text_node, 0);
    }

    pub(super) fn push_source_text_fragment(
        &mut self,
        text: &str,
        box_idx: u32,
        dom_text_node: u32,
        source_utf16_offset: u32,
    ) {
        if text.is_empty() {
            return;
        }
        let glyph_start = self.inline.characters().len() as u32;
        let mut utf16_offset = source_utf16_offset;
        for character in text.chars() {
            self.inline.push_source_character(character, utf16_offset);
            utf16_offset = utf16_offset.saturating_add(character.len_utf16() as u32);
        }
        let glyph_end = self.inline.characters().len() as u32;
        self.push_item(
            InlineItemKind::Text {
                glyphs: glyph_start..glyph_end,
            },
            box_idx,
            Some(dom_text_node),
        );
    }

    pub(super) fn push_word_break(&mut self, box_idx: u32) {
        // HTML represents wbr as a zero-width space for line breaking. It has
        // no DOM text node or authored UTF-16 offset.
        self.push_generated_text("\u{200b}", box_idx, |glyphs| InlineItemKind::Text {
            glyphs,
        });
    }

    fn push_generated_text(
        &mut self,
        text: &str,
        box_idx: u32,
        kind: impl FnOnce(Range<u32>) -> InlineItemKind,
    ) {
        let glyph_start = self.inline.characters().len() as u32;
        for character in text.chars() {
            self.inline.push_character(character);
        }
        let glyph_end = self.inline.characters().len() as u32;
        self.push_item(kind(glyph_start..glyph_end), box_idx, None);
    }

    pub(super) fn finish_inline_box(&mut self, box_idx: u32, start: u32) -> bool {
        let end = self.item_position();
        if let Some(LayoutMode::Inline(range) | LayoutMode::Anonymous(range)) =
            self.layout_mode_mut(box_idx)
        {
            *range = start..end;
        }
        start < end
    }

    pub(super) fn set_inline_fragment_edges(&mut self, box_idx: u32, first: bool, last: bool) {
        self.tree.set_inline_fragment_edges(box_idx, first, last);
    }

    pub(super) fn set_split_inline_position_ancestors(
        &mut self,
        box_idx: u32,
        ancestors: Vec<StyleIndices>,
    ) {
        self.tree
            .set_split_inline_position_ancestors(box_idx, ancestors);
    }

    pub(super) fn set_box_children(&mut self, box_idx: u32, children: Children) {
        match self.layout_mode_mut(box_idx) {
            Some(LayoutMode::Block(block)) => block.children = children,
            Some(LayoutMode::TableCell(cell)) => cell.children = children,
            _ => {}
        }
    }

    pub(super) fn set_flex_grid_children(&mut self, box_idx: u32, children: Vec<u32>) {
        match self.layout_mode_mut(box_idx) {
            Some(LayoutMode::Flex(container)) => container.children = children,
            Some(LayoutMode::Grid(container)) => container.children = children,
            _ => unreachable!("flex/grid child construction requires a flex/grid layout mode"),
        }
    }

    pub(super) fn extend_with_group_rows(&self, group_idx: u32, rows: &mut Vec<u32>) {
        if let Some(LayoutMode::Block(group)) = self.tree.get_box_layout_mode(group_idx as usize)
            && let Children::Blocks(group_rows) = &group.children
        {
            rows.extend(group_rows.iter().copied().filter(|&row| {
                matches!(
                    self.tree.get_box_layout_mode(row as usize),
                    Some(LayoutMode::TableRow(_))
                )
            }));
        }
    }

    pub(super) fn finish_table(&mut self, table_idx: u32, table: TableBox) {
        // HTML rowspan=0 covers the remaining rows of the current row group.
        // Resolve it before publishing the table so layout and queries share
        // the same positive spans, including for anonymous row groups.
        let mut group_start = 0;
        while group_start < table.rows.len() {
            let parent = self.tree.get_box_parent(table.rows[group_start] as usize);
            let mut group_end = group_start + 1;
            while group_end < table.rows.len()
                && self.tree.get_box_parent(table.rows[group_end] as usize) == parent
            {
                group_end += 1;
            }
            for (offset, &row_idx) in table.rows[group_start..group_end].iter().enumerate() {
                let Some(LayoutMode::TableRow(row)) = self.tree.get_box_layout_mode(row_idx as usize)
                else {
                    continue;
                };
                let cell_count = row.cells.len();
                for position in 0..cell_count {
                    let Some(LayoutMode::TableRow(row)) =
                        self.tree.get_box_layout_mode(row_idx as usize)
                    else {
                        continue;
                    };
                    let cell_idx = row.cells[position];
                    if let Some(LayoutMode::TableCell(cell)) = self.layout_mode_mut(cell_idx)
                        && cell.rowspan == 0
                    {
                        cell.rowspan = group_end - group_start - offset;
                    }
                }
            }
            group_start = group_end;
        }
        if let Some(LayoutMode::Table(current)) = self.layout_mode_mut(table_idx) {
            *current = table;
        }
    }

    pub(super) fn finish_row(&mut self, row_idx: u32, row: TableRowBox) {
        if let Some(LayoutMode::TableRow(current)) = self.layout_mode_mut(row_idx) {
            *current = row;
        }
    }

    pub(super) fn emit_marker(
        &mut self,
        list_item_box: u32,
        style: StyleIndices,
        position: ListStylePosition,
        text: &str,
    ) {
        let marker_style = self.styles.anonymous_box_indices(style).unwrap_or(style);
        let marker_box = self.push_synthetic_box(
            list_item_box,
            Some(marker_style),
            LayoutMode::Inline(Range::default()),
        );
        let start = self.item_position();
        self.push_generated_text(text, marker_box, |glyphs| InlineItemKind::Marker { glyphs });
        self.finish_inline_box(marker_box, start);
        self.tree.set_list_marker(
            list_item_box,
            ListItemMarker {
                marker_box,
                position,
            },
        );
    }

    pub(super) fn emit_generated_inline(&mut self, parent: u32, plan: GeneratedInlinePlan) {
        let GeneratedInlinePlan {
            content,
            start_edge,
            end_edge,
        } = plan;
        let style = self.styles.view(content.style).expect("validated generated style");
        let absolute = style.layout.position == PositionMode::Absolute;
        if absolute || style.float() != Float::None {
            // Blockification does not put a positioned/floated pseudo into
            // normal block flow. Materialize its own principal box, then
            // retain the same zero-width source anchor as a real element.
            let pseudo_box = if matches!(content.display, Display::Table | Display::InlineTable) {
                let Some(plan) = plan_generated_table_principal(self.styles, content) else { return; };
                let Some(principal) = self.materialize_generated_table(parent, plan) else { return; };
                principal
            } else {
                self.push_generated_text_box(parent, &content)
            };
            let kind = if absolute { InlineItemKind::AbsoluteAnchor { box_idx: pseudo_box } }
                else { InlineItemKind::FloatAnchor { box_idx: pseudo_box } };
            self.push_item(kind, parent, None);
            return;
        }
        if content.display == Display::InlineBlock {
            // Generated inline-blocks are atomic inline-level boxes. Keeping
            // them as text-only inline fragments would discard their authored
            // width, height, and baseline contribution when `content` is empty.
            let pseudo_box = self.push_generated_text_box(parent, &content);
            self.push_item(
                InlineItemKind::AtomicBox {
                    box_idx: pseudo_box,
                },
                pseudo_box,
                None,
            );
            return;
        }
        let start = self.item_position();
        let style = if content.display == html_style_model::Display::Contents {
            self.styles.anonymous_box_indices(content.style)
        } else {
            Some(content.style)
        };
        let pseudo_box =
            self.push_synthetic_box(parent, style, LayoutMode::Inline(Range::default()));
        if content.text.is_empty() {
            self.push_inline_boundary(pseudo_box, start_edge, end_edge);
        } else {
            self.push_generated_text(&content.text, pseudo_box, |glyphs| InlineItemKind::Text {
                glyphs,
            });
        }
        self.finish_inline_box(pseudo_box, start);
    }

    pub(super) fn push_synthetic_box(
        &mut self,
        parent: u32,
        style: Option<StyleIndices>,
        layout_mode: LayoutMode,
    ) -> u32 {
        self.tree.push_box(LayoutBox {
            layout_mode,
            dom_element: None,
            parent: Some(parent),
            style,
        })
    }

    fn push_anonymous_box(
        &mut self,
        parent: u32,
        layout_mode: LayoutMode,
        inherited_from: StyleIndices,
    ) -> u32 {
        let style = self.styles.anonymous_box_indices(inherited_from);
        self.push_synthetic_box(parent, style, layout_mode)
    }

    pub(super) fn push_generated_text_box(
        &mut self,
        parent: u32,
        content: &ResolvedGeneratedContent,
    ) -> u32 {
        let box_idx = self.push_synthetic_box(
            parent,
            Some(content.style),
            LayoutMode::Block(BlockBox::default()),
        );
        self.set_generated_text_children(box_idx, content.style, &content.text, false);
        box_idx
    }

    pub(super) fn set_generated_text_children(
        &mut self,
        container: u32,
        style: StyleIndices,
        text: &str,
        anonymous_text_box: bool,
    ) {
        let start = self.item_position();
        let run_box = if anonymous_text_box {
            self.push_anonymous_box(container, LayoutMode::Inline(Range::default()), style)
        } else {
            container
        };
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

    pub(super) fn materialize_generated_table(
        &mut self,
        parent_box: u32,
        plan: GeneratedTablePlan,
    ) -> Option<u32> {
        let GeneratedTablePlan {
            source: resolved,
            boxes,
            principal_box,
            content_target,
            anonymous_text_box,
        } = plan;
        let style = resolved.style;
        let mut box_indices = Vec::with_capacity(boxes.len());
        for (index, planned) in boxes.iter().enumerate() {
            let parent = planned
                .parent
                .map_or(parent_box, |parent| box_indices[parent]);
            let box_idx = if index == principal_box {
                self.push_synthetic_box(parent, Some(style), planned.role.layout_mode())
            } else {
                self.push_anonymous_box(parent, planned.role.layout_mode(), style)
            };
            box_indices.push(box_idx);
            match planned.role {
                TableRole::Table => {}
                TableRole::RowGroup | TableRole::Row | TableRole::Caption(_) => {
                    if let Some(table) = nearest_table(&boxes, planned.parent)
                        && let Some(LayoutMode::Table(table)) =
                            self.layout_mode_mut(box_indices[table])
                    {
                        let children = match planned.role {
                            TableRole::RowGroup => &mut table.row_groups,
                            TableRole::Row => &mut table.rows,
                            TableRole::Caption(CaptionSide::Top) => &mut table.captions_top,
                            TableRole::Caption(CaptionSide::Bottom) => &mut table.captions_bottom,
                            _ => unreachable!("only table children enter this branch"),
                        };
                        children.push(box_idx);
                    }
                    if matches!(planned.role, TableRole::Row)
                        && let Some(group) = planned
                            .parent
                            .filter(|parent| matches!(boxes[*parent].role, TableRole::RowGroup))
                        && let Some(LayoutMode::Block(group)) =
                            self.layout_mode_mut(box_indices[group])
                    {
                        match &mut group.children {
                            children @ Children::Empty => {
                                *children = Children::Blocks(vec![box_idx])
                            }
                            Children::Blocks(rows) => rows.push(box_idx),
                            Children::InlineItems(_) => {
                                unreachable!("planned table row groups cannot contain inline items")
                            }
                        }
                    }
                }
                TableRole::Cell => {
                    if let Some(row) = planned
                        .parent
                        .filter(|parent| matches!(boxes[*parent].role, TableRole::Row))
                        && let Some(LayoutMode::TableRow(row)) =
                            self.layout_mode_mut(box_indices[row])
                    {
                        row.cells.push(box_idx);
                    }
                }
            }
        }
        self.set_generated_text_children(
            box_indices[content_target],
            style,
            &resolved.text,
            anonymous_text_box,
        );
        box_indices.first().copied()
    }
}
