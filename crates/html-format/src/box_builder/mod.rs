use html_dom::storage;
use crate::layout_model::{
    BlockBox, Children, InlineContent, InlineItemKind, LayoutMode, LayoutTree, TableBox,
    TableRowBox, parse_positive_span,
};
use html_dom::element_is_note_target;
use html_dom::{Document, DomNodeId, NodeRef};
use html_engine_model::NoteFlow;
use html_style_model::{ComputedStyles, Display, StyleIndices};
use html_text::{FirstLetterPart, FirstLetterSelection};
use rustc_data_structures::fx::FxHashMap;
use std::ops::Range;

mod block;
mod classification;
mod construction_flex;
mod construction_inline;
mod construction_table;
mod generated;
mod inline;
mod lists;
mod table;
mod writer;

use self::block::{BlockChildAction, plan_block_children};
use self::classification::{
    display_for_element, is_absolutely_positioned,
    is_clearing_break, is_floated, layout_mode_for_display, pseudo_display, pseudo_is_block_level,
};
use self::generated::GeneratedContentResolver;
use self::inline::{
    InlineFragmentPlan, InlineNodePlan, SplitInlineEvent, SplitInlinePart,
    has_block_child, plan_generated_inline, plan_inline_node, plan_split_inline,
    trim_inline_segment_nodes,
};
use self::lists::ListItemOrdinals;
use self::table::{
    AnonymousCellPart, AnonymousTablePlan, TableChildAction, TableRole, TableRowChildAction,
    plan_anonymous_table_children, plan_generated_table_principal, plan_table_children,
    plan_table_row_children,
};
use self::writer::BoxTreeWriter;

struct LayoutTreeBuilder<'a, 'out> {
    document: &'a Document,
    styles: &'a ComputedStyles,
    output: BoxTreeWriter<'a, 'out>,
    generated: GeneratedContentResolver<'a>,
    list_item_ordinals: ListItemOrdinals,
    floated_first_letter: Option<FloatedFirstLetterCapture>,
    contents_text_styles: FxHashMap<DomNodeId, StyleIndices>,
    note_flow: NoteFlow,
}

struct InlineFragmentFrame {
    node: DomNodeId,
    box_idx: u32,
    item_start: u32,
    first_fragment: bool,
}

struct FloatedFirstLetterCapture {
    box_idx: u32,
    item_start: u32,
    item_end: Option<u32>,
    selection: FirstLetterSelection,
}

#[derive(Clone, Copy)]
enum FlexInlineSource {
    Node(DomNodeId),
    GeneratedPseudo { origin: DomNodeId, before: bool },
}

impl<'a, 'out> LayoutTreeBuilder<'a, 'out> {
    fn new(
        document: &'a Document,
        styles: &'a ComputedStyles,
        layout_tree: &'out mut LayoutTree,
        inline_content: &'out mut InlineContent,
        note_flow: NoteFlow,
    ) -> Self {
        let mut contents_text_styles = FxHashMap::default();
        if let Some(root) = document.dom_root() {
            collect_contents_text_styles(document, styles, root, None, &mut contents_text_styles);
        }
        Self {
            document,
            styles,
            output: BoxTreeWriter::new(styles, layout_tree, inline_content),
            generated: GeneratedContentResolver::new(document, styles),
            list_item_ordinals: ListItemOrdinals::new(document, styles),
            floated_first_letter: None,
            contents_text_styles,
            note_flow,
        }
    }

    fn build(&mut self) {
        if let Some(root_idx) = self.document.dom_root() {
            self.build_from(root_idx);
        }
    }

    fn build_from(&mut self, root: DomNodeId) {
        let root_box = self.build_box_for_element(root, None);
        self.output.set_root_box(root_box);
    }

    fn build_box_for_element(
        &mut self,
        elem_id: DomNodeId,
        parent_box: Option<u32>,
    ) -> Option<u32> {
        let element = self.document.element_ref(elem_id)?;
        let style_indices = self.styles.style_for_node(element.node_id())?;
        let mut display = display_for_element(self.styles, element);

        // A note held back for the embedder's own presentation generates no
        // boxes, exactly as `display: none` would. The subtree stays in the DOM
        // so link targets still resolve and a scoped layout can lay the note
        // out on its own terms. Suppression is skipped when this element is
        // itself the build root, which is how that scoped layout reaches it.
        if self.note_flow == NoteFlow::Excluded
            && parent_box.is_some()
            && element_is_note_target(element)
        {
            display = Display::None;
        }

        // The document root must continue to establish the initial containing
        // block. CSS Display gives `contents` on the root a block-level used
        // value rather than removing the root principal box.
        if parent_box.is_none() && display == Display::Contents {
            display = Display::Block;
        }

        if display == Display::None {
            return None;
        }

        self.generated.apply_for_node(elem_id);

        let mut layout_mode = layout_mode_for_display(display)?;
        if is_clearing_break(self.styles, element) {
            layout_mode = LayoutMode::Block(BlockBox::default());
        }
        if let LayoutMode::TableCell(cell) = &mut layout_mode {
            cell.colspan = parse_positive_span(element.attr("colspan"));
            // Zero grows to the end of the row group once all rows are built.
            cell.rowspan = element
                .attr("rowspan")
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(1)
                .min(65_534);
        }
        if let LayoutMode::Inline(_) = layout_mode
            && (is_floated(self.styles, element) || is_absolutely_positioned(self.styles, element))
        {
            layout_mode = LayoutMode::Block(BlockBox::default());
        }
        let box_idx =
            self.output
                .push_dom_box(element, parent_box, Some(style_indices), layout_mode);

        self.generated.enter_sibling_scope();

        if let Some(image_idx) = storage::image_idx(element) {
            self.build_replaced_content(box_idx, image_idx);
        } else {
            match display {
                Display::Table | Display::InlineTable => self.build_table_children_from_nodes(
                    element.children().map(DomNodeId::raw),
                    box_idx,
                ),
                Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup => {
                    self.build_table_row_group_children(element, box_idx)
                }
                Display::TableRow => {
                    self.build_row_children(element.children().map(DomNodeId::raw), box_idx)
                }
                Display::Flex | Display::InlineFlex | Display::Grid | Display::InlineGrid => {
                    self.build_flex_grid_children(element, box_idx)
                }
                _ => {
                    if has_block_child(self.styles, element) {
                        self.build_block_children(elem_id, box_idx);
                    } else {
                        self.build_inline_children(element, box_idx);
                    }
                }
            }
        }

        if matches!(display, Display::ListItem | Display::FlowRootListItem) {
            let ordinal = self.list_item_ordinals.get(elem_id);
            self.build_list_marker(box_idx, style_indices, ordinal);
        }

        self.generated.leave_sibling_scope();

        Some(box_idx)
    }

    fn build_list_marker(&mut self, list_item_box: u32, style_indices: StyleIndices, ordinal: i64) {
        let style = self
            .styles
            .view(style_indices)
            .expect("validated style handle");
        let list_style_type = style.list_style_type();
        let position = style.list_style_position();
        let Some(text) = super::list_marker::marker_text(list_style_type, ordinal) else {
            return;
        };

        self.output
            .emit_marker(list_item_box, style_indices, position, &text);
    }

    fn build_block_children(&mut self, elem_id: DomNodeId, box_idx: u32) {
        let mut direct_children = Vec::new();
        self.append_block_children(elem_id, box_idx, &mut direct_children);
        self.output
            .set_box_children(box_idx, Children::Blocks(direct_children));
    }

    fn append_block_children(
        &mut self,
        elem_id: DomNodeId,
        box_idx: u32,
        direct_children: &mut Vec<u32>,
    ) {
        for action in plan_block_children(self.document, self.styles, elem_id) {
            let child = match action {
                BlockChildAction::GeneratedPseudo { before } => {
                    self.build_generated_block_pseudo(elem_id, box_idx, before)
                }
                BlockChildAction::InlineSegment {
                    mut nodes,
                    before,
                    after,
                } => self.flush_inline_segment_with_generated_edges(
                    &mut nodes,
                    box_idx,
                    before.then_some(elem_id),
                    after.then_some(elem_id),
                ),
                BlockChildAction::AnonymousTable(mut nodes) => {
                    self.flush_anonymous_table_segment(&mut nodes, box_idx)
                }
                BlockChildAction::Principal(node) => {
                    self.build_box_for_element(node, Some(box_idx))
                }
                BlockChildAction::SplitInline(node) => {
                    self.build_split_inline_child(node, box_idx, direct_children);
                    continue;
                }
                BlockChildAction::ContentsWithTableInternals(node) => {
                    self.generated.apply_for_node(node);
                    self.append_block_children(node, box_idx, direct_children);
                    continue;
                }
            };
            direct_children.extend(child);
        }
    }

    fn flush_anonymous_table_segment(
        &mut self,
        nodes: &mut Vec<u32>,
        parent_box_idx: u32,
    ) -> Option<u32> {
        if nodes.is_empty() {
            return None;
        }
        let table_idx = self
            .output
            .push_anonymous_child(parent_box_idx, LayoutMode::Table(TableBox::default()));
        self.build_table_children_from_nodes(std::mem::take(nodes), table_idx);
        Some(table_idx)
    }
}

fn collect_contents_text_styles(
    document: &Document,
    styles: &ComputedStyles,
    node: DomNodeId,
    inherited_from_contents: Option<StyleIndices>,
    output: &mut FxHashMap<DomNodeId, StyleIndices>,
) {
    match document.node_ref(node) {
        Some(NodeRef::Text(_)) => {
            if let Some(style) = inherited_from_contents {
                output.insert(node, style);
            }
        }
        Some(NodeRef::Element(element)) => {
            let display = display_for_element(styles, element);
            if display == Display::None {
                return;
            }
            let inherited = if display == Display::Contents {
                styles.style_for_node(node).or(inherited_from_contents)
            } else {
                None
            };
            for child in element.children() {
                collect_contents_text_styles(document, styles, child, inherited, output);
            }
        }
        None => {}
    }
}

pub fn build_layout_inputs(
    document: &Document,
    styles: &ComputedStyles,
    layout_tree: &mut LayoutTree,
    inline_content: &mut InlineContent,
    note_flow: NoteFlow,
) {
    LayoutTreeBuilder::new(document, styles, layout_tree, inline_content, note_flow).build();
}

/// Builds a box tree rooted at `root` rather than at the document root, reusing
/// the document's computed styles. Inherited values are already resolved, so a
/// subtree laid out this way keeps the typography it would have had in place.
pub fn build_layout_inputs_from(
    document: &Document,
    styles: &ComputedStyles,
    layout_tree: &mut LayoutTree,
    inline_content: &mut InlineContent,
    note_flow: NoteFlow,
    root: DomNodeId,
) {
    LayoutTreeBuilder::new(document, styles, layout_tree, inline_content, note_flow)
        .build_from(root);
}

include!("tests.rs");
