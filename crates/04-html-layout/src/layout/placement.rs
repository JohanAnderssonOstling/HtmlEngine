use super::fragment_writer::FragmentWriter;
use crate::layout_model::BoxGeometry;
use kurbo::Vec2;

pub type PlacementId = u32;

#[derive(Clone, Copy)]
pub struct BoxPlacement {
    pub box_group: PlacementId,
    pub content_group: PlacementId,
    previous_group: PlacementId,
}

impl BoxPlacement {
    pub fn previous_group(self) -> PlacementId {
        self.previous_group
    }
}

const NO_OFFSET: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct PlacementNode {
    parent: PlacementId,
    offset_index: u32,
}

impl Default for PlacementNode {
    fn default() -> Self {
        Self {
            parent: 0,
            offset_index: NO_OFFSET,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PlacementPhase {
    #[default]
    Building,
    Published,
}

/// Session-owned retained transform tree and fragment ownership.
///
/// Layout writes local coordinates while this state is `Building`. Publication
/// resolves every transform and converts the canonical output coordinates in
/// place. No second local-coordinate snapshot is retained.
#[derive(Default)]
pub struct PlacementState {
    nodes: Vec<PlacementNode>,
    offsets: Vec<Vec2>,
    resolved_offsets: Vec<Vec2>,
    box_groups: Vec<PlacementId>,
    line_groups: Vec<PlacementId>,
    current_group: PlacementId,
    has_transforms: bool,
    retain_output: bool,
    phase: PlacementPhase,
}

impl PlacementState {
    pub(super) fn reset(&mut self, box_count: usize) {
        self.nodes.clear();
        self.nodes.push(PlacementNode::default());
        self.offsets.clear();
        self.resolved_offsets.clear();
        self.box_groups.resize(box_count, 0);
        self.box_groups.fill(0);
        self.line_groups.clear();
        self.current_group = 0;
        self.has_transforms = false;
        self.retain_output = true;
        self.phase = PlacementPhase::Building;
    }

    pub(super) fn reset_for_measurement(&mut self) {
        self.nodes.clear();
        self.nodes.push(PlacementNode::default());
        self.offsets.clear();
        self.resolved_offsets.clear();
        self.box_groups.clear();
        self.line_groups.clear();
        self.current_group = 0;
        self.has_transforms = false;
        self.retain_output = false;
        self.phase = PlacementPhase::Building;
    }

    pub fn current_group(&self) -> PlacementId { self.current_group }
    pub fn retains_output(&self) -> bool { self.retain_output }

    pub fn select(&mut self, group: PlacementId) {
        self.assert_building();
        self.current_group = group;
    }

    pub fn begin_box(&mut self, box_idx: usize, split_content_group: bool) -> BoxPlacement {
        self.assert_building();
        let previous_group = self.current_group;
        let box_group = self.push_group(previous_group);
        let content_group = if split_content_group {
            self.push_group(box_group)
        } else {
            box_group
        };
        if self.retain_output {
            self.box_groups[box_idx] = box_group;
        }
        self.current_group = content_group;
        BoxPlacement {
            box_group,
            content_group,
            previous_group,
        }
    }

    pub fn translate(&mut self, group: PlacementId, offset: Vec2) {
        self.assert_building();
        if offset == Vec2::ZERO {
            return;
        }
        self.has_transforms = true;
        let node = &mut self.nodes[group as usize];
        if node.offset_index == NO_OFFSET {
            node.offset_index =
                u32::try_from(self.offsets.len()).expect("placement offset arena exhausted");
            self.offsets.push(offset);
        } else {
            self.offsets[node.offset_index as usize] += offset;
        }
    }

    pub fn record_line(&mut self) {
        self.assert_building();
        if self.retain_output {
            self.line_groups.push(self.current_group);
        }
    }

    /// Publishes absolute coordinates directly in the owning output buffers.
    /// Borrowed readers observe this single representation after publication.
    pub(super) fn publish_absolute(
        &mut self,
        geometry: &mut BoxGeometry,
        fragments: &mut FragmentWriter<'_>,
    ) {
        self.assert_building();
        assert_eq!(
            self.line_groups.len(),
            fragments.line_len(),
            "every line must retain placement ownership until publication"
        );
        assert_eq!(
            fragments.decoration_len(),
            0,
            "paint runs only after geometry publication"
        );

        if self.has_transforms {
            self.resolve();
            for box_idx in 0..geometry.len() {
                let offset = self.resolved_offsets[self.box_groups[box_idx] as usize];
                if offset != Vec2::ZERO {
                    geometry.set_point(box_idx, geometry.point(box_idx) + offset);
                }
            }
            fragments.materialize_line_positions(&self.line_groups, &self.resolved_offsets);
        }

        fragments
            .state_mut()
            .paint_program
            .publish(&self.resolved_offsets);
        self.phase = PlacementPhase::Published;
        self.current_group = 0;
    }

    pub fn memory_usage_bytes(&self) -> usize {
        self.nodes.capacity() * std::mem::size_of::<PlacementNode>()
            + self.offsets.capacity() * std::mem::size_of::<Vec2>()
            + self.resolved_offsets.capacity() * std::mem::size_of::<Vec2>()
            + self.box_groups.capacity() * std::mem::size_of::<PlacementId>()
            + self.line_groups.capacity() * std::mem::size_of::<PlacementId>()
    }

    fn assert_building(&self) {
        assert_eq!(
            self.phase,
            PlacementPhase::Building,
            "placement output has already been published"
        );
    }

    fn push_group(&mut self, parent: PlacementId) -> PlacementId {
        let id = u32::try_from(self.nodes.len()).expect("placement group arena exhausted");
        self.nodes.push(PlacementNode {
            parent,
            offset_index: NO_OFFSET,
        });
        id
    }

    fn resolve(&mut self) {
        self.resolved_offsets.resize(self.nodes.len(), Vec2::ZERO);
        self.resolved_offsets[0] = Vec2::ZERO;
        for index in 1..self.nodes.len() {
            let node = self.nodes[index];
            let local = if node.offset_index == NO_OFFSET {
                Vec2::ZERO
            } else {
                self.offsets[node.offset_index as usize]
            };
            self.resolved_offsets[index] = self.resolved_offsets[node.parent as usize] + local;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PlacementState;
    use crate::layout::fragment_writer::FragmentWriter;
    use crate::layout_model::{BoxGeometry, LayoutState};
    use kurbo::{Point, Vec2};

    #[test]
    fn content_groups_are_allocated_only_for_split_boxes() {
        let mut placement = PlacementState::default();
        placement.reset(2);

        let ordinary = placement.begin_box(0, false);
        assert_eq!(ordinary.box_group, ordinary.content_group);
        assert_eq!(placement.nodes.len(), 2, "root plus one ordinary box group");

        placement.select(ordinary.previous_group());
        let split = placement.begin_box(1, true);
        assert_ne!(split.box_group, split.content_group);
        assert_eq!(
            placement.nodes.len(),
            4,
            "a split box adds separate border and content groups"
        );
    }

    #[test]
    fn absolute_publication_updates_the_owning_box_coordinates() {
        let mut geometry_output = BoxGeometry::default();
        let mut fragment_output = LayoutState::default();
        let mut fragments = FragmentWriter::new(&mut fragment_output, Vec::new());
        let mut placement = PlacementState::default();
        geometry_output.reset(1);
        fragments.reset();
        placement.reset(1);

        geometry_output.set_point(0, Point::new(10.0, 20.0));
        let box_placement = placement.begin_box(0, false);
        placement.translate(box_placement.box_group, Vec2::new(5.0, 7.0));
        placement.publish_absolute(&mut geometry_output, &mut fragments);

        assert_eq!(geometry_output.point(0), Point::new(15.0, 27.0));
    }

    #[test]
    #[should_panic(expected = "placement output has already been published")]
    fn absolute_publication_is_a_one_shot_transition() {
        let mut geometry_output = BoxGeometry::default();
        let mut fragment_output = LayoutState::default();
        let mut fragments = FragmentWriter::new(&mut fragment_output, Vec::new());
        let mut placement = PlacementState::default();
        geometry_output.reset(0);
        fragments.reset();
        placement.reset(0);

        placement.publish_absolute(&mut geometry_output, &mut fragments);
        placement.publish_absolute(&mut geometry_output, &mut fragments);
    }
}
