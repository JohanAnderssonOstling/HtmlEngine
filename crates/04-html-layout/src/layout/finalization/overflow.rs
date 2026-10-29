use crate::layout_model::{BoxGeometry, OverflowClip};
use kurbo::Rect;

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

    fn combine(ancestor: Option<OverflowClip>, own: Option<OverflowClip>) -> Option<OverflowClip> {
        match (ancestor, own) {
            (Some(a), Some(b)) => {
                let x0 = if a.x && b.x {
                    a.rect.x0.max(b.rect.x0)
                } else if a.x {
                    a.rect.x0
                } else {
                    b.rect.x0
                };
                let x1 = if a.x && b.x {
                    a.rect.x1.min(b.rect.x1)
                } else if a.x {
                    a.rect.x1
                } else {
                    b.rect.x1
                };
                let y0 = if a.y && b.y {
                    a.rect.y0.max(b.rect.y0)
                } else if a.y {
                    a.rect.y0
                } else {
                    b.rect.y0
                };
                let y1 = if a.y && b.y {
                    a.rect.y1.min(b.rect.y1)
                } else if a.y {
                    a.rect.y1
                } else {
                    b.rect.y1
                };
                Some(OverflowClip {
                    rect: Rect::new(x0, y0, x1.max(x0), y1.max(y0)),
                    x: a.x || b.x,
                    y: a.y || b.y,
                })
            }
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    // Boxes are built in DOM preorder, so an owning parent's resolved clip
    // is available before every child. This is also the ordering used for
    // decoration paint keys.
    let box_count = reader.box_count();
    content_clips.resize(box_count, None);
    for idx in 0..box_count {
        let parent_clip = reader.get_parent(idx).and_then(|parent| {
            debug_assert!(
                parent < idx,
                "layout boxes must be stored in parent-before-child order"
            );
            content_clips.get(parent).copied().flatten()
        });
        let style = reader.style(idx);
        let (overflow_x, overflow_y) = reader.effective_overflow_modes(idx);
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
