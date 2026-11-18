use crate::GroupedArena;
use html_dom::{Document, DomNodeId, ElementRef, MemoryUsageReport};
use html_style_model::{
    ComputedStyles, Display, Float, ListStylePosition, OverflowMode, PositionMode, StyleIndices,
};
use rustc_data_structures::fx::FxHashMap;
use std::iter::{once, successors};
use std::num::NonZeroU32;
use std::ops::Range;

/// A list item's generated marker (`::marker`). `marker_box` is a real layout
/// box holding the marker text run; it is laid out and positioned by the engine
/// relative to the list item's first line rather than flowing as a child, so it
/// is kept out of the item's `Children`.
#[derive(Clone, Copy)]
pub struct ListItemMarker {
    pub marker_box: u32,
    pub position: ListStylePosition,
}

/// Output of the box-construction phase: the box tree plus the inline text
/// content it references (glyph codepoints + inline items). Produced by
/// `build_layout_inputs`, consumed read-only by shaping and layout.
#[derive(Default, Clone)]
pub struct LayoutTree {
    pub boxes: Vec<LayoutBox>,
    /// Ordered direct-child IDs. Content ranges are views into this same arena.
    child_indices: Vec<u32>,
    /// Cold table payloads, addressed by compact handles in layout boxes.
    tables: Vec<TableBox>,
    pub root_box: Option<u32>,
    pub body_box: Option<u32>,
    pub has_floats: bool,
    pub tracks_overflow_clips: bool,
    /// Generated markers keyed by their list-item box index.
    pub list_markers: FxHashMap<u32, ListItemMarker>,
    /// Decoded images keyed by their principal layout box index.
    pub image_indices: FxHashMap<u32, u32>,
    /// Inline-axis endpoints for block-in-inline fragments. Ordinary inline
    /// boxes implicitly own both endpoints.
    pub inline_fragment_edges: FxHashMap<u32, (bool, bool)>,
    /// Relatively positioned inline ancestors that were split away from a
    /// block descendant by CSS block-in-inline box generation.
    pub split_inline_position_ancestors: FxHashMap<u32, Box<[StyleIndices]>>,
}

impl LayoutTree {
    /// Style-bearing formatting structures without their own layout box.
    /// Shaping must prepare these handles before layout or paint reads them.
    pub fn non_box_styles(&self) -> impl Iterator<Item = StyleIndices> + '_ {
        self.tables
            .iter()
            .flat_map(|table| {
                table
                    .columns
                    .iter()
                    .flat_map(|column| once(column.width_style).chain(column.fallback_style))
                    .chain(table.column_groups.iter().map(|group| group.style))
            })
            .chain(
                self.split_inline_position_ancestors
                    .values()
                    .flat_map(|styles| styles.iter().copied()),
            )
    }

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_vec_storage("LayoutTree.boxes.storage", &self.boxes);
        report.add_vec_storage("LayoutTree.child_indices.storage", &self.child_indices);
        report.add_vec_storage("LayoutTree.tables.storage", &self.tables);
        for table in &self.tables {
            report.extend_prefixed("LayoutTree.tables", table.memory_usage_report());
        }
        for layout_box in &self.boxes {
            report.extend_prefixed("LayoutTree.boxes", layout_box.memory_usage_report());
        }
        report.add_hash_map_storage("LayoutTree.list_markers.storage", &self.list_markers);
        report.add_hash_map_storage("LayoutTree.image_indices.storage", &self.image_indices);
        report.add_hash_map_storage(
            "LayoutTree.inline_fragment_edges.storage",
            &self.inline_fragment_edges,
        );
        report.add_hash_map_storage(
            "LayoutTree.split_inline_position_ancestors.storage",
            &self.split_inline_position_ancestors,
        );
        for ancestors in self.split_inline_position_ancestors.values() {
            report.add_slice(
                "LayoutTree.split_inline_position_ancestors.entries",
                ancestors,
            );
        }
        report
    }
}

impl LayoutTree {
    pub fn box_count(&self) -> usize {
        self.boxes.len()
    }

    pub fn box_at(&self, box_idx: usize) -> Option<&LayoutBox> {
        self.boxes.get(box_idx)
    }

    pub fn box_at_mut(&mut self, box_idx: usize) -> Option<&mut LayoutBox> {
        self.boxes.get_mut(box_idx)
    }

    pub fn root_box(&self) -> Option<usize> {
        self.root_box.map(|idx| idx as usize)
    }

    pub fn set_root_box(&mut self, root_box: Option<u32>) {
        self.root_box = root_box;
    }

    pub fn body_box(&self) -> Option<usize> {
        self.body_box.map(|idx| idx as usize)
    }

    pub fn set_body_box(&mut self, body_box: u32) {
        self.body_box = Some(body_box);
    }

    pub fn get_box_style_indices(&self, box_idx: usize) -> Option<StyleIndices> {
        self.boxes.get(box_idx)?.style()
    }

    pub fn get_box_dom_element(&self, box_idx: usize) -> Option<DomNodeId> {
        self.boxes.get(box_idx)?.dom_element()
    }

    /// Resolve the source element of a formatting box, if it has one.
    pub fn element_for_box<'a>(
        &self,
        box_idx: usize,
        document: &'a Document,
    ) -> Option<ElementRef<'a>> {
        self.box_at(box_idx)?.get_element(document)
    }

    pub fn get_box_image_idx(&self, box_idx: usize) -> Option<u32> {
        self.image_indices.get(&(box_idx as u32)).copied()
    }

    pub fn get_box_parent(&self, box_idx: usize) -> Option<usize> {
        self.boxes.get(box_idx)?.parent().map(|p| p as usize)
    }

    /// All direct formatting children, including generated markers.
    pub fn children(&self, box_idx: usize) -> &[u32] {
        self.child_slice(&self.boxes[box_idx].children)
    }

    /// A layout-role view into the same child arena (for example block content).
    pub fn child_slice(&self, range: &Range<u32>) -> &[u32] {
        &self.child_indices[range.start as usize..range.end as usize]
    }

    /// Record construction-defined content order before publishing topology.
    pub fn store_children(&mut self, children: impl IntoIterator<Item = u32>) -> Range<u32> {
        let start =
            u32::try_from(self.child_indices.len()).expect("child arena exceeds u32 capacity");
        self.child_indices.extend(children);
        start..u32::try_from(self.child_indices.len()).expect("child arena exceeds u32 capacity")
    }

    /// Publish one entry per parent-child relationship after construction.
    /// Explicit content order comes first; remaining structural children follow
    /// in construction order. Content views exclude those additional children.
    pub fn publish_children(&mut self) {
        let mut children = GroupedArena::default();
        children.reset_groups(self.boxes.len());
        let mut included = vec![false; self.boxes.len()];
        for (owner, layout_box) in self.boxes.iter_mut().enumerate() {
            if let Some(content) = layout_box.layout_mode.content_children_mut() {
                for &child in &self.child_indices[content.start as usize..content.end as usize] {
                    assert!(!included[child as usize], "duplicate content child");
                    included[child as usize] = true;
                    children.push_to(owner, child);
                }
            }
        }
        for (child, layout_box) in self.boxes.iter().enumerate() {
            if let Some(parent) = layout_box.parent
                && !included[child]
            {
                children.push_to(parent as usize, child as u32);
            }
        }
        let (indices, ranges) = children.into_parts();
        self.child_indices = indices;
        for (layout_box, range) in self.boxes.iter_mut().zip(ranges) {
            if let Some(content) = layout_box.layout_mode.content_children_mut() {
                *content = range.start..range.start + (content.end - content.start);
            }
            layout_box.children = range;
        }
        debug_assert!(self.boxes.iter().enumerate().all(|(owner, _)| {
            self.children(owner)
                .iter()
                .all(|&child| self.boxes[child as usize].parent == Some(owner as u32))
        }));
    }

    /// Formatting ancestors, starting with the immediate parent and ending at
    /// the root. Anonymous and generated boxes follow their formatting parents.
    pub fn ancestors(&self, box_idx: usize) -> impl Iterator<Item = usize> + Clone + '_ {
        successors(self.get_box_parent(box_idx), |&parent| {
            self.get_box_parent(parent)
        })
    }

    pub fn get_box_layout_mode(&self, box_idx: usize) -> Option<&LayoutMode> {
        self.boxes
            .get(box_idx)
            .map(|layout_box| layout_box.layout_mode())
    }

    pub fn table(&self, id: TableId) -> Option<&TableBox> {
        self.tables.get(id.0?.get() as usize - 1)
    }

    pub fn get_box_table(&self, box_idx: usize) -> Option<&TableBox> {
        let LayoutMode::Table(id) = self.get_box_layout_mode(box_idx)? else {
            return None;
        };
        self.table(*id)
    }

    pub fn get_box_table_mut(&mut self, box_idx: usize) -> Option<&mut TableBox> {
        let LayoutMode::Table(id) = self.get_box_layout_mode(box_idx)? else {
            return None;
        };
        let index = id.0?.get() as usize - 1;
        self.tables.get_mut(index)
    }

    pub fn push_box(&mut self, mut layout_box: LayoutBox, styles: &ComputedStyles) -> u32 {
        if let LayoutMode::Table(id) = &mut layout_box.layout_mode {
            // A default handle requests an empty table. Existing handles remain
            // valid when a box is moved within this tree.
            if id.0.is_none() {
                let index = u32::try_from(self.tables.len())
                    .ok()
                    .and_then(|index| index.checked_add(1))
                    .and_then(NonZeroU32::new)
                    .expect("layout table pool exceeds u32 capacity");
                self.tables.push(TableBox::default());
                *id = TableId(Some(index));
            }
        }
        let idx = self.boxes.len() as u32;
        let style = styles
            .view(layout_box.style.unwrap_or_else(|| styles.default_indices()))
            .expect("validated style handle");
        if layout_box.dom_element.is_none() {
            layout_box.facts.display = style.display();
        }
        layout_box.facts.float = style.float();
        layout_box.facts.absolute = style.layout.position == PositionMode::Absolute;
        layout_box.facts.flex_grid_item = layout_box.parent.is_some_and(|parent| {
            matches!(
                self.boxes[parent as usize].layout_mode,
                LayoutMode::Flex(_) | LayoutMode::Grid(_)
            )
        });
        layout_box.facts.overflow = (style.overflow_x(), style.overflow_y());
        // Body overflow propagated to the viewport does not clip the body or
        // establish its formatting context. The root is inserted before it.
        if self.body_box == Some(idx)
            && self.root_box.is_some_and(|root| {
                self.boxes[root as usize].facts.overflow
                    == (OverflowMode::Visible, OverflowMode::Visible)
            })
        {
            layout_box.facts.overflow = (OverflowMode::Visible, OverflowMode::Visible);
        }
        layout_box.facts.atomic_inline = layout_box.facts.display.is_atomic_inline();
        layout_box.facts.formatting_context = layout_box.parent.is_none()
            || layout_box.facts.absolute
            || layout_box.facts.float != Float::None
            || layout_box.facts.flex_grid_item
            || matches!(layout_box.layout_mode, LayoutMode::TableCell(_))
            || layout_box
                .facts
                .display
                .establishes_formatting_context(layout_box.facts.overflow);
        layout_box.facts.flow_root = if matches!(layout_box.layout_mode, LayoutMode::Inline(_)) {
            layout_box
                .parent
                .map_or(idx, |parent| self.boxes[parent as usize].facts.flow_root)
        } else {
            idx
        };
        self.has_floats |= !layout_box.facts.absolute && layout_box.facts.float != Float::None;
        self.tracks_overflow_clips |=
            layout_box.facts.overflow.0.clips() || layout_box.facts.overflow.1.clips();
        if layout_box.parent.is_none() {
            self.root_box = Some(idx);
        }
        self.boxes.push(layout_box);
        idx
    }

    pub fn set_list_marker(&mut self, list_item_box: u32, marker: ListItemMarker) {
        self.list_markers.insert(list_item_box, marker);
    }

    pub fn set_box_image_idx(&mut self, box_idx: u32, image_idx: u32) {
        self.image_indices.insert(box_idx, image_idx);
    }

    pub fn list_marker(&self, list_item_box: usize) -> Option<ListItemMarker> {
        self.list_markers.get(&(list_item_box as u32)).copied()
    }

    pub fn set_inline_fragment_edges(
        &mut self,
        box_idx: u32,
        inline_start: bool,
        inline_end: bool,
    ) {
        self.inline_fragment_edges
            .insert(box_idx, (inline_start, inline_end));
    }

    pub fn inline_fragment_edges(&self, box_idx: usize) -> (bool, bool) {
        self.inline_fragment_edges
            .get(&(box_idx as u32))
            .copied()
            .unwrap_or((true, true))
    }

    pub fn set_split_inline_position_ancestors(
        &mut self,
        box_idx: u32,
        ancestors: Vec<StyleIndices>,
    ) {
        if !ancestors.is_empty() {
            self.split_inline_position_ancestors
                .insert(box_idx, ancestors.into_boxed_slice());
        }
    }

    pub fn split_inline_position_ancestors(&self, box_idx: usize) -> &[StyleIndices] {
        self.split_inline_position_ancestors
            .get(&(box_idx as u32))
            .map(Box::as_ref)
            .unwrap_or_default()
    }
}

/// Stable box-construction facts shared by shaping, layout, and paint.
#[derive(Clone, Copy, Default)]
pub struct BoxFacts {
    pub display: Display,
    pub float: Float,
    pub absolute: bool,
    pub formatting_context: bool,
    pub flex_grid_item: bool,
    pub atomic_inline: bool,
    pub overflow: (OverflowMode, OverflowMode),
    pub flow_root: u32,
}

#[derive(Clone)]
pub struct LayoutBox {
    pub facts: BoxFacts,
    pub layout_mode: LayoutMode,

    // DOM node backing this box (None for anonymous boxes). Element identity
    // (tag/id/classes/href) is read through this rather than duplicated here.
    pub dom_element: Option<DomNodeId>,

    // Tree structure (layout tree)
    pub parent: Option<u32>,  // index of parent box
    pub children: Range<u32>, // structural direct children in the shared arena

    // Style indices into split style vecs
    pub style: Option<StyleIndices>,
}

impl LayoutBox {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.extend_prefixed(
            "LayoutBox.layout_mode",
            self.layout_mode.memory_usage_report(),
        );
        report
    }
}

// Type alias retained for readability at layout-mode match sites.
pub type BoxType = LayoutMode;

impl LayoutBox {
    pub fn new_anonymous_box(range: Range<u32>, parent: Option<u32>) -> Self {
        Self {
            facts: BoxFacts::default(),
            layout_mode: LayoutMode::Anonymous(range),
            dom_element: None, // anonymous boxes have no DOM element
            parent,
            children: 0..0,
            style: None,
        }
    }

    /// Get element data from DOM tree
    pub fn get_element<'a>(&self, doc: &'a Document) -> Option<ElementRef<'a>> {
        self.dom_element.map(|node_id| doc.element_ref(node_id))
    }

    pub fn layout_mode(&self) -> &LayoutMode {
        &self.layout_mode
    }

    pub fn layout_mode_mut(&mut self) -> &mut LayoutMode {
        &mut self.layout_mode
    }

    pub fn style(&self) -> Option<StyleIndices> {
        self.style
    }

    pub fn set_style(&mut self, style: Option<StyleIndices>) {
        self.style = style;
    }

    pub fn dom_element(&self) -> Option<DomNodeId> {
        self.dom_element
    }

    pub fn parent(&self) -> Option<u32> {
        self.parent
    }
}

/// Tree-local table handle. The default requests allocation in `push_box`.
#[derive(Clone, Copy, Default)]
#[repr(transparent)]
pub struct TableId(Option<NonZeroU32>);

#[derive(Clone)]
pub enum LayoutMode {
    Block(Children),
    Table(TableId),
    TableRow(TableRowBox),
    TableCell(TableCellBox),
    Flex(Range<u32>),
    Grid(Range<u32>),
    Inline(Range<u32>), // range of inline items
    Anonymous(Range<u32>), // range of inline items (no element)
                        // Future layout modes (not yet implemented)
                        // InlineBlock(Children),
                        // Replaced(ReplacedBox),
}

impl LayoutMode {
    fn content_children_mut(&mut self) -> Option<&mut Range<u32>> {
        match self {
            Self::Block(Children::Blocks(range))
            | Self::TableCell(TableCellBox {
                children: Children::Blocks(range),
                ..
            })
            | Self::Flex(range)
            | Self::Grid(range) => Some(range),
            _ => None,
        }
    }

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        // Other modes contain only ranges or handles into LayoutTree storage.
        if let Self::TableRow(row) = self {
            report.extend_prefixed("LayoutMode::TableRow", row.memory_usage_report());
        }
        report
    }
}

#[derive(Clone, Default)]
pub struct TableBox {
    pub rows: Vec<u32>,
    pub row_groups: Vec<u32>,
    pub captions_top: Vec<u32>,
    pub captions_bottom: Vec<u32>,
    pub columns: Vec<TableColumnTrack>,
    pub column_groups: Vec<TableColumnGroupSpan>,
}

#[derive(Clone)]
pub struct TableColumnTrack {
    // Style used for column width resolution (explicit width / min / max hints).
    pub width_style: StyleIndices,
    // Group width fallback when the column width is auto.
    pub fallback_style: Option<StyleIndices>,
    // Explicit columns paint their own style; group-only tracks do not.
    pub paints_background: bool,
}

#[derive(Clone)]
pub struct TableColumnGroupSpan {
    pub style: StyleIndices,
    pub start: usize,
    pub span: usize,
}

impl TableBox {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_vec_storage("TableBox.rows.storage", &self.rows);
        report.add_vec_storage("TableBox.row_groups.storage", &self.row_groups);
        report.add_vec_storage("TableBox.captions_top.storage", &self.captions_top);
        report.add_vec_storage("TableBox.captions_bottom.storage", &self.captions_bottom);
        report.add_vec_storage("TableBox.columns.storage", &self.columns);
        report.add_vec_storage("TableBox.column_groups.storage", &self.column_groups);
        report
    }
}

#[derive(Clone, Default)]
pub struct TableRowBox {
    pub cells: Vec<u32>,
    /// Positioned descendants that are not table cells still belong to the
    /// row's static-position context and must not be discarded by table fixup.
    pub out_of_flow: Vec<u32>,
}

impl TableRowBox {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_vec_storage("TableRowBox.cells.storage", &self.cells);
        report.add_vec_storage("TableRowBox.out_of_flow.storage", &self.out_of_flow);
        report
    }
}

#[derive(Clone)]
pub struct TableCellBox {
    pub children: Children,
    pub colspan: usize,
    pub rowspan: usize,
}

impl TableCellBox {
    pub fn new(children: Children) -> Self {
        Self {
            children,
            colspan: 1,
            rowspan: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Children {
    Blocks(Range<u32>),
    InlineItems(Range<u32>),
    #[default]
    Empty,
}

impl html_text::TextStructure for LayoutTree {
    fn flow_root(&self, context: usize) -> usize {
        self.box_at(context)
            .map_or(context, |layout_box| layout_box.facts.flow_root as usize)
    }

    fn get_box_parent(&self, context: usize) -> Option<usize> {
        self.get_box_parent(context)
    }

    fn get_box_style_indices(&self, context: usize) -> Option<StyleIndices> {
        self.get_box_style_indices(context)
    }

    fn get_box_dom_element(&self, context: usize) -> Option<DomNodeId> {
        self.get_box_dom_element(context)
    }

    fn is_inline(&self, context: usize) -> bool {
        matches!(
            self.get_box_layout_mode(context),
            Some(LayoutMode::Inline(_))
        )
    }

    fn is_flex_or_grid(&self, context: usize) -> bool {
        matches!(
            self.get_box_layout_mode(context),
            Some(LayoutMode::Flex(_) | LayoutMode::Grid(_))
        )
    }
}
