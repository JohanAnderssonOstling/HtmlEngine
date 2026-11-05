use html_dom::{Document, ElementRef, MemoryUsageReport};
use html_style_model::{ListStylePosition, StyleIndices};
use std::iter::{once, successors};
use rustc_data_structures::fx::FxHashMap;
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
    pub root_box: Option<u32>,
    pub body_box: Option<u32>,
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
        self.boxes
            .iter()
            .filter_map(|layout_box| match &layout_box.layout_mode {
                LayoutMode::Table(table) => Some(table),
                _ => None,
            })
            .flat_map(|table| {
                table
                    .columns
                    .iter()
                    .flat_map(|column| once(column.width_style).chain(column.background_style))
                    .chain(table.column_groups.iter().map(|group| group.style))
                    .chain(
                        table
                            .column_width_hints
                            .iter()
                            .flat_map(|hint| once(hint.style).chain(hint.fallback_style)),
                    )
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

    pub fn get_box_dom_element(&self, box_idx: usize) -> Option<u32> {
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

    pub fn push_box(&mut self, layout_box: LayoutBox) -> u32 {
        let idx = self.boxes.len() as u32;
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

#[derive(Clone)]
pub struct LayoutBox {
    pub layout_mode: LayoutMode,

    // DOM node backing this box (None for anonymous boxes). Element identity
    // (tag/id/classes/href) is read through this rather than duplicated here.
    pub dom_element: Option<u32>,

    // Tree structure (layout tree)
    pub parent: Option<u32>, // index of parent box

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
            layout_mode: LayoutMode::Anonymous(range),
            dom_element: None, // anonymous boxes have no DOM element
            parent,
            style: None,
        }
    }

    /// Get element data from DOM tree
    pub fn get_element<'a>(&self, doc: &'a Document) -> Option<ElementRef<'a>> {
        self.dom_element
            .and_then(|idx| doc.node_id_from_raw(idx))
            .and_then(|node_id| doc.element_ref(node_id))
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

    pub fn dom_element(&self) -> Option<u32> {
        self.dom_element
    }

    pub fn parent(&self) -> Option<u32> {
        self.parent
    }
}

#[derive(Clone)]
pub enum LayoutMode {
    Block(BlockBox),
    Table(TableBox),
    TableRow(TableRowBox),
    TableCell(TableCellBox),
    Flex(FlexBox),
    Grid(GridBox),
    Inline(Range<u32>), // range of inline items
    Anonymous(Range<u32>), // range of inline items (no element)
                        // Future layout modes (not yet implemented)
                        // InlineBlock(BlockBox),
                        // Replaced(ReplacedBox),
}

impl LayoutMode {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        match self {
            LayoutMode::Block(block) => {
                report.extend_prefixed("LayoutMode::Block", block.memory_usage_report())
            }
            LayoutMode::Table(table) => {
                report.extend_prefixed("LayoutMode::Table", table.memory_usage_report())
            }
            LayoutMode::TableRow(row) => {
                report.extend_prefixed("LayoutMode::TableRow", row.memory_usage_report())
            }
            LayoutMode::TableCell(cell) => {
                report.extend_prefixed("LayoutMode::TableCell", cell.memory_usage_report())
            }
            LayoutMode::Flex(container) => {
                report.extend_prefixed("LayoutMode::Flex", container.memory_usage_report())
            }
            LayoutMode::Grid(container) => {
                report.extend_prefixed("LayoutMode::Grid", container.memory_usage_report())
            }
            LayoutMode::Inline(_) => {}
            LayoutMode::Anonymous(_) => {}
        }
        report
    }
}

#[derive(Clone, Default)]
pub struct FlexBox {
    pub children: Vec<u32>,
}

impl FlexBox {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_vec_storage("FlexBox.children.storage", &self.children);
        report
    }
}

#[derive(Clone, Default)]
pub struct GridBox {
    pub children: Vec<u32>,
}

impl GridBox {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_vec_storage("GridBox.children.storage", &self.children);
        report
    }
}

#[derive(Clone, Default)]
pub struct BlockBox {
    pub children: Children,
}

impl BlockBox {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        self.children.memory_usage_report()
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
    pub column_width_hints: Vec<TableColumnWidthHint>,
}

#[derive(Clone)]
pub struct TableColumnTrack {
    // Style used for column width resolution (explicit width / min / max hints).
    pub width_style: StyleIndices,
    // Style used for column background layering, if authored by <col>.
    pub background_style: Option<StyleIndices>,
}

#[derive(Clone)]
pub struct TableColumnGroupSpan {
    pub style: StyleIndices,
    pub start: usize,
    pub span: usize,
}

/// A table column does not generate a layout box, so retain the opaque style
/// handle needed to turn its computed width into a shaped, used width.
#[derive(Clone, Copy)]
pub struct TableColumnWidthHint {
    pub style: StyleIndices,
    pub fallback_style: Option<StyleIndices>,
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
        report.add_vec_storage(
            "TableBox.column_width_hints.storage",
            &self.column_width_hints,
        );
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

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        self.children.memory_usage_report()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Children {
    Blocks(Vec<u32>),
    InlineItems(Range<u32>),
    #[default]
    Empty,
}

impl Children {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        if let Children::Blocks(children) = self {
            report.add_vec_storage("Children::Blocks.storage", children);
        }
        report
    }
}

impl html_text::TextStructure for LayoutTree {
    fn get_box_parent(&self, context: usize) -> Option<usize> {
        self.get_box_parent(context)
    }

    fn get_box_style_indices(&self, context: usize) -> Option<StyleIndices> {
        self.get_box_style_indices(context)
    }

    fn get_box_dom_element(&self, context: usize) -> Option<u32> {
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
