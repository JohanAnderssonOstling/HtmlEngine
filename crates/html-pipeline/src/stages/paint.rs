//! A document owns paint bindings; palette candidates and scoped notes borrow them.

use super::lifecycle::Storage;
use crate::layout_model::LayoutTree;
use html_dom::{Document, MemoryUsageReport};
use html_style_model::{ComputedStyles, ReaderStyleOverrides, StyleIndices, StylePaintColors};
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use std::mem::replace;

use html_engine_model::{PAINT_LINK as LINK, PAINT_ROOT as ROOT, PaintView};

struct PaintBindings<'a> {
    contexts: Box<[u8]>,
    linked_nodes: Storage<'a, Vec<u8>>,
    styles: Storage<'a, Vec<StyleIndices>>,
}

pub(crate) struct PaintStyles<'a> {
    overrides: ReaderStyleOverrides,
    bindings: Storage<'a, PaintBindings<'a>>,
    boxes: Box<[StylePaintColors]>,
    other_styles: Storage<'a, FxHashMap<StyleIndices, StylePaintColors>>,
    linked_styles: Storage<'a, FxHashMap<StyleIndices, StylePaintColors>>,
}

fn linked_nodes(document: &Document) -> Vec<u8> {
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
    states
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
) -> Box<[StylePaintColors]> {
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

impl<'a> PaintStyles<'a> {
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
        for indices in styles.bound_styles().chain(tree.non_box_styles()) {
            retain(indices);
        }
        Self {
            overrides: ReaderStyleOverrides::default(),
            bindings: Storage::owned(PaintBindings {
                contexts,
                linked_nodes: Storage::owned(linked_nodes),
                styles: Storage::owned(handles),
            }),
            boxes: Box::new([]),
            other_styles: Storage::owned(FxHashMap::default()),
            linked_styles: Storage::owned(FxHashMap::default()),
        }
    }

    pub(crate) fn with_overrides(
        &self,
        styles: &ComputedStyles,
        tree: &LayoutTree,
        overrides: &ReaderStyleOverrides,
    ) -> PaintStyles<'_> {
        let overrides = overrides.paint_overrides();
        if overrides == ReaderStyleOverrides::default() {
            return PaintStyles {
                overrides,
                bindings: Storage::Borrowed(&self.bindings),
                boxes: Box::new([]),
                other_styles: Storage::owned(FxHashMap::default()),
                linked_styles: Storage::owned(FxHashMap::default()),
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
        PaintStyles {
            overrides,
            bindings: Storage::Borrowed(&self.bindings),
            boxes,
            other_styles: Storage::owned(other_styles),
            linked_styles: Storage::owned(linked_styles),
        }
    }

    pub(crate) fn for_tree(
        &self,
        document: &Document,
        styles: &ComputedStyles,
        tree: &LayoutTree,
    ) -> PaintStyles<'_> {
        let bindings = Storage::owned(PaintBindings {
            contexts: box_contexts(document, tree, &self.bindings.linked_nodes),
            linked_nodes: Storage::Borrowed(&self.bindings.linked_nodes),
            styles: Storage::Borrowed(&self.bindings.styles),
        });
        let boxes = if self.is_default() {
            Box::new([])
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
        PaintStyles {
            overrides: self.overrides.clone(),
            bindings,
            boxes,
            other_styles: Storage::Borrowed(&self.other_styles),
            linked_styles: Storage::Borrowed(&self.linked_styles),
        }
    }

    pub(crate) fn is_default(&self) -> bool {
        self.view().is_default()
    }
    pub(crate) fn box_colors(&self, index: usize) -> Option<&StylePaintColors> {
        self.view().box_colors(index)
    }

    pub(crate) fn resolve(
        &self,
        styles: &ComputedStyles,
        indices: StyleIndices,
        context_box: usize,
    ) -> StylePaintColors {
        self.view().resolve(styles, indices, context_box)
    }

    pub(crate) fn view(&self) -> PaintView<'_> {
        PaintView::new(
            &self.overrides,
            &self.bindings.contexts,
            &self.boxes,
            &self.other_styles,
            &self.linked_styles,
        )
    }

    pub(crate) fn into_palette(self) -> PaintPalette {
        PaintPalette {
            overrides: self.overrides,
            boxes: self.boxes,
            other_styles: self
                .other_styles
                .into_owned()
                .expect("candidate owns its palette"),
            linked_styles: self
                .linked_styles
                .into_owned()
                .expect("candidate owns its palette"),
        }
    }

    pub(crate) fn replace_palette(&mut self, palette: PaintPalette) -> PaintPalette {
        let old = PaintPalette {
            overrides: replace(&mut self.overrides, palette.overrides),
            boxes: replace(&mut self.boxes, palette.boxes),
            other_styles: replace(&mut self.other_styles, Storage::owned(palette.other_styles))
                .into_owned()
                .expect("palette owner"),
            linked_styles: replace(
                &mut self.linked_styles,
                Storage::owned(palette.linked_styles),
            )
            .into_owned()
            .expect("palette owner"),
        };
        old
    }

    pub(crate) fn into_note_paint(self) -> NotePaint {
        NotePaint {
            overrides: self.overrides,
            boxes: self.boxes,
            contexts: self
                .bindings
                .into_owned()
                .expect("scoped bindings owner")
                .contexts,
        }
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

pub(crate) struct PaintPalette {
    overrides: ReaderStyleOverrides,
    boxes: Box<[StylePaintColors]>,
    other_styles: FxHashMap<StyleIndices, StylePaintColors>,
    linked_styles: FxHashMap<StyleIndices, StylePaintColors>,
}

pub(crate) struct NotePaint {
    overrides: ReaderStyleOverrides,
    contexts: Box<[u8]>,
    boxes: Box<[StylePaintColors]>,
}

impl NotePaint {
    pub(crate) fn view<'a>(&'a self, parent: &'a PaintStyles<'_>) -> Option<PaintView<'a>> {
        if self.overrides != parent.overrides {
            return None;
        }
        Some(PaintView::new(
            &self.overrides,
            &self.contexts,
            &self.boxes,
            &parent.other_styles,
            &parent.linked_styles,
        ))
    }
}
