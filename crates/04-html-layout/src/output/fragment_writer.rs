use html_engine_model::paint_program::PaintLayers;
use html_engine_model::{DecorationStore, LayoutState, LineInlineBoxFragment, OverflowClip};
use kurbo::{Point, Vec2};
use std::ops::Range;

use super::placement::{BoxPlacement, PlacementState};

#[derive(Clone, Copy)]
pub struct OutputCursor {
    lines: usize,
    images: usize,
    paint_commands: usize,
}

#[derive(Clone)]
pub struct OutputRanges {
    pub placement: BoxPlacement,
    pub lines: Range<usize>,
    pub images: Range<usize>,
    pub paint_commands: Range<usize>,
}

impl OutputRanges {
    pub fn has_lines_or_images(&self) -> bool {
        !self.lines.is_empty() || !self.images.is_empty()
    }
}

/// Metadata committed with a complete decoration batch.
pub(crate) struct DecorationMetadata {
    pub block_owner: Option<u32>,
    pub line: u32,
    pub paint_order: u32,
    pub layers: PaintLayers,
    pub clip: Option<Option<OverflowClip>>,
}

/// Writes published line, fragment and decoration-owner output.
pub struct FragmentWriter<'out> {
    state: &'out mut LayoutState,
    last_inline_fragments: Vec<u32>,
}

impl<'out> FragmentWriter<'out> {
    pub(crate) fn new(state: &'out mut LayoutState) -> Self {
        Self {
            state,
            last_inline_fragments: Vec::new(),
        }
    }

    /// Resets all published fragment state while retaining reusable allocation.
    pub fn reset(&mut self) {
        self.last_inline_fragments.clear();

        self.state.clear();
    }

    pub fn state(&self) -> &LayoutState {
        self.state
    }

    pub fn state_mut(&mut self) -> &mut LayoutState {
        self.state
    }

    pub fn output_cursor(&self) -> OutputCursor {
        OutputCursor {
            lines: self.state.line_output.lines.len(),
            images: self.state.fragment_output.image_fragments.len(),
            paint_commands: self.state.paint_program.commands.len(),
        }
    }

    pub fn output_since(&self, cursor: OutputCursor, placement: BoxPlacement) -> OutputRanges {
        OutputRanges {
            placement,
            lines: cursor.lines..self.state.line_output.lines.len(),
            images: cursor.images..self.state.fragment_output.image_fragments.len(),
            paint_commands: cursor.paint_commands..self.state.paint_program.commands.len(),
        }
    }

    pub fn record_paint(
        &mut self,
        placement: &PlacementState,
        operation: crate::paint_program::PaintOperation,
    ) {
        if placement.retains_output() {
            self.state
                .paint_program
                .record(operation, placement.current_group());
        }
    }

    pub fn mark_positioned_layer(
        &mut self,
        output: &OutputRanges,
        negative: bool,
        independent: bool,
    ) {
        let layers = PaintLayers::positioned(negative, independent);
        for command in &mut self.state.paint_program.commands[output.paint_commands.clone()] {
            command.layers.include(layers);
        }
        let lines = &mut self.state.line_output;
        lines
            .paint_layers
            .resize(lines.lines.len(), PaintLayers::default());
        // Ancestors add their layer without erasing a nested descendant's
        // negative or independent positioned layer.
        for line in &mut lines.paint_layers[output.lines.clone()] {
            line.include(layers);
        }
    }

    pub fn baseline_offsets(
        &self,
        lines: Range<usize>,
        origin_y: f64,
    ) -> (Option<f64>, Option<f64>) {
        let mut first: Option<(Point, f64)> = None;
        let mut last: Option<(Point, f64)> = None;
        for line in &self.state.line_output.lines[lines] {
            let point = line.point;
            if point.y >= origin_y - 0.01 {
                if first.is_none_or(|(current, _)| point.y < current.y) {
                    first = Some((point, line.baseline));
                }
                if last.is_none_or(|(current, _)| point.y > current.y) {
                    last = Some((point, line.baseline));
                }
            }
        }
        let offset = |(point, baseline): (Point, f64)| point.y + baseline - origin_y;
        (first.map(offset), last.map(offset))
    }

    pub fn line_point(&self, line_idx: usize) -> Point {
        self.state.line_output.lines[line_idx].point
    }

    pub fn lines_before_y(&self, lines: Range<usize>, y: f64) -> usize {
        lines
            .filter(|&line_idx| self.line_point(line_idx).y < y)
            .count()
    }

    pub(crate) fn line_len(&self) -> usize {
        self.state.line_output.lines.len()
    }

    pub(crate) fn materialize_line_positions(
        &mut self,
        line_groups: &[u32],
        resolved_offsets: &[Vec2],
    ) {
        for (index, line) in self.state.line_output.lines.iter_mut().enumerate() {
            let offset = resolved_offsets[line_groups[index] as usize];
            if offset != Vec2::ZERO {
                line.point += offset;
            }
        }
    }

    /// Publishes one line's inline fragments and links continuations without
    /// adding per-line allocations.
    pub fn push_inline_box_fragments(
        &mut self,
        fragments: impl IntoIterator<Item = LineInlineBoxFragment>,
        box_count: usize,
    ) -> Range<u32> {
        const NONE: u32 = u32::MAX;
        self.last_inline_fragments.resize(box_count, NONE);
        let start = u32::try_from(self.state.line_output.inline_box_fragments.len())
            .expect("inline fragment arena exhausted");
        for mut fragment in fragments {
            let previous = self.last_inline_fragments[fragment.box_idx as usize];
            if previous != NONE
                && (previous as usize) < self.state.line_output.inline_box_fragments.len()
            {
                self.state.line_output.inline_box_fragments[previous as usize].flags &=
                    !LineInlineBoxFragment::INLINE_END;
                fragment.flags &= !LineInlineBoxFragment::INLINE_START;
            }
            let current = u32::try_from(self.state.line_output.inline_box_fragments.len())
                .expect("inline fragment arena exhausted");
            self.last_inline_fragments[fragment.box_idx as usize] = current;
            self.state.line_output.inline_box_fragments.push(fragment);
        }
        start
            ..u32::try_from(self.state.line_output.inline_box_fragments.len())
                .expect("inline fragment arena exhausted")
    }

    pub(crate) fn decoration_len(&self) -> usize {
        self.state.fragment_output.decorations.len()
    }

    pub(crate) fn reset_decorations(&mut self) {
        let output = &mut self.state.fragment_output;
        output.decorations.clear();
        output.block_decoration_owners.clear();
        output.decoration_line_indices.clear();
        output.decoration_paint_orders.clear();
        output.decoration_paint_layers.clear();
        output.decoration_clips.clear();
        output.decoration_fragments_by_line.clear();
    }

    /// Emit primitives and their metadata together. Block ownership remains a
    /// block-only prefix; optional clips cover every primitive when enabled.
    pub(crate) fn write_decorations(
        &mut self,
        metadata: DecorationMetadata,
        emit: impl FnOnce(&mut DecorationStore),
    ) {
        let output = &mut self.state.fragment_output;
        let start = output.decorations.len();
        emit(&mut output.decorations);
        for decoration in &output.decorations.fragments()[start..] {
            if let Some(owner) = metadata.block_owner {
                output.block_decoration_owners.push(owner);
            }
            output.decoration_line_indices.push(metadata.line);
            output.decoration_paint_orders.push(metadata.paint_order);
            output
                .decoration_paint_layers
                .push(if metadata.block_owner.is_some() {
                    metadata.layers.for_decoration(decoration.is_foreground())
                } else {
                    metadata.layers
                });
            if let Some(clip) = metadata.clip {
                output.decoration_clips.push(clip);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use html_engine_model::paint_program::PaintOperation;
    use html_engine_model::{DecorationFragment, Line};
    use kurbo::Rect;

    #[test]
    fn nested_layers_survive_command_replay_and_late_inline_decorations() {
        let mut state = LayoutState::default();
        let mut fragments = FragmentWriter::new(&mut state);
        let mut placement = PlacementState::default();
        placement.reset(1);
        let owner = placement.begin_box(0, false);
        let cursor = fragments.output_cursor();
        fragments
            .state_mut()
            .line_output
            .lines
            .push(Line::default());
        fragments.record_paint(
            &placement,
            PaintOperation::Background {
                owner: 0,
                rect: Rect::ZERO,
            },
        );
        let output = fragments.output_since(cursor, owner);
        fragments.mark_positioned_layer(&output, true, true);
        fragments.mark_positioned_layer(&output, false, false);
        let layers = fragments.state().line_output.paint_layers[0];
        assert!(layers.is_positioned() && layers.is_negative() && layers.is_independent());
        assert_eq!(layers, fragments.state().paint_program.commands[0].layers);
        assert_eq!(size_of::<PaintLayers>(), 1);

        fragments.write_decorations(
            DecorationMetadata {
                block_owner: Some(0),
                line: u32::MAX,
                paint_order: u32::MAX,
                layers,
                clip: None,
            },
            |decorations| {
                decorations.push(DecorationFragment::background_rect(Rect::ZERO, 0, false));
                decorations.push(DecorationFragment::foreground_rect(Rect::ZERO, 0, false));
            },
        );
        let decorations = &fragments.state().fragment_output.decoration_paint_layers;
        assert_eq!(decorations[0], layers);
        assert!(decorations[1].is_positioned() && decorations[1].is_negative());
        assert!(!decorations[1].is_independent());

        fragments.write_decorations(
            DecorationMetadata {
                block_owner: None,
                line: 0,
                paint_order: 0,
                layers,
                clip: None,
            },
            |decorations| {
                decorations.push(DecorationFragment::background_rect(Rect::ZERO, 0, true));
            },
        );
        assert_eq!(
            fragments.state().fragment_output.decoration_paint_layers[2],
            layers
        );
    }

    #[test]
    fn baseline_endpoints_keep_first_ties_and_ignore_lines_above_the_origin() {
        let mut state = LayoutState::default();
        for (y, baseline) in [
            (-1.0, 9.0),
            (0.0, 3.0),
            (0.0, 8.0),
            (10.0, 2.0),
            (f64::NAN, 100.0),
        ] {
            state.line_output.lines.push(Line {
                point: Point::new(0.0, y),
                baseline,
                ..Line::default()
            });
        }
        let fragments = FragmentWriter::new(&mut state);
        assert_eq!(
            fragments.baseline_offsets(0..5, 0.0),
            (Some(3.0), Some(12.0))
        );
        assert_eq!(
            fragments.baseline_offsets(2..4, 0.0),
            (Some(8.0), Some(12.0))
        );
        assert_eq!(
            fragments.baseline_offsets(0..5, 5.0),
            (Some(7.0), Some(7.0))
        );
        assert_eq!(fragments.baseline_offsets(0..5, 100.0), (None, None));
    }
}
