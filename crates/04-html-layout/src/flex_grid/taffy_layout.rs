use super::measurement::{MeasuredItem, MeasurementMode};
use super::{SolvedFlexGrid, TaffyContainerKind, finite_f32, nonnegative_size};
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
/// Intrinsic probes and final solves share this backend. An explicit measurement
/// mode selects renderer sizing; the adapter translates sizes and baselines
/// into Taffy's low-level tree contract. It retains only root children and the
/// root baseline because renderer-owned items are opaque leaves here.
pub(super) struct TaffyLayoutTree<'session, 'input, 'out> {
    measurement: MeasurementMode<'session, 'input, 'out>,
    kind: TaffyContainerKind,
    nodes: Vec<Node>,
    root_first_baseline: Option<f32>,
}

impl<'session, 'input, 'out> TaffyLayoutTree<'session, 'input, 'out> {
    pub(super) fn solve(
        measurement: MeasurementMode<'session, 'input, 'out>,
        kind: TaffyContainerKind,
        root_style: Style,
        child_styles: Vec<(Style, Option<u32>)>,
        available: Size<AvailableSpace>,
    ) -> Self {
        let nodes: Vec<_> = std::iter::once((root_style, None))
            .chain(child_styles)
            .map(|(style, context)| Node {
                style,
                context,
                layout: Layout::new(),
            })
            .collect();
        let mut tree = Self {
            measurement,
            kind,
            nodes,
            root_first_baseline: None,
        };
        tree.compute(available);
        tree
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

    pub(super) fn into_result(self) -> SolvedFlexGrid {
        SolvedFlexGrid {
            first_baseline: self.root_first_baseline.map(f64::from),
            size: nonnegative_size(self.root_layout().size),
            items: self
                .nodes
                .into_iter()
                .filter_map(|node| node.context.map(|box_idx| (node.layout, box_idx)))
                .collect(),
        }
    }
}

impl Node {
    fn layout_leaf(
        &self,
        inputs: LayoutInput,
        mut measure: impl FnMut(usize, Size<Option<f32>>, Size<AvailableSpace>) -> MeasuredItem,
    ) -> LayoutOutput {
        if self.style.display == taffy::style::Display::None {
            return LayoutOutput::HIDDEN;
        }

        let mut first_baseline = None;
        let mut output = compute_leaf_layout(
            inputs,
            &self.style,
            |_, _| 0.0,
            |known, available| {
                let Some(box_idx) = self.context else {
                    return Size::ZERO;
                };
                let measured = measure(box_idx as usize, known, available);
                first_baseline = measured.first_baseline;
                measured.size
            },
        );
        output.first_baselines.y = first_baseline.map(finite_f32);
        output
    }
}

impl TraversePartialTree for TaffyLayoutTree<'_, '_, '_> {
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

impl LayoutPartialTree for TaffyLayoutTree<'_, '_, '_> {
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
        let output = if index == 0 && self.nodes.len() > 1 {
            match self.kind {
                TaffyContainerKind::Flex => compute_flexbox_layout(self, node_id, inputs),
                TaffyContainerKind::Grid => compute_grid_layout(self, node_id, inputs),
            }
        } else {
            self.nodes[index].layout_leaf(inputs, |owner, known, available| {
                self.measurement.measure(owner, known, available)
            })
        };
        if index == 0 {
            self.root_first_baseline = output.first_baselines.y;
        }
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
        self.get_core_container_style(node_id)
    }

    fn get_flexbox_child_style(&self, child_node_id: NodeId) -> Self::FlexboxItemStyle<'_> {
        self.get_core_container_style(child_node_id)
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
        self.get_core_container_style(node_id)
    }

    fn get_grid_child_style(&self, child_node_id: NodeId) -> Self::GridItemStyle<'_> {
        self.get_core_container_style(child_node_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocation_test_support::count_allocations;
    use crate::flex_grid::measurement::MeasuredItem;
    use taffy::RunMode;

    #[test]
    fn leaf_measurement_borrows_the_style_without_allocating() {
        let leaf_style = Style {
            grid_template_column_names: vec![vec!["column".to_owned()]],
            grid_template_row_names: vec![vec!["row".to_owned()]],
            ..Style::default()
        };
        let leaf = Node {
            style: leaf_style,
            context: Some(7),
            layout: Layout::new(),
        };
        let (output, allocations) = count_allocations(|| {
            leaf.layout_leaf(
                LayoutInput {
                    run_mode: RunMode::ComputeSize,
                    ..LayoutInput::HIDDEN
                },
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
