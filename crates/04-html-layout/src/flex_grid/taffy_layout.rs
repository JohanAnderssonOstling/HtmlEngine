use super::measurement::MeasuredItem;
use super::{TaffyContainerKind, finite_f32};
use std::iter::Map;
use std::ops::Range;
use taffy::geometry::Size;
use taffy::prelude::{
    AvailableSpace, Dimension, Layout, LayoutPartialTree, NodeId, Style, TraversePartialTree,
};
use taffy::{
    LayoutFlexboxContainer, LayoutGridContainer, LayoutInput, LayoutOutput, compute_flexbox_layout,
    compute_grid_layout, compute_leaf_layout, compute_root_layout,
};

struct Node {
    style: Style,
    context: Option<u32>,
    layout: Layout,
}

/// A shallow Taffy tree for one renderer-owned Flex/Grid formatting context.
///
/// Intrinsic probes and final solves share this backend. The supplied measure
/// callback owns renderer sizing; the adapter translates sizes and baselines
/// into Taffy's low-level tree contract. It retains only root children and the
/// root baseline because renderer-owned items are opaque leaves here.
pub(super) struct TaffyLayoutTree<M> {
    measure: M,
    kind: TaffyContainerKind,
    nodes: Vec<Node>,
    root_first_baseline: Option<f32>,
}

impl<M> TaffyLayoutTree<M>
where
    M: FnMut(usize, Size<Option<f32>>, Size<AvailableSpace>) -> MeasuredItem,
{
    pub(super) fn new(
        measure: M,
        kind: TaffyContainerKind,
        root_style: Style,
        child_styles: Vec<(Style, Option<u32>)>,
    ) -> Self {
        let nodes: Vec<_> = std::iter::once((root_style, None))
            .chain(child_styles)
            .map(|(style, context)| Node {
                style,
                context,
                layout: Layout::new(),
            })
            .collect();
        Self {
            measure,
            kind,
            nodes,
            root_first_baseline: None,
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
        self.nodes[0].style.size.height = Dimension::length(height.max(0.0));
        self.compute(available_space);
    }

    pub(super) fn root_layout(&self) -> Layout {
        self.nodes[0].layout
    }

    pub(super) fn root_first_baseline(&self) -> Option<f32> {
        self.root_first_baseline
    }

    pub(super) fn item_layouts(&self) -> impl Iterator<Item = (Layout, u32)> + '_ {
        self.nodes
            .iter()
            .filter_map(|node| node.context.map(|box_idx| (node.layout, box_idx)))
    }
}

impl<M> TraversePartialTree for TaffyLayoutTree<M> {
    type ChildIter<'a>
        = Map<Range<usize>, fn(usize) -> NodeId>
    where
        Self: 'a;

    fn child_ids(&self, parent_node_id: NodeId) -> Self::ChildIter<'_> {
        let end = if usize::from(parent_node_id) == 0 {
            self.nodes.len()
        } else {
            1
        };
        (1..end).map(NodeId::from)
    }

    fn child_count(&self, parent_node_id: NodeId) -> usize {
        self.child_ids(parent_node_id).len()
    }

    fn get_child_id(&self, parent_node_id: NodeId, child_index: usize) -> NodeId {
        self.child_ids(parent_node_id)
            .nth(child_index)
            .expect("child index out of bounds")
    }
}

impl<M> LayoutPartialTree for TaffyLayoutTree<M>
where
    M: FnMut(usize, Size<Option<f32>>, Size<AvailableSpace>) -> MeasuredItem,
{
    type CoreContainerStyle<'a>
        = &'a Style
    where
        Self: 'a;
    type CustomIdent = String;

    fn get_core_container_style(&self, node_id: NodeId) -> Self::CoreContainerStyle<'_> {
        &self.nodes[usize::from(node_id)].style
    }

    fn set_unrounded_layout(&mut self, node_id: NodeId, layout: &Layout) {
        self.nodes[usize::from(node_id)].layout = *layout;
    }

    fn compute_child_layout(&mut self, node_id: NodeId, inputs: LayoutInput) -> LayoutOutput {
        let index = usize::from(node_id);
        if index == 0 && self.nodes.len() > 1 {
            let output = match self.kind {
                TaffyContainerKind::Flex => compute_flexbox_layout(self, node_id, inputs),
                TaffyContainerKind::Grid => compute_grid_layout(self, node_id, inputs),
            };
            self.root_first_baseline = output.first_baselines.y;
            return output;
        }
        if self.nodes[index].style.display == taffy::style::Display::None {
            return LayoutOutput::HIDDEN;
        }

        let style = &self.nodes[index].style;
        let context = self.nodes[index].context;
        let mut first_baseline = None;
        let mut output = compute_leaf_layout(
            inputs,
            style,
            |_, _| 0.0,
            |known, available| {
                let Some(box_idx) = context else {
                    return Size::ZERO;
                };
                let measured = (self.measure)(box_idx as usize, known, available);
                first_baseline = measured.first_baseline;
                measured.size
            },
        );
        output.first_baselines.y = first_baseline.map(finite_f32);
        if index == 0 {
            self.root_first_baseline = output.first_baselines.y;
        }
        output
    }
}

impl<M> LayoutFlexboxContainer for TaffyLayoutTree<M>
where
    M: FnMut(usize, Size<Option<f32>>, Size<AvailableSpace>) -> MeasuredItem,
{
    type FlexboxContainerStyle<'a>
        = &'a Style
    where
        Self: 'a;
    type FlexboxItemStyle<'a>
        = &'a Style
    where
        Self: 'a;

    fn get_flexbox_container_style(&self, node_id: NodeId) -> Self::FlexboxContainerStyle<'_> {
        &self.nodes[usize::from(node_id)].style
    }

    fn get_flexbox_child_style(&self, child_node_id: NodeId) -> Self::FlexboxItemStyle<'_> {
        &self.nodes[usize::from(child_node_id)].style
    }
}

impl<M> LayoutGridContainer for TaffyLayoutTree<M>
where
    M: FnMut(usize, Size<Option<f32>>, Size<AvailableSpace>) -> MeasuredItem,
{
    type GridContainerStyle<'a>
        = &'a Style
    where
        Self: 'a;
    type GridItemStyle<'a>
        = &'a Style
    where
        Self: 'a;

    fn get_grid_container_style(&self, node_id: NodeId) -> Self::GridContainerStyle<'_> {
        &self.nodes[usize::from(node_id)].style
    }

    fn get_grid_child_style(&self, child_node_id: NodeId) -> Self::GridItemStyle<'_> {
        &self.nodes[usize::from(child_node_id)].style
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocation_test_support::count_allocations;
    use taffy::RunMode;

    #[test]
    fn leaf_measurement_borrows_the_style_without_allocating() {
        let leaf_style = Style {
            grid_template_column_names: vec![vec!["column".to_owned()]],
            grid_template_row_names: vec![vec!["row".to_owned()]],
            ..Style::default()
        };
        let mut tree = TaffyLayoutTree::new(
            |box_idx, _, _| {
                assert_eq!(box_idx, 7);
                MeasuredItem {
                    size: Size {
                        width: 20.0,
                        height: 30.0,
                    },
                    first_baseline: Some(17.0),
                }
            },
            TaffyContainerKind::Flex,
            Style::default(),
            vec![(leaf_style, Some(7))],
        );
        let (output, allocations) = count_allocations(|| {
            tree.compute_child_layout(
                NodeId::from(1usize),
                LayoutInput {
                    run_mode: RunMode::ComputeSize,
                    ..LayoutInput::HIDDEN
                },
            )
        });
        assert_eq!(
            output.size,
            Size {
                width: 20.0,
                height: 30.0
            }
        );
        assert_eq!(output.first_baselines.y, Some(17.0));
        assert_eq!(
            allocations, 0,
            "leaf measurement must borrow named grid styles"
        );
    }
}
