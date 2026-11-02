//! Replay retained commands using finalized geometry, without a layout executor.
use super::decorations::DecorationEmitter;
use super::finalization::{
    FinalizationScratch,
    ordering::{
        build_block_decoration_traversal, build_scene_paint_order,
        rebuild_decoration_fragments_by_line,
    },
};
use super::{FragmentWriter, LayoutReader};
use crate::layout_model::{BoxGeometry, InlineContent};

pub(super) fn replay(
    reader: &LayoutReader<'_>,
    inline_content: &InlineContent,
    geometry: &BoxGeometry,
    fragments: &mut FragmentWriter<'_>,
    scratch: &mut FinalizationScratch,
) {
    profile_scope!("layout.paint_replay");
    let commands = std::mem::take(&mut fragments.state_mut().paint_program.commands);
    let output = &mut fragments.state_mut().fragment_output;
    output.decorations.clear();
    output.decoration_line_indices.clear();
    output.decoration_paint_orders.clear();
    output.decoration_paint_layers.clear();
    output.decoration_clips.clear();
    output.decoration_fragments_by_line.clear();
    let mut emitter = DecorationEmitter {
        reader,
        inline_content,
        geometry,
        fragments,
        retained_rect: None,
    };
    for &command in &commands {
        emitter.replay_command(command);
    }
    build_block_decoration_traversal(reader, emitter.fragments, scratch);
    emitter.collect_inline_decorations();
    rebuild_decoration_fragments_by_line(fragments);
    build_scene_paint_order(reader, fragments);
    if reader.tracks_overflow_clips() {
        let (layout, owners) = fragments.decoration_parts();
        layout
            .fragment_output
            .decoration_clips
            .extend(owners.iter().map(|&owner| {
                reader.get_parent(owner as usize).and_then(|parent| {
                    layout
                        .paint_program
                        .content_clips
                        .get(parent)
                        .copied()
                        .flatten()
                })
            }));
    }
    fragments.state_mut().paint_program.commands = commands;
}
