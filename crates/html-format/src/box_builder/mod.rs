use crate::layout_model::{
    Children, InlineContent, InlineItemKind, LayoutMode, LayoutTree, TableBox, TableRowBox,
    parse_positive_span,
};
use html_dom::element_is_note_target;
use html_dom::{Document, DomNodeId, NodeRef};
use html_engine_model::NoteFlow;
use html_style_model::{ComputedStyles, Display, StyleIndices};
use html_text::{FirstLetterPart, FirstLetterSelection};
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

use self::classification::{
    BoxClassification, BoxClassifications, layout_mode_for_display, pseudo_display,
};
use self::generated::GeneratedContentResolver;
use self::inline::{InlineFragmentPlan, InlineNodePlan, SplitInlineEvent};
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
    classifications: BoxClassifications,
    output: BoxTreeWriter<'a, 'out>,
    generated: GeneratedContentResolver<'a>,
    list_item_ordinals: ListItemOrdinals,
    floated_first_letter: Option<FloatedFirstLetterCapture>,
    node_scratch: Vec<DomNodeId>,
    split_events: Vec<SplitInlineEvent>,
    split_active: Vec<InlineFragmentPlan>,
    inline_frames: Vec<InlineFragmentFrame>,
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
enum BoxSource {
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
        Self {
            document,
            styles,
            classifications: BoxClassifications::new(document, styles),
            output: BoxTreeWriter::new(styles, layout_tree, inline_content),
            generated: GeneratedContentResolver::new(document, styles),
            list_item_ordinals: ListItemOrdinals::new(document, styles),
            floated_first_letter: None,
            node_scratch: Vec::new(),
            split_events: Vec::new(),
            split_active: Vec::new(),
            inline_frames: Vec::new(),
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
        self.output.finish(root_box);
    }

    fn classify_source(&self, source: BoxSource) -> Option<BoxClassification> {
        match source {
            BoxSource::Node(node) => self
                .document
                .try_element_ref(node)
                .map(|element| self.classifications.get(element)),
            BoxSource::GeneratedPseudo { origin, before } => {
                BoxClassification::pseudo(self.styles, origin, before)
            }
        }
    }

    fn build_source(
        &mut self,
        source: BoxSource,
        parent: u32,
        classification: BoxClassification,
    ) -> Option<u32> {
        let (origin, before) = match source {
            BoxSource::Node(node) => {
                return self.build_classified_element(node, Some(parent), classification);
            }
            BoxSource::GeneratedPseudo { origin, before } => (origin, before),
        };
        if matches!(
            classification.display,
            Display::None | Display::TableColumn | Display::TableColumnGroup
        ) {
            return None;
        }
        let content = self.generated.resolve_pseudo(origin, before)?;
        if classification.splits_inline()
            && (content.display.is_table_internal() || content.display == Display::Table)
        {
            return self.output.materialize_generated_table(
                parent,
                plan_generated_table_principal(self.styles, content)?,
            );
        }
        let box_idx = self.output.push_generated_text_box(parent, &content);
        if classification.splits_inline()
            && matches!(
                content.display,
                Display::ListItem | Display::FlowRootListItem
            )
        {
            self.build_list_marker(box_idx, content.style, 1);
        }
        Some(box_idx)
    }

    fn process_inline_source(&mut self, source: BoxSource, parent: u32) {
        match source {
            BoxSource::Node(node) => self.process_inline_node(node, parent),
            BoxSource::GeneratedPseudo { origin, before } => {
                self.append_generated_inline_pseudo(origin, parent, before)
            }
        }
    }

    fn build_box_for_element(
        &mut self,
        elem_id: DomNodeId,
        parent_box: Option<u32>,
    ) -> Option<u32> {
        let element = self.document.element_ref(elem_id);
        let classification = self.classifications.get(element);
        self.build_classified_element(elem_id, parent_box, classification)
    }

    fn build_classified_element(
        &mut self,
        elem_id: DomNodeId,
        parent_box: Option<u32>,
        classification: BoxClassification,
    ) -> Option<u32> {
        let element = self.document.element_ref(elem_id);
        let style_indices = classification.style?;
        let mut display = classification.display;

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
        if classification.clearing_break {
            layout_mode = LayoutMode::Block(Children::default());
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
            && (classification.floated || classification.absolute)
        {
            display = Display::Block;
            layout_mode = LayoutMode::Block(Children::default());
        }
        let box_idx = self.output.push_dom_box(
            element,
            display,
            parent_box,
            Some(style_indices),
            layout_mode,
        );

        self.generated.enter_sibling_scope();

        if let Some(image_idx) = element.image_idx() {
            self.build_replaced_content(box_idx, image_idx);
        } else {
            match display {
                Display::Table | Display::InlineTable => {
                    self.build_table_children_from_nodes(element.children(), box_idx)
                }
                Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup => {
                    self.build_table_row_group_children(element, box_idx)
                }
                Display::TableRow => self.build_row_children(element.children(), box_idx),
                Display::Flex | Display::InlineFlex | Display::Grid | Display::InlineGrid => {
                    self.build_flex_grid_children(element, box_idx)
                }
                _ => {
                    if classification.has_block_child {
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
        self.output.set_child_boxes(box_idx, direct_children);
    }

    fn flush_anonymous_table_segment(
        &mut self,
        nodes: &mut Vec<DomNodeId>,
        parent_box_idx: u32,
    ) -> Option<u32> {
        if nodes.is_empty() {
            return None;
        }
        let table_idx = self
            .output
            .push_anonymous_child(parent_box_idx, LayoutMode::Table(Default::default()));
        self.build_table_children_from_nodes(std::mem::take(nodes), table_idx);
        Some(table_idx)
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
