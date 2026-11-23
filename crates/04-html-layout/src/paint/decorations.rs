//! Materialize retained commands using finalized geometry and paint styles.
use super::PaintReader;
use super::block::{emit_block_background, emit_block_border_and_outline};
use super::borders::emit_border_fragments;
use crate::FragmentWriter;
use crate::output::DecorationMetadata;
use crate::paint_program::{PaintCommand, PaintLayers, PaintOperation, PaintStyleSource};
use html_engine_model::{
    DecorationFragment, DecorationStore, InlineContent, OverflowClip, RoundedDecoration,
};
use html_style_model::{CornerRadii, computed_style};
use kurbo::Rect;

/// Paint reads finalized geometry and writes decoration output only.
pub(super) struct DecorationEmitter<'scope, 'input, 'output> {
    pub(super) reader: &'scope PaintReader<'input>,
    pub(super) inline_content: &'input InlineContent,
    pub(super) fragments: &'scope mut FragmentWriter<'output>,
}

pub(super) fn emit_solid_border(
    fragments: &mut DecorationStore,
    rect: Rect,
    radii: CornerRadii,
    uniform_border: Option<(f64, u32)>,
    borders: [Option<(f64, u32)>; 4],
    is_inline: bool,
) {
    if let Some((width, color)) =
        uniform_border.filter(|_| !computed_style::corner_radii_is_zero(radii))
    {
        fragments.push_rounded_border(
            rect,
            color,
            is_inline,
            RoundedDecoration {
                radii,
                border_width: Some(width as f32),
            },
        );
    } else {
        emit_border_fragments(fragments, rect, borders, is_inline);
    }
}

pub(super) fn push_background(
    fragments: &mut DecorationStore,
    rect: Rect,
    color: u32,
    is_inline: bool,
    radii: CornerRadii,
) {
    if !computed_style::corner_radii_is_zero(radii) {
        let rounded = RoundedDecoration {
            radii,
            border_width: None,
        };
        fragments.push_rounded_background(rect, color, is_inline, rounded);
    } else {
        fragments.push(DecorationFragment::background_rect(rect, color, is_inline));
    }
}
impl DecorationEmitter<'_, '_, '_> {
    pub(super) fn clip(&self, owner: u32) -> Option<Option<OverflowClip>> {
        self.reader.tracks_overflow_clips().then(|| {
            self.reader.get_parent(owner as usize).and_then(|parent| {
                self.fragments
                    .state()
                    .paint_program
                    .content_clips
                    .get(parent)
                    .copied()
                    .flatten()
            })
        })
    }

    pub(super) fn replay_command(&mut self, mut command: PaintCommand) {
        let owner = match command.operation {
            PaintOperation::Background { owner, .. }
            | PaintOperation::BorderOutline { owner, .. }
            | PaintOperation::BackgroundRect { owner, .. } => owner,
            PaintOperation::CollapsedBorder { fragment_owner, .. } => {
                command.layers.include(PaintLayers::collapsed_border());
                fragment_owner
            }
        };
        let metadata = DecorationMetadata {
            block_owner: Some(owner),
            line: command.line,
            paint_order: command.paint_order,
            layers: command.layers,
            clip: self.clip(owner),
        };
        let reader = self.reader;
        let resolve_style = |source| match source {
            PaintStyleSource::Box(index) => reader.paint_style(index as usize),
            PaintStyleSource::Style { indices, context } => {
                reader.paint_used_style(indices, context as usize)
            }
        };
        self.fragments
            .write_decorations(metadata, |fragments| match command.operation {
                PaintOperation::Background { owner, rect } => {
                    emit_block_background(reader, fragments, owner as usize, rect)
                }
                PaintOperation::BorderOutline { owner, rect } => {
                    emit_block_border_and_outline(reader, fragments, owner as usize, rect)
                }
                PaintOperation::BackgroundRect {
                    owner,
                    source,
                    rect,
                } => {
                    let color = resolve_style(source).background_color();
                    if color & 0xff != 0
                        && reader.paint_style(owner as usize).visibility()
                            != html_style_model::Visibility::Hidden
                    {
                        push_background(fragments, rect, color, false, CornerRadii::default());
                    }
                }
                PaintOperation::CollapsedBorder {
                    source,
                    side,
                    pattern,
                    rect,
                    ..
                } => {
                    let style = resolve_style(source);
                    let color = [
                        style.border_top_color(),
                        style.border_right_color(),
                        style.border_bottom_color(),
                        style.border_left_color(),
                    ][side as usize];
                    if color & 0xff != 0 {
                        fragments.push(DecorationFragment::patterned(
                            rect, color, false, false, pattern,
                        ));
                    }
                }
            });
    }
}
