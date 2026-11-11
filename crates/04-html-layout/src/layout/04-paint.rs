//! Replay retained commands using finalized geometry, without a layout executor.
use super::decorations::DecorationEmitter;
use super::s03_finalize::{
    FinalizationScratch,
    ordering::{
        build_block_decoration_traversal, build_scene_paint_order,
        rebuild_decoration_fragments_by_line}};
use super::{FragmentWriter, LayoutReader};
use html_engine_model::{BoxGeometry, InlineContent};

pub(super) fn replay(
    reader: &LayoutReader<'_>,
    inline_content: &InlineContent,
    geometry: &BoxGeometry,
    fragments: &mut FragmentWriter<'_>,
    scratch: &mut FinalizationScratch,
) {
    let commands = std::mem::take(&mut fragments.state_mut().paint_program.commands);
    let output = &mut fragments.state_mut().fragment_output;
    output.decorations.clear();
    output.block_decoration_owners.clear();
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
        let layout = fragments.state_mut();
        let owners = &layout.fragment_output.block_decoration_owners;
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
    // Inline owners are needed for clipping during replay; the published
    // owner index covers the block-decoration prefix only.
    let output = &mut fragments.state_mut().fragment_output;
    output
        .block_decoration_owners
        .truncate(output.block_decoration_count as usize);
    fragments.state_mut().paint_program.commands = commands;
}
