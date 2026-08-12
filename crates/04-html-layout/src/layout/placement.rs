use super::fragment_writer::FragmentWriter;
use super::geometry_writer::GeometryWriter;
use kurbo::Vec2;

pub(crate) type PlacementId = u32;

#[derive(Clone, Copy)]
pub(crate) struct BoxPlacement {
    pub(crate) box_group: PlacementId,
    pub(crate) content_group: PlacementId,
    previous_group: PlacementId,
}

impl BoxPlacement {
    pub(crate) fn previous_group(self) -> PlacementId {
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
/// resolves every transform and converts all public output to absolute
/// coordinates in one transition. No writer carries an independent cursor.
#[derive(Default)]
pub(super) struct PlacementState {
    nodes: Vec<PlacementNode>,
    offsets: Vec<Vec2>,
    resolved_offsets: Vec<Vec2>,
    box_groups: Vec<PlacementId>,
    line_groups: Vec<PlacementId>,
    decoration_groups: Vec<PlacementId>,
    current_group: PlacementId,
    active: bool,
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
        self.decoration_groups.clear();
        self.current_group = 0;
        self.active = false;
        self.phase = PlacementPhase::Building;
    }

    pub(crate) fn select(&mut self, group: PlacementId) {
        self.assert_building();
        self.current_group = group;
    }

    pub(crate) fn begin_box(&mut self, box_idx: usize, split_content_group: bool) -> BoxPlacement {
        self.assert_building();
        let previous_group = self.current_group;
        let box_group = self.push_group(previous_group);
        let content_group = if split_content_group {
            self.push_group(box_group)
        } else {
            box_group
        };
        self.box_groups[box_idx] = box_group;
        self.current_group = content_group;
        BoxPlacement {
            box_group,
            content_group,
            previous_group,
        }
    }

    pub(crate) fn translate(&mut self, group: PlacementId, offset: Vec2) {
        self.assert_building();
        if offset == Vec2::ZERO {
            return;
        }
        self.active = true;
        let node = &mut self.nodes[group as usize];
        if node.offset_index == NO_OFFSET {
            node.offset_index =
                u32::try_from(self.offsets.len()).expect("placement offset arena exhausted");
            self.offsets.push(offset);
        } else {
            self.offsets[node.offset_index as usize] += offset;
        }
    }

    pub(crate) fn record_line(&mut self) {
        self.assert_building();
        self.line_groups.push(self.current_group);
    }

    /// Records decorations emitted in local coordinates. Decorations created
    /// after publication (inline decorations derived from absolute lines) need
    /// no retained owner and are deliberately ignored.
    pub(crate) fn record_decorations_since(&mut self, start: usize, end: usize) {
        if self.phase == PlacementPhase::Published {
            return;
        }
        assert!(
            start <= end,
            "decoration cursor must precede emitted output"
        );
        assert!(
            start <= self.decoration_groups.len(),
            "decoration placement ownership must not leave an unowned gap"
        );
        assert!(
            self.decoration_groups.len() <= end,
            "decoration placement ownership cannot extend past emitted output"
        );
        self.decoration_groups.resize(end, self.current_group);
    }

    /// Atomically converts every pre-publication output arena to absolute
    /// coordinates and consumes the unresolved placement lifecycle.
    pub(super) fn publish_absolute(
        &mut self,
        geometry: &mut GeometryWriter<'_>,
        fragments: &mut FragmentWriter<'_>,
    ) {
        self.assert_building();
        assert_eq!(
            self.line_groups.len(),
            fragments.line_len(),
            "every line must retain placement ownership until publication"
        );
        assert_eq!(
            self.decoration_groups.len(),
            fragments.decoration_len(),
            "every pre-publication decoration must retain placement ownership"
        );

        if self.active {
            self.resolve();
            for box_idx in 0..geometry.len() {
                let offset = self.resolved_offsets[self.box_groups[box_idx] as usize];
                if offset != Vec2::ZERO {
                    geometry.set_point(box_idx, geometry.point(box_idx) + offset);
                }
            }
            fragments.materialize_absolute_positions(
                &self.line_groups,
                &self.decoration_groups,
                &self.resolved_offsets,
            );
        }

        self.phase = PlacementPhase::Published;
        self.current_group = 0;
        self.active = false;
        self.nodes.clear();
        self.offsets.clear();
        self.resolved_offsets.clear();
        self.box_groups.clear();
        self.line_groups.clear();
        self.decoration_groups.clear();
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
    use crate::layout::geometry_writer::GeometryWriter;
    use crate::layout_model::{BoxGeometry, LayoutState};

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
    #[should_panic(expected = "placement output has already been published")]
    fn absolute_publication_is_a_one_shot_transition() {
        let mut geometry_output = BoxGeometry::default();
        let mut fragment_output = LayoutState::default();
        let mut geometry = GeometryWriter::new(&mut geometry_output);
        let mut fragments = FragmentWriter::new(&mut fragment_output, Vec::new(), Vec::new());
        let mut placement = PlacementState::default();
        geometry.reset(0);
        fragments.reset();
        placement.reset(0);

        placement.publish_absolute(&mut geometry, &mut fragments);
        placement.publish_absolute(&mut geometry, &mut fragments);
    }
}
