//! GPUI focus and gestures backed by the engine's logical selection and geometry.

use gpui::{FocusHandle, Pixels, Point};
use html::geometry::{Geometry, WordHitPolicy};
use html::layout::RenderView;
use html::selection::{SelectionMode, SelectionState};
use kurbo::{Point as DocumentPoint, Rect};

pub(crate) struct FragmentSelection {
    pub focus: FocusHandle,
    logical: SelectionState,
}

fn document_point(point: Point<Pixels>) -> DocumentPoint {
    DocumentPoint::new(f64::from(f32::from(point.x)), f64::from(f32::from(point.y)))
}

impl FragmentSelection {
    pub fn new(focus: FocusHandle) -> Self {
        Self {
            focus,
            logical: SelectionState::default(),
        }
    }

    pub fn clear(&mut self) {
        self.logical = SelectionState::default();
    }
    pub fn is_empty(&self) -> bool {
        self.logical.range().is_none()
    }
    pub fn dragging(&self) -> bool {
        self.logical.interaction().is_selecting()
    }
    pub fn finish(&mut self) {
        self.logical.finish_interaction();
    }

    pub fn selected_text(&self, view: RenderView<'_>) -> String {
        self.logical
            .range()
            .and_then(|range| view.export().plain_text_with_paragraph_breaks(range))
            .unwrap_or_default()
    }

    pub fn rects(&self, view: RenderView<'_>) -> Vec<Rect> {
        self.logical
            .range()
            .map_or_else(Vec::new, |range| Geometry::new(view).selection_rects(range))
    }

    pub fn select_all(&mut self, view: RenderView<'_>) {
        self.logical
            .select_range(0, view.text().glyph_count() as u32, SelectionMode::Plain);
    }

    pub fn begin(&mut self, view: RenderView<'_>, position: Point<Pixels>, clicks: usize) -> bool {
        self.clear();
        let geometry = Geometry::new(view);
        let position = document_point(position);
        if clicks >= 2 {
            let range = if clicks == 2 {
                geometry.word_at(position, WordHitPolicy::Nearest)
            } else {
                geometry.block_range_at(position)
            };
            let Some(range) = range else { return false };
            self.logical
                .select_range(range.start, range.end, SelectionMode::Plain);
        } else {
            let Some(hit) = geometry.hit_test(position) else {
                return false;
            };
            self.logical.begin_at_glyph(hit.glyph, SelectionMode::Plain);
        }
        true
    }

    pub fn extend(&mut self, view: RenderView<'_>, position: Point<Pixels>) {
        let glyph = Geometry::new(view)
            .hit_test(document_point(position))
            .map(|hit| hit.glyph);
        self.logical.update_interaction(glyph, SelectionMode::Plain);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::GpuiTextCache;
    use gpui::{EmptyView, TestAppContext, point, px};
    use html::render::{FragmentRenderOptions, FragmentRenderer};

    fn render<'a>(
        renderer: &'a mut FragmentRenderer,
        shaper: &mut GpuiTextCache,
        source: &str,
    ) -> RenderView<'a> {
        let options = FragmentRenderOptions::new(300.0, None)
            .unwrap()
            .with_user_styles(vec![
                "html,body,p { margin:0; padding:0; font-size:20px; line-height:30px; }".into(),
            ]);
        renderer.render(source, &options, shaper).unwrap();
        renderer.render_view().unwrap()
    }

    #[gpui::test]
    fn description_selection_can_start_in_paragraph_padding(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, cx| {
                let mut renderer = FragmentRenderer::new();
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let view = render(&mut renderer, &mut shaper, "<p>Hi</p>");
                let mut selection = FragmentSelection::new(cx.focus_handle());
                assert!(selection.begin(view, point(px(-10.), px(15.)), 1));
                selection.extend(view, point(px(300.), px(15.)));
                assert_eq!(selection.selected_text(view), "Hi");
                assert!(selection.begin(view, point(px(300.), px(15.)), 1));
                selection.extend(view, point(px(-10.), px(15.)));
                assert_eq!(selection.selected_text(view), "Hi");
            })
            .unwrap();
    }

    #[gpui::test]
    fn selection_keeps_clusters_whole_and_respects_rtl_carets(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, cx| {
                let mut renderer = FragmentRenderer::new();
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let view = render(&mut renderer, &mut shaper, "<p>A👩‍👧e\u{301}Z</p>");
                let geometry = Geometry::new(view);
                let emoji = geometry.selection_bounds(1..4).unwrap();
                let accent = geometry.selection_bounds(4..6).unwrap();
                let start = point(px(emoji.x0 as f32 + 0.1), px(emoji.center().y as f32));
                let end = point(px(accent.x1 as f32 - 0.1), px(accent.center().y as f32));
                let mut selection = FragmentSelection::new(cx.focus_handle());
                assert!(selection.begin(view, start, 1));
                selection.extend(view, end);
                assert_eq!(selection.selected_text(view), "👩‍👧e\u{301}");
                assert!(selection.begin(view, end, 1));
                selection.extend(view, start);
                assert_eq!(selection.selected_text(view), "👩‍👧e\u{301}");

                let view = render(&mut renderer, &mut shaper, "<p dir='rtl'>אב</p>");
                let bounds = Geometry::new(view).selection_bounds(0..2).unwrap();
                let right = point(px(bounds.x1 as f32 - 0.1), px(bounds.center().y as f32));
                let left = point(px(bounds.x0 as f32 + 0.1), px(bounds.center().y as f32));
                assert!(selection.begin(view, right, 1));
                selection.extend(view, left);
                assert_eq!(selection.selected_text(view), "אב");
            })
            .unwrap();
    }

    #[gpui::test]
    fn selection_respects_overflow_clip_and_triple_clicks_authored_blocks(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window.update(cx, |_, window, cx| {
            let mut renderer = FragmentRenderer::new();
            let mut shaper = GpuiTextCache::new(window.text_system().clone());
            let view = render(&mut renderer, &mut shaper, "<div style='height:30px;overflow:hidden'><p>Visible</p><p>Clipped</p></div><p>Last <em>paragraph</em></p>");
            let mut selection = FragmentSelection::new(cx.focus_handle());
            selection.select_all(view);
            // The hidden line must never be chosen when dragging through its original position.
            let hidden = view.text().find_matches("Clipped", Default::default())[0].clone();
            selection.logical.select_range(hidden.start, hidden.end, SelectionMode::Plain);
            assert!(selection.rects(view).is_empty(), "clipped text must have no selection ink");
            let hit = Geometry::new(view).hit_test(DocumentPoint::new(10.0, 45.0)).unwrap();
            assert!(!hidden.contains(&hit.glyph));
            let last = view.text().find_matches("Last", Default::default())[0].clone();
            let rect = Geometry::new(view).selection_bounds(last).unwrap();
            assert!(selection.begin(view, point(px(rect.center().x as f32), px(rect.center().y as f32)), 3));
            assert_eq!(selection.selected_text(view), "Last paragraph");
            let view = render(&mut renderer, &mut shaper, "<p>office</p>");
            let ligature = Geometry::new(view).selection_bounds(1..3).unwrap();
            let point = point(px(ligature.center().x as f32), px(ligature.center().y as f32));
            assert!(selection.begin(view, point, 2));
            assert_eq!(selection.selected_text(view), "office");

        }).unwrap();
    }
}
