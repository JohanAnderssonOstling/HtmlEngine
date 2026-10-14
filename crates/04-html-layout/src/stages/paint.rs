//! A compiled paint schema is shared by palettes and scoped note views.

use crate::layout_model::LayoutTree;
use html_dom::{Document, MemoryUsageReport};
use html_style_model::{ComputedStyles, ReaderStyleOverrides, StyleIndices, StylePaintColors};
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use std::sync::Arc;

const ROOT: u8 = 1;
const LINK: u8 = 2;

struct PaintBindings {
    contexts: Box<[u8]>,
    linked_nodes: Arc<[u8]>,
    styles: Arc<[StyleIndices]>,
}

pub(crate) struct PaintStyles {
    overrides: ReaderStyleOverrides,
    bindings: Arc<PaintBindings>,
    boxes: Arc<[StylePaintColors]>,
    other_styles: Arc<FxHashMap<StyleIndices, StylePaintColors>>,
    linked_styles: Arc<FxHashMap<StyleIndices, StylePaintColors>>,
}

fn linked_nodes(document: &Document) -> Arc<[u8]> {
    // Memoize ancestry without assuming node IDs are in preorder. Each node
    // is visited once, including when the source DOM has been reparented.
    let mut states = vec![0u8; document.node_count()];
    let mut path = Vec::new();
    for node in document.node_ids() {
        path.clear();
        let mut current = node;
        let linked = loop {
            if states[current.index()] != 0 {
                break states[current.index()] == 2;
            }
            path.push(current.index());
            if document.element_ref(current).is_some_and(|element| {
                element.tag().eq_ignore_ascii_case("a") && element.attr("href").is_some()
            }) {
                break true;
            }
            let Some(parent) = document.get_dom_parent(current) else {
                break false;
            };
            current = parent;
        };
        for &index in &path {
            states[index] = if linked { 2 } else { 1 };
        }
    }
    for state in &mut states {
        *state = u8::from(*state == 2);
    }
    states.into()
}

fn box_contexts(document: &Document, tree: &LayoutTree, linked: &[u8]) -> Box<[u8]> {
    let mut contexts = vec![0; tree.box_count()];
    for index in 0..tree.box_count() {
        let node = tree
            .get_box_dom_element(index)
            .and_then(|raw| document.node_id_from_raw(raw));
        let is_link = node.map_or_else(
            || {
                tree.get_box_parent(index)
                    .is_some_and(|parent| contexts[parent] & LINK != 0)
            },
            |node| linked[node.index()] != 0,
        );
        let is_root = (tree.root_box() == Some(index) || tree.body_box() == Some(index))
            && node
                .and_then(|node| document.element_ref(node))
                .is_some_and(|element| matches!(element.tag(), "html" | "body"));
        contexts[index] = if is_root { ROOT } else { 0 } | if is_link { LINK } else { 0 };
    }
    contexts.into()
}

fn box_colors(
    styles: &ComputedStyles,
    tree: &LayoutTree,
    bindings: &PaintBindings,
    overrides: &ReaderStyleOverrides,
    plain: &FxHashMap<StyleIndices, StylePaintColors>,
    linked: &FxHashMap<StyleIndices, StylePaintColors>,
) -> Arc<[StylePaintColors]> {
    let mut contextual = FxHashMap::default();
    (0..tree.box_count())
        .map(|index| {
            let indices = tree
                .get_box_style_indices(index)
                .unwrap_or_else(|| styles.default_indices());
            let flags = bindings.contexts[index];
            if flags == 0
                && let Some(&colors) = plain.get(&indices)
            {
                return colors;
            }
            if flags == LINK
                && let Some(&colors) = linked.get(&indices)
            {
                return colors;
            }
            *contextual.entry((indices, flags)).or_insert_with(|| {
                styles
                    .reader_paint_colors(indices, overrides, flags & ROOT != 0, flags & LINK != 0)
                    .expect("validated paint binding")
            })
        })
        .collect()
}

impl PaintStyles {
    pub(crate) fn new(document: &Document, styles: &ComputedStyles, tree: &LayoutTree) -> Self {
        let linked_nodes = linked_nodes(document);
        let contexts = box_contexts(document, tree, &linked_nodes);
        let mut seen = FxHashSet::default();
        let mut handles = Vec::new();
        let mut retain = |indices| {
            if seen.insert(indices) {
                handles.push(indices);
            }
        };
        retain(styles.default_indices());
        for index in 0..tree.box_count() {
            retain(
                tree.get_box_style_indices(index)
                    .unwrap_or_else(|| styles.default_indices()),
            );
        }
        for node in document.node_ids() {
            for indices in [
                styles.style_for_node(node),
                styles.first_line_style_for_node(node),
                styles.first_letter_style_for_node(node),
                styles.before_first_letter_style_for_node(node),
                styles.before_style_for_node(node).map(|entry| entry.0),
                styles.after_style_for_node(node).map(|entry| entry.0),
            ]
            .into_iter()
            .flatten()
            {
                retain(indices);
            }
        }
        Self {
            overrides: ReaderStyleOverrides::default(),
            bindings: Arc::new(PaintBindings {
                contexts,
                linked_nodes,
                styles: handles.into(),
            }),
            boxes: Arc::from([]),
            other_styles: Arc::new(FxHashMap::default()),
            linked_styles: Arc::new(FxHashMap::default()),
        }
    }

    pub(crate) fn with_overrides(
        &self,
        styles: &ComputedStyles,
        tree: &LayoutTree,
        overrides: &ReaderStyleOverrides,
    ) -> Self {
        let overrides = overrides.paint_overrides();
        if overrides == ReaderStyleOverrides::default() {
            return Self {
                overrides,
                bindings: self.bindings.clone(),
                boxes: Arc::from([]),
                other_styles: Arc::new(FxHashMap::default()),
                linked_styles: Arc::new(FxHashMap::default()),
            };
        }
        // Palette changes evaluate each distinct style once. They never walk
        // DOM ancestry, rebuild topology, or reintern typography properties.
        let other_styles: FxHashMap<_, _> = self
            .bindings
            .styles
            .iter()
            .copied()
            .map(|indices| {
                (
                    indices,
                    styles
                        .reader_paint_colors(indices, &overrides, false, false)
                        .expect("validated publication style"),
                )
            })
            .collect();
        let linked_styles: FxHashMap<_, _> = if overrides.link_foreground.is_some()
            && self.bindings.linked_nodes.iter().any(|&linked| linked != 0)
        {
            self.bindings
                .styles
                .iter()
                .copied()
                .map(|indices| {
                    (
                        indices,
                        styles
                            .reader_paint_colors(indices, &overrides, false, true)
                            .expect("validated linked paint binding"),
                    )
                })
                .collect()
        } else {
            FxHashMap::default()
        };
        let boxes = box_colors(
            styles,
            tree,
            &self.bindings,
            &overrides,
            &other_styles,
            &linked_styles,
        );
        Self {
            overrides,
            bindings: self.bindings.clone(),
            boxes,
            other_styles: Arc::new(other_styles),
            linked_styles: Arc::new(linked_styles),
        }
    }

    pub(crate) fn for_tree(
        &self,
        document: &Document,
        styles: &ComputedStyles,
        tree: &LayoutTree,
    ) -> Self {
        let bindings = Arc::new(PaintBindings {
            contexts: box_contexts(document, tree, &self.bindings.linked_nodes),
            linked_nodes: self.bindings.linked_nodes.clone(),
            styles: self.bindings.styles.clone(),
        });
        let boxes = if self.is_default() {
            Arc::from([])
        } else {
            box_colors(
                styles,
                tree,
                &bindings,
                &self.overrides,
                &self.other_styles,
                &self.linked_styles,
            )
        };
        Self {
            overrides: self.overrides.clone(),
            bindings,
            boxes,
            other_styles: self.other_styles.clone(),
            linked_styles: self.linked_styles.clone(),
        }
    }

    pub(crate) fn is_default(&self) -> bool {
        self.overrides == ReaderStyleOverrides::default()
    }
    pub(crate) fn box_colors(&self, index: usize) -> Option<&StylePaintColors> {
        self.boxes.get(index)
    }
    pub(crate) fn style_colors(
        &self,
        indices: StyleIndices,
        context: usize,
    ) -> Option<&StylePaintColors> {
        if self
            .bindings
            .contexts
            .get(context)
            .is_some_and(|flags| flags & LINK != 0)
        {
            self.linked_styles
                .get(&indices)
                .or_else(|| self.other_styles.get(&indices))
        } else {
            self.other_styles.get(&indices)
        }
    }
    pub(crate) fn resolve(
        &self,
        styles: &ComputedStyles,
        indices: StyleIndices,
        context_box: usize,
    ) -> StylePaintColors {
        let flags = self
            .bindings
            .contexts
            .get(context_box)
            .copied()
            .unwrap_or(0);
        if flags == 0
            && let Some(&colors) = self.other_styles.get(&indices)
        {
            return colors;
        }
        if flags == LINK
            && let Some(&colors) = self.linked_styles.get(&indices)
        {
            return colors;
        }
        styles
            .reader_paint_colors(
                indices,
                &self.overrides,
                flags & ROOT != 0,
                flags & LINK != 0,
            )
            .expect("validated paint binding")
    }
    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<u8>(
            "PaintBindings.contexts",
            self.bindings.contexts.len(),
            self.bindings.contexts.len(),
        );
        report.add_slice_storage::<u8>(
            "PaintBindings.linked_nodes",
            self.bindings.linked_nodes.len(),
            self.bindings.linked_nodes.len(),
        );
        report.add_slice_storage::<StyleIndices>(
            "PaintBindings.styles",
            self.bindings.styles.len(),
            self.bindings.styles.len(),
        );
        report.add_slice_storage::<StylePaintColors>(
            "PaintStyles.boxes",
            self.boxes.len(),
            self.boxes.len(),
        );
        report.add_slice_storage::<(StyleIndices, StylePaintColors)>(
            "PaintStyles.other_styles",
            self.other_styles.capacity(),
            self.other_styles.len(),
        );
        report.add_slice_storage::<(StyleIndices, StylePaintColors)>(
            "PaintStyles.linked_styles",
            self.linked_styles.capacity(),
            self.linked_styles.len(),
        );
        report
    }
}
