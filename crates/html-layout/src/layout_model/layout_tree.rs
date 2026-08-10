use html_dom::{Document, ElementRef, MemoryUsageReport};
use html_style_model::{ComputedStyles, ListStylePosition, StyleIndices};
use rustc_data_structures::fx::FxHashMap;
use std::ops::Range;

/// A list item's generated marker (`::marker`). `marker_box` is a real layout
/// box holding the marker text run; it is laid out and positioned by the engine
/// relative to the list item's first line rather than flowing as a child, so it
/// is kept out of the item's `Children`.
#[derive(Clone, Copy)]
pub(crate) struct ListItemMarker {
    pub marker_box: u32,
    pub position: ListStylePosition,
}

impl ListItemMarker {
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        MemoryUsageReport::new()
    }
}

/// Output of the box-construction phase: the box tree plus the inline text
/// content it references (glyph codepoints + inline items). Produced by
/// `build_layout_inputs`, consumed read-only by shaping and layout.
#[derive(Default, Clone)]
pub(crate) struct LayoutTree {
    pub(super) boxes: Vec<LayoutBox>,
    pub(super) root_box: Option<u32>,
    pub(super) body_box: Option<u32>,
    /// Generated markers keyed by their list-item box index.
    pub(super) list_markers: FxHashMap<u32, ListItemMarker>,
    /// Inline-axis endpoints for block-in-inline fragments. Ordinary inline
    /// boxes implicitly own both endpoints.
    pub(super) inline_fragment_edges: FxHashMap<u32, (bool, bool)>,
    /// Relatively positioned inline ancestors that were split away from a
    /// block descendant by CSS block-in-inline box generation.
    pub(super) split_inline_position_ancestors: FxHashMap<u32, Box<[StyleIndices]>>,
}

impl LayoutTree {
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<LayoutBox>("LayoutTree.boxes.storage", self.boxes.capacity(), self.boxes.len());
        for layout_box in &self.boxes {
            report.extend_prefixed("LayoutTree.boxes", layout_box.memory_usage_report());
        }
        report.add_slice_storage::<(u32, ListItemMarker)>("LayoutTree.list_markers.storage", self.list_markers.capacity(), self.list_markers.len());
        report.add_slice_storage::<(u32, (bool, bool))>("LayoutTree.inline_fragment_edges.storage", self.inline_fragment_edges.capacity(), self.inline_fragment_edges.len());
        report.add_slice_storage::<(u32, Box<[StyleIndices]>)>("LayoutTree.split_inline_position_ancestors.storage", self.split_inline_position_ancestors.capacity(), self.split_inline_position_ancestors.len());
        for ancestors in self.split_inline_position_ancestors.values() {
            report.add_slice_storage::<StyleIndices>("LayoutTree.split_inline_position_ancestors.entries", ancestors.len(), ancestors.len());
        }
        report
    }
}

impl LayoutTree {
    pub(crate) fn box_count(&self) -> usize {
        self.boxes.len()
    }

    pub(crate) fn box_at(&self, box_idx: usize) -> Option<&LayoutBox> {
        self.boxes.get(box_idx)
    }

    pub(crate) fn box_at_mut(&mut self, box_idx: usize) -> Option<&mut LayoutBox> {
        self.boxes.get_mut(box_idx)
    }

    pub(crate) fn root_box(&self) -> Option<usize> {
        self.root_box.map(|idx| idx as usize)
    }

    pub(crate) fn set_root_box(&mut self, root_box: Option<u32>) {
        self.root_box = root_box;
    }

    pub(crate) fn body_box(&self) -> Option<usize> {
        self.body_box.map(|idx| idx as usize)
    }

    pub(crate) fn set_body_box(&mut self, body_box: u32) {
        self.body_box = Some(body_box);
    }

    pub(crate) fn get_box_style_indices(&self, box_idx: usize) -> Option<StyleIndices> {
        self.boxes.get(box_idx)?.style()
    }

    pub(crate) fn get_box_dom_element(&self, box_idx: usize) -> Option<u32> {
        self.boxes.get(box_idx)?.dom_element()
    }

    pub(crate) fn get_box_parent(&self, box_idx: usize) -> Option<usize> {
        self.boxes.get(box_idx)?.parent().map(|p| p as usize)
    }

    pub(crate) fn get_box_layout_mode(&self, box_idx: usize) -> Option<&LayoutMode> {
        self.boxes.get(box_idx).map(|layout_box| layout_box.layout_mode())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }

    pub(crate) fn print_box_tree(&self, doc: &Document, styles: &ComputedStyles) {
        if !self.boxes.is_empty() {
            self.print_box_node(doc, styles, 0, 0);
        }
    }

    fn print_box_node(&self, doc: &Document, styles: &ComputedStyles, idx: usize, depth: usize) {
        let indent = "  ".repeat(depth);
        let b = &self.boxes[idx];
        let tag = b.get_element(doc).map(|element| element.tag()).unwrap_or("");
        let dom_id = b.dom_element().map(|i| i.to_string()).unwrap_or("-".into());
        let display = b.style().and_then(|indices| styles.box_model_style(indices)).map(|style| format!("{:?}", style.display)).unwrap_or("none".into());
        let mode = match &b.layout_mode {
            LayoutMode::Block(bb) => format!("Block({:?})", bb.children),
            LayoutMode::Table(tb) => format!("Table(rows={})", tb.rows.len()),
            LayoutMode::TableRow(tr) => format!("TableRow(cells={})", tr.cells.len()),
            LayoutMode::TableCell(tc) => format!("TableCell(colspan={}, rowspan={}, {:?})", tc.colspan, tc.rowspan, tc.children),
            LayoutMode::Flex(container) => format!("Flex(items={})", container.children.len()),
            LayoutMode::Grid(container) => format!("Grid(items={})", container.children.len()),
            LayoutMode::Inline(r) => format!("Inline({}..{})", r.start, r.end),
            LayoutMode::Anonymous(r) => format!("Anon({}..{})", r.start, r.end),
        };
        println!("{}[{}] <{}> dom={} display={} {}", indent, idx, tag, dom_id, display, mode);
        match &b.layout_mode {
            LayoutMode::Block(bb) => {
                if let Children::Blocks(indices) = &bb.children {
                    for &i in indices {
                        self.print_box_node(doc, styles, i as usize, depth + 1);
                    }
                }
            }
            LayoutMode::Table(tb) => {
                for &i in &tb.rows {
                    self.print_box_node(doc, styles, i as usize, depth + 1);
                }
            }
            LayoutMode::TableRow(tr) => {
                for &i in &tr.cells {
                    self.print_box_node(doc, styles, i as usize, depth + 1);
                }
            }
            LayoutMode::Flex(container) => {
                for &i in &container.children {
                    self.print_box_node(doc, styles, i as usize, depth + 1);
                }
            }
            LayoutMode::Grid(container) => {
                for &i in &container.children {
                    self.print_box_node(doc, styles, i as usize, depth + 1);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn push_box(&mut self, layout_box: LayoutBox) -> u32 {
        let idx = self.boxes.len() as u32;
        self.boxes.push(layout_box);
        idx
    }

    pub(crate) fn set_list_marker(&mut self, list_item_box: u32, marker: ListItemMarker) {
        self.list_markers.insert(list_item_box, marker);
    }

    pub(crate) fn list_marker(&self, list_item_box: usize) -> Option<ListItemMarker> {
        self.list_markers.get(&(list_item_box as u32)).copied()
    }

    pub(crate) fn set_inline_fragment_edges(&mut self, box_idx: u32, inline_start: bool, inline_end: bool) {
        self.inline_fragment_edges.insert(box_idx, (inline_start, inline_end));
    }

    pub(crate) fn inline_fragment_edges(&self, box_idx: usize) -> (bool, bool) {
        self.inline_fragment_edges.get(&(box_idx as u32)).copied().unwrap_or((true, true))
    }

    pub(crate) fn set_split_inline_position_ancestors(&mut self, box_idx: u32, ancestors: Vec<StyleIndices>) {
        if !ancestors.is_empty() {
            self.split_inline_position_ancestors.insert(box_idx, ancestors.into_boxed_slice());
        }
    }

    pub(crate) fn split_inline_position_ancestors(&self, box_idx: usize) -> &[StyleIndices] {
        self.split_inline_position_ancestors.get(&(box_idx as u32)).map(Box::as_ref).unwrap_or_default()
    }
}

#[derive(Clone)]
pub(crate) struct LayoutBox {
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
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.extend_prefixed("LayoutBox.layout_mode", self.layout_mode.memory_usage_report());
        report
    }
}

// Type alias retained for readability at layout-mode match sites.
pub(crate) type BoxType = LayoutMode;

impl LayoutBox {
    pub(crate) fn new_anonymous_box(range: Range<u32>, parent: Option<u32>) -> Self {
        Self {
            layout_mode: LayoutMode::Anonymous(range),
            dom_element: None, // anonymous boxes have no DOM element
            parent,
            style: None,
        }
    }

    /// Get element data from DOM tree
    pub(crate) fn get_element<'a>(&self, doc: &'a Document) -> Option<ElementRef<'a>> {
        self.dom_element.and_then(|idx| doc.node_id_from_raw(idx)).and_then(|node_id| doc.element_ref(node_id))
    }

    pub(crate) fn layout_mode(&self) -> &LayoutMode {
        &self.layout_mode
    }

    pub(crate) fn layout_mode_mut(&mut self) -> &mut LayoutMode {
        &mut self.layout_mode
    }

    pub(crate) fn style(&self) -> Option<StyleIndices> {
        self.style
    }

    pub(crate) fn set_style(&mut self, style: Option<StyleIndices>) {
        self.style = style;
    }

    pub(crate) fn dom_element(&self) -> Option<u32> {
        self.dom_element
    }

    pub(crate) fn set_dom_element(&mut self, dom_element: Option<u32>) {
        self.dom_element = dom_element;
    }

    pub(crate) fn parent(&self) -> Option<u32> {
        self.parent
    }

    pub(crate) fn set_parent(&mut self, parent: Option<u32>) {
        self.parent = parent;
    }
}

#[derive(Clone)]
pub(crate) enum LayoutMode {
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
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        match self {
            LayoutMode::Block(block) => report.extend_prefixed("LayoutMode::Block", block.memory_usage_report()),
            LayoutMode::Table(table) => report.extend_prefixed("LayoutMode::Table", table.memory_usage_report()),
            LayoutMode::TableRow(row) => report.extend_prefixed("LayoutMode::TableRow", row.memory_usage_report()),
            LayoutMode::TableCell(cell) => report.extend_prefixed("LayoutMode::TableCell", cell.memory_usage_report()),
            LayoutMode::Flex(container) => report.extend_prefixed("LayoutMode::Flex", container.memory_usage_report()),
            LayoutMode::Grid(container) => report.extend_prefixed("LayoutMode::Grid", container.memory_usage_report()),
            LayoutMode::Inline(_) => {}
            LayoutMode::Anonymous(_) => {}
        }
        report
    }
}

#[derive(Clone, Default)]
pub(crate) struct FlexBox {
    pub children: Vec<u32>,
}

impl FlexBox {
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<u32>("FlexBox.children.storage", self.children.capacity(), self.children.len());
        report
    }
}

#[derive(Clone, Default)]
pub(crate) struct GridBox {
    pub children: Vec<u32>,
}

impl GridBox {
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<u32>("GridBox.children.storage", self.children.capacity(), self.children.len());
        report
    }
}

#[derive(Clone)]
pub(crate) struct BlockBox {
    pub children: Children,
}

impl BlockBox {
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        self.children.memory_usage_report()
    }
}

#[derive(Clone)]
pub(crate) struct TableBox {
    pub rows: Vec<u32>,
    pub row_groups: Vec<u32>,
    pub captions_top: Vec<u32>,
    pub captions_bottom: Vec<u32>,
    pub columns: Vec<TableColumnTrack>,
    pub column_groups: Vec<TableColumnGroupSpan>,
    pub column_width_hints: Vec<TableColumnWidthHint>,
}

#[derive(Clone)]
pub(crate) struct TableColumnTrack {
    // Style used for column width resolution (explicit width / min / max hints).
    pub width_style: StyleIndices,
    // Style used for column background layering, if authored by <col>.
    pub background_style: Option<StyleIndices>,
}

#[derive(Clone)]
pub(crate) struct TableColumnGroupSpan {
    pub style: StyleIndices,
    pub start: usize,
    pub span: usize,
}

/// A table column does not generate a layout box, so retain the opaque style
/// handle needed to turn its computed width into a shaped, used width.
#[derive(Clone, Copy)]
pub(crate) struct TableColumnWidthHint {
    pub style: StyleIndices,
    pub fallback_style: Option<StyleIndices>,
}

impl TableBox {
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<u32>("TableBox.rows.storage", self.rows.capacity(), self.rows.len());
        report.add_slice_storage::<u32>("TableBox.row_groups.storage", self.row_groups.capacity(), self.row_groups.len());
        report.add_slice_storage::<u32>("TableBox.captions_top.storage", self.captions_top.capacity(), self.captions_top.len());
        report.add_slice_storage::<u32>("TableBox.captions_bottom.storage", self.captions_bottom.capacity(), self.captions_bottom.len());
        report.add_slice_storage::<TableColumnTrack>("TableBox.columns.storage", self.columns.capacity(), self.columns.len());
        report.add_slice_storage::<TableColumnGroupSpan>("TableBox.column_groups.storage", self.column_groups.capacity(), self.column_groups.len());
        report.add_slice_storage::<TableColumnWidthHint>("TableBox.column_width_hints.storage", self.column_width_hints.capacity(), self.column_width_hints.len());
        report
    }
}

#[derive(Clone)]
pub(crate) struct TableRowBox {
    pub cells: Vec<u32>,
    /// Positioned descendants that are not table cells still belong to the
    /// row's static-position context and must not be discarded by table fixup.
    pub out_of_flow: Vec<u32>,
}

impl TableRowBox {
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<u32>("TableRowBox.cells.storage", self.cells.capacity(), self.cells.len());
        report.add_slice_storage::<u32>("TableRowBox.out_of_flow.storage", self.out_of_flow.capacity(), self.out_of_flow.len());
        report
    }
}

#[derive(Clone)]
pub(crate) struct TableCellBox {
    pub children: Children,
    pub colspan: usize,
    pub rowspan: usize,
}

impl TableCellBox {
    pub(crate) fn new(children: Children) -> Self {
        Self { children, colspan: 1, rowspan: 1 }
    }

    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        self.children.memory_usage_report()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Children {
    Blocks(Vec<u32>),
    InlineItems(Range<u32>),
    Empty,
}

impl Children {
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        if let Children::Blocks(children) = self {
            report.add_slice_storage::<u32>("Children::Blocks.storage", children.capacity(), children.len());
        }
        report
    }
}
