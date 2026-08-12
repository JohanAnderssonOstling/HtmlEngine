use super::measurement::measure_item_with_baseline;
use super::{TaffyContainerKind, finite_f32};
use crate::layout::LayoutEngine;
use taffy::geometry::Size;
use taffy::prelude::{
    AvailableSpace, Dimension, Layout, LayoutPartialTree, NodeId, Style, TraversePartialTree,
};
use taffy::{
    LayoutFlexboxContainer, LayoutGridContainer, LayoutInput, LayoutOutput, compute_flexbox_layout,
    compute_grid_layout, compute_leaf_layout, compute_root_layout,
};

/// A shallow Taffy tree for one renderer-owned Flex/Grid formatting context.
///
/// Taffy's high-level `TaffyTree::compute_layout_with_measure` callback only
/// returns a size, so leaf baselines are discarded before Grid's own baseline
/// shim algorithm runs. This adapter implements Taffy's public low-level tree
/// traits and returns the renderer's measured baselines in `LayoutOutput`.
/// Track sizing, placement, and alignment remain entirely Taffy-owned.
pub(super) struct TaffyLayoutTree<'tree, 'document, 'output> {
    session: &'tree mut LayoutEngine<'document, 'output>,
    kind: TaffyContainerKind,
    styles: Vec<Style>,
    contexts: Vec<Option<u32>>,
    children: Vec<Vec<NodeId>>,
    layouts: Vec<Layout>,
    outputs: Vec<LayoutOutput>,
}

impl<'tree, 'document, 'output> TaffyLayoutTree<'tree, 'document, 'output> {
    pub(super) fn new(
        session: &'tree mut LayoutEngine<'document, 'output>,
        kind: TaffyContainerKind,
        root_style: Style,
        child_styles: Vec<(Style, Option<u32>)>,
    ) -> Self {
        let mut styles = Vec::with_capacity(child_styles.len() + 1);
        let mut contexts = Vec::with_capacity(child_styles.len() + 1);
        styles.push(root_style);
        contexts.push(None);
        for (style, context) in child_styles {
            styles.push(style);
            contexts.push(context);
        }
        let root_children = (1..styles.len()).map(NodeId::from).collect::<Vec<_>>();
        let mut children = vec![Vec::new(); styles.len()];
        children[0] = root_children;
        let layouts = vec![Layout::new(); styles.len()];
        let outputs = vec![LayoutOutput::DEFAULT; styles.len()];
        Self {
            session,
            kind,
            styles,
            contexts,
            children,
            layouts,
            outputs,
        }
    }

    pub(super) fn compute(&mut self, available_space: Size<AvailableSpace>) {
        compute_root_layout(self, NodeId::from(0usize), available_space);
    }

    pub(super) fn compute_with_definite_root_height(
        &mut self,
        available_space: Size<AvailableSpace>,
        height: f32,
    ) {
        self.styles[0].size.height = Dimension::length(height.max(0.0));
        compute_root_layout(self, NodeId::from(0usize), available_space);
    }

    pub(super) fn layout(&self, node: NodeId) -> Layout {
        self.layouts[usize::from(node)]
    }

    pub(super) fn root(&self) -> NodeId {
        NodeId::from(0usize)
    }

    pub(super) fn root_first_baseline(&self) -> Option<f32> {
        self.outputs[0].first_baselines.y
    }

    pub(super) fn child(&self, index: usize) -> NodeId {
        NodeId::from(index + 1)
    }
}

impl TraversePartialTree for TaffyLayoutTree<'_, '_, '_> {
    type ChildIter<'a>
        = std::iter::Copied<std::slice::Iter<'a, NodeId>>
    where
        Self: 'a;

    fn child_ids(&self, parent_node_id: NodeId) -> Self::ChildIter<'_> {
        self.children[usize::from(parent_node_id)].iter().copied()
    }

    fn child_count(&self, parent_node_id: NodeId) -> usize {
        self.children[usize::from(parent_node_id)].len()
    }

    fn get_child_id(&self, parent_node_id: NodeId, child_index: usize) -> NodeId {
        self.children[usize::from(parent_node_id)][child_index]
    }
}

impl LayoutPartialTree for TaffyLayoutTree<'_, '_, '_> {
    type CoreContainerStyle<'a>
        = &'a Style
    where
        Self: 'a;
    type CustomIdent = String;

    fn get_core_container_style(&self, node_id: NodeId) -> Self::CoreContainerStyle<'_> {
        &self.styles[usize::from(node_id)]
    }

    fn set_unrounded_layout(&mut self, node_id: NodeId, layout: &Layout) {
        self.layouts[usize::from(node_id)] = *layout;
    }

    fn compute_child_layout(&mut self, node_id: NodeId, inputs: LayoutInput) -> LayoutOutput {
        let index = usize::from(node_id);
        if !self.children[index].is_empty() {
            let output = match self.kind {
                TaffyContainerKind::Flex => compute_flexbox_layout(self, node_id, inputs),
                TaffyContainerKind::Grid => compute_grid_layout(self, node_id, inputs),
            };
            self.outputs[index] = output;
            return output;
        }
        if self.styles[index].display == taffy::style::Display::None {
            return LayoutOutput::HIDDEN;
        }

        let style = self.styles[index].clone();
        let context = self.contexts[index];
        let mut first_baseline = None;
        let mut output = compute_leaf_layout(
            inputs,
            &style,
            |_, _| 0.0,
            |known, available| {
                let Some(box_idx) = context else {
                    return Size::ZERO;
                };
                let measured =
                    measure_item_with_baseline(self.session, box_idx as usize, known, available);
                first_baseline = measured.first_baseline;
                measured.size
            },
        );
        output.first_baselines.y = first_baseline.map(finite_f32);
        self.outputs[index] = output;
        output
    }
}

impl LayoutFlexboxContainer for TaffyLayoutTree<'_, '_, '_> {
    type FlexboxContainerStyle<'a>
        = &'a Style
    where
        Self: 'a;
    type FlexboxItemStyle<'a>
        = &'a Style
    where
        Self: 'a;

    fn get_flexbox_container_style(&self, node_id: NodeId) -> Self::FlexboxContainerStyle<'_> {
        &self.styles[usize::from(node_id)]
    }

    fn get_flexbox_child_style(&self, child_node_id: NodeId) -> Self::FlexboxItemStyle<'_> {
        &self.styles[usize::from(child_node_id)]
    }
}

impl LayoutGridContainer for TaffyLayoutTree<'_, '_, '_> {
    type GridContainerStyle<'a>
        = &'a Style
    where
        Self: 'a;
    type GridItemStyle<'a>
        = &'a Style
    where
        Self: 'a;

    fn get_grid_container_style(&self, node_id: NodeId) -> Self::GridContainerStyle<'_> {
        &self.styles[usize::from(node_id)]
    }

    fn get_grid_child_style(&self, child_node_id: NodeId) -> Self::GridItemStyle<'_> {
        &self.styles[usize::from(child_node_id)]
    }
}
