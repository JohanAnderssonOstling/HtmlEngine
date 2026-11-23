use html_engine_model::{BoxGeometry, OverflowClip};
use kurbo::Rect;
use std::ops::Range;

use super::{FragmentWriter, LayoutReader};

/// Resolves overflow ownership once layout geometry is final. The common
/// `overflow: visible` path leaves both output vectors empty; renderers can
/// therefore test one slice and continue without issuing clip commands.
pub(super) fn rebuild_overflow_clips(
    reader: &LayoutReader<'_>,
    geometry: &BoxGeometry,
    fragments: &mut FragmentWriter<'_>,
) {
    let layout = fragments.state_mut();
    let content_clips = &mut layout.paint_program.content_clips;
    let output = &mut layout.line_output;
    if !reader.tracks_overflow_clips() {
        content_clips.clear();
        output.line_clips.clear();
        return;
    }

    // Finalization validates parent-before-child order before these passes.
    let box_count = reader.box_count();
    content_clips.resize(box_count, None);
    for idx in 0..box_count {
        let parent_clip = reader
            .get_parent(idx)
            .and_then(|parent| content_clips[parent]);
        let style = reader.style(idx);
        let (overflow_x, overflow_y) = reader.box_facts(idx).overflow;
        let clip_x = overflow_x.clips();
        let clip_y = overflow_y.clips();
        let own_clip = (clip_x || clip_y).then(|| {
            let point = geometry.point(idx);
            let size = geometry.size(idx);
            let left = style.border_left_width() as f64;
            let top = style.border_top_width() as f64;
            let right = style.border_right_width() as f64;
            let bottom = style.border_bottom_width() as f64;
            OverflowClip {
                rect: Rect::new(
                    point.x + left,
                    point.y + top,
                    (point.x + size.width - right).max(point.x + left),
                    (point.y + size.height - bottom).max(point.y + top),
                ),
                x: clip_x,
                y: clip_y,
            }
        });
        content_clips[idx] = combine(parent_clip, own_clip);
    }

    output.line_clips.clear();
    output.line_clips.extend(output.lines.iter().map(|line| {
        content_clips
            .get(line.owner_box_idx as usize)
            .copied()
            .flatten()
    }));
}

fn combine(ancestor: Option<OverflowClip>, own: Option<OverflowClip>) -> Option<OverflowClip> {
    match (ancestor, own) {
        (Some(a), Some(b)) => {
            let x = combine_axis(a.rect.x0..a.rect.x1, b.rect.x0..b.rect.x1, a.x, b.x);
            let y = combine_axis(a.rect.y0..a.rect.y1, b.rect.y0..b.rect.y1, a.y, b.y);
            Some(OverflowClip {
                rect: Rect::new(x.start, y.start, x.end.max(x.start), y.end.max(y.start)),
                x: a.x || b.x,
                y: a.y || b.y,
            })
        }
        (Some(clip), None) | (None, Some(clip)) => Some(clip),
        (None, None) => None,
    }
}

fn combine_axis(a: Range<f64>, b: Range<f64>, clip_a: bool, clip_b: bool) -> Range<f64> {
    match (clip_a, clip_b) {
        (true, true) => a.start.max(b.start)..a.end.min(b.end),
        (true, false) => a,
        (false, _) => b,
    }
}
