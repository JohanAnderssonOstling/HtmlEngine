//! Resolve paint output from retained commands and finalized layout geometry.
//! Layout records bounds and ownership; paint materializes appearance without
//! a layout executor. Palette-only updates reuse the same replay path.

mod block;
mod reader;
pub(crate) use reader::PaintReader;
mod borders;
mod decorations;
mod inline;
mod ordering;
mod text;

use crate::engine::StyleReader;
use crate::{FragmentWriter, LayoutInputs};
use decorations::DecorationEmitter;
use html_engine_model::{InlineContent, LayoutState};
use rustc_data_structures::fx::FxHashMap;
pub(crate) use ordering::{PaintItem, PaintOrderCache};
use ordering::{
    build_block_decoration_traversal, build_scene_paint_order, rebuild_decoration_fragments_by_line,
};
pub use text::prepare_text_paint;

/// Paint-only updates retain all box and line geometry.
pub fn repaint(inputs: LayoutInputs<'_>, state: &mut LayoutState) {
    let mut resolved = FxHashMap::default();
    let styles: Vec<_> = (0..inputs.topology.box_count())
        .map(|box_idx| {
            let indices = inputs
                .topology
                .get_box_style_indices(box_idx)
                .unwrap_or_else(|| inputs.styles.default_indices());
            *resolved
                .entry((indices, inputs.font_metrics.box_metric_id(box_idx)))
                .or_insert_with(|| {
                    inputs
                        .font_metrics
                        .used_style(inputs.styles, indices, box_idx)
                        .expect("validated style handle")
                })
        })
        .collect();
    let reader = PaintReader {
        shared: StyleReader::new(&inputs, &styles),
        paint: inputs.paint,
    };
    let mut fragments = FragmentWriter::new(state);
    replay(
        &reader,
        inputs.inline_content,
        &mut fragments,
        &mut PaintOrderCache::default(),
    );
}

pub(crate) fn replay(
    reader: &PaintReader<'_>,
    inline_content: &InlineContent,
    fragments: &mut FragmentWriter<'_>,
    cache: &mut PaintOrderCache,
) {
    // Commands emit block primitives and complete metadata first. Their traversal
    // establishes the block prefix; inline emission follows in line order. The
    // final indexes group line-owned primitives and merge the complete scene.
    let commands = std::mem::take(&mut fragments.state_mut().paint_program.commands);
    fragments.reset_decorations();
    let mut emitter = DecorationEmitter {
        reader,
        inline_content,
        fragments,
    };
    for &command in &commands {
        emitter.replay_command(command);
    }
    build_block_decoration_traversal(reader, emitter.fragments, cache);
    emitter.collect_inline_decorations();
    rebuild_decoration_fragments_by_line(fragments);
    build_scene_paint_order(reader, fragments, cache);
    fragments.state_mut().paint_program.commands = commands;
    cache.clear();
}

#[cfg(test)]
include!("tests.rs");
