use crate::layout_model::{BlockBox, Children, InlineContent, InlineItemKind, LayoutMode, LayoutTree};
use html_dom::{Document, DomNodeId, NodeRef};
use html_style_model::{ComputedStyles, Display, Float, ListStyleType, StyleIndices};
use rustc_data_structures::fx::FxHashMap;
use std::ops::Range;
use unicode_categories::UnicodeCategories;

mod block;
mod classification;
mod generated;
mod inline;
mod lists;
mod table;
mod writer;

use self::block::{BlockChildAction, plan_block_children};
use self::classification::{display_for_element, empty_table_box, is_block_level_pseudo_display, is_clearing_break, is_table_internal_display, layout_mode_for_display};
use self::generated::GeneratedContentResolver;
use self::inline::{InlineElementPlan, InlineFragmentPlan, InlineNodePlan, SplitInlineEvent, SplitInlinePart, has_block_child, plan_generated_inline, plan_inline_node, plan_split_inline, trim_inline_segment_nodes};
use self::lists::ListItemOrdinals;
use self::table::{AnonymousCellPart, AnonymousTablePlan, TableChildAction, TablePrincipal, TableRole, TableRowChildAction, plan_anonymous_table_children, plan_generated_table_principal, plan_table_children, plan_table_row_children};
use self::writer::BoxTreeWriter;

struct LayoutTreeBuilder<'a, 'out> {
    document: &'a Document,
    styles: &'a ComputedStyles,
    output: BoxTreeWriter<'a, 'out>,
    generated: GeneratedContentResolver<'a>,
    list_item_ordinals: ListItemOrdinals,
    floated_first_letter: Option<FloatedFirstLetterCapture>,
    contents_text_styles: FxHashMap<DomNodeId, StyleIndices>,
}

struct InlineFragmentFrame {
    node: DomNodeId,
    box_idx: u32,
    item_start: u32,
    first_fragment: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum FirstLetterCapturePhase {
    Leading,
    Letter,
    Trailing,
    Done,
}

struct FloatedFirstLetterCapture {
    box_idx: u32,
    item_start: u32,
    item_end: Option<u32>,
    phase: FirstLetterCapturePhase,
}

#[derive(Clone, Copy)]
enum FlexInlineSource {
    Node(u32),
    GeneratedPseudo { origin: DomNodeId, before: bool },
}

fn is_first_letter_punctuation(character: char) -> bool {
    character.is_punctuation_open() || character.is_punctuation_close() || character.is_punctuation_initial_quote() || character.is_punctuation_final_quote() || character.is_punctuation_other()
}

impl<'a, 'out> LayoutTreeBuilder<'a, 'out> {
    fn new(document: &'a Document, styles: &'a ComputedStyles, layout_tree: &'out mut LayoutTree, inline_content: &'out mut InlineContent) -> Self {
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
        }
    }

    fn build(&mut self) {
        if let Some(root_idx) = self.document.dom_root() {
            let root_box = self.build_box_for_node(root_idx, None);
            self.output.set_root_box(root_box);
        }
    }

    fn build_box_for_node(&mut self, node_id: DomNodeId, parent_box: Option<u32>) -> Option<u32> {
        match self.document.node_ref(node_id)? {
            NodeRef::Element(_) => self.build_box_for_element(node_id, parent_box),
            NodeRef::Text(_) => None,
        }
    }

    fn build_box_for_element(&mut self, elem_id: DomNodeId, parent_box: Option<u32>) -> Option<u32> {
        let element = self.document.element_ref(elem_id)?;
        let style_indices = self.styles.style_for_node(element.node_id())?;
        let mut display = display_for_element(self.styles, element);

        // The document root must continue to establish the initial containing
        // block. CSS Display gives `contents` on the root a block-level used
        // value rather than removing the root principal box.
        if parent_box.is_none() && display == Display::Contents {
            display = Display::Block;
        }

        if display == Display::None {
            return None;
        }

        if let Some(directives) = self.styles.counter_directives_for_node(elem_id) {
            self.generated.apply(directives);
        }

        let has_block_child = has_block_child(self.document, self.styles, element);
        let mut layout_mode = layout_mode_for_display(display)?;
        if is_clearing_break(self.styles, element) {
            layout_mode = LayoutMode::Block(BlockBox { children: Children::Empty });
        }
        if let LayoutMode::TableCell(cell) = &mut layout_mode {
            cell.colspan = crate::table::parse_positive_span(self.document.get_dom_attr(elem_id, "colspan"));
            cell.rowspan = crate::table::parse_positive_span(self.document.get_dom_attr(elem_id, "rowspan"));
        }
        if matches!(self.styles.box_model_style(style_indices).expect("validated style handle").float, Float::Left | Float::Right) && matches!(layout_mode, LayoutMode::Inline(_)) {
            layout_mode = LayoutMode::Block(BlockBox { children: Children::Empty });
        }
        if self.styles.layout_style(style_indices).is_some_and(|style| style.position == html_style_model::PositionMode::Absolute) && matches!(layout_mode, LayoutMode::Inline(_)) {
            layout_mode = LayoutMode::Block(BlockBox { children: Children::Empty });
        }
        let box_idx = self.push_layout_box(elem_id, parent_box, Some(style_indices), layout_mode);

        self.generated.enter_sibling_scope();

        if let Some(image_idx) = element.image_idx() {
            self.build_replaced_content(box_idx, image_idx);
        } else {
            match display {
                Display::Table | Display::InlineTable => self.build_table_children(elem_id, box_idx),
                Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup => self.build_table_row_group_children(elem_id, box_idx),
                Display::TableRow => self.build_table_row_children(elem_id, box_idx),
                Display::TableCell => self.build_cell_children(elem_id, box_idx),
                Display::Flex | Display::InlineFlex | Display::Grid | Display::InlineGrid => self.build_flex_grid_children(elem_id, box_idx),
                _ => {
                    if has_block_child {
                        self.build_block_children(elem_id, box_idx);
                    } else {
                        self.build_inline_children(elem_id, box_idx);
                    }
                }
            }
        }

        if matches!(display, Display::ListItem | Display::FlowRootListItem) {
            self.build_list_marker(box_idx, elem_id, style_indices);
        }

        self.generated.leave_sibling_scope();

        Some(box_idx)
    }

    fn build_list_marker(&mut self, list_item_box: u32, elem_id: DomNodeId, style_indices: StyleIndices) {
        let list_style_type = self.styles.view(style_indices).expect("validated style handle").list_style_type();
        let ordinal = if list_style_type.is_bullet() { 0 } else { self.list_item_ordinals.get(elem_id) };
        self.build_list_marker_for_ordinal(list_item_box, style_indices, ordinal);
    }

    fn build_list_marker_for_ordinal(&mut self, list_item_box: u32, style_indices: StyleIndices, ordinal: i64) {
        let style = self.styles.view(style_indices).expect("validated style handle");
        let list_style_type = style.list_style_type();
        let position = style.list_style_position();
        if list_style_type == ListStyleType::None {
            return;
        }

        let Some(text) = super::list_marker::marker_text(list_style_type, ordinal) else {
            return;
        };

        self.output.emit_marker(list_item_box, style_indices, position, &text);
    }

    fn build_block_children(&mut self, elem_id: DomNodeId, box_idx: u32) {
        let mut direct_children = Vec::new();
        self.append_block_children(elem_id, box_idx, &mut direct_children);
        self.output.set_box_children(box_idx, Children::Blocks(direct_children));
    }

    fn append_block_children(&mut self, elem_id: DomNodeId, box_idx: u32, direct_children: &mut Vec<u32>) {
        for action in plan_block_children(self.document, self.styles, elem_id) {
            match action {
                BlockChildAction::GeneratedPseudo { before } => {
                    if let Some(pseudo) = self.build_generated_block_pseudo(elem_id, box_idx, before) {
                        direct_children.push(pseudo);
                    }
                }
                BlockChildAction::InlineSegment { mut nodes, before, after } => {
                    if let Some(anonymous) = self.flush_inline_segment_with_generated_edges(&mut nodes, box_idx, before.then_some(elem_id), after.then_some(elem_id)) {
                        direct_children.push(anonymous);
                    }
                }
                BlockChildAction::AnonymousTable(mut nodes) => {
                    if let Some(table) = self.flush_anonymous_table_segment(&mut nodes, box_idx) {
                        direct_children.push(table);
                    }
                }
                BlockChildAction::Principal(node) => {
                    if let Some(child) = self.build_box_for_node(node, Some(box_idx)) {
                        direct_children.push(child);
                    }
                }
                BlockChildAction::SplitInline(node) => self.build_split_inline_child(node, box_idx, direct_children),
                BlockChildAction::ContentsWithTableInternals(node) => {
                    if let Some(directives) = self.styles.counter_directives_for_node(node) {
                        self.generated.apply(directives);
                    }
                    self.append_block_children(node, box_idx, direct_children);
                }
            }
        }
    }

    fn flush_anonymous_table_segment(&mut self, nodes: &mut Vec<u32>, parent_box_idx: u32) -> Option<u32> {
        if nodes.is_empty() {
            return None;
        }
        let table_idx = self.output.push_anonymous_table(parent_box_idx, LayoutMode::Table(empty_table_box()));
        self.build_table_children_from_nodes(std::mem::take(nodes), table_idx);
        Some(table_idx)
    }

    fn build_split_inline_child(&mut self, elem_id: DomNodeId, parent_box_idx: u32, direct_children: &mut Vec<u32>) {
        for part in plan_split_inline(self.document, self.styles, elem_id) {
            match part {
                SplitInlinePart::Segment(events) => {
                    if let Some(anonymous) = self.materialize_split_inline_segment(events, parent_box_idx) {
                        direct_children.push(anonymous);
                    }
                }
                SplitInlinePart::Block { node, position_ancestors } => {
                    if let Some(block_idx) = self.build_box_for_node(node, Some(parent_box_idx)) {
                        self.output.set_split_inline_position_ancestors(block_idx, position_ancestors);
                        direct_children.push(block_idx);
                    }
                }
                SplitInlinePart::GeneratedBlock { origin, before, position_ancestors } => {
                    if let Some(block_idx) = self.build_generated_block_pseudo(origin, parent_box_idx, before) {
                        self.output.set_split_inline_position_ancestors(block_idx, position_ancestors);
                        direct_children.push(block_idx);
                    }
                }
            }
        }
    }

    fn materialize_split_inline_segment(&mut self, events: Vec<SplitInlineEvent>, parent_box_idx: u32) -> Option<u32> {
        if events.is_empty() {
            return None;
        }
        let (anonymous_idx, item_start) = self.output.push_anonymous_inline(parent_box_idx);

        let mut fragments = Vec::<InlineFragmentFrame>::new();
        for event in events {
            match event {
                SplitInlineEvent::Open { fragment, first_fragment } => {
                    let fragment_parent = fragments.last().map_or(anonymous_idx, |frame| frame.box_idx);
                    let frame = self.begin_inline_fragment(fragment.node, fragment_parent, fragment.style, first_fragment);
                    if first_fragment {
                        self.append_generated_inline_pseudo(fragment.node, frame.box_idx, true);
                        if fragment.start_edge {
                            self.output.push_item(InlineItemKind::InlineBoundary { inline_start: true, inline_end: false }, frame.box_idx, None);
                        }
                    }
                    fragments.push(frame);
                }
                SplitInlineEvent::Close { fragment, last_fragment } => {
                    let Some(frame) = fragments.pop() else { continue };
                    debug_assert_eq!(frame.node, fragment.node);
                    if last_fragment {
                        self.append_generated_inline_pseudo(fragment.node, frame.box_idx, false);
                        if fragment.end_edge {
                            self.output.push_item(InlineItemKind::InlineBoundary { inline_start: false, inline_end: true }, frame.box_idx, None);
                        }
                    }
                    self.output.set_inline_fragment_edges(frame.box_idx, frame.first_fragment, last_fragment);
                    self.finish_inline_fragment(frame, last_fragment);
                }
                SplitInlineEvent::Node(node) => {
                    let owner = fragments.last().map_or(anonymous_idx, |frame| frame.box_idx);
                    self.process_inline_node(node, owner);
                }
                SplitInlineEvent::EnterContents(node) => {
                    if let Some(directives) = self.styles.counter_directives_for_node(node) {
                        self.generated.apply(directives);
                    }
                }
                SplitInlineEvent::ExitContents => {}
                SplitInlineEvent::GeneratedPseudo { origin, before } => {
                    let owner = fragments.last().map_or(anonymous_idx, |frame| frame.box_idx);
                    self.append_generated_inline_pseudo(origin, owner, before);
                }
            }
        }
        debug_assert!(fragments.is_empty());
        self.output.finish_anonymous_box(anonymous_idx, item_start).then_some(anonymous_idx)
    }

    fn build_inline_children(&mut self, elem_id: DomNodeId, box_idx: u32) {
        let dom = self.document;
        let Some(element) = dom.element_ref(elem_id) else {
            return;
        };

        let start = self.output.item_position();
        let image_idx = element.image_idx();
        let children: Vec<u32> = element.children().map(|idx| idx.raw()).collect();

        if let Some(image_idx) = image_idx {
            self.output.push_item(InlineItemKind::Image { image_idx }, box_idx, None);
        }

        self.append_generated_inline_pseudo(elem_id, box_idx, true);

        let owns_floated_first_letter = image_idx.is_none() && self.begin_floated_first_letter(elem_id, box_idx);

        // Collapsible whitespace at the edges of a block container does not
        // generate a line box. This matters in particular when all intervening
        // element children compute to display:none (for example the unusual
        // HTML elements whose display:contents used value is none).
        let children = if image_idx.is_none() && self.styles.before_style_for_node(elem_id).is_none() && self.styles.after_style_for_node(elem_id).is_none() {
            let white_space = self.output.style_for_box(box_idx as usize).white_space();
            trim_inline_segment_nodes(self.document, self.styles, &children, white_space)
        } else {
            &children
        };
        self.process_inline_nodes(children, box_idx);

        if owns_floated_first_letter {
            self.finish_floated_first_letter();
        }

        self.append_generated_inline_pseudo(elem_id, box_idx, false);

        let end = self.output.item_position();

        if start < end {
            self.output.set_box_children(box_idx, Children::InlineItems(start..end));
        }
    }

    fn build_replaced_content(&mut self, box_idx: u32, image_idx: u32) {
        let start = self.output.item_position();
        self.output.push_item(InlineItemKind::Image { image_idx }, box_idx, None);
        self.output.set_box_children(box_idx, Children::InlineItems(start..self.output.item_position()));
    }

    fn build_cell_children(&mut self, elem_id: DomNodeId, box_idx: u32) {
        let Some(element) = self.document.element_ref(elem_id) else {
            return;
        };

        if has_block_child(self.document, self.styles, element) {
            self.build_block_children(elem_id, box_idx);
        } else {
            self.build_inline_children(elem_id, box_idx);
        }
    }

    /// Each in-flow element child is a flex/grid item regardless of its outer
    /// display type. Consecutive text nodes become one anonymous item. This is
    /// deliberately distinct from block-flow construction, which wraps inline
    /// elements together with text in an anonymous block.
    fn build_flex_grid_children(&mut self, elem_id: DomNodeId, box_idx: u32) {
        let Some(element) = self.document.element_ref(elem_id) else {
            return;
        };
        let children: Vec<u32> = element.children().map(DomNodeId::raw).collect();
        let mut items = Vec::new();
        let mut inline_segment = Vec::new();

        self.append_flex_grid_pseudo(elem_id, box_idx, true, &mut items, &mut inline_segment);
        self.build_flex_grid_items_from_nodes(&children, box_idx, &mut items, &mut inline_segment);
        self.append_flex_grid_pseudo(elem_id, box_idx, false, &mut items, &mut inline_segment);
        if let Some(anonymous) = self.flush_flex_inline_sources(&mut inline_segment, box_idx) {
            items.push(anonymous);
        }

        self.output.set_flex_grid_children(box_idx, items);
    }

    fn append_flex_grid_pseudo(&mut self, origin: DomNodeId, box_idx: u32, before: bool, items: &mut Vec<u32>, inline_segment: &mut Vec<FlexInlineSource>) {
        if self.generated.pseudo_display(origin, before) == Some(Display::Contents) {
            inline_segment.push(FlexInlineSource::GeneratedPseudo { origin, before });
            return;
        }
        let Some(pseudo) = self.build_generated_flex_grid_pseudo(origin, box_idx, before) else { return };
        if let Some(anonymous) = self.flush_flex_inline_sources(inline_segment, box_idx) {
            items.push(anonymous);
        }
        items.push(pseudo);
    }

    fn build_flex_grid_items_from_nodes(&mut self, children: &[u32], box_idx: u32, items: &mut Vec<u32>, inline_segment: &mut Vec<FlexInlineSource>) {
        for &raw in children {
            let Some(child_id) = self.document.node_id_from_raw(raw) else { continue };
            match self.document.node_ref(child_id) {
                Some(NodeRef::Text(_)) => inline_segment.push(FlexInlineSource::Node(raw)),
                Some(NodeRef::Element(child)) => {
                    let display = display_for_element(self.styles, child);
                    if display == Display::Contents {
                        if let Some(directives) = self.styles.counter_directives_for_node(child_id) {
                            self.generated.apply(directives);
                        }
                        if self.generated.pseudo_display(child_id, true) == Some(Display::Contents) {
                            inline_segment.push(FlexInlineSource::GeneratedPseudo { origin: child_id, before: true });
                        } else if let Some(pseudo) = self.build_generated_flex_grid_pseudo(child_id, box_idx, true) {
                            if let Some(anonymous) = self.flush_flex_inline_sources(inline_segment, box_idx) {
                                items.push(anonymous);
                            }
                            items.push(pseudo);
                        }
                        let grandchildren: Vec<u32> = child.children().map(DomNodeId::raw).collect();
                        self.build_flex_grid_items_from_nodes(&grandchildren, box_idx, items, inline_segment);
                        if self.generated.pseudo_display(child_id, false) == Some(Display::Contents) {
                            inline_segment.push(FlexInlineSource::GeneratedPseudo { origin: child_id, before: false });
                        } else if let Some(pseudo) = self.build_generated_flex_grid_pseudo(child_id, box_idx, false) {
                            if let Some(anonymous) = self.flush_flex_inline_sources(inline_segment, box_idx) {
                                items.push(anonymous);
                            }
                            items.push(pseudo);
                        }
                        continue;
                    }
                    if let Some(anonymous) = self.flush_flex_inline_sources(inline_segment, box_idx) {
                        items.push(anonymous);
                    }
                    if display != Display::None
                        && let Some(item) = self.build_box_for_node(child_id, Some(box_idx))
                    {
                        items.push(item);
                    }
                }
                None => {}
            }
        }
    }

    fn flush_flex_inline_sources(&mut self, sources: &mut Vec<FlexInlineSource>, parent_box: u32) -> Option<u32> {
        if sources.is_empty() {
            return None;
        }
        if sources.iter().all(|source| match source {
            FlexInlineSource::Node(raw) => self.document.node_id_from_raw(*raw).is_none_or(|node| !self.contents_text_styles.contains_key(&node)),
            FlexInlineSource::GeneratedPseudo { .. } => false,
        }) {
            let mut nodes = sources
                .drain(..)
                .map(|source| match source {
                    FlexInlineSource::Node(raw) => raw,
                    FlexInlineSource::GeneratedPseudo { .. } => unreachable!(),
                })
                .collect();
            return self.flush_inline_segment(&mut nodes, parent_box);
        }

        let whitespace_only = sources.iter().all(|source| match *source {
            FlexInlineSource::Node(raw) => {
                let collapsible = self.document.node_id_from_raw(raw).and_then(|node| self.contents_text_styles.get(&node).copied()).and_then(|style| self.styles.view(style)).map_or(true, |style| !style.white_space().preserves_spaces());
                collapsible && self.document.node_id_from_raw(raw).and_then(|node| self.document.text_ref(node)).is_some_and(|text| text.text().chars().all(char::is_whitespace))
            }
            FlexInlineSource::GeneratedPseudo { .. } => false,
        });
        if whitespace_only {
            sources.clear();
            return None;
        }

        let (anonymous, item_start) = self.output.push_anonymous_inline(parent_box);
        for source in sources.drain(..) {
            match source {
                FlexInlineSource::Node(raw) => {
                    if let Some(node) = self.document.node_id_from_raw(raw) {
                        self.process_inline_node(node, anonymous);
                    }
                }
                FlexInlineSource::GeneratedPseudo { origin, before } => self.append_generated_inline_pseudo(origin, anonymous, before),
            }
        }
        self.output.finish_anonymous_box(anonymous, item_start).then_some(anonymous)
    }

    fn build_generated_flex_grid_pseudo(&mut self, origin: DomNodeId, parent_box: u32, before: bool) -> Option<u32> {
        let display = self.generated.pseudo_display(origin, before)?;
        if matches!(display, Display::None | Display::TableColumn | Display::TableColumnGroup) {
            return None;
        }
        if is_block_level_pseudo_display(display) {
            return self.build_generated_block_pseudo(origin, parent_box, before);
        }
        let resolved = self.generated.resolve_pseudo(origin, before)?;
        let style = resolved.style;
        let pseudo_box = self.output.push_synthetic_box(parent_box, style, LayoutMode::Block(BlockBox { children: Children::Empty }));
        self.output.set_generated_text_children(pseudo_box, style, &resolved.text, false);
        Some(pseudo_box)
    }

    fn build_table_children(&mut self, elem_id: DomNodeId, table_box_idx: u32) {
        let dom = self.document;
        let Some(element) = dom.element_ref(elem_id) else {
            return;
        };
        let children: Vec<u32> = element.children().map(|idx| idx.raw()).collect();

        self.build_table_children_from_nodes(children, table_box_idx);
    }

    fn build_table_children_from_nodes(&mut self, children: Vec<u32>, table_box_idx: u32) {
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
                TableChildAction::Anonymous(nodes) => self.materialize_anonymous_table_children(nodes, table_box_idx, &mut body_rows),
                TableChildAction::Principal(TablePrincipal { source: node, role }) => {
                    let group_display = self.document.element_ref(node).map(|element| display_for_element(self.styles, element));
                    if let Some(box_idx) = self.build_box_for_node(node, Some(table_box_idx)) {
                        match role {
                            TableRole::Caption(html_style_model::CaptionSide::Bottom) => captions_bottom.push(box_idx),
                            TableRole::Caption(html_style_model::CaptionSide::Top) => captions_top.push(box_idx),
                            TableRole::Row => body_rows.push(box_idx),
                            TableRole::RowGroup => {
                                let (rows, groups) = match group_display {
                                    Some(html_style_model::Display::TableHeaderGroup) => (&mut header_rows, &mut header_groups),
                                    Some(html_style_model::Display::TableFooterGroup) => (&mut footer_rows, &mut footer_groups),
                                    _ => (&mut body_rows, &mut body_groups),
                                };
                                self.output.extend_with_group_rows(box_idx, rows);
                                groups.push(box_idx);
                            }
                            TableRole::Table | TableRole::Cell => unreachable!("direct table children cannot be table or cell principals"),
                        }
                    }
                }
            }
        }

        // CSS table layout has a visual group order independent of source
        // order: headers precede ordinary rows and footers follow them. Keep
        // construction in source order, but expose the normalized grid order
        // to every sizing, border, and painting stage.
        let rows = header_rows.into_iter().chain(body_rows).chain(footer_rows).collect();
        let row_groups = header_groups.into_iter().chain(body_groups).chain(footer_groups).collect();

        self.output.finish_table(table_box_idx, rows, row_groups, captions_top, captions_bottom, plan.columns, plan.column_groups, plan.column_width_hints);
    }

    /// CSS table fixup wraps otherwise-improper table children in one
    /// anonymous row and cell. Keeping those boxes in the private layout model
    /// preserves the parsed DOM while ensuring ordinary block/float content
    /// contributes to table intrinsic sizing instead of being dropped.
    fn materialize_anonymous_table_children(&mut self, nodes: Vec<u32>, table_box_idx: u32, rows: &mut Vec<u32>) {
        if nodes.iter().any(|&raw| self.document.node_id_from_raw(raw).and_then(|node| self.document.element_ref(node)).is_some_and(|element| display_for_element(self.styles, element) == Display::TableCell)) {
            let row_idx = self.output.push_anonymous_row(table_box_idx);
            let mut cells = Vec::new();
            let mut out_of_flow = Vec::new();
            for action in plan_table_row_children(self.document, self.styles, nodes) {
                match action {
                    TableRowChildAction::Anonymous(nodes) => self.materialize_anonymous_row_children(nodes, row_idx, &mut cells),
                    TableRowChildAction::Cell(node) => {
                        if let Some(cell) = self.build_box_for_node(node, Some(row_idx)) {
                            cells.push(cell);
                        }
                    }
                    TableRowChildAction::OutOfFlow(node) => {
                        if let Some(child) = self.build_box_for_node(node, Some(row_idx)) {
                            out_of_flow.push(child);
                        }
                    }
                }
            }
            self.output.set_row_children(row_idx, cells, out_of_flow);
            rows.push(row_idx);
            return;
        }

        let white_space = self.output.style_for_box(table_box_idx as usize).white_space();
        let plan = plan_anonymous_table_children(self.document, self.styles, &nodes, white_space);
        let (cell_nodes, parts) = match plan {
            AnonymousTablePlan::Empty => return,
            AnonymousTablePlan::Cells(cells) => (Some(cells), None),
            AnonymousTablePlan::Cell(parts) => (None, Some(parts)),
        };
        let row_idx = self.output.push_anonymous_row(table_box_idx);
        if let Some(cell_nodes) = cell_nodes {
            let cells = cell_nodes.into_iter().filter_map(|node| self.build_box_for_element(node, Some(row_idx))).collect();
            self.output.set_row_cells(row_idx, cells);
            rows.push(row_idx);
            return;
        }

        let cell_idx = self.output.push_anonymous_cell(row_idx, true);
        let parts = parts.expect("anonymous cell plan has content parts");
        self.materialize_anonymous_cell_parts(parts, cell_idx);

        self.output.append_row_cell(row_idx, cell_idx);
        rows.push(row_idx);
    }

    fn build_table_row_children(&mut self, elem_id: DomNodeId, row_box_idx: u32) {
        let Some(element) = self.document.element_ref(elem_id) else {
            return;
        };
        let children: Vec<u32> = element.children().map(|idx| idx.raw()).collect();
        let mut cells = Vec::new();
        let mut out_of_flow = Vec::new();
        for action in plan_table_row_children(self.document, self.styles, children) {
            match action {
                TableRowChildAction::Anonymous(nodes) => self.materialize_anonymous_row_children(nodes, row_box_idx, &mut cells),
                TableRowChildAction::Cell(node) => {
                    if let Some(cell) = self.build_box_for_node(node, Some(row_box_idx)) {
                        cells.push(cell);
                    }
                }
                TableRowChildAction::OutOfFlow(node) => {
                    if let Some(child) = self.build_box_for_node(node, Some(row_box_idx)) {
                        out_of_flow.push(child);
                    }
                }
            }
        }

        self.output.set_row_children(row_box_idx, cells, out_of_flow);
    }

    fn build_table_row_group_children(&mut self, elem_id: DomNodeId, group_box_idx: u32) {
        let Some(element) = self.document.element_ref(elem_id) else {
            return;
        };
        let children = element.children().collect::<Vec<_>>();
        let mut rows = Vec::new();
        let mut anonymous = Vec::new();
        for node in children {
            let display = self.document.element_ref(node).map(|element| display_for_element(self.styles, element));
            match display {
                Some(Display::TableRow) => {
                    self.materialize_anonymous_group_row(std::mem::take(&mut anonymous), group_box_idx, &mut rows);
                    if let Some(row) = self.build_box_for_node(node, Some(group_box_idx)) {
                        rows.push(row);
                    }
                }
                Some(Display::None) => {}
                _ => anonymous.push(node.raw()),
            }
        }
        self.materialize_anonymous_group_row(anonymous, group_box_idx, &mut rows);
        self.output.set_box_children(group_box_idx, Children::Blocks(rows));
    }

    fn materialize_anonymous_group_row(&mut self, nodes: Vec<u32>, group_box_idx: u32, rows: &mut Vec<u32>) {
        if nodes.is_empty() || nodes.iter().all(|&raw| self.document.node_id_from_raw(raw).and_then(|node| self.document.text_ref(node)).is_some_and(|text| text.text().chars().all(char::is_whitespace))) {
            return;
        }
        let row_idx = self.output.push_anonymous_row(group_box_idx);
        let mut cells = Vec::new();
        let mut out_of_flow = Vec::new();
        for action in plan_table_row_children(self.document, self.styles, nodes) {
            match action {
                TableRowChildAction::Anonymous(nodes) => self.materialize_anonymous_row_children(nodes, row_idx, &mut cells),
                TableRowChildAction::Cell(node) => {
                    if let Some(cell) = self.build_box_for_node(node, Some(row_idx)) {
                        cells.push(cell);
                    }
                }
                TableRowChildAction::OutOfFlow(node) => {
                    if let Some(child) = self.build_box_for_node(node, Some(row_idx)) {
                        out_of_flow.push(child);
                    }
                }
            }
        }
        self.output.set_row_children(row_idx, cells, out_of_flow);
        rows.push(row_idx);
    }

    fn materialize_anonymous_row_children(&mut self, nodes: Vec<u32>, row_box_idx: u32, cells: &mut Vec<u32>) {
        let white_space = self.output.style_for_box(row_box_idx as usize).white_space();
        let parts = match plan_anonymous_table_children(self.document, self.styles, &nodes, white_space) {
            AnonymousTablePlan::Empty => return,
            AnonymousTablePlan::Cells(cell_nodes) => {
                cells.extend(cell_nodes.into_iter().filter_map(|node| self.build_box_for_element(node, Some(row_box_idx))));
                return;
            }
            AnonymousTablePlan::Cell(parts) => parts,
        };
        let cell_idx = self.output.push_anonymous_cell(row_box_idx, true);
        self.materialize_anonymous_cell_parts(parts, cell_idx);
        cells.push(cell_idx);
    }

    fn materialize_anonymous_cell_parts(&mut self, parts: Vec<AnonymousCellPart>, cell_idx: u32) {
        let contains_block = parts.iter().any(|part| matches!(part, AnonymousCellPart::Block(_)));
        if contains_block {
            let mut direct_children = Vec::new();
            let mut table_segment = Vec::new();
            for part in parts {
                match part {
                    AnonymousCellPart::Inline(mut nodes) => {
                        if table_segment.is_empty() {
                            if let Some(anonymous) = self.flush_inline_segment(&mut nodes, cell_idx) {
                                direct_children.push(anonymous);
                            }
                        } else {
                            table_segment.append(&mut nodes);
                        }
                    }
                    AnonymousCellPart::Block(node) if self.document.element_ref(node).is_some_and(|element| is_table_internal_display(display_for_element(self.styles, element))) => {
                        table_segment.push(node.raw());
                    }
                    AnonymousCellPart::Block(node) => {
                        if let Some(table) = self.flush_anonymous_table_segment(&mut table_segment, cell_idx) {
                            direct_children.push(table);
                        }
                        if let Some(child) = self.build_box_for_node(node, Some(cell_idx)) {
                            direct_children.push(child);
                        }
                    }
                }
            }
            if let Some(table) = self.flush_anonymous_table_segment(&mut table_segment, cell_idx) {
                direct_children.push(table);
            }
            self.output.set_box_children(cell_idx, Children::Blocks(direct_children));
        } else {
            let start = self.output.item_position();
            for part in parts {
                if let AnonymousCellPart::Inline(nodes) = part {
                    self.process_inline_nodes(&nodes, cell_idx);
                }
            }
            let end = self.output.item_position();
            if start < end {
                self.output.set_box_children(cell_idx, Children::InlineItems(start..end));
            }
        }
    }

    fn begin_inline_fragment(&mut self, node: DomNodeId, parent: u32, style: Option<StyleIndices>, first_fragment: bool) -> InlineFragmentFrame {
        if first_fragment {
            if let Some(directives) = self.styles.counter_directives_for_node(node) {
                self.generated.apply(directives);
            }
            self.generated.enter_sibling_scope();
        }
        let item_start = self.output.item_position();
        let box_idx = self.push_layout_box(node, Some(parent), style, LayoutMode::Inline(Range::default()));
        InlineFragmentFrame { node, box_idx, item_start, first_fragment }
    }

    fn finish_inline_fragment(&mut self, frame: InlineFragmentFrame, last_fragment: bool) {
        self.output.finish_inline_box(frame.box_idx, frame.item_start);
        if last_fragment {
            self.generated.leave_sibling_scope();
        }
    }

    fn process_inline_node(&mut self, node_id: DomNodeId, box_idx: u32) {
        match plan_inline_node(self.document, self.styles, node_id) {
            InlineNodePlan::Ignore => {}
            InlineNodePlan::Text(node) => self.process_text_node_with_contents_style(node, box_idx),
            InlineNodePlan::Break { clear } => self.output.push_item(InlineItemKind::Break { clear }, box_idx, None),
            InlineNodePlan::GeneratedBreak { node, style } => {
                let frame = self.begin_inline_fragment(node, box_idx, style, true);
                self.append_generated_inline_pseudo(node, frame.box_idx, true);
                self.append_generated_inline_pseudo(node, frame.box_idx, false);
                self.finish_inline_fragment(frame, true);
            }
            InlineNodePlan::OutOfFlow(node) => {
                if let Some(element_box) = self.build_box_for_element(node, Some(box_idx)) {
                    let kind = self
                        .styles
                        .style_for_node(node)
                        .and_then(|style| self.styles.layout_style(style))
                        .filter(|style| style.position == html_style_model::PositionMode::Absolute)
                        .map_or(InlineItemKind::FloatAnchor { box_idx: element_box }, |_| InlineItemKind::AbsoluteAnchor { box_idx: element_box });
                    self.output.push_item(kind, box_idx, None);
                }
            }
            InlineNodePlan::Atomic(node) => {
                if let Some(element_box) = self.build_box_for_element(node, Some(box_idx)) {
                    self.output.push_item(InlineItemKind::AtomicBox { box_idx: element_box }, element_box, None);
                }
            }
            InlineNodePlan::Contents { node, children } => {
                if let Some(directives) = self.styles.counter_directives_for_node(node) {
                    self.generated.apply(directives);
                }
                self.append_generated_inline_pseudo(node, box_idx, true);
                self.process_inline_nodes(&children, box_idx);
                self.append_generated_inline_pseudo(node, box_idx, false);
            }
            InlineNodePlan::Element(InlineElementPlan { fragment: InlineFragmentPlan { node, style, start_edge, end_edge }, image, children }) => {
                let frame = self.begin_inline_fragment(node, box_idx, style, true);
                if let Some(image_idx) = image {
                    self.output.push_item(InlineItemKind::Image { image_idx }, frame.box_idx, None);
                } else {
                    self.append_generated_inline_pseudo(node, frame.box_idx, true);
                    if children.is_empty() {
                        self.output.push_item(InlineItemKind::InlineBoundary { inline_start: start_edge, inline_end: end_edge }, frame.box_idx, None);
                    } else {
                        if start_edge {
                            self.output.push_item(InlineItemKind::InlineBoundary { inline_start: true, inline_end: false }, frame.box_idx, None);
                        }
                        self.process_inline_nodes(&children, frame.box_idx);
                        self.append_generated_inline_pseudo(node, frame.box_idx, false);
                        if end_edge {
                            self.output.push_item(InlineItemKind::InlineBoundary { inline_start: false, inline_end: true }, frame.box_idx, None);
                        }
                    }
                    if children.is_empty() {
                        self.append_generated_inline_pseudo(node, frame.box_idx, false);
                    }
                }
                self.finish_inline_fragment(frame, true);
            }
        }
    }

    fn process_inline_nodes(&mut self, nodes: &[u32], box_idx: u32) {
        let mut table_segment = Vec::new();
        let mut pending_table_whitespace = Vec::new();
        for &raw in nodes {
            let Some(node_id) = self.document.node_id_from_raw(raw) else { continue };
            let table_internal = self.document.element_ref(node_id).is_some_and(|element| is_table_internal_display(display_for_element(self.styles, element)));
            if table_internal {
                table_segment.append(&mut pending_table_whitespace);
                table_segment.push(raw);
                continue;
            }
            if !table_segment.is_empty() && matches!(self.document.node_ref(node_id), Some(NodeRef::Text(text)) if text.text().chars().all(char::is_whitespace)) {
                pending_table_whitespace.push(raw);
                continue;
            }
            self.flush_inline_anonymous_table(&mut table_segment, box_idx);
            for whitespace in pending_table_whitespace.drain(..) {
                if let Some(node) = self.document.node_id_from_raw(whitespace) {
                    self.process_inline_node(node, box_idx);
                }
            }
            self.process_inline_node(node_id, box_idx);
        }
        self.flush_inline_anonymous_table(&mut table_segment, box_idx);
        for whitespace in pending_table_whitespace {
            if let Some(node) = self.document.node_id_from_raw(whitespace) {
                self.process_inline_node(node, box_idx);
            }
        }
    }

    fn flush_inline_anonymous_table(&mut self, nodes: &mut Vec<u32>, parent_box_idx: u32) {
        let Some(table_idx) = self.flush_anonymous_table_segment(nodes, parent_box_idx) else { return };
        self.output.push_item(InlineItemKind::AtomicBox { box_idx: table_idx }, table_idx, None);
    }

    fn process_text_node(&mut self, node_id: DomNodeId, box_idx: u32) {
        let Some(text_node) = self.document.text_ref(node_id) else {
            return;
        };
        let text = text_node.text();

        if text.is_empty() {
            return;
        }

        if self.floated_first_letter.is_none() {
            self.output.push_source_text(text, box_idx, node_id.raw());
            return;
        }

        let mut fragment_start = 0;
        let mut fragment_target = None;
        let mut source_utf16_offset = 0u32;
        for (byte_offset, character) in text.char_indices() {
            let target = self.first_letter_target_box(character, box_idx);
            if fragment_target.is_none() && target == box_idx && self.floated_first_letter.as_ref().is_some_and(|capture| capture.phase == FirstLetterCapturePhase::Done && capture.item_end.is_none()) {
                let end = self.output.item_position();
                self.floated_first_letter.as_mut().expect("active first-letter capture").item_end = Some(end);
            }
            if fragment_target.is_some() && fragment_target != Some(target) {
                let previous = fragment_target.expect("text fragment target");
                let fragment = &text[fragment_start..byte_offset];
                self.output.push_source_text_fragment(fragment, previous, node_id.raw(), source_utf16_offset);
                if self.floated_first_letter.as_ref().is_some_and(|capture| capture.box_idx == previous && capture.item_end.is_none()) {
                    let end = self.output.item_position();
                    self.floated_first_letter.as_mut().expect("active first-letter capture").item_end = Some(end);
                }
                source_utf16_offset += fragment.encode_utf16().count() as u32;
                fragment_start = byte_offset;
            }
            fragment_target = Some(target);
        }
        if let Some(target) = fragment_target {
            self.output.push_source_text_fragment(&text[fragment_start..], target, node_id.raw(), source_utf16_offset);
        }
    }

    fn process_text_node_with_contents_style(&mut self, node_id: DomNodeId, box_idx: u32) {
        let Some(&style) = self.contents_text_styles.get(&node_id) else {
            self.process_text_node(node_id, box_idx);
            return;
        };
        let (carrier, item_start) = self.output.push_contents_style_box(box_idx, style);
        self.process_text_node(node_id, carrier);
        self.output.finish_inline_box(carrier, item_start);
    }

    fn begin_floated_first_letter(&mut self, origin: DomNodeId, parent_box: u32) -> bool {
        if self.floated_first_letter.is_some() {
            return false;
        }
        let Some(style) = self.styles.first_letter_style_for_node(origin) else { return false };
        let Some(box_style) = self.styles.box_model_style(style) else { return false };
        if !matches!(box_style.float, Float::Left | Float::Right) {
            return false;
        }

        let pseudo_box = self.output.push_synthetic_box(parent_box, style, LayoutMode::Block(BlockBox { children: Children::Empty }));
        self.output.push_item(InlineItemKind::FloatAnchor { box_idx: pseudo_box }, parent_box, None);
        self.floated_first_letter = Some(FloatedFirstLetterCapture { box_idx: pseudo_box, item_start: self.output.item_position(), item_end: None, phase: FirstLetterCapturePhase::Leading });
        true
    }

    fn first_letter_target_box(&mut self, character: char, ordinary_box: u32) -> u32 {
        let Some(capture) = self.floated_first_letter.as_mut() else { return ordinary_box };
        let selected = match capture.phase {
            FirstLetterCapturePhase::Leading if character.is_whitespace() => true,
            FirstLetterCapturePhase::Leading if is_first_letter_punctuation(character) => true,
            FirstLetterCapturePhase::Leading => {
                capture.phase = FirstLetterCapturePhase::Letter;
                true
            }
            FirstLetterCapturePhase::Letter if character.is_mark() => true,
            FirstLetterCapturePhase::Letter if is_first_letter_punctuation(character) => {
                capture.phase = FirstLetterCapturePhase::Trailing;
                true
            }
            FirstLetterCapturePhase::Letter => false,
            FirstLetterCapturePhase::Trailing if character.is_mark() || is_first_letter_punctuation(character) => true,
            FirstLetterCapturePhase::Trailing => false,
            FirstLetterCapturePhase::Done => false,
        };
        if selected {
            capture.box_idx
        } else {
            capture.phase = FirstLetterCapturePhase::Done;
            ordinary_box
        }
    }

    fn finish_floated_first_letter(&mut self) {
        let Some(capture) = self.floated_first_letter.take() else { return };
        let item_end = capture.item_end.unwrap_or_else(|| self.output.item_position());
        if capture.item_start < item_end {
            self.output.set_box_children(capture.box_idx, Children::InlineItems(capture.item_start..item_end));
        }
    }

    fn append_generated_inline_pseudo(&mut self, origin: DomNodeId, parent_box: u32, before: bool) {
        let Some(display) = self.generated.pseudo_display(origin, before) else { return };
        if matches!(display, Display::None | Display::TableColumn | Display::TableColumnGroup) {
            return;
        }
        let resolved = self.generated.resolve_pseudo(origin, before).expect("pseudo existence was checked above");
        self.output.emit_generated_inline(parent_box, plan_generated_inline(self.styles, resolved));
    }

    /// Build a generated principal box that participates in block flow.  A
    /// pseudo-element is an actual CSS box, not merely a text run: its own
    /// margins, borders, sizing, display, and list marker all belong to this
    /// synthetic box just inside the originating element.
    fn build_generated_block_pseudo(&mut self, origin: DomNodeId, parent_box: u32, before: bool) -> Option<u32> {
        if !self.generated.pseudo_is_block_level(origin, before) {
            return None;
        }
        let resolved = self.generated.resolve_pseudo(origin, before)?;
        match resolved.display {
            Display::Block | Display::FlowRoot | Display::ListItem | Display::FlowRootListItem => {
                let style = resolved.style;
                let pseudo_box = self.output.push_synthetic_box(parent_box, style, LayoutMode::Block(BlockBox { children: Children::Empty }));
                self.output.set_generated_text_children(pseudo_box, style, &resolved.text, false);
                if matches!(resolved.display, Display::ListItem | Display::FlowRootListItem) {
                    self.build_list_marker_for_ordinal(pseudo_box, style, 1);
                }
                Some(pseudo_box)
            }
            _ => self.output.materialize_generated_table(parent_box, plan_generated_table_principal(self.styles, resolved)?),
        }
    }

    fn flush_inline_segment(&mut self, nodes: &mut Vec<u32>, parent_box_idx: u32) -> Option<u32> {
        self.flush_inline_segment_with_generated_edges(nodes, parent_box_idx, None, None)
    }

    fn flush_inline_segment_with_generated_edges(&mut self, nodes: &mut Vec<u32>, parent_box_idx: u32, before: Option<DomNodeId>, after: Option<DomNodeId>) -> Option<u32> {
        if nodes.is_empty() && before.is_none() && after.is_none() {
            return None;
        }

        let white_space = self.output.style_for_box(parent_box_idx as usize).white_space();
        let segment = trim_inline_segment_nodes(self.document, self.styles, nodes.as_slice(), white_space);
        if segment.is_empty() && before.is_none() && after.is_none() {
            nodes.clear();
            return None;
        }

        let (anonymous_idx, item_start) = self.output.push_anonymous_inline(parent_box_idx);
        if let Some(origin) = before {
            self.append_generated_inline_pseudo(origin, anonymous_idx, true);
        }
        for &node_idx in segment {
            let Some(node_id) = self.document.node_id_from_raw(node_idx) else {
                continue;
            };
            self.process_inline_node(node_id, anonymous_idx);
        }
        if let Some(origin) = after {
            self.append_generated_inline_pseudo(origin, anonymous_idx, false);
        }
        nodes.clear();
        self.output.finish_anonymous_box(anonymous_idx, item_start);
        Some(anonymous_idx)
    }

    fn push_layout_box(&mut self, dom_node_id: DomNodeId, parent: Option<u32>, style: Option<StyleIndices>, layout_mode: LayoutMode) -> u32 {
        let element = self.document.element_ref(dom_node_id).expect("layout boxes are only created for element nodes");
        let is_body = element.tag().eq_ignore_ascii_case("body");
        self.output.push_dom_box(dom_node_id.raw(), parent, style, layout_mode, is_body)
    }
}

fn collect_contents_text_styles(document: &Document, styles: &ComputedStyles, node: DomNodeId, inherited_from_contents: Option<StyleIndices>, output: &mut FxHashMap<DomNodeId, StyleIndices>) {
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
            let inherited = if display == Display::Contents { styles.style_for_node(node).or(inherited_from_contents) } else { None };
            for child in element.children() {
                collect_contents_text_styles(document, styles, child, inherited, output);
            }
        }
        None => {}
    }
}

pub(crate) fn build_layout_inputs(document: &Document, styles: &ComputedStyles, layout_tree: &mut LayoutTree, inline_content: &mut InlineContent) {
    LayoutTreeBuilder::new(document, styles, layout_tree, inline_content).build();
}

include!("tests.rs");
